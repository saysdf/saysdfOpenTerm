//! Saved connections: `<config_dir>/saysdf/db-connections.toml` (design D6).
//! The file holds no password (neither does [`ConnectionConfig`]): it lives in `saysdf-secrets`
//! (`db:<id>`).

use saysdf_db::{ConnectionConfig, Kind, TlsMode};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-db");

pub const FILE_NAME: &str = "db-connections.toml";

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "connection")]
    connection: Vec<ConnectionConfig>,
}

pub fn default_path() -> Option<PathBuf> {
    let base = saysdf_identity::user_config_dir()?;
    Some(base.join("saysdf").join(FILE_NAME))
}

pub fn parse(text: &str) -> Result<Vec<ConnectionConfig>, String> {
    toml::from_str::<File>(text)
        .map(|f| f.connection)
        .map_err(|e| e.to_string())
}

pub fn to_toml(conns: &[ConnectionConfig]) -> Result<String, String> {
    toml::to_string_pretty(&File {
        connection: conns.to_vec(),
    })
    .map_err(|e| e.to_string())
}

/// A missing file gives an empty list.
pub fn load(path: &Path) -> Result<Vec<ConnectionConfig>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => parse(&t).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn save(path: &Path, conns: &[ConnectionConfig]) -> Result<(), String> {
    let text = to_toml(conns)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::ssh::write_atomic(path, text.as_bytes(), true).map_err(|e| e.to_string())
}

pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err(I18N.t("1–64 characters").into_owned());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(I18N.t("only Latin letters, digits, - _ .").into_owned());
    }
    Ok(())
}

pub fn parse_kind(s: &str) -> Option<Kind> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "sqlite" => Kind::Sqlite,
        "postgres" | "postgresql" | "pg" => Kind::Postgres,
        "mysql" => Kind::Mysql,
        "dolt" => Kind::Dolt,
        "postgres-ssh" | "postgresql-ssh" | "pg-ssh" => Kind::PgSsh,
        _ => return None,
    })
}

pub fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::Sqlite => "sqlite",
        Kind::Postgres => "postgres",
        Kind::Mysql => "mysql",
        Kind::Dolt => "dolt",
        Kind::PgSsh => "postgres-ssh",
    }
}

pub fn parse_tls(s: &str) -> Option<TlsMode> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "" | "require" => TlsMode::Require,
        "verify-full" => TlsMode::VerifyFull,
        "disable" => TlsMode::Disable,
        _ => return None,
    })
}

pub fn tls_str(t: TlsMode) -> &'static str {
    match t {
        TlsMode::Require => "require",
        TlsMode::VerifyFull => "verify-full",
        TlsMode::Disable => "disable",
    }
}

/// Add or replace by `id`.
pub fn upsert(conns: &mut Vec<ConnectionConfig>, c: ConnectionConfig) {
    match conns.iter_mut().find(|x| x.id == c.id) {
        Some(x) => *x = c,
        None => conns.push(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_roundtrip_has_no_password() {
        let mut pg: ConnectionConfig = serde_json::from_str(
            r#"{"id":"prod","title":"Production","description":"SELECT-only role","kind":"postgres","host":"db","user":"ro","database":"app","remember_password":true}"#,
        )
        .unwrap();
        pg.port = Some(5432);
        let conns = vec![pg, ConnectionConfig::sqlite("local", "C:/tmp/a.db")];
        let text = to_toml(&conns).unwrap();
        assert!(text.contains("[[connection]]"), "{text}");
        assert!(
            text.lines()
                .all(|l| !l.trim_start().starts_with("password")),
            "{text}"
        );
        assert!(text.contains("remember_password = true"), "{text}");
        assert_eq!(parse(&text).unwrap(), conns);
    }

    #[test]
    fn upsert_replaces_and_validates() {
        let mut v = vec![ConnectionConfig::sqlite("a", "x.db")];
        upsert(&mut v, ConnectionConfig::sqlite("a", "y.db"));
        upsert(&mut v, ConnectionConfig::sqlite("b", "z.db"));
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].path.as_deref(), Some("y.db"));
        assert!(validate_id("prod-1").is_ok());
        assert!(validate_id("a b").is_err());
        assert!(validate_id("").is_err());
        assert_eq!(parse_kind("PostgreSQL"), Some(Kind::Postgres));
        assert_eq!(parse_tls("verify-full"), Some(TlsMode::VerifyFull));
        assert!(parse_tls("prefer").is_none());
        assert_eq!(parse_kind("postgres-ssh"), Some(Kind::PgSsh));
        assert_eq!(kind_str(Kind::PgSsh), "postgres-ssh");
        let ssh = ConnectionConfig::pg_ssh("prod", "prodhost", Some("db-1"), "g", "ro");
        let text = to_toml(std::slice::from_ref(&ssh)).unwrap();
        assert!(text.contains("kind = \"postgres-ssh\""), "{text}");
        assert!(text.contains("ssh_alias = \"prodhost\""), "{text}");
        assert_eq!(parse(&text).unwrap(), vec![ssh]);
    }
}
