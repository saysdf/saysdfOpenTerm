//! Connection description (design D6). There is deliberately no password field: secrets live
//! in `saysdf-secrets` under `db:<id>`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Sqlite,
    Postgres,
    Mysql,
    Dolt,
    /// PostgreSQL reached through `ssh <alias> [docker exec -i <container>] psql` (change
    /// `db-over-ssh`): no open ports or tunnels, the database socket stays on the server.
    #[serde(rename = "postgres-ssh")]
    PgSsh,
}

impl Kind {
    /// Network kinds that may ask for a password. `PgSsh` relies on the server-side socket
    /// (trust/peer) and never takes a password.
    pub fn needs_password(self) -> bool {
        matches!(self, Kind::Postgres | Kind::Mysql | Kind::Dolt)
    }
}

/// TLS for network databases. Default `require`: encrypted, certificate not verified
/// (libpq semantics); `verify-full` also checks the chain and host name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    Disable,
    #[default]
    Require,
    VerifyFull,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// SQLite file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `postgres-ssh`: ssh host alias from `~/.ssh/config` (validated, see `pg_ssh`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_alias: Option<String>,
    /// `postgres-ssh`: docker container with psql; none — psql runs directly on the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(default)]
    pub tls: TlsMode,
    #[serde(default)]
    pub remember_password: bool,
}

impl ConnectionConfig {
    pub fn sqlite(id: &str, path: &str) -> Self {
        Self {
            id: id.into(),
            title: None,
            description: None,
            kind: Kind::Sqlite,
            host: None,
            port: None,
            database: None,
            user: None,
            path: Some(path.into()),
            ssh_alias: None,
            container: None,
            tls: TlsMode::Require,
            remember_password: false,
        }
    }

    /// `postgres-ssh` connection; values are validated when the connection is opened.
    pub fn pg_ssh(
        id: &str,
        ssh_alias: &str,
        container: Option<&str>,
        database: &str,
        user: &str,
    ) -> Self {
        let mut c = Self::sqlite(id, "");
        c.kind = Kind::PgSsh;
        c.path = None;
        c.ssh_alias = Some(ssh_alias.into());
        c.container = container.map(String::from);
        c.database = Some(database.into());
        c.user = Some(user.into());
        c
    }

    /// The name the user types to confirm write mode: `database`, or the SQLite file name.
    pub fn database_name(&self) -> String {
        if let Some(db) = &self.database {
            return db.clone();
        }
        self.path
            .as_deref()
            .map(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.to_string())
            })
            .unwrap_or_default()
    }

    pub fn host_or_default(&self) -> &str {
        self.host.as_deref().unwrap_or("localhost")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_has_no_password_and_tls_require_default() {
        let c: ConnectionConfig =
            serde_json::from_str(r#"{"id":"p","kind":"postgres","host":"db","user":"u"}"#).unwrap();
        assert_eq!(c.tls, TlsMode::Require);
        let s = serde_json::to_string(&c).unwrap();
        assert!(!s.contains("\"password\""), "{s}");
        assert!(s.contains("\"tls\":\"require\""));
        assert_eq!(
            ConnectionConfig::sqlite("x", "/tmp/a.db").database_name(),
            "a.db"
        );
    }
}
