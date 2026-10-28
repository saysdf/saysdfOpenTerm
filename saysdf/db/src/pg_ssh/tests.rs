//! Tests on a fake psql transport: no network, no ssh.

use super::*;
use crate::writemode::{ButtonActivated, WriteMode};
use std::sync::atomic::{AtomicUsize, Ordering};

/// What the fake "server" answers to one SQL statement.
enum Answer {
    /// CSV lines (header first); `{N}` is replaced by the NULL marker.
    Rows(Vec<&'static str>),
    Error(&'static str, &'static str),
    /// Close the stream (process died) instead of answering.
    Die,
    /// Never answer.
    Hang,
}

type Handler = Arc<dyn Fn(&str) -> Answer + Send + Sync>;

/// Interprets the psql scripts the backend sends: `\echo` (with `:ERROR`, `:SQLSTATE`,
/// `:LAST_ERROR_MESSAGE`), `\pset null`, SQL statements ending with `;`.
struct FakePsql {
    handler: Handler,
    log: Arc<Mutex<Vec<String>>>,
    out: VecDeque<String>,
    null: String,
    error: bool,
    sqlstate: String,
    last_error: String,
    dead: bool,
    hang: bool,
}

impl FakePsql {
    fn exec(&mut self, sql: &str) {
        let sql = sql.trim().trim_end_matches(';').trim();
        if sql.is_empty() {
            return;
        }
        self.log.lock().unwrap().push(sql.to_string());
        match (self.handler)(sql) {
            Answer::Rows(lines) => {
                self.error = false;
                self.sqlstate = "00000".into();
                for l in lines {
                    self.out.push_back(l.replace("{N}", &self.null));
                }
            }
            Answer::Error(state, msg) => {
                self.error = true;
                self.sqlstate = state.into();
                self.last_error = msg.into();
            }
            Answer::Die => self.dead = true,
            Answer::Hang => self.hang = true,
        }
    }
}

impl Transport for FakePsql {
    fn send(&mut self, data: &str) -> io::Result<()> {
        if self.dead {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "dead"));
        }
        self.log.lock().unwrap().push(format!("<script>{data}"));
        let mut stmt = String::new();
        for line in data.lines() {
            if self.dead || self.hang {
                break;
            }
            if let Some(rest) = line.strip_prefix("\\echo ") {
                let v = rest
                    .replace(":ERROR", if self.error { "true" } else { "false" })
                    .replace(":SQLSTATE", &self.sqlstate)
                    .replace(":LAST_ERROR_MESSAGE", &self.last_error);
                self.out.push_back(v);
            } else if let Some(rest) = line.strip_prefix("\\pset null ") {
                self.null = rest.trim().to_string();
            } else if line.starts_with('\\') {
            } else {
                stmt.push_str(line);
                stmt.push('\n');
                if line.trim_end().ends_with(';') {
                    let s = std::mem::take(&mut stmt);
                    self.exec(&s);
                }
            }
        }
        Ok(())
    }
    fn recv_line(&mut self, _timeout: Duration) -> Recv {
        match self.out.pop_front() {
            Some(l) => Recv::Line(l),
            None if self.hang => Recv::Timeout,
            None => Recv::Eof,
        }
    }
    fn stderr_tail(&self) -> String {
        "fake: connection reset".into()
    }
}

struct Rig {
    log: Arc<Mutex<Vec<String>>>,
    spawns: Arc<AtomicUsize>,
}

impl Rig {
    fn statements(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| !l.starts_with("<script>"))
            .cloned()
            .collect()
    }
    fn scripts(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|l| l.strip_prefix("<script>").map(String::from))
            .collect()
    }
}

fn open_fake(access: Access, handler: Handler) -> Result<(PgSshBackend, Rig)> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let spawns = Arc::new(AtomicUsize::new(0));
    let (l2, s2) = (log.clone(), spawns.clone());
    let spawn: Spawner = Box::new(move || {
        s2.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakePsql {
            handler: handler.clone(),
            log: l2.clone(),
            out: VecDeque::new(),
            null: String::new(),
            error: false,
            sqlstate: "00000".into(),
            last_error: String::new(),
            dead: false,
            hang: false,
        }) as Box<dyn Transport>)
    });
    let t = Timeouts {
        connect: Duration::from_millis(50),
        query: Duration::from_millis(50),
    };
    let b = PgSshBackend::with_spawner(spawn, access, t, Arc::new(Mutex::new(None)))?;
    Ok((b, Rig { log, spawns }))
}

fn ok_rows(sql: &str) -> Answer {
    if sql.starts_with("SET ") || sql.starts_with("BEGIN") || sql == "ROLLBACK" {
        return Answer::Rows(vec![]);
    }
    if sql.contains("multi") {
        return Answer::Rows(vec![
            "id,note,empty,nothing",
            "1,\"line one",
            "line two, with comma\",\"\",{N}",
            "2,\"say \"\"hi\"\"\",x,{N}",
        ]);
    }
    if sql.contains("broken") {
        return Answer::Error("42P01", "ERROR:  relation \"broken\" does not exist");
    }
    Answer::Rows(vec!["?column?", "1"])
}

fn grant(db: &str) -> crate::WriteGrant {
    let mut m = WriteMode::default();
    m.request_write(ButtonActivated::WriteButton, db);
    m.confirm(db, "test").unwrap()
}

#[test]
fn names_are_validated_against_injection() {
    for good in ["prodhost", "app-db-1", "reader_ro", "db.example", "A_1-b.c"] {
        assert!(validate_name("x", good).is_ok(), "{good}");
    }
    for bad in [
        "", "a;b", "$(id)", "a b", "a`id`", "a|b", "a&b", "a>b", "-oProxyCommand", "-v", "a'b",
        "a\"b", "a\nb", "ä", "a/b", "a\\b", "a=b",
    ] {
        assert!(validate_name("x", bad).is_err(), "{bad:?}");
    }
    assert!(validate_name("x", &"a".repeat(129)).is_err());
    let mut cfg = ConnectionConfig::pg_ssh("p", "prodhost", Some("db; rm -rf /"), "g", "u");
    assert!(PgSshBackend::open(&cfg, Access::ReadOnly).is_err());
    cfg.container = Some("c".into());
    cfg.user = Some("$(whoami)".into());
    assert!(SshTarget::from_config(&cfg).is_err());
    cfg.user = None;
    assert!(SshTarget::from_config(&cfg).is_err());
}

#[test]
fn argv_is_a_plain_list() {
    let cfg = ConnectionConfig::pg_ssh(
        "prod",
        "prodhost",
        Some("app-db-1"),
        "prodhost",
        "reader_ro",
    );
    let t = SshTarget::from_config(&cfg).unwrap();
    assert_eq!(
        t.ssh_argv().unwrap().join(" "),
        "-T -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 \
         -o ServerAliveCountMax=3 -- prodhost docker exec -i app-db-1 \
         psql -X -q -v ON_ERROR_STOP=0 --csv -U reader_ro -d prodhost"
    );
    let mut host = t.clone();
    host.container = None;
    let v = host.ssh_argv().unwrap();
    assert_eq!(&v[11..13], &["psql".to_string(), "-X".to_string()]);
    assert!(!v.iter().any(|a| a == "docker"));
    // empty container in the config means "no docker"
    let c2 = ConnectionConfig::pg_ssh("p", "h", Some(""), "d", "u");
    assert_eq!(SshTarget::from_config(&c2).unwrap().container, None);
}

#[test]
fn psql_text_check() {
    for bad in [
        "select 1 \\! ls",
        "select '\\gexec'",
        "select 'abc",
        "select \"abc",
        "select /* x",
        "select /* a /* b */ c",
        "select $$abc",
        "select $t$abc$$",
        "select 1\0",
    ] {
        assert!(check_psql_text(bad).is_err(), "{bad:?}");
    }
    for good in [
        "select 'a''b'",
        "select $$x;y$$, $t$ ' $t$",
        "select $1",
        "select \"a\"\"b\" -- it's a comment",
        "select /* a /* nested */ b */ 1",
        "select a$b$ from t",
    ] {
        assert!(check_psql_text(good).is_ok(), "{good:?}");
    }
}

#[test]
fn bind_quotes_and_skips_quoted_text() {
    assert_eq!(
        bind("x = $1 AND y = $2 AND z = '$1' AND \"c$1\"", &[Some("it's"), None]).unwrap(),
        "x = E'it''s' AND y = NULL AND z = '$1' AND \"c$1\""
    );
    assert_eq!(literal("a\\b").unwrap(), "E'a\\\\b'");
    assert!(bind("$2", &[Some("a")]).is_err());
}

#[test]
fn handshake_sets_read_only_and_null_marker() {
    let (_b, rig) = open_fake(Access::ReadOnly, Arc::new(ok_rows)).unwrap();
    let hs = &rig.scripts()[0];
    assert!(hs.contains("\\pset null SAYSDF"), "{hs}");
    assert!(rig
        .statements()
        .contains(&"SET default_transaction_read_only = on".to_string()));
    assert!(rig
        .statements()
        .contains(&"SET standard_conforming_strings = on".to_string()));
}

#[test]
fn csv_multiline_quotes_null_and_empty() {
    let (mut b, rig) = open_fake(Access::ReadOnly, Arc::new(ok_rows)).unwrap();
    let rs = b.query("select multi from t", 100).unwrap();
    assert_eq!(rs.columns, ["id", "note", "empty", "nothing"]);
    assert_eq!(
        rs.rows[0],
        vec![
            Value::Text("1".into()),
            Value::Text("line one\nline two, with comma".into()),
            Value::Text(String::new()),
            Value::Null
        ]
    );
    assert_eq!(rs.rows[1][1], Value::Text("say \"hi\"".into()));
    assert_eq!(rs.rows.len(), 2);
    // every user statement in a read-only transaction
    let st = rig.statements();
    let i = st.iter().position(|s| s == "select multi from t").unwrap();
    assert_eq!(st[i - 1], "BEGIN READ ONLY");
    assert_eq!(st[i + 1], "ROLLBACK");
}

#[test]
fn max_rows_truncates_and_drains() {
    let (mut b, _rig) = open_fake(Access::ReadOnly, Arc::new(ok_rows)).unwrap();
    let rs = b.query("select multi from t", 1).unwrap();
    assert_eq!(rs.rows.len(), 1);
    assert!(rs.truncated);
    // the stream stayed in sync
    let rs = b.query("select 1", 10).unwrap();
    assert_eq!(rs.rows, vec![vec![Value::Text("1".into())]]);
}

#[test]
fn psql_error_is_reported_and_session_continues() {
    let (mut b, _rig) = open_fake(Access::ReadOnly, Arc::new(ok_rows)).unwrap();
    let e = b.query("select * from broken", 10).unwrap_err().to_string();
    assert!(e.contains("relation \"broken\" does not exist"), "{e}");
    assert!(e.contains("SQLSTATE 42P01"), "{e}");
    assert_eq!(b.query("select 1", 10).unwrap().rows.len(), 1);
}

#[test]
fn rejected_before_sending() {
    let (mut b, rig) = open_fake(Access::ReadOnly, Arc::new(ok_rows)).unwrap();
    let n = rig.scripts().len();
    assert!(b.query("SET default_transaction_read_only = off", 10).is_err());
    assert!(b.query("select 1; select 2", 10).is_err());
    assert!(b.query("select 1 \\! id", 10).is_err());
    assert!(b.query("select 'open", 10).is_err());
    assert!(b.query("copy t to stdout", 10).is_err());
    assert_eq!(rig.scripts().len(), n);
}

#[test]
fn write_mode_lifts_session_read_only_only() {
    let (mut b, rig) = open_fake(Access::Write(grant("g")), Arc::new(ok_rows)).unwrap();
    assert!(rig
        .statements()
        .contains(&"SET default_transaction_read_only = off".to_string()));
    b.query("update t set a = 1", 10).unwrap();
    let st = rig.statements();
    let i = st.iter().position(|s| s == "update t set a = 1").unwrap();
    assert_ne!(st[i - 1], "BEGIN READ ONLY");
    // export always re-runs read-only, even in write mode
    let mut c = Collector::new(10);
    b.query_stream("select 1", &mut c).unwrap();
    let st = rig.statements();
    let i = st.iter().rposition(|s| s == "select 1").unwrap();
    assert_eq!(st[i - 1], "BEGIN READ ONLY");
    assert!(b.query_stream("commit", &mut c).is_err());
}

#[test]
fn timeout_resets_and_next_request_reconnects() {
    let h: Handler = Arc::new(|sql: &str| {
        if sql.contains("pg_sleep") {
            Answer::Hang
        } else {
            ok_rows(sql)
        }
    });
    let (mut b, rig) = open_fake(Access::ReadOnly, h).unwrap();
    let e = b.query("select pg_sleep(100)", 10).unwrap_err().to_string();
    assert!(e.contains("timed out"), "{e}");
    assert_eq!(b.query("select 1", 10).unwrap().rows.len(), 1);
    // first spawn + retry after the timeout + reconnect for the next query
    assert!(rig.spawns.load(Ordering::SeqCst) >= 2);
}

#[test]
fn lost_connection_retried_once_when_read_only() {
    let died = Arc::new(AtomicUsize::new(0));
    let d2 = died.clone();
    let h: Handler = Arc::new(move |sql: &str| {
        if sql == "select 42" && d2.fetch_add(1, Ordering::SeqCst) == 0 {
            Answer::Die
        } else {
            ok_rows(sql)
        }
    });
    let (mut b, rig) = open_fake(Access::ReadOnly, h).unwrap();
    assert_eq!(b.query("select 42", 10).unwrap().rows.len(), 1);
    assert_eq!(rig.spawns.load(Ordering::SeqCst), 2);
}

#[test]
fn lost_connection_not_retried_in_write_mode() {
    let h: Handler = Arc::new(|sql: &str| {
        if sql.starts_with("update") {
            Answer::Die
        } else {
            ok_rows(sql)
        }
    });
    let (mut b, rig) = open_fake(Access::Write(grant("g")), h).unwrap();
    let e = b.query("update t set a = 1", 10).unwrap_err().to_string();
    assert!(e.contains("connection closed"), "{e}");
    assert_eq!(rig.spawns.load(Ordering::SeqCst), 1);
    let updates = rig
        .statements()
        .iter()
        .filter(|s| s.starts_with("update"))
        .count();
    assert_eq!(updates, 1);
}

#[test]
fn introspection_reuses_pg_queries_with_text_booleans() {
    let h: Handler = Arc::new(|sql: &str| {
        if sql.contains("information_schema.tables") {
            assert!(sql.contains("COALESCE(E'public'::text"), "{sql}");
            Answer::Rows(vec![
                "table_name,table_type,table_schema",
                "games,BASE TABLE,public",
                "v_stats,VIEW,public",
            ])
        } else if sql.contains("information_schema.columns") {
            Answer::Rows(vec![
                "column_name,data_type,is_nullable,column_default,exists",
                "id,integer,NO,{N},t",
                "name,text,YES,'x'::text,f",
            ])
        } else if sql.contains("pg_index") {
            Answer::Rows(vec!["relname,indisunique,a", "games_pkey,t,id"])
        } else if sql.contains("pg_constraint") {
            Answer::Rows(vec!["conname,a,b,c"])
        } else {
            ok_rows(sql)
        }
    });
    let (mut b, _rig) = open_fake(Access::ReadOnly, h).unwrap();
    let t = b.list_tables(Some("public")).unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!(t[1].kind, crate::ObjectKind::View);
    let d = b
        .describe(&TableRef {
            schema: Some("public".into()),
            name: "games".into(),
        })
        .unwrap();
    assert!(d.columns[0].primary_key);
    assert_eq!(d.columns[0].default, None);
    assert!(!d.columns[1].primary_key);
    assert!(d.indexes[0].unique);
    assert_eq!(d.indexes[0].columns, ["id"]);
    assert!(d.foreign_keys.is_empty());
}

#[test]
fn page_binds_filter_literal() {
    let h: Handler = Arc::new(|sql: &str| {
        if sql.starts_with("SELECT * FROM") {
            assert!(sql.contains("LIKE E'%o''k%'"), "{sql}");
            assert!(sql.ends_with("LIMIT 10 OFFSET 0"), "{sql}");
            Answer::Rows(vec!["id", "1"])
        } else {
            ok_rows(sql)
        }
    });
    let (mut b, _rig) = open_fake(Access::ReadOnly, h).unwrap();
    let rs = b
        .fetch_page(&PageRequest {
            table: TableRef {
                schema: Some("public".into()),
                name: "games".into(),
            },
            order_by: None,
            filter: Some(("name".into(), "%o'k%".into())),
            offset: 0,
            limit: 10,
            as_of: None,
        })
        .unwrap();
    assert_eq!(rs.rows.len(), 1);
}

#[test]
fn response_parser_skips_noise_before_marker() {
    struct Lines(VecDeque<String>);
    impl Transport for Lines {
        fn send(&mut self, _: &str) -> io::Result<()> {
            Ok(())
        }
        fn recv_line(&mut self, _: Duration) -> Recv {
            self.0.pop_front().map(Recv::Line).unwrap_or(Recv::Eof)
        }
    }
    let m = Markers::new("TOK".into());
    let mut t = Lines(
        [
            "NOTICE noise",
            "TOKB",
            "a,b",
            "\"x\ny\",2",
            "TOKS false 00000",
            "",
            "TOKZ",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    let mut c = Collector::new(10);
    let n = read_response(&mut t, &m, Instant::now() + Duration::from_secs(1), &mut c).unwrap();
    assert_eq!(n, 1);
    assert_eq!(c.rs.rows[0][0], Value::Text("x\ny".into()));
    // script shape
    let s = m.script("BEGIN READ ONLY;\n", "select 1;;  \n", "ROLLBACK;\n");
    assert_eq!(
        s,
        "BEGIN READ ONLY;\n\\echo TOKB\nselect 1\n;\n\\echo TOKS :ERROR :SQLSTATE\n\
         \\echo :LAST_ERROR_MESSAGE\nROLLBACK;\n\\echo TOKZ\n"
    );
}
