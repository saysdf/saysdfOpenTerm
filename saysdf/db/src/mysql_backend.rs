//! MySQL / Dolt backend (sync `mysql`, `native-tls`). Read-only: `SET SESSION TRANSACTION
//! READ ONLY` at connect, every statement in `START TRANSACTION READ ONLY` + `ROLLBACK`,
//! prepared statements only (one statement), plus the MySQL allow-list in [`guard`].
//! `program_name` in connect attributes is `saysdfTerm` (the crate would send the exe path).

use std::collections::HashMap;

use mysql::prelude::Queryable;
use mysql::{AccessMode, Conn, Opts, OptsBuilder, Params, SslOpts, TxOpts};

use crate::{
    dolt, guard, page_sql, Access, Backend, CancelHandle, Collector, ColumnInfo, ConnectionConfig,
    DoltBranch, DoltCommit, Error, ForeignKey, IndexInfo, Kind, ObjectKind, PageRequest, Result,
    ResultSet, RowSink, TableDesc, TableInfo, TableRef, TlsMode, Value,
};

pub struct MysqlBackend {
    conn: Conn,
    opts: Opts,
    access: Access,
    kind: Kind,
}

fn to_value(v: mysql::Value) -> Value {
    use mysql::Value as M;
    match v {
        M::NULL => Value::Null,
        M::Bytes(b) => match String::from_utf8(b) {
            Ok(s) => Value::Text(s),
            Err(e) => Value::Bytes(e.into_bytes()),
        },
        M::Int(i) => Value::Int(i),
        M::UInt(u) => Value::UInt(u),
        M::Float(f) => Value::Float(f as f64),
        M::Double(f) => Value::Float(f),
        M::Date(y, mo, d, h, mi, s, us) => {
            if h == 0 && mi == 0 && s == 0 && us == 0 {
                Value::Text(format!("{y:04}-{mo:02}-{d:02}"))
            } else if us == 0 {
                Value::Text(format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}"))
            } else {
                Value::Text(format!(
                    "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}"
                ))
            }
        }
        M::Time(neg, d, h, mi, s, us) => {
            let sign = if neg { "-" } else { "" };
            let h = d * 24 + h as u32;
            if us == 0 {
                Value::Text(format!("{sign}{h:02}:{mi:02}:{s:02}"))
            } else {
                Value::Text(format!("{sign}{h:02}:{mi:02}:{s:02}.{us:06}"))
            }
        }
    }
}

fn collect<Q: Queryable>(
    q: &mut Q,
    sql: &str,
    params: Params,
    max_rows: usize,
) -> Result<ResultSet> {
    let mut col = Collector::new(max_rows);
    let (affected, _) = stream(q, sql, params, &mut col)?;
    col.rs.affected = affected;
    Ok(col.rs)
}

/// Streams rows into `sink`; returns (rows affected for non-SELECT, rows sent).
fn stream<Q: Queryable>(
    q: &mut Q,
    sql: &str,
    params: Params,
    sink: &mut dyn RowSink,
) -> Result<(Option<u64>, u64)> {
    let mut result = q.exec_iter(sql, params)?;
    let columns: Vec<String> = result
        .columns()
        .as_ref()
        .iter()
        .map(|c| c.name_str().into_owned())
        .collect();
    sink.columns(&columns)?;
    let mut n = 0u64;
    let mut failed = None;
    for row in result.by_ref() {
        let row = row?;
        match sink.row(row.unwrap().into_iter().map(to_value).collect()) {
            Ok(true) => n += 1,
            Ok(false) => break,
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    // drain the rest so the connection stays usable
    for _ in result.by_ref() {}
    if let Some(e) = failed {
        return Err(e);
    }
    let affected = columns.is_empty().then(|| result.affected_rows());
    Ok((affected, n))
}

/// Options without the password in any log/format path; the password is only passed to
/// the driver.
pub fn build_opts(cfg: &ConnectionConfig, password: Option<&str>, read_only: bool) -> Opts {
    let mut attrs = HashMap::new();
    attrs.insert("program_name", saysdf_identity::APP_NAME);
    let ssl = match cfg.tls {
        TlsMode::Disable => None,
        TlsMode::Require => Some(
            SslOpts::default()
                .with_danger_accept_invalid_certs(true)
                .with_danger_skip_domain_validation(true),
        ),
        TlsMode::VerifyFull => Some(SslOpts::default()),
    };
    let mut b = OptsBuilder::new()
        .ip_or_hostname(Some(cfg.host_or_default()))
        .tcp_port(cfg.port.unwrap_or(3306))
        .user(cfg.user.as_deref())
        .pass(password)
        .db_name(cfg.database.as_deref())
        .prefer_socket(false)
        .ssl_opts(ssl)
        .connect_attrs(Some(attrs));
    if read_only {
        b = b.init(vec!["SET SESSION TRANSACTION READ ONLY"]);
    }
    b.into()
}

impl MysqlBackend {
    pub fn open(cfg: &ConnectionConfig, password: Option<&str>, access: Access) -> Result<Self> {
        let opts = build_opts(cfg, password, access.is_read_only());
        let conn = Conn::new(opts.clone())?;
        Ok(Self {
            conn,
            opts,
            access,
            kind: cfg.kind,
        })
    }

    /// Runs one statement: read-only → inside `START TRANSACTION READ ONLY` … `ROLLBACK`.
    fn run(&mut self, sql: &str, params: Params, max_rows: usize) -> Result<ResultSet> {
        if self.access.is_read_only() {
            let mut tx = self
                .conn
                .start_transaction(TxOpts::default().set_access_mode(Some(AccessMode::ReadOnly)))?;
            let r = collect(&mut tx, sql, params, max_rows);
            tx.rollback()?;
            r
        } else {
            collect(&mut self.conn, sql, params, max_rows)
        }
    }

    fn rows(&mut self, sql: &str, params: Params) -> Result<Vec<Vec<Option<String>>>> {
        let rs = self.run(sql, params, 100_000)?;
        Ok(rs
            .rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|v| match v {
                        Value::Null => None,
                        v => Some(v.to_string()),
                    })
                    .collect()
            })
            .collect())
    }
}

fn s(v: &[Option<String>], i: usize) -> String {
    v.get(i).cloned().flatten().unwrap_or_default()
}

fn split(v: &str) -> Vec<String> {
    v.split(',')
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

struct MysqlCancel {
    opts: Opts,
    id: u32,
}

impl CancelHandle for MysqlCancel {
    fn cancel(&self) -> Result<()> {
        let mut c = Conn::new(self.opts.clone())?;
        c.query_drop(format!("KILL QUERY {}", self.id))?;
        Ok(())
    }
}

impl Backend for MysqlBackend {
    fn kind(&self) -> Kind {
        self.kind
    }
    fn access(&self) -> &Access {
        &self.access
    }
    fn list_databases(&mut self) -> Result<Vec<String>> {
        Ok(self
            .rows(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA ORDER BY SCHEMA_NAME",
                Params::Empty,
            )?
            .iter()
            .map(|r| s(r, 0))
            .collect())
    }
    fn list_schemas(&mut self) -> Result<Vec<String>> {
        self.list_databases()
    }
    fn list_tables(&mut self, schema: Option<&str>) -> Result<Vec<TableInfo>> {
        let rows = self.rows(
            "SELECT TABLE_NAME, TABLE_TYPE, TABLE_SCHEMA FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) ORDER BY TABLE_NAME",
            Params::Positional(vec![schema.into()]),
        )?;
        Ok(rows
            .iter()
            .map(|r| TableInfo {
                schema: Some(s(r, 2)),
                name: s(r, 0),
                kind: if s(r, 1).contains("VIEW") {
                    ObjectKind::View
                } else {
                    ObjectKind::Table
                },
            })
            .collect())
    }
    fn describe(&mut self, t: &TableRef) -> Result<TableDesc> {
        let p = || Params::Positional(vec![t.schema.as_deref().into(), t.name.as_str().into()]);
        let cols = self.rows(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY \
             FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) \
             AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
            p(),
        )?;
        let idx = self.rows(
            "SELECT INDEX_NAME, MAX(NON_UNIQUE), GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',') \
             FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) \
             AND TABLE_NAME = ? GROUP BY INDEX_NAME ORDER BY INDEX_NAME",
            p(),
        )?;
        let fks = self.rows(
            "SELECT CONSTRAINT_NAME, GROUP_CONCAT(COLUMN_NAME ORDER BY ORDINAL_POSITION SEPARATOR ','), \
             CONCAT(MAX(REFERENCED_TABLE_SCHEMA), '.', MAX(REFERENCED_TABLE_NAME)), \
             GROUP_CONCAT(REFERENCED_COLUMN_NAME ORDER BY ORDINAL_POSITION SEPARATOR ',') \
             FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) \
             AND TABLE_NAME = ? AND REFERENCED_TABLE_NAME IS NOT NULL GROUP BY CONSTRAINT_NAME",
            p(),
        )?;
        if cols.is_empty() {
            return Err(Error::InvalidArgument(format!("no such table: {}", t.name)));
        }
        Ok(TableDesc {
            columns: cols
                .iter()
                .map(|r| ColumnInfo {
                    name: s(r, 0),
                    data_type: s(r, 1),
                    nullable: s(r, 2) == "YES",
                    default: r.get(3).cloned().flatten(),
                    primary_key: s(r, 4) == "PRI",
                })
                .collect(),
            indexes: idx
                .iter()
                .map(|r| IndexInfo {
                    name: s(r, 0),
                    unique: s(r, 1) == "0",
                    columns: split(&s(r, 2)),
                })
                .collect(),
            foreign_keys: fks
                .iter()
                .map(|r| ForeignKey {
                    name: Some(s(r, 0)),
                    columns: split(&s(r, 1)),
                    ref_table: s(r, 2),
                    ref_columns: split(&s(r, 3)),
                })
                .collect(),
        })
    }
    fn query(&mut self, sql: &str, max_rows: usize) -> Result<ResultSet> {
        guard::check(sql, self.kind, self.access.is_read_only())?;
        self.run(sql, Params::Empty, max_rows)
    }
    fn query_stream(&mut self, sql: &str, sink: &mut dyn RowSink) -> Result<u64> {
        guard::check(sql, self.kind, true)?;
        let mut tx = self
            .conn
            .start_transaction(TxOpts::default().set_access_mode(Some(AccessMode::ReadOnly)))?;
        let r = stream(&mut tx, sql, Params::Empty, sink);
        tx.rollback()?;
        Ok(r?.1)
    }
    fn fetch_page(&mut self, req: &PageRequest) -> Result<ResultSet> {
        let (sql, has_param) = page_sql(self.kind, req, "?")?;
        let params = match (&req.filter, has_param) {
            (Some((_, v)), true) => Params::Positional(vec![v.as_str().into()]),
            _ => Params::Empty,
        };
        self.run(&sql, params, req.limit as usize)
    }
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        Some(Box::new(MysqlCancel {
            opts: self.opts.clone(),
            id: self.conn.connection_id(),
        }))
    }
    fn dolt_branches(&mut self) -> Result<Vec<DoltBranch>> {
        if self.kind != Kind::Dolt {
            return Err(Error::Unsupported("branches (Dolt only)"));
        }
        Ok(self
            .rows(dolt::BRANCHES_SQL, Params::Empty)?
            .iter()
            .map(|r| DoltBranch {
                name: s(r, 0),
                hash: s(r, 1),
                latest_commit_message: s(r, 2),
            })
            .collect())
    }
    fn dolt_log(&mut self, limit: u32, at: Option<&str>) -> Result<Vec<DoltCommit>> {
        if self.kind != Kind::Dolt {
            return Err(Error::Unsupported("history (Dolt only)"));
        }
        let sql = dolt::log_sql(at)?;
        Ok(self
            .rows(&sql, Params::Positional(vec![limit.into()]))?
            .iter()
            .map(|r| DoltCommit {
                hash: s(r, 0),
                committer: s(r, 1),
                date: s(r, 2),
                message: s(r, 3),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opts_policy() {
        let mut cfg = ConnectionConfig::sqlite("m", "");
        cfg.kind = Kind::Mysql;
        cfg.path = None;
        cfg.host = Some("db.example".into());
        cfg.user = Some("u".into());
        let o = build_opts(&cfg, Some("pw"), true);
        assert_eq!(
            o.get_connect_attrs()
                .and_then(|a| a.get("program_name"))
                .map(String::as_str),
            Some(saysdf_identity::APP_NAME)
        );
        assert!(o.get_ssl_opts().is_some(), "TLS on by default");
        assert!(o.get_init().iter().any(|s| s.contains("READ ONLY")));
        let w = build_opts(&cfg, None, false);
        assert!(w.get_init().is_empty());
        cfg.tls = TlsMode::Disable;
        assert!(build_opts(&cfg, None, true).get_ssl_opts().is_none());
    }
}
