//! Export to CSV (RFC 4180 via `csv`) and JSON (array of objects), streamed row by row.
//! NULL: empty CSV field / JSON `null`; BLOB: `0x..` hex in CSV, `{"$hex": ".."}` in JSON.
//!
//! [`StreamWriter`] is a [`RowSink`]: the whole table ([`export_table`], page by page) or
//! the whole query result ([`export_query`], one read-only statement) goes to the file
//! without being held in memory.

use std::io::Write;

use crate::{Backend, Error, PageRequest, Result, ResultSet, RowSink, Value};

/// Rows per page when exporting a whole table.
pub const EXPORT_PAGE: u64 = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
}

impl Format {
    /// `.json` → JSON, anything else → CSV.
    pub fn from_path(p: &std::path::Path) -> Self {
        if p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            Format::Json
        } else {
            Format::Csv
        }
    }
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

fn json_value(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Null => J::Null,
        Value::Int(i) => J::from(*i),
        Value::UInt(u) => J::from(*u),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or_else(|| J::String(f.to_string())),
        Value::Text(s) => J::String(s.clone()),
        Value::Bytes(b) => {
            let mut m = serde_json::Map::new();
            m.insert("$hex".into(), J::String(hex(b)));
            J::Object(m)
        }
    }
}

enum Out<W: Write> {
    Csv(Box<csv::Writer<W>>),
    Json(W),
}

/// Incremental CSV/JSON writer. Call [`StreamWriter::finish`] at the end (JSON closes `]`).
pub struct StreamWriter<W: Write> {
    out: Out<W>,
    columns: Vec<String>,
    header_done: bool,
    rows: u64,
}

fn io_err(e: impl std::fmt::Display) -> Error {
    Error::Driver(format!("export: {e}"))
}

impl<W: Write> StreamWriter<W> {
    pub fn new(format: Format, out: W) -> Self {
        Self {
            out: match format {
                Format::Csv => Out::Csv(Box::new(csv::Writer::from_writer(out))),
                Format::Json => Out::Json(out),
            },
            columns: vec![],
            header_done: false,
            rows: 0,
        }
    }

    pub fn rows(&self) -> u64 {
        self.rows
    }

    fn header(&mut self) -> Result<()> {
        if self.header_done {
            return Ok(());
        }
        self.header_done = true;
        match &mut self.out {
            Out::Csv(w) => w.write_record(&self.columns).map_err(io_err),
            Out::Json(w) => w.write_all(b"[").map_err(io_err),
        }
    }

    pub fn finish(mut self) -> Result<()> {
        self.header()?;
        match &mut self.out {
            Out::Csv(w) => w.flush().map_err(io_err),
            Out::Json(w) => {
                let end: &[u8] = if self.rows == 0 { b"]\n" } else { b"\n]\n" };
                w.write_all(end).map_err(io_err)?;
                w.flush().map_err(io_err)
            }
        }
    }
}

impl<W: Write> RowSink for StreamWriter<W> {
    fn columns(&mut self, columns: &[String]) -> Result<()> {
        if !self.header_done {
            self.columns = columns.to_vec();
        }
        Ok(())
    }

    fn row(&mut self, row: Vec<Value>) -> Result<bool> {
        self.header()?;
        match &mut self.out {
            Out::Csv(w) => w
                .write_record(row.iter().map(|v| match v {
                    Value::Null => String::new(),
                    other => other.to_string(),
                }))
                .map_err(io_err)?,
            Out::Json(w) => {
                let mut m = serde_json::Map::new();
                for (c, v) in self.columns.iter().zip(&row) {
                    m.insert(c.clone(), json_value(v));
                }
                let sep: &[u8] = if self.rows == 0 { b"\n  " } else { b",\n  " };
                w.write_all(sep).map_err(io_err)?;
                serde_json::to_writer(&mut *w, &serde_json::Value::Object(m)).map_err(io_err)?;
            }
        }
        self.rows += 1;
        Ok(true)
    }
}

fn write_rs<W: Write>(rs: &ResultSet, format: Format, out: W) -> Result<()> {
    let mut w = StreamWriter::new(format, out);
    w.columns(&rs.columns)?;
    for row in &rs.rows {
        w.row(row.clone())?;
    }
    w.finish()
}

/// An already loaded result (`--print`).
pub fn to_csv<W: Write>(rs: &ResultSet, out: W) -> Result<()> {
    write_rs(rs, Format::Csv, out)
}

pub fn to_json<W: Write>(rs: &ResultSet, out: W) -> Result<()> {
    write_rs(rs, Format::Json, out)
}

/// The whole table (sort, filter and `AS OF` from `req`; its offset/limit are ignored),
/// fetched page by page. Returns the number of rows written.
pub fn export_table<W: Write>(
    be: &mut dyn Backend,
    req: &PageRequest,
    format: Format,
    out: W,
) -> Result<u64> {
    let mut w = StreamWriter::new(format, out);
    let mut page = PageRequest {
        offset: 0,
        limit: EXPORT_PAGE,
        ..req.clone()
    };
    loop {
        let rs = be.fetch_page(&page)?;
        w.columns(&rs.columns)?;
        let n = rs.rows.len() as u64;
        for row in rs.rows {
            w.row(row)?;
        }
        if n < page.limit {
            break;
        }
        page.offset += page.limit;
    }
    let rows = w.rows();
    w.finish()?;
    Ok(rows)
}

/// All rows of one query, re-run read-only ([`Backend::query_stream`]).
pub fn export_query<W: Write>(
    be: &mut dyn Backend,
    sql: &str,
    format: Format,
    out: W,
) -> Result<u64> {
    let mut w = StreamWriter::new(format, out);
    be.query_stream(sql, &mut w)?;
    let rows = w.rows();
    w.finish()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteBackend;
    use crate::{Access, TableRef};

    fn rs() -> ResultSet {
        ResultSet {
            columns: vec!["id".into(), "text".into(), "blob".into()],
            rows: vec![
                vec![
                    Value::Int(1),
                    Value::Text("a, \"quoted\"\nline".into()),
                    Value::Bytes(vec![0, 255]),
                ],
                vec![Value::Int(2), Value::Null, Value::Null],
            ],
            ..Default::default()
        }
    }

    #[test]
    fn csv_quotes_commas_null() {
        let mut out = vec![];
        to_csv(&rs(), &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(
            s,
            "id,text,blob\n1,\"a, \"\"quoted\"\"\nline\",0x00ff\n2,,\n"
        );
    }

    #[test]
    fn json_null_and_blob() {
        let mut out = vec![];
        to_json(&rs(), &mut out).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v[0]["text"], "a, \"quoted\"\nline");
        assert_eq!(v[0]["blob"]["$hex"], "00ff");
        assert!(v[1]["text"].is_null());
        assert_eq!(v[1]["id"], 2);
        let mut empty = vec![];
        to_json(&ResultSet::default(), &mut empty).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&empty).unwrap();
        assert_eq!(v, serde_json::json!([]));
    }

    /// More rows than one export page: table and query export write all of them.
    #[test]
    fn whole_table_and_query_streamed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.db");
        let n = EXPORT_PAGE * 2 + 7;
        {
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute_batch(&format!(
                "CREATE TABLE t (id INTEGER PRIMARY KEY, s TEXT);
                 WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM c WHERE i < {n})
                 INSERT INTO t SELECT i, 'r' || i FROM c;"
            ))
            .unwrap();
        }
        let mut be = SqliteBackend::open_path(path.to_str().unwrap(), Access::ReadOnly).unwrap();
        let req = PageRequest {
            table: TableRef {
                schema: None,
                name: "t".into(),
            },
            order_by: Some(("id".into(), crate::SortDir::Asc)),
            filter: None,
            offset: 400,
            limit: 200,
            as_of: None,
        };
        let mut csv_out = vec![];
        let got = export_table(&mut be, &req, Format::Csv, &mut csv_out).unwrap();
        assert_eq!(got, n);
        let s = String::from_utf8(csv_out).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len() as u64, n + 1);
        assert_eq!(lines[0], "id,s");
        assert_eq!(lines[1], "1,r1");
        assert_eq!(*lines.last().unwrap(), format!("{n},r{n}"));

        let mut json_out = vec![];
        let sql = "SELECT id FROM t WHERE id % 2 = 0";
        let got = export_query(&mut be, sql, Format::Json, &mut json_out).unwrap();
        assert_eq!(got, n / 2);
        let v: serde_json::Value = serde_json::from_slice(&json_out).unwrap();
        assert_eq!(v.as_array().unwrap().len() as u64, n / 2);
        assert_eq!(v[0]["id"], 2);

        assert!(export_query(&mut be, "DELETE FROM t", Format::Csv, &mut vec![]).is_err());
    }
}
