//! saysdfTerm database backends for `saysdf db` (change `database-viewer`).
//!
//! Read-only by default, enforced in several layers (design D4):
//! 1. connection flag (SQLite `SQLITE_OPEN_READ_ONLY` + `query_only` + no ATTACH,
//!    Postgres `default_transaction_read_only=on`, MySQL `SET SESSION TRANSACTION READ ONLY`);
//! 2. every user statement in a `READ ONLY` transaction followed by `ROLLBACK` (network DBs);
//! 3. exactly one statement per query (prepared / extended protocol, checked by [`guard`]);
//! 4. deny-list of mode-changing statements ([`guard::check`]).
//!
//! Write access needs a [`writemode::WriteGrant`], obtainable only from
//! [`writemode::WriteMode::Write`] (button event + typed database name).

pub mod dolt;
pub mod export;
pub mod guard;
pub mod model;
pub mod mysql_backend;
pub mod pg;
pub mod pg_ssh;
pub mod sqlite;
pub mod writemode;

use std::fmt;

pub use model::{ConnectionConfig, Kind, TlsMode};
pub use writemode::{WriteGrant, WriteMode};

/// Rows per query when the caller does not say otherwise.
pub const DEFAULT_MAX_ROWS: usize = 1000;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("NULL"),
            Value::Int(v) => write!(f, "{v}"),
            Value::UInt(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Text(s) => f.write_str(s),
            Value::Bytes(b) => {
                f.write_str("0x")?;
                for x in b {
                    write!(f, "{x:02x}")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// More rows existed than the limit allowed.
    pub truncated: bool,
    /// Rows affected (write mode, non-SELECT statements), if the driver reports it.
    pub affected: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Table,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub schema: Option<String>,
    pub name: String,
    pub kind: ObjectKind,
}

/// A table in a schema (`schema` = SQLite database name / PG schema / MySQL database).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableDesc {
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// One page of a table: `ORDER BY` + `LIMIT/OFFSET`, optional `column LIKE ?` filter
/// (the value is always a bound parameter, identifiers are quoted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    pub table: TableRef,
    pub order_by: Option<(String, SortDir)>,
    pub filter: Option<(String, String)>,
    pub offset: u64,
    pub limit: u64,
    /// Dolt only: commit/branch/tag (validated by [`dolt::validate_ref`]).
    pub as_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoltBranch {
    pub name: String,
    pub hash: String,
    pub latest_commit_message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoltCommit {
    pub hash: String,
    pub committer: String,
    pub date: String,
    pub message: String,
}

/// Errors carry driver messages, never the password (connection strings are not formatted
/// into them).
#[derive(Debug)]
pub enum Error {
    /// Rejected before reaching the database ([`guard`]).
    Denied(String),
    /// More than one statement in a query.
    MultipleStatements,
    Unsupported(&'static str),
    InvalidArgument(String),
    Driver(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Denied(s) => write!(f, "denied in read-only mode: {s}"),
            Error::MultipleStatements => f.write_str("only one statement per query is allowed"),
            Error::Unsupported(s) => write!(f, "not supported: {s}"),
            Error::InvalidArgument(s) => write!(f, "invalid argument: {s}"),
            Error::Driver(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Driver(e.to_string())
    }
}
impl From<postgres::Error> for Error {
    fn from(e: postgres::Error) -> Self {
        Error::Driver(e.to_string())
    }
}
impl From<mysql::Error> for Error {
    fn from(e: mysql::Error) -> Self {
        Error::Driver(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Access level of an open connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    Write(WriteGrant),
}

impl Access {
    pub fn is_read_only(&self) -> bool {
        matches!(self, Access::ReadOnly)
    }
}

/// Receives rows one by one (streaming export, design D8): nothing is buffered beyond one row.
pub trait RowSink {
    fn columns(&mut self, columns: &[String]) -> Result<()>;
    /// `Ok(false)` stops reading (the rest is drained by the driver).
    fn row(&mut self, row: Vec<Value>) -> Result<bool>;
}

/// [`RowSink`] that builds a [`ResultSet`] with at most `max_rows` rows.
pub(crate) struct Collector {
    pub rs: ResultSet,
    pub max_rows: usize,
}

impl Collector {
    pub fn new(max_rows: usize) -> Self {
        Self {
            rs: ResultSet::default(),
            max_rows,
        }
    }
}

impl RowSink for Collector {
    fn columns(&mut self, columns: &[String]) -> Result<()> {
        self.rs.columns = columns.to_vec();
        Ok(())
    }
    fn row(&mut self, row: Vec<Value>) -> Result<bool> {
        if self.rs.rows.len() >= self.max_rows {
            self.rs.truncated = true;
            return Ok(false);
        }
        self.rs.rows.push(row);
        Ok(true)
    }
}

/// Cancels the query running on another thread.
pub trait CancelHandle: Send + Sync {
    fn cancel(&self) -> Result<()>;
}

/// Common driver interface. Methods take `&mut self`: one connection, one worker thread.
pub trait Backend: Send {
    fn kind(&self) -> Kind;
    fn access(&self) -> &Access;
    fn list_databases(&mut self) -> Result<Vec<String>>;
    fn list_schemas(&mut self) -> Result<Vec<String>>;
    fn list_tables(&mut self, schema: Option<&str>) -> Result<Vec<TableInfo>>;
    fn describe(&mut self, table: &TableRef) -> Result<TableDesc>;
    /// One user statement, at most `max_rows` rows returned.
    fn query(&mut self, sql: &str, max_rows: usize) -> Result<ResultSet>;
    fn fetch_page(&mut self, req: &PageRequest) -> Result<ResultSet>;
    /// One user statement, all rows streamed into `sink`. Always read-only, whatever the
    /// connection access: export re-runs the query and must never repeat a write.
    /// Returns the number of rows passed to the sink.
    fn query_stream(&mut self, sql: &str, sink: &mut dyn RowSink) -> Result<u64>;
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        None
    }
    fn dolt_branches(&mut self) -> Result<Vec<DoltBranch>> {
        Err(Error::Unsupported("branches (Dolt only)"))
    }
    /// History of the current branch, or of `at` (branch/tag/commit, `AS OF`).
    fn dolt_log(&mut self, _limit: u32, _at: Option<&str>) -> Result<Vec<DoltCommit>> {
        Err(Error::Unsupported("history (Dolt only)"))
    }
}

/// Opens a connection. `password` comes from `saysdf-secrets` or a masked prompt and is
/// never stored in [`ConnectionConfig`].
pub fn open(
    cfg: &ConnectionConfig,
    password: Option<&str>,
    access: Access,
) -> Result<Box<dyn Backend>> {
    if let Access::Write(grant) = &access {
        if grant.database() != cfg.database_name() {
            return Err(Error::InvalidArgument(
                "write grant was issued for another database".into(),
            ));
        }
    }
    Ok(match cfg.kind {
        Kind::Sqlite => Box::new(sqlite::SqliteBackend::open(cfg, access)?),
        Kind::Postgres => Box::new(pg::PgBackend::open(cfg, password, access)?),
        Kind::Mysql | Kind::Dolt => {
            Box::new(mysql_backend::MysqlBackend::open(cfg, password, access)?)
        }
        // no password: server-side socket auth (see `pg_ssh`)
        Kind::PgSsh => Box::new(pg_ssh::PgSshBackend::open(cfg, access)?),
    })
}

/// Identifier quoting: `"x"` (SQLite, Postgres) or `` `x` `` (MySQL, Dolt).
pub(crate) fn quote_ident(kind: Kind, ident: &str) -> String {
    let q = match kind {
        Kind::Mysql | Kind::Dolt => '`',
        _ => '"',
    };
    let mut s = String::with_capacity(ident.len() + 2);
    s.push(q);
    for c in ident.chars() {
        if c == q {
            s.push(q);
        }
        s.push(c);
    }
    s.push(q);
    s
}

pub(crate) fn qualified(kind: Kind, t: &TableRef) -> String {
    match &t.schema {
        Some(s) => format!("{}.{}", quote_ident(kind, s), quote_ident(kind, &t.name)),
        None => quote_ident(kind, &t.name),
    }
}

/// `SELECT * FROM t [AS OF 'r'] [WHERE c LIKE <p>] [ORDER BY c dir] LIMIT n OFFSET m`,
/// `placeholder` is `?` or `$1`. Returns SQL and whether the filter parameter is used.
pub(crate) fn page_sql(kind: Kind, req: &PageRequest, placeholder: &str) -> Result<(String, bool)> {
    if req.limit == 0 || req.limit > 100_000 {
        return Err(Error::InvalidArgument(
            "page size must be 1..=100000".into(),
        ));
    }
    let mut sql = format!("SELECT * FROM {}", qualified(kind, &req.table));
    if let Some(r) = &req.as_of {
        if kind != Kind::Dolt {
            return Err(Error::Unsupported("AS OF (Dolt only)"));
        }
        sql.push_str(&dolt::as_of_clause(r)?);
    }
    let mut has_param = false;
    if let Some((col, _)) = &req.filter {
        let col = quote_ident(kind, col);
        if kind == Kind::Postgres {
            sql.push_str(&format!(" WHERE CAST({col} AS TEXT) LIKE {placeholder}"));
        } else {
            sql.push_str(&format!(" WHERE {col} LIKE {placeholder}"));
        }
        has_param = true;
    }
    if let Some((col, dir)) = &req.order_by {
        let d = match dir {
            SortDir::Asc => "ASC",
            SortDir::Desc => "DESC",
        };
        sql.push_str(&format!(" ORDER BY {} {d}", quote_ident(kind, col)));
    }
    sql.push_str(&format!(" LIMIT {} OFFSET {}", req.limit, req.offset));
    Ok((sql, has_param))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes() {
        assert_eq!(quote_ident(Kind::Sqlite, "a\"b"), "\"a\"\"b\"");
        assert_eq!(quote_ident(Kind::Mysql, "a`b"), "`a``b`");
    }

    #[test]
    fn page_sql_shape() {
        let req = PageRequest {
            table: TableRef {
                schema: Some("main".into()),
                name: "t".into(),
            },
            order_by: Some(("id".into(), SortDir::Desc)),
            filter: Some(("name".into(), "%x%".into())),
            offset: 20,
            limit: 10,
            as_of: None,
        };
        let (sql, p) = page_sql(Kind::Sqlite, &req, "?").unwrap();
        assert!(p);
        assert_eq!(
            sql,
            "SELECT * FROM \"main\".\"t\" WHERE \"name\" LIKE ? ORDER BY \"id\" DESC LIMIT 10 OFFSET 20"
        );
        let mut r2 = req.clone();
        r2.as_of = Some("main".into());
        assert!(page_sql(Kind::Sqlite, &r2, "?").is_err());
    }
}
