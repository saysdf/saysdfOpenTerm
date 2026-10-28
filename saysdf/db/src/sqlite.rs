//! SQLite backend. Read-only: `SQLITE_OPEN_READ_ONLY`, `PRAGMA query_only=ON`,
//! `SQLITE_LIMIT_ATTACHED=0`; extension loading is not compiled in (rusqlite feature
//! `load_extension` is off).

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

use crate::{
    guard, page_sql, quote_ident, Access, Backend, Collector, ColumnInfo, ConnectionConfig, Error,
    ForeignKey, IndexInfo, Kind, ObjectKind, PageRequest, Result, ResultSet, RowSink, TableDesc,
    TableInfo, TableRef, Value,
};

pub struct SqliteBackend {
    conn: Connection,
    access: Access,
}

fn to_value(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Int(i),
        ValueRef::Real(f) => Value::Float(f),
        ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::Bytes(b.to_vec()),
    }
}

impl SqliteBackend {
    pub fn open(cfg: &ConnectionConfig, access: Access) -> Result<Self> {
        let path = cfg
            .path
            .as_deref()
            .ok_or_else(|| Error::InvalidArgument("sqlite connection needs a path".into()))?;
        Self::open_path(path, access)
    }

    pub fn open_path(path: &str, access: Access) -> Result<Self> {
        let base = OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let flags = if access.is_read_only() {
            base | OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            // no CREATE: write mode never creates a new file by accident
            base | OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        let conn = Connection::open_with_flags(path, flags)?;
        if access.is_read_only() {
            conn.pragma_update(None, "query_only", "ON")?;
            // SAFETY: valid open handle owned by `conn`; sqlite3_limit only adjusts a limit.
            unsafe {
                rusqlite::ffi::sqlite3_limit(
                    conn.handle(),
                    rusqlite::ffi::SQLITE_LIMIT_ATTACHED,
                    0,
                );
            }
        }
        Ok(Self { conn, access })
    }

    fn run(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        max_rows: usize,
    ) -> Result<ResultSet> {
        let mut c = Collector::new(max_rows);
        let (affected, _) = self.run_sink(sql, params, &mut c, self.access.is_read_only())?;
        c.rs.affected = affected;
        Ok(c.rs)
    }

    /// Streams rows into `sink`; returns (rows affected for non-SELECT, rows sent).
    fn run_sink(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        sink: &mut dyn RowSink,
        read_only: bool,
    ) -> Result<(Option<u64>, u64)> {
        let mut stmt = self.conn.prepare(sql)?;
        if read_only && !stmt.readonly() {
            return Err(Error::Denied("statement writes to the database".into()));
        }
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let ncol = columns.len();
        sink.columns(&columns)?;
        if ncol == 0 {
            return Ok((Some(stmt.execute(params)? as u64), 0));
        }
        let mut rows = stmt.query(params)?;
        let mut n = 0u64;
        while let Some(row) = rows.next()? {
            let mut r = Vec::with_capacity(ncol);
            for i in 0..ncol {
                r.push(to_value(row.get_ref(i)?));
            }
            if !sink.row(r)? {
                break;
            }
            n += 1;
        }
        Ok((None, n))
    }

    fn strings(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(sql)?;
        let v = stmt
            .query_map(params, |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }
}

impl Backend for SqliteBackend {
    fn kind(&self) -> Kind {
        Kind::Sqlite
    }
    fn access(&self) -> &Access {
        &self.access
    }
    fn list_databases(&mut self) -> Result<Vec<String>> {
        self.strings("SELECT name FROM pragma_database_list ORDER BY seq", &[])
    }
    fn list_schemas(&mut self) -> Result<Vec<String>> {
        self.list_databases()
    }
    fn list_tables(&mut self, schema: Option<&str>) -> Result<Vec<TableInfo>> {
        let schema = schema.unwrap_or("main");
        let sql = format!(
            "SELECT name, type FROM {}.sqlite_schema WHERE type IN ('table','view') \
             AND name NOT LIKE 'sqlite_%' ORDER BY name",
            quote_ident(Kind::Sqlite, schema)
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let v = stmt
            .query_map([], |r| {
                let kind: String = r.get(1)?;
                Ok(TableInfo {
                    schema: Some(schema.to_string()),
                    name: r.get(0)?,
                    kind: if kind == "view" {
                        ObjectKind::View
                    } else {
                        ObjectKind::Table
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }
    fn describe(&mut self, t: &TableRef) -> Result<TableDesc> {
        let schema = t.schema.as_deref().unwrap_or("main");
        let mut d = TableDesc::default();
        {
            let mut stmt = self.conn.prepare(
                "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1, ?2) ORDER BY cid",
            )?;
            d.columns = stmt
                .query_map([&t.name, schema], |r| {
                    Ok(ColumnInfo {
                        name: r.get(0)?,
                        data_type: r.get(1)?,
                        nullable: r.get::<_, i64>(2)? == 0,
                        default: r.get(3)?,
                        primary_key: r.get::<_, i64>(4)? > 0,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }
        {
            let mut stmt = self
                .conn
                .prepare("SELECT name, \"unique\" FROM pragma_index_list(?1, ?2) ORDER BY seq")?;
            let idx = stmt
                .query_map([&t.name, schema], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (name, unique) in idx {
                let cols = self.strings(
                    "SELECT coalesce(name, '<expr>') FROM pragma_index_info(?1, ?2) ORDER BY seqno",
                    &[&name, &schema],
                )?;
                d.indexes.push(IndexInfo {
                    name,
                    columns: cols,
                    unique,
                });
            }
        }
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, \"table\", \"from\", coalesce(\"to\", '') FROM pragma_foreign_key_list(?1, ?2) ORDER BY id, seq",
            )?;
            let rows = stmt
                .query_map([&t.name, schema], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut last_id = None;
            for (id, table, from, to) in rows {
                if last_id != Some(id) {
                    d.foreign_keys.push(ForeignKey {
                        name: None,
                        columns: vec![],
                        ref_table: table,
                        ref_columns: vec![],
                    });
                    last_id = Some(id);
                }
                if let Some(fk) = d.foreign_keys.last_mut() {
                    fk.columns.push(from);
                    fk.ref_columns.push(to);
                }
            }
        }
        if d.columns.is_empty() {
            return Err(Error::InvalidArgument(format!("no such table: {}", t.name)));
        }
        Ok(d)
    }
    fn query(&mut self, sql: &str, max_rows: usize) -> Result<ResultSet> {
        guard::check(sql, Kind::Sqlite, self.access.is_read_only())?;
        self.run(sql, &[], max_rows)
    }
    fn query_stream(&mut self, sql: &str, sink: &mut dyn RowSink) -> Result<u64> {
        guard::check(sql, Kind::Sqlite, true)?;
        Ok(self.run_sink(sql, &[], sink, true)?.1)
    }
    fn fetch_page(&mut self, req: &PageRequest) -> Result<ResultSet> {
        let (sql, has_param) = page_sql(Kind::Sqlite, req, "?1")?;
        let limit = req.limit as usize;
        match (&req.filter, has_param) {
            (Some((_, v)), true) => self.run(&sql, &[v], limit),
            _ => self.run(&sql, &[], limit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writemode::{ButtonActivated, WriteMode};
    use crate::SortDir;

    fn fixture() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE a (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
             CREATE TABLE b (id INTEGER PRIMARY KEY, a_id INTEGER REFERENCES a(id), note TEXT DEFAULT 'x');
             CREATE INDEX b_a ON b(a_id);
             CREATE VIEW v AS SELECT * FROM a;
             INSERT INTO a(name) VALUES ('one'),('two'),('three'),('four'),('five');",
        )
        .unwrap();
        (dir, path.to_string_lossy().into_owned())
    }

    fn file_bytes(p: &str) -> Vec<u8> {
        std::fs::read(p).unwrap()
    }

    #[test]
    fn read_only_rejects_writes_and_file_unchanged() {
        let (dir, path) = fixture();
        let before = file_bytes(&path);
        let mut b = SqliteBackend::open_path(&path, Access::ReadOnly).unwrap();
        let attach = format!(
            "ATTACH DATABASE '{}' AS x",
            dir.path().join("x.db").to_string_lossy()
        );
        for sql in [
            "INSERT INTO a(name) VALUES ('evil')",
            "CREATE TABLE c (x)",
            "PRAGMA query_only=OFF",
            attach.as_str(),
            "DELETE FROM a",
            "VACUUM INTO 'copy.db'",
        ] {
            assert!(b.query(sql, 10).is_err(), "{sql} must fail");
        }
        // bypassing the guard: the database itself refuses
        assert!(b
            .run("INSERT INTO a(name) VALUES ('evil')", &[], 10)
            .is_err());
        assert!(b
            .conn
            .execute("INSERT INTO a(name) VALUES ('evil')", [])
            .is_err());
        assert!(b
            .conn
            .execute_batch("PRAGMA query_only=OFF; INSERT INTO a(name) VALUES ('e')")
            .is_err());
        assert!(b.conn.execute_batch(&attach).is_err());
        drop(b);
        assert_eq!(before, file_bytes(&path));
        assert!(!dir.path().join("x.db").exists());
    }

    #[test]
    fn write_mode_insert_passes() {
        let (_dir, path) = fixture();
        let cfg = ConnectionConfig::sqlite("t", &path);
        let mut m = WriteMode::default();
        m.request_write(ButtonActivated::WriteButton, &cfg.database_name());
        let grant = m.confirm("t.db", "test").unwrap();
        let mut b = crate::open(&cfg, None, Access::Write(grant)).unwrap();
        let rs = b.query("INSERT INTO a(name) VALUES ('six')", 10).unwrap();
        assert_eq!(rs.affected, Some(1));
        let rs = b.query("SELECT count(*) FROM a", 10).unwrap();
        assert_eq!(rs.rows[0][0], Value::Int(6));
        // still one statement only
        assert!(b
            .query("INSERT INTO a(name) VALUES ('7'); DELETE FROM a", 10)
            .is_err());
    }

    #[test]
    fn grant_for_other_database_rejected() {
        let (_dir, path) = fixture();
        let mut m = WriteMode::default();
        m.request_write(ButtonActivated::WriteButton, "other.db");
        let grant = m.confirm("other.db", "").unwrap();
        let cfg = ConnectionConfig::sqlite("t", &path);
        assert!(crate::open(&cfg, None, Access::Write(grant)).is_err());
    }

    #[test]
    fn introspection() {
        let (_dir, path) = fixture();
        let mut b = SqliteBackend::open_path(&path, Access::ReadOnly).unwrap();
        assert_eq!(b.list_databases().unwrap(), vec!["main".to_string()]);
        let tables = b.list_tables(None).unwrap();
        let names: Vec<_> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert_eq!(
            names,
            vec![
                ("a", ObjectKind::Table),
                ("b", ObjectKind::Table),
                ("v", ObjectKind::View)
            ]
        );
        let d = b
            .describe(&TableRef {
                schema: None,
                name: "b".into(),
            })
            .unwrap();
        assert_eq!(d.columns.len(), 3);
        assert!(d.columns[0].primary_key);
        assert_eq!(d.columns[2].default.as_deref(), Some("'x'"));
        assert_eq!(d.indexes[0].name, "b_a");
        assert_eq!(d.indexes[0].columns, vec!["a_id".to_string()]);
        assert_eq!(d.foreign_keys.len(), 1);
        assert_eq!(d.foreign_keys[0].ref_table, "a");
        assert_eq!(d.foreign_keys[0].columns, vec!["a_id".to_string()]);
        assert_eq!(d.foreign_keys[0].ref_columns, vec!["id".to_string()]);
        let da = b
            .describe(&TableRef {
                schema: Some("main".into()),
                name: "a".into(),
            })
            .unwrap();
        assert!(da.indexes.iter().any(|i| i.unique));
        assert!(b
            .describe(&TableRef {
                schema: None,
                name: "nope".into()
            })
            .is_err());
    }

    #[test]
    fn pages_do_not_overlap_and_query_limit() {
        let (_dir, path) = fixture();
        let mut b = SqliteBackend::open_path(&path, Access::ReadOnly).unwrap();
        let mut req = PageRequest {
            table: TableRef {
                schema: None,
                name: "a".into(),
            },
            order_by: Some(("id".into(), SortDir::Asc)),
            filter: None,
            offset: 0,
            limit: 2,
            as_of: None,
        };
        let mut seen = vec![];
        loop {
            let rs = b.fetch_page(&req).unwrap();
            if rs.rows.is_empty() {
                break;
            }
            for r in rs.rows {
                assert!(!seen.contains(&r[0]));
                seen.push(r[0].clone());
            }
            req.offset += req.limit;
        }
        assert_eq!(seen.len(), 5);
        req.offset = 0;
        req.filter = Some(("name".into(), "t%".into()));
        req.order_by = Some(("name".into(), SortDir::Desc));
        let rs = b.fetch_page(&req).unwrap();
        assert_eq!(rs.rows.len(), 2);
        assert_eq!(rs.rows[0][1], Value::Text("two".into()));
        // injection attempt in the filter value is just a parameter
        req.filter = Some(("name".into(), "'; DROP TABLE a; --".into()));
        assert!(b.fetch_page(&req).unwrap().rows.is_empty());

        let rs = b.query("SELECT * FROM a", 3).unwrap();
        assert_eq!(rs.rows.len(), 3);
        assert!(rs.truncated);
        assert_eq!(rs.columns, vec!["id".to_string(), "name".to_string()]);
        assert!(matches!(
            b.query("SELECT 1; SELECT 2", 10),
            Err(Error::MultipleStatements)
        ));
    }
}
