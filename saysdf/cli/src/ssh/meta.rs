//! Connection metadata `ssh-connections.json` (design D4): `{ alias: {title, description, group, color} }`.
//! Foreign entries are preserved; writing is temp file + rename. There are no secrets here.

use super::{check_text, write_atomic, Result};
use serde_json::{Map, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-ssh");

pub const FILE_NAME: &str = "ssh-connections.json";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub title: String,
    pub description: String,
    pub group: String,
    pub color: String,
}

impl Meta {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.description.is_empty() && self.group.is_empty() && self.color.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        check_text("title", &self.title)?;
        check_text("description", &self.description)?;
        check_text("group", &self.group)?;
        check_text("color", &self.color)
    }
}

/// Default path: `$SAYSDFTERM_CONFIG_DIR/saysdf/…`, otherwise `~/.config/saysdfTerm/saysdf/…`.
/// The `saysdf.ssh` module passes the path explicitly (`--meta-file`, from `wezterm.config_dir`).
pub fn default_path() -> Option<PathBuf> {
    let base = saysdf_identity::user_config_dir()?;
    Some(base.join("saysdf").join(FILE_NAME))
}

/// Read the object; a missing file gives an empty one. A non-object is an error (we do not overwrite foreign data).
pub fn read(path: &Path) -> io::Result<Map<String, Value>> {
    match fs::read(path) {
        Ok(b) if b.iter().all(u8::is_ascii_whitespace) => Ok(Map::new()),
        Ok(b) => match serde_json::from_slice::<Value>(&b) {
            Ok(Value::Object(m)) => Ok(m),
            Ok(_) => Err(io::Error::new(io::ErrorKind::InvalidData, I18N.t("a JSON object was expected").into_owned())),
            Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e),
    }
}

/// Add or replace the alias entry, keeping the others.
pub fn upsert(path: &Path, alias: &str, meta: &Meta) -> Result<()> {
    meta.validate()?;
    let mut map = read(path)?;
    let mut obj = Map::new();
    for (k, v) in [
        ("title", &meta.title),
        ("description", &meta.description),
        ("group", &meta.group),
        ("color", &meta.color),
    ] {
        if !v.is_empty() {
            obj.insert(k.into(), Value::String(v.clone()));
        }
    }
    map.insert(alias.into(), Value::Object(obj));
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(map)).map_err(io::Error::other)?;
    write_atomic(path, format!("{text}\n").as_bytes(), false)?;
    Ok(())
}
