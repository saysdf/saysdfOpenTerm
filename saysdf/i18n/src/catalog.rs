//! Translation catalogs: TOML data files, parsing with limits and validation, loading
//! of the embedded catalogs and of the user's catalogs (change `i18n`, design D1–D4).
use crate::format::{placeholder_counts, placeholders};
use crate::text::is_invisible_format;
use std::collections::HashMap;
use std::path::Path;

/// Largest user catalog file that is read.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Most user catalog files read per language.
pub const MAX_FILES_PER_LANG: usize = 64;
/// Most entries in one catalog file.
pub const MAX_ENTRIES_PER_FILE: usize = 4096;
/// Longest key or translation, in bytes.
pub const MAX_TEXT_BYTES: usize = 4096;
/// Reserved table with per-language menu accelerators: `entry id = "key"`.
pub const ACCEL_TABLE: &str = "_accel";
/// Most occurrences of one placeholder in a translation (security review L2).
pub const MAX_PLACEHOLDER_REPEATS: usize = 4;
/// Most placeholder occurrences in a translation, all names together.
pub const MAX_PLACEHOLDERS: usize = 16;
/// Longest menu entry id in `_accel`, in bytes.
pub const MAX_ACCEL_ID_BYTES: usize = 128;

const PLURAL_FORMS: &[&str] = &["zero", "one", "two", "few", "many", "other"];

/// One translated message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Text(String),
    /// CLDR category -> translation (`one`, `few`, `many`, `other`, ...).
    Plural(HashMap<String, String>),
}

/// One parsed catalog file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct File {
    pub messages: HashMap<String, Msg>,
    pub accel: HashMap<String, String>,
}

/// All catalogs of one language, merged.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub lang: String,
    pub domains: HashMap<String, HashMap<String, Msg>>,
    pub accel: HashMap<String, String>,
    /// User catalog files skipped as broken (details are in the load warnings); the UI tells
    /// the user once per config load (`saysdf.setup`).
    pub rejected_user_files: usize,
}

impl Catalog {
    pub fn get(&self, domain: &str, key: &str) -> Option<&Msg> {
        self.domains.get(domain)?.get(key)
    }

    fn merge(&mut self, domain: &str, file: File) {
        self.domains
            .entry(domain.to_string())
            .or_default()
            .extend(file.messages);
        self.accel.extend(file.accel);
    }
}

fn check_text(what: &str, s: &str) -> Result<(), String> {
    if s.len() > MAX_TEXT_BYTES {
        return Err(format!("{what} is longer than {MAX_TEXT_BYTES} bytes"));
    }
    // Translations are shown in the terminal UI: no control characters except newline. A tab
    // is menu-bar markup (label<TAB>hotkey caption), so it is rejected too (security review L1).
    if let Some(c) = s.chars().find(|c| c.is_control() && *c != '\n') {
        return Err(format!("{what} contains control character U+{:04X}", c as u32));
    }
    // Bidi overrides, zero-width and other invisible characters make text look like other
    // text (security review M1)
    if let Some(c) = s.chars().find(|c| is_invisible_format(*c)) {
        return Err(format!(
            "{what} contains invisible format character U+{:04X}",
            c as u32
        ));
    }
    Ok(())
}

/// Placeholder repetition limits of a translation: a template that repeats a large argument
/// many times would build a huge string (security review L2).
fn check_repeats(s: &str) -> Result<(), String> {
    let counts = placeholder_counts(s)?;
    if let Some((name, n)) = counts.iter().find(|(_, n)| **n > MAX_PLACEHOLDER_REPEATS) {
        return Err(format!(
            "placeholder {{{name}}} is used {n} times (at most {MAX_PLACEHOLDER_REPEATS})"
        ));
    }
    let total: usize = counts.values().sum();
    if total > MAX_PLACEHOLDERS {
        return Err(format!("{total} placeholders (at most {MAX_PLACEHOLDERS})"));
    }
    Ok(())
}

/// Canonical form of a menu accelerator (`"q"` -> `"Q"`, `"ctrl+shift+q"` -> `"Ctrl+Shift+Q"`)
/// in the grammar of the menu bar (`saysdf_keys::parse`, as `saysdf_menu::parse_menu_accel`),
/// or `None` if the bar would reject it.
pub fn accel_key(spec: &str) -> Option<String> {
    saysdf_keys::parse(spec)
        .ok()
        .map(|k| saysdf_keys::display(&k))
}

/// One `_accel` entry: the id is plain text, the value a key the menu bar accepts
/// (security review L4: a bad value must not reach `saysdf_set_bar`).
fn check_accel(id: &str, v: &toml::Value) -> Result<String, String> {
    if id.is_empty() || id.len() > MAX_ACCEL_ID_BYTES {
        return Err(format!(
            "{ACCEL_TABLE}.{id:?}: entry id must be 1..{MAX_ACCEL_ID_BYTES} bytes"
        ));
    }
    check_text("entry id", id).map_err(|e| format!("{ACCEL_TABLE}.{id:?}: {e}"))?;
    let bad = || {
        format!(
            "{ACCEL_TABLE}.{id}: expected a key name accepted by the menu (\"q\", \"F2\", \"Ctrl+Shift+Q\")"
        )
    };
    let toml::Value::String(k) = v else {
        return Err(bad());
    };
    if k.is_empty() || k.len() > 32 || check_text("key", k).is_err() || accel_key(k).is_none() {
        return Err(bad());
    }
    Ok(k.clone())
}

/// Parses one catalog. Values: a string, or a table of plural forms; the reserved
/// `_accel` table maps menu entry ids to accelerator keys. Placeholders of a translation must
/// be a subset of the key's (`n` is always allowed in plural forms). Any error rejects the file.
pub fn parse(src: &str) -> Result<File, String> {
    let table: toml::Table = src.parse().map_err(|e: toml::de::Error| e.message().to_string())?;
    if table.len() > MAX_ENTRIES_PER_FILE {
        return Err(format!("more than {MAX_ENTRIES_PER_FILE} entries"));
    }
    let mut file = File::default();
    for (key, value) in table {
        check_text("key", &key).map_err(|e| format!("{key:?}: {e}"))?;
        if key == ACCEL_TABLE {
            let toml::Value::Table(t) = value else {
                return Err(format!("{ACCEL_TABLE} must be a table"));
            };
            if t.len() > MAX_ENTRIES_PER_FILE {
                return Err(format!("{ACCEL_TABLE}: more than {MAX_ENTRIES_PER_FILE} entries"));
            }
            for (id, v) in t {
                let k = check_accel(&id, &v)?;
                file.accel.insert(id, k);
            }
            continue;
        }
        let allowed = placeholders(&key).map_err(|e| format!("key {key:?}: {e}"))?;
        let check = |form: &str, s: &str, plural: bool| -> Result<(), String> {
            check_text("translation", s).map_err(|e| format!("{key:?}{form}: {e}"))?;
            check_repeats(s).map_err(|e| format!("{key:?}{form}: {e}"))?;
            let used = placeholders(s).map_err(|e| format!("{key:?}{form}: {e}"))?;
            if let Some(bad) = used.iter().find(|p| !allowed.contains(*p) && !(plural && *p == "n")) {
                return Err(format!("{key:?}{form}: unknown placeholder {{{bad}}}"));
            }
            Ok(())
        };
        let msg = match value {
            toml::Value::String(s) => {
                check("", &s, false)?;
                Msg::Text(s)
            }
            toml::Value::Table(t) => {
                let mut forms = HashMap::new();
                for (form, v) in t {
                    if !PLURAL_FORMS.contains(&form.as_str()) {
                        return Err(format!("{key:?}: unknown plural form {form:?}"));
                    }
                    let toml::Value::String(s) = v else {
                        return Err(format!("{key:?}.{form}: expected a string"));
                    };
                    check(&format!(".{form}"), &s, true)?;
                    forms.insert(form, s);
                }
                if forms.is_empty() {
                    return Err(format!("{key:?}: empty plural table"));
                }
                Msg::Plural(forms)
            }
            _ => return Err(format!("{key:?}: expected a string or a table of plural forms")),
        };
        file.messages.insert(key, msg);
    }
    Ok(file)
}

/// Windows device names (`con`, `nul`, `com1`, ...): never a catalog file or directory name,
/// on any OS (security review I1).
fn windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.as_bytes()[3].is_ascii_digit())
}

/// A language tag that is safe as a directory name: `ll`, `lll`, `ll-RR`, `ll-Script-RR`.
pub fn valid_lang(lang: &str) -> bool {
    if windows_reserved(lang) {
        return false;
    }
    let mut parts = lang.split('-');
    let first = parts.next().unwrap_or("");
    (2..=3).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_lowercase())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// A domain name that is safe as a file stem: `[a-z0-9_-]+`.
pub fn valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && !windows_reserved(domain)
        && domain.len() <= 64
        && domain
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Languages with embedded catalogs.
pub fn embedded_languages(embedded: &[(&str, &str, &str)]) -> Vec<String> {
    let mut v: Vec<String> = embedded.iter().map(|(l, _, _)| l.to_string()).collect();
    v.dedup();
    v
}

/// Languages that have a user catalog dir with at least one `.toml` file.
pub fn user_languages(user_dir: Option<&Path>) -> Vec<String> {
    let Some(dir) = user_dir else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|l| valid_lang(l) && !user_files(dir, l).0.is_empty())
        .collect();
    v.sort();
    v
}

/// `.toml` files of `<dir>/<lang>/`, sorted, at most [`MAX_FILES_PER_LANG`]; plus a warning
/// when there were more.
fn user_files(dir: &Path, lang: &str) -> (Vec<std::path::PathBuf>, Option<String>) {
    let Ok(rd) = std::fs::read_dir(dir.join(lang)) else {
        return (Vec::new(), None);
    };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .collect();
    files.sort();
    let mut warning = None;
    if files.len() > MAX_FILES_PER_LANG {
        warning = Some(format!(
            "{}: more than {MAX_FILES_PER_LANG} catalog files, the rest are ignored",
            dir.join(lang).display()
        ));
        files.truncate(MAX_FILES_PER_LANG);
    }
    (files, warning)
}

fn read_user_file(path: &Path) -> Result<(String, File), String> {
    let domain = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|d| valid_domain(d))
        .ok_or("file name must be <domain>.toml with [a-z0-9_-] only")?
        .to_string();
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("larger than {MAX_FILE_BYTES} bytes"));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("larger than {MAX_FILE_BYTES} bytes"));
    }
    let src = String::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())?;
    Ok((domain, parse(&src)?))
}

/// Catalog of `lang`: embedded files, then the user's `<user_dir>/<lang>/*.toml` on top
/// (entry by entry). A broken user file is skipped as a whole, with a warning.
pub fn load(
    lang: &str,
    embedded: &[(&str, &str, &str)],
    user_dir: Option<&Path>,
) -> (Catalog, Vec<String>) {
    let mut cat = Catalog {
        lang: lang.to_string(),
        ..Default::default()
    };
    let mut warnings = Vec::new();
    for (l, domain, src) in embedded {
        if *l != lang {
            continue;
        }
        match parse(src) {
            Ok(file) => cat.merge(domain, file),
            Err(e) => warnings.push(format!("embedded catalog {l}/{domain}.toml: {e}")),
        }
    }
    if let (Some(dir), true) = (user_dir, valid_lang(lang)) {
        let (files, warning) = user_files(dir, lang);
        warnings.extend(warning);
        for path in files {
            match read_user_file(&path) {
                Ok((domain, file)) => cat.merge(&domain, file),
                Err(e) => {
                    cat.rejected_user_files += 1;
                    warnings.push(format!("{}: {e}; file ignored", path.display()))
                }
            }
        }
    }
    (cat, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_plural_accel() {
        let f = parse(
            r#"
            "Open" = "Открыть" # i18n-allow-cyrillic
            "{n} file" = { one = "{n} файл", few = "{n} файла", many = "{n} файлов" } # i18n-allow-cyrillic
            [_accel]
            "x.open" = "o"
            "#,
        )
        .unwrap();
        assert_eq!(f.messages["Open"], Msg::Text("Открыть".into())); // i18n-allow-cyrillic
        let Msg::Plural(p) = &f.messages["{n} file"] else {
            panic!()
        };
        assert_eq!(p["few"], "{n} файла"); // i18n-allow-cyrillic
        assert_eq!(f.accel["x.open"], "o");
    }

    #[test]
    fn parse_accepts_literal_brace_escapes() {
        let f = parse(r#""x = {{ a = 1 }} for {n}" = "y = {{ a = 1 }} for {n}!""#).unwrap();
        assert_eq!(f.messages["x = {{ a = 1 }} for {n}"], Msg::Text("y = {{ a = 1 }} for {n}!".into()));
        // A translation must not turn an escape into a placeholder
        assert!(parse(r#""x = {{a}}" = "y = {a}""#).unwrap_err().contains("unknown placeholder"));
    }

    #[test]
    fn parse_rejects_bad_files() {
        for (src, needle) in [
            ("x = 1", "expected a string"),
            ("\"a\" = \"{b}\"", "unknown placeholder {b}"),
            ("\"{a\" = \"x\"", "unclosed"),
            ("\"a\" = { lots = \"x\" }", "unknown plural form"),
            ("\"a\" = {}", "empty plural"),
            ("\"a\" = \"\\u001b[31m\"", "control character"),
            ("_accel = \"x\"", "must be a table"),
            ("[_accel]\nx = \"\"", "key name accepted by the menu"),
            ("not toml at all", ""),
        ] {
            let e = parse(src).unwrap_err();
            assert!(e.contains(needle), "{src}: {e}");
        }
        let long = format!("\"a\" = \"{}\"", "x".repeat(MAX_TEXT_BYTES + 1));
        assert!(parse(&long).unwrap_err().contains("longer"));
    }

    // Malicious catalog samples from security-reviews/2026-10-21-i18n.md (TOML escapes, so
    // the source stays readable).
    #[test]
    fn parse_rejects_disguised_text() {
        for (src, needle) in [
            // M1: RLO/RLI reverse the visible text; zero-width chars hide a difference
            (r#""Open" = "Open\u202Eevil""#, "U+202E"),
            (r#""Open" = "\u2067Open\u2069""#, "U+2067"),
            (r#""Open" = "Op\u200Den""#, "U+200D"),
            (r#""Open" = "Op\u200Ben""#, "U+200B"),
            (r#""Open" = "\uFEFFOpen""#, "U+FEFF"),
            (r#""Open" = "Op\u00ADen""#, "U+00AD"),
            (r#""Open" = "\u061COpen""#, "U+061C"),
            (r#""Open" = "Open\u2028x""#, "U+2028"),
            (r#""a" = { one = "\u202Ex" }"#, "U+202E"),
            (r#""Op\u202Een" = "Open""#, "U+202E"),
            // ESC (SGR/OSC injection) stays rejected as a control character
            (r#""Open" = "\u001b]0;x\u0007""#, "control character"),
            // L1: a tab would show a fake hotkey caption in the menu bar
            (r#""Copy" = "Copy\tCtrl+Shift+Q""#, "U+0009"),
        ] {
            let e = parse(src).unwrap_err();
            assert!(e.contains(needle), "{src}: {e}");
        }
        // Newlines stay allowed (multi-line help text); `&` is plain text here, the menu
        // escapes it
        parse(r#""Help" = "line 1\nline 2 & more""#).unwrap();
    }

    #[test]
    fn parse_limits_placeholder_repeats() {
        // L2: a template that repeats a large argument many times
        let tr = |n| "{text}".repeat(n);
        let e = parse(&format!(r#""Copied: {{text}}" = "{}""#, tr(5))).unwrap_err();
        assert!(e.contains("{text} is used 5 times"), "{e}");
        parse(&format!(r#""Copied: {{text}}" = "{}""#, tr(MAX_PLACEHOLDER_REPEATS))).unwrap();
        // The total over all names
        let key: String = (0..5).map(|i| format!("{{p{i}}}")).collect();
        let e = parse(&format!(r#""{key}" = "{}""#, key.repeat(4))).unwrap_err();
        assert!(e.contains("20 placeholders"), "{e}");
        // Plural forms are checked too
        let e = parse(&format!(r#""{{n}} x" = {{ one = "{}" }}"#, "{n}".repeat(5))).unwrap_err();
        assert!(e.contains("{n} is used 5 times"), "{e}");
    }

    #[test]
    fn parse_validates_accel() {
        // L4: values the menu bar would reject never get past the loader
        for v in [
            "Ctrl+Nope+Q",
            r"\u0001",
            r"\u202Eq",
            r"q\tx",
            r"\u0439",
            "a key name far too long for any menu",
            "",
        ] {
            let e = parse(&format!("[_accel]\n\"core.quit\" = \"{v}\"")).unwrap_err();
            assert!(e.contains("_accel"), "{v}: {e}");
        }
        assert!(parse("[_accel]\n\"core.quit\" = 1").is_err());
        assert!(parse(r#"_accel = { "x\u202E" = "q" }"#)
            .unwrap_err()
            .contains("U+202E"));
        assert!(parse(r#"_accel = { "" = "q" }"#).is_err());
        let f = parse("[_accel]\na = \"q\"\nb = \"F2\"\nc = \"Ctrl+Shift+Q\"\nd = \"=\"").unwrap();
        assert_eq!(f.accel.len(), 4);
        assert_eq!(accel_key("q").as_deref(), Some("Q"));
        assert_eq!(accel_key("ctrl+shift+q"), accel_key("Ctrl+Shift+Q"));
        assert_eq!(accel_key("Ctrl+Nope+Q"), None);
    }

    #[test]
    fn plural_may_use_n() {
        parse("\"one file\" = { one = \"{n} файл\" }").unwrap(); // i18n-allow-cyrillic
        assert!(parse("\"one file\" = \"{n} файл\"").is_err()); // i18n-allow-cyrillic
    }

    #[test]
    fn names() {
        for ok in ["ru", "en", "pt-BR", "zh-Hans-CN", "fil"] {
            assert!(valid_lang(ok), "{ok}");
        }
        for bad in ["", "r", "RU", "../x", "ru/..", "ru-", "ru_RU", "русский"] { // i18n-allow-cyrillic
            assert!(!valid_lang(bad), "{bad}");
        }
        assert!(valid_domain("cli-db"));
        assert!(!valid_domain("../x") && !valid_domain("A") && !valid_domain(""));
        // I1: Windows device names
        for bad in ["con", "prn", "aux", "nul", "com1", "lpt9", "CON"] {
            assert!(!valid_domain(bad) && !valid_lang(bad), "{bad}");
        }
        assert!(valid_domain("console") && valid_domain("com") && valid_domain("auxiliary"));
        assert!(valid_lang("com") && valid_lang("nl"));
    }

    static EMB: &[(&str, &str, &str)] = &[
        ("de", "m", "\"Open\" = \"Öffnen\""),
        ("ru", "m", "\"Open\" = \"Открыть\"\n\"Close\" = \"Закрыть\""), // i18n-allow-cyrillic
    ];

    #[test]
    fn user_files_override_and_broken_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let ru = dir.path().join("ru");
        std::fs::create_dir(&ru).unwrap();
        std::fs::write(ru.join("m.toml"), "\"Open\" = \"Раскрыть\"\n[_accel]\n\"m.open\" = \"r\"").unwrap(); // i18n-allow-cyrillic
        std::fs::write(ru.join("n.toml"), "\"Open\" = 5").unwrap();
        std::fs::write(ru.join("Bad Name.toml"), "\"Open\" = \"x\"").unwrap();
        std::fs::write(ru.join("notes.txt"), "ignored").unwrap();
        let (cat, warnings) = load("ru", EMB, Some(dir.path()));
        assert_eq!(cat.get("m", "Open"), Some(&Msg::Text("Раскрыть".into()))); // i18n-allow-cyrillic
        assert_eq!(cat.get("m", "Close"), Some(&Msg::Text("Закрыть".into()))); // i18n-allow-cyrillic
        assert_eq!(cat.get("n", "Open"), None);
        assert_eq!(cat.accel["m.open"], "r");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("n.toml") && w.contains("ignored")));
        // n.toml and "Bad Name.toml"; notes.txt is not a catalog
        assert_eq!(cat.rejected_user_files, 2);

        let big = dir.path().join("eo");
        std::fs::create_dir(&big).unwrap();
        std::fs::write(big.join("m.toml"), "#".repeat(MAX_FILE_BYTES as usize + 1)).unwrap();
        let (cat, warnings) = load("eo", EMB, Some(dir.path()));
        assert!(cat.domains.is_empty());
        assert!(warnings[0].contains("larger"), "{warnings:?}");

        assert_eq!(user_languages(Some(dir.path())), ["eo", "ru"]);
        assert_eq!(embedded_languages(EMB), ["de", "ru"]);
        // A language name that is not a safe dir name is never looked up
        let (cat, _) = load("../ru", EMB, Some(dir.path()));
        assert!(cat.domains.is_empty());
    }
}
