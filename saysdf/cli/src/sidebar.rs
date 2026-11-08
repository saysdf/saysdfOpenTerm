//! Result channel of the side panel (change tool-sidebar, design D3–D4): a pick list from a file
//! in runtime_dir, the answer to the GUI is the user var `SAYSDF_SIDEBAR` =
//! `<token>:<kind>:<payload>` (OSC 1337). The GUI (`saysdf.sidebar`) accepts the value only from
//! its own pane, with `saysdf` in the foreground, with the one-time token of this opening and no
//! longer than [`crate::bridge::MAX_VALUE`]. The channel itself is [`crate::bridge`].

use crate::bridge;
use std::io;
use std::path::Path;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-git");

/// Name of the result user var.
pub const VAR: &str = "SAYSDF_SIDEBAR";
/// Environment variable with the one-time token of the opening.
pub const TOKEN_ENV: &str = "SAYSDF_SIDEBAR_TOKEN";
/// List file limit.
pub const MAX_FILE: u64 = 4 * 1024 * 1024;
/// Send retries: the GUI's foreground cache may lag by a tick (D3).
const RETRIES: usize = 3;

/// A pick list item.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub label: String,
}

/// Takes the token from the environment and removes it there (see [`bridge::take_token`]).
pub fn take_token() -> Option<String> {
    bridge::take_token(TOKEN_ENV)
}

#[cfg(test)]
pub use crate::bridge::{parse_value, valid_token, value, MAX_VALUE};

/// Strips control characters (labels are plain text without escape sequences).
fn plain(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Parses JSON `[{id,label}]`; items without a non-empty string id are skipped.
pub fn parse_items(text: &str) -> Result<Vec<Item>, String> {
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| I18N.f("list: {error}", &[("error", &e)]))?;
    let arr = v
        .as_array()
        .ok_or_else(|| I18N.t("list: an array is expected").into_owned())?;
    Ok(arr
        .iter()
        .filter_map(|e| {
            let id = e.get("id")?.as_str()?;
            if id.is_empty() || id.chars().any(char::is_control) {
                return None;
            }
            let label = e.get("label").and_then(|l| l.as_str()).unwrap_or(id);
            Some(Item { id: id.to_owned(), label: plain(label) })
        })
        .collect())
}

/// Reads and removes the list file (limit [`MAX_FILE`]).
pub fn read_items(path: &Path) -> Result<Vec<Item>, String> {
    let res = (|| {
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.len() > MAX_FILE {
            let mib = MAX_FILE / 1024 / 1024;
            return Err(I18N.f("the list is larger than {size} MB", &[("size", &mib)]));
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        parse_items(&text)
    })();
    let _ = std::fs::remove_file(path);
    res
}

/// OSC 1337 SetUserVar of [`VAR`].
#[cfg(test)]
pub fn osc(value: &str) -> String {
    bridge::osc(VAR, value)
}

/// Sends the result with retries (D3). The terminal must already be restored.
pub fn send(token: &str, kind: &str, payload: &str) -> io::Result<()> {
    bridge::Channel { var: VAR, token: token.to_owned(), retries: RETRIES }.send(kind, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_roundtrip_and_limit() {
        let v = value("abcd1234", "pick", "id:with:colons").unwrap();
        assert_eq!(v, "abcd1234:pick:id:with:colons");
        assert_eq!(parse_value(&v), Some(("abcd1234", "pick", "id:with:colons")));
        assert_eq!(parse_value("tok:pick:"), Some(("tok", "pick", "")));
        assert_eq!(parse_value("tok"), None);
        assert!(value("t:x", "pick", "a").is_none());
        assert!(value("abcd1234", "pick", &"x".repeat(MAX_VALUE)).is_none());
        assert!(value("abcd1234", "pick", &"x".repeat(MAX_VALUE - 14)).is_some());
    }

    #[test]
    fn osc_encoding() {
        assert_eq!(osc("a:b:c"), "\x1b]1337;SetUserVar=SAYSDF_SIDEBAR=YTpiOmM=\x07");
    }

    #[test]
    fn tokens() {
        assert!(valid_token("0123abcd-89ef0011"));
        assert!(!valid_token("short"));
        assert!(!valid_token("0123abcd;rm -rf"));
    }

    #[test]
    fn items_sanitized() {
        let items = parse_items(
            r#"[{"id":"a","label":"A\u001b[31m red"},{"id":""},{"label":"no id"},{"id":"b"},{"id":"c\u0007"}]"#,
        )
        .unwrap();
        assert_eq!(
            items,
            vec![
                Item { id: "a".into(), label: "A[31m red".into() },
                Item { id: "b".into(), label: "b".into() },
            ]
        );
        assert!(parse_items("{}").is_err());
    }

    #[test]
    fn read_items_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sidebar-x.json");
        std::fs::write(&p, r#"[{"id":"1","label":"one"}]"#).unwrap();
        assert_eq!(read_items(&p).unwrap().len(), 1);
        assert!(!p.exists());
    }
}
