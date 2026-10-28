//! PostgreSQL backend (sync `postgres` + `postgres-native-tls`). TLS `require` by default
//! (the crate's default is `prefer`). Read-only: `options=-c default_transaction_read_only=on`
//! and every statement in `BEGIN READ ONLY` … `ROLLBACK`; extended protocol (prepare) means
//! one statement per query. Note: a read-only transaction does not stop functions with side
//! effects (e.g. `pg_terminate_backend`) — use a role with SELECT only.

use postgres::fallible_iterator::FallibleIterator;
use postgres::types::{FromSql, ToSql, Type};
use postgres::{CancelToken, Client, Config, GenericClient, NoTls};
use postgres_native_tls::MakeTlsConnector;

use crate::{
    guard, page_sql, Access, Backend, CancelHandle, Collector, ConnectionConfig, Error, Kind,
    PageRequest, Result, ResultSet, RowSink, TableDesc, TableInfo, TableRef, TlsMode, Value,
};

pub struct PgBackend {
    client: Client,
    tls: Option<MakeTlsConnector>,
    access: Access,
}

/// Any column value, decoded from the binary format for common types.
struct Cell(Value);

fn civil(days_since_2000: i64) -> (i64, u32, u32) {
    // Howard Hinnant's civil_from_days
    let z = days_since_2000 + 10957 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn time_of_day(us: i64) -> String {
    let (s, frac) = (us.div_euclid(1_000_000), us.rem_euclid(1_000_000));
    let t = format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60);
    if frac == 0 {
        t
    } else {
        format!("{t}.{frac:06}")
    }
}

fn timestamp(us: i64) -> String {
    let days = us.div_euclid(86_400_000_000);
    let (y, m, d) = civil(days);
    format!(
        "{y:04}-{m:02}-{d:02} {}",
        time_of_day(us.rem_euclid(86_400_000_000))
    )
}

pub(crate) fn numeric(raw: &[u8]) -> Option<String> {
    let rd =
        |i: usize| -> Option<i16> { Some(i16::from_be_bytes([*raw.get(i)?, *raw.get(i + 1)?])) };
    let ndigits = rd(0)? as i64;
    let weight = rd(2)? as i64;
    let sign = rd(4)? as u16;
    let dscale = rd(6)? as usize;
    match sign {
        0xC000 => return Some("NaN".into()),
        0xD000 => return Some("Infinity".into()),
        0xF000 => return Some("-Infinity".into()),
        _ => {}
    }
    let dig = |i: i64| -> i16 {
        if (0..ndigits).contains(&i) {
            rd(8 + 2 * i as usize).unwrap_or(0)
        } else {
            0
        }
    };
    let mut s = String::new();
    if sign == 0x4000 {
        s.push('-');
    }
    if weight < 0 {
        s.push('0');
    } else {
        for i in 0..=weight {
            if i == 0 {
                s.push_str(&dig(i).to_string());
            } else {
                s.push_str(&format!("{:04}", dig(i)));
            }
        }
    }
    if dscale > 0 {
        let mut frac = String::new();
        let mut i = weight + 1;
        while frac.len() < dscale {
            frac.push_str(&format!("{:04}", dig(i)));
            i += 1;
        }
        frac.truncate(dscale);
        s.push('.');
        s.push_str(&frac);
    }
    Some(s)
}

fn decode(ty: &Type, raw: &[u8]) -> Value {
    let be8 = || {
        raw.get(..8)
            .map(|b| i64::from_be_bytes(b.try_into().unwrap_or([0; 8])))
    };
    let be4 = || {
        raw.get(..4)
            .map(|b| <[u8; 4]>::try_from(b).unwrap_or([0; 4]))
    };
    let opt =
        |v: Option<Value>| v.unwrap_or_else(|| Value::Text(format!("<invalid {}>", ty.name())));
    match *ty {
        Type::BOOL => Value::Text((raw.first() == Some(&1)).to_string()),
        Type::INT2 => opt(raw
            .get(..2)
            .map(|b| Value::Int(i16::from_be_bytes([b[0], b[1]]) as i64))),
        Type::INT4 => opt(be4().map(|b| Value::Int(i32::from_be_bytes(b) as i64))),
        Type::OID => opt(be4().map(|b| Value::UInt(u32::from_be_bytes(b) as u64))),
        Type::INT8 => opt(be8().map(Value::Int)),
        Type::FLOAT4 => opt(be4().map(|b| Value::Float(f32::from_be_bytes(b) as f64))),
        Type::FLOAT8 => opt(be8().map(|v| Value::Float(f64::from_bits(v as u64)))),
        Type::BYTEA => Value::Bytes(raw.to_vec()),
        Type::JSONB => {
            Value::Text(String::from_utf8_lossy(raw.get(1..).unwrap_or(&[])).into_owned())
        }
        Type::JSON | Type::XML => Value::Text(String::from_utf8_lossy(raw).into_owned()),
        Type::NUMERIC => opt(numeric(raw).map(Value::Text)),
        Type::DATE => opt(be4().map(|b| {
            let (y, m, d) = civil(i32::from_be_bytes(b) as i64);
            Value::Text(format!("{y:04}-{m:02}-{d:02}"))
        })),
        Type::TIMESTAMP => opt(be8().map(|v| Value::Text(timestamp(v)))),
        Type::TIMESTAMPTZ => opt(be8().map(|v| Value::Text(format!("{}+00", timestamp(v))))),
        Type::TIME => opt(be8().map(|v| Value::Text(time_of_day(v)))),
        Type::UUID if raw.len() == 16 => {
            let h: String = raw.iter().map(|b| format!("{b:02x}")).collect();
            Value::Text(format!(
                "{}-{}-{}-{}-{}",
                &h[0..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..32]
            ))
        }
        _ if <String as FromSql>::accepts(ty) => {
            Value::Text(String::from_utf8_lossy(raw).into_owned())
        }
        _ => Value::Text(format!("<{} value, {} bytes>", ty.name(), raw.len())),
    }
}

impl<'a> FromSql<'a> for Cell {
    fn from_sql(
        ty: &Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Cell(decode(ty, raw)))
    }
    fn from_sql_null(
        _ty: &Type,
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Cell(Value::Null))
    }
    fn accepts(_ty: &Type) -> bool {
        true
    }
}

type Param<'a> = &'a (dyn ToSql + Sync);

fn collect<C: GenericClient>(
    c: &mut C,
    sql: &str,
    params: &[Param<'_>],
    max_rows: usize,
) -> Result<ResultSet> {
    let mut col = Collector::new(max_rows);
    let (affected, _) = stream(c, sql, params, &mut col)?;
    col.rs.affected = affected;
    Ok(col.rs)
}

/// Streams rows into `sink`; returns (rows affected for non-SELECT, rows sent).
fn stream<C: GenericClient>(
    c: &mut C,
    sql: &str,
    params: &[Param<'_>],
    sink: &mut dyn RowSink,
) -> Result<(Option<u64>, u64)> {
    let stmt = c.prepare(sql)?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    sink.columns(&columns)?;
    if columns.is_empty() {
        return Ok((Some(c.execute(&stmt, params)?), 0));
    }
    let mut it = c.query_raw(&stmt, params.iter().copied())?;
    let mut n = 0u64;
    while let Some(row) = it.next()? {
        let mut r = Vec::with_capacity(columns.len());
        for i in 0..columns.len() {
            r.push(row.try_get::<_, Cell>(i)?.0);
        }
        if !sink.row(r)? {
            break;
        }
        n += 1;
    }
    Ok((None, n))
}

fn tls_connector(mode: TlsMode) -> Result<Option<MakeTlsConnector>> {
    let mut b = native_tls::TlsConnector::builder();
    match mode {
        TlsMode::Disable => return Ok(None),
        TlsMode::Require => {
            b.danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }
        TlsMode::VerifyFull => {}
    }
    let c = b
        .build()
        .map_err(|e| Error::Driver(format!("TLS setup: {e}")))?;
    Ok(Some(MakeTlsConnector::new(c)))
}

/// Connection parameters; the password is only handed to the driver.
pub fn build_config(cfg: &ConnectionConfig, password: Option<&str>, read_only: bool) -> Config {
    let mut c = Config::new();
    c.host(cfg.host_or_default())
        .port(cfg.port.unwrap_or(5432))
        .application_name(saysdf_identity::APP_NAME)
        .ssl_mode(match cfg.tls {
            TlsMode::Disable => postgres::config::SslMode::Disable,
            _ => postgres::config::SslMode::Require,
        });
    if let Some(u) = &cfg.user {
        c.user(u);
    }
    if let Some(p) = password {
        c.password(p);
    }
    if let Some(db) = &cfg.database {
        c.dbname(db);
    }
    if read_only {
        c.options("-c default_transaction_read_only=on");
    }
    c
}

impl PgBackend {
    pub fn open(cfg: &ConnectionConfig, password: Option<&str>, access: Access) -> Result<Self> {
        let config = build_config(cfg, password, access.is_read_only());
        let tls = tls_connector(cfg.tls)?;
        let client = match &tls {
            Some(t) => config.connect(t.clone())?,
            None => config.connect(NoTls)?,
        };
        Ok(Self {
            client,
            tls,
            access,
        })
    }

    fn run(&mut self, sql: &str, params: &[Param<'_>], max_rows: usize) -> Result<ResultSet> {
        if self.access.is_read_only() {
            let mut tx = self.client.build_transaction().read_only(true).start()?;
            let r = collect(&mut tx, sql, params, max_rows);
            tx.rollback()?;
            r
        } else {
            collect(&mut self.client, sql, params, max_rows)
        }
    }

    fn rows(&mut self, sql: &str, params: &[Param<'_>]) -> Result<intro::Rows> {
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

struct PgCancel {
    token: CancelToken,
    tls: Option<MakeTlsConnector>,
}

impl CancelHandle for PgCancel {
    fn cancel(&self) -> Result<()> {
        match &self.tls {
            Some(t) => self.token.cancel_query(t.clone())?,
            None => self.token.cancel_query(NoTls)?,
        }
        Ok(())
    }
}

impl Backend for PgBackend {
    fn kind(&self) -> Kind {
        Kind::Postgres
    }
    fn access(&self) -> &Access {
        &self.access
    }
    fn list_databases(&mut self) -> Result<Vec<String>> {
        Ok(intro::first_column(self.rows(intro::DATABASES, &[])?))
    }
    fn list_schemas(&mut self) -> Result<Vec<String>> {
        Ok(intro::first_column(self.rows(intro::SCHEMAS, &[])?))
    }
    fn list_tables(&mut self, schema: Option<&str>) -> Result<Vec<TableInfo>> {
        Ok(intro::tables(self.rows(&intro::tables_sql(), &[&schema])?))
    }
    fn describe(&mut self, t: &TableRef) -> Result<TableDesc> {
        let schema = t.schema.as_deref();
        let name = t.name.as_str();
        let cols = self.rows(&intro::columns_sql(), &[&schema, &name])?;
        let idx = self.rows(&intro::indexes_sql(), &[&schema, &name])?;
        let fks = self.rows(&intro::foreign_keys_sql(), &[&schema, &name])?;
        intro::table_desc(t, cols, idx, fks)
    }
    fn query(&mut self, sql: &str, max_rows: usize) -> Result<ResultSet> {
        guard::check(sql, Kind::Postgres, self.access.is_read_only())?;
        self.run(sql, &[], max_rows)
    }
    fn query_stream(&mut self, sql: &str, sink: &mut dyn RowSink) -> Result<u64> {
        guard::check(sql, Kind::Postgres, true)?;
        let mut tx = self.client.build_transaction().read_only(true).start()?;
        let r = stream(&mut tx, sql, &[], sink);
        tx.rollback()?;
        Ok(r?.1)
    }
    fn fetch_page(&mut self, req: &PageRequest) -> Result<ResultSet> {
        let (sql, has_param) = page_sql(Kind::Postgres, req, "$1")?;
        let limit = req.limit as usize;
        match (&req.filter, has_param) {
            (Some((_, v)), true) => self.run(&sql, &[v], limit),
            _ => self.run(&sql, &[], limit),
        }
    }
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        Some(Box::new(PgCancel {
            token: self.client.cancel_token(),
            tls: self.tls.clone(),
        }))
    }
}

/// Introspection SQL and row mapping shared by [`PgBackend`] and the `postgres-ssh` backend
/// ([`crate::pg_ssh`]): same queries, different transport. Parameters are `$1` (schema or
/// NULL = current schema) and `$2` (table name).
pub(crate) mod intro {
    use crate::{
        ColumnInfo, Error, ForeignKey, IndexInfo, ObjectKind, Result, TableDesc, TableInfo,
        TableRef,
    };

    /// Rows as optional strings (NULL = `None`).
    pub type Rows = Vec<Vec<Option<String>>>;

    const SCHEMA: &str = "COALESCE($1::text, current_schema()::text)";

    pub const DATABASES: &str =
        "SELECT datname::text FROM pg_database WHERE NOT datistemplate ORDER BY 1";
    pub const SCHEMAS: &str = "SELECT schema_name::text FROM information_schema.schemata \
         WHERE schema_name NOT LIKE 'pg_toast%' AND schema_name NOT LIKE 'pg_temp%' ORDER BY 1";

    pub fn tables_sql() -> String {
        format!(
            "SELECT table_name::text, table_type::text, table_schema::text \
             FROM information_schema.tables WHERE table_schema = {SCHEMA} ORDER BY 1"
        )
    }

    pub fn columns_sql() -> String {
        format!(
            "SELECT c.column_name::text, c.data_type::text, c.is_nullable::text, c.column_default::text, \
             EXISTS (SELECT 1 FROM information_schema.table_constraints tc \
               JOIN information_schema.key_column_usage k ON k.constraint_name = tc.constraint_name \
                AND k.table_schema = tc.table_schema AND k.table_name = tc.table_name \
               WHERE tc.constraint_type = 'PRIMARY KEY' AND tc.table_schema = c.table_schema \
                AND tc.table_name = c.table_name AND k.column_name = c.column_name) \
             FROM information_schema.columns c WHERE c.table_schema = {SCHEMA} AND c.table_name = $2 \
             ORDER BY c.ordinal_position"
        )
    }

    pub fn indexes_sql() -> String {
        format!(
            "SELECT i.relname::text, ix.indisunique, array_to_string(ARRAY( \
               SELECT a.attname::text FROM unnest(ix.indkey) WITH ORDINALITY k(attnum, ord) \
               JOIN pg_attribute a ON a.attrelid = ix.indrelid AND a.attnum = k.attnum ORDER BY k.ord), ',') \
             FROM pg_index ix JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN pg_class t ON t.oid = ix.indrelid JOIN pg_namespace n ON n.oid = t.relnamespace \
             WHERE n.nspname = {SCHEMA} AND t.relname = $2 ORDER BY 1"
        )
    }

    pub fn foreign_keys_sql() -> String {
        format!(
            "SELECT c.conname::text, array_to_string(ARRAY( \
               SELECT a.attname::text FROM unnest(c.conkey) WITH ORDINALITY k(n, o) \
               JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.n ORDER BY k.o), ','), \
             rn.nspname::text || '.' || rt.relname::text, array_to_string(ARRAY( \
               SELECT a.attname::text FROM unnest(c.confkey) WITH ORDINALITY k(n, o) \
               JOIN pg_attribute a ON a.attrelid = c.confrelid AND a.attnum = k.n ORDER BY k.o), ',') \
             FROM pg_constraint c JOIN pg_class t ON t.oid = c.conrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_class rt ON rt.oid = c.confrelid JOIN pg_namespace rn ON rn.oid = rt.relnamespace \
             WHERE c.contype = 'f' AND n.nspname = {SCHEMA} AND t.relname = $2 ORDER BY 1"
        )
    }

    fn s(v: &[Option<String>], i: usize) -> String {
        v.get(i).cloned().flatten().unwrap_or_default()
    }

    /// Booleans: `true` from the binary protocol, `t` from psql text output.
    fn is_true(v: &str) -> bool {
        matches!(v, "true" | "t")
    }

    fn split(v: &str) -> Vec<String> {
        v.split(',')
            .filter(|x| !x.is_empty())
            .map(String::from)
            .collect()
    }

    pub fn first_column(rows: Rows) -> Vec<String> {
        rows.iter().map(|r| s(r, 0)).collect()
    }

    pub fn tables(rows: Rows) -> Vec<TableInfo> {
        rows.iter()
            .map(|r| TableInfo {
                schema: Some(s(r, 2)),
                name: s(r, 0),
                kind: if s(r, 1) == "VIEW" {
                    ObjectKind::View
                } else {
                    ObjectKind::Table
                },
            })
            .collect()
    }

    pub fn table_desc(t: &TableRef, cols: Rows, idx: Rows, fks: Rows) -> Result<TableDesc> {
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
                    primary_key: is_true(&s(r, 4)),
                })
                .collect(),
            indexes: idx
                .iter()
                .map(|r| IndexInfo {
                    name: s(r, 0),
                    unique: is_true(&s(r, 1)),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_policy() {
        let mut cfg = ConnectionConfig::sqlite("p", "");
        cfg.kind = Kind::Postgres;
        cfg.path = None;
        cfg.user = Some("u".into());
        let c = build_config(&cfg, Some("pw"), true);
        assert_eq!(c.get_ssl_mode(), postgres::config::SslMode::Require);
        assert_eq!(c.get_options(), Some("-c default_transaction_read_only=on"));
        assert_eq!(c.get_application_name(), Some(saysdf_identity::APP_NAME));
        let w = build_config(&cfg, None, false);
        assert_eq!(w.get_options(), None);
        cfg.tls = TlsMode::Disable;
        assert_eq!(
            build_config(&cfg, None, true).get_ssl_mode(),
            postgres::config::SslMode::Disable
        );
    }

    #[test]
    fn decoders() {
        // 12345.678 : ndigits 3, weight 1, sign 0, dscale 3, digits [1, 2345, 6780]
        let raw = [0, 3, 0, 1, 0, 0, 0, 3, 0, 1, 0x09, 0x29, 0x1a, 0x7c];
        assert_eq!(numeric(&raw).as_deref(), Some("12345.678"));
        // -0.05 : ndigits 1, weight -1, sign 0x4000, dscale 2, digits [500]
        let raw = [0, 1, 0xff, 0xff, 0x40, 0, 0, 2, 0x01, 0xf4];
        assert_eq!(numeric(&raw).as_deref(), Some("-0.05"));
        assert_eq!(civil(0), (2000, 1, 1));
        assert_eq!(civil(-1), (1999, 12, 31));
        assert_eq!(
            timestamp(86_400_000_000 + 3_661_000_000),
            "2000-01-02 01:01:01"
        );
    }
}
