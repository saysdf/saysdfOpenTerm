//! PostgreSQL over SSH without open ports or tunnels (change `db-over-ssh`).
//!
//! One long-lived process per connection:
//! `ssh -T -o BatchMode=yes … <alias> [docker exec -i <container>] psql -X -q -v ON_ERROR_STOP=0
//! --csv -U <user> -d <db>`. The local side passes arguments as a list (no local shell); the
//! remote side gets a command line built only from values validated by [`validate_name`]
//! (`[A-Za-z0-9_.-]`, not starting with `-`), so nothing can be injected into the remote shell.
//!
//! Protocol: every request is a small psql script; results are framed by `\echo` lines with a
//! random per-session token, the status comes from psql's `:ERROR`, `:SQLSTATE` and
//! `:LAST_ERROR_MESSAGE` variables, data is CSV (`--csv`), NULL is printed as a token-derived
//! marker (`\pset null`). User SQL must not contain backslashes (psql meta-commands such as `\!`
//! would run on the server) and must have balanced quotes/comments ([`check_psql_text`]).
//!
//! Read-only (same layers as [`crate::pg`]): `SET default_transaction_read_only = on` right after
//! start, every statement in `BEGIN READ ONLY` … `ROLLBACK`, [`guard::check`] (one statement,
//! deny-list). Write mode (a [`crate::WriteGrant`]) only switches the session default off and
//! drops the per-statement wrapper; a SELECT-only role still cannot write — that is expected.
//!
//! Passwords are not supported: the server-side socket is expected to use trust/peer auth.
//! If ever needed, a password must go through stdin, never through the command line (`ps`).

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::pg::intro;
use crate::{
    guard, page_sql, Access, Backend, CancelHandle, Collector, ConnectionConfig, Error, Kind,
    PageRequest, Result, ResultSet, RowSink, TableDesc, TableInfo, TableRef, Value,
};

/// Server-side target. Every field is validated by [`validate_name`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub alias: String,
    /// `None` — psql runs directly on the host, without docker.
    pub container: Option<String>,
    pub database: String,
    pub user: String,
}

/// Only `[A-Za-z0-9_.-]`, 1–128 chars, not starting with `-` (would be read as an option by
/// ssh/docker/psql). Anything else is refused: these values end up in a remote shell command.
pub fn validate_name(field: &str, v: &str) -> Result<()> {
    if v.is_empty() || v.len() > 128 {
        return Err(Error::InvalidArgument(format!("{field}: 1–128 characters")));
    }
    if v.starts_with('-') {
        return Err(Error::InvalidArgument(format!(
            "{field}: must not start with '-'"
        )));
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err(Error::InvalidArgument(format!(
            "{field}: only A-Z a-z 0-9 _ . - are allowed"
        )));
    }
    Ok(())
}

impl SshTarget {
    pub fn from_config(cfg: &ConnectionConfig) -> Result<Self> {
        let req = |v: &Option<String>, f: &str| {
            v.clone()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| Error::InvalidArgument(format!("{f} is required")))
        };
        let t = Self {
            alias: req(&cfg.ssh_alias, "ssh alias")?,
            container: cfg.container.clone().filter(|s| !s.is_empty()),
            database: req(&cfg.database, "database")?,
            user: req(&cfg.user, "user")?,
        };
        t.validate()?;
        Ok(t)
    }

    pub fn validate(&self) -> Result<()> {
        validate_name("ssh alias", &self.alias)?;
        if let Some(c) = &self.container {
            validate_name("container", c)?;
        }
        validate_name("database", &self.database)?;
        validate_name("user", &self.user)
    }

    /// Arguments for the local `ssh` program (no shell involved locally).
    pub fn ssh_argv(&self) -> Result<Vec<String>> {
        self.validate()?;
        let mut v: Vec<String> = [
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "--",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        v.push(self.alias.clone());
        if let Some(c) = &self.container {
            v.extend(["docker", "exec", "-i"].map(String::from));
            v.push(c.clone());
        }
        v.extend(
            ["psql", "-X", "-q", "-v", "ON_ERROR_STOP=0", "--csv", "-U"].map(String::from),
        );
        v.push(self.user.clone());
        v.push("-d".into());
        v.push(self.database.clone());
        Ok(v)
    }
}

/// Reject text psql would not pass to the server as one plain statement: any backslash
/// (meta-commands, e.g. `\!` runs a shell on the server), NUL, and unterminated quotes,
/// dollar quotes or block comments (psql would swallow the protocol lines that follow).
pub fn check_psql_text(sql: &str) -> Result<()> {
    if sql.contains('\\') {
        return Err(Error::InvalidArgument(
            "backslash is not allowed over SSH (psql meta-commands)".into(),
        ));
    }
    if sql.contains('\0') {
        return Err(Error::InvalidArgument("NUL character in query".into()));
    }
    let b = sql.as_bytes();
    let mut i = 0;
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    while i < b.len() {
        let c = b[i];
        match c {
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 1;
                i += 2;
                while depth > 0 {
                    match (b.get(i), b.get(i + 1)) {
                        (None, _) => {
                            return Err(Error::InvalidArgument("unterminated comment".into()))
                        }
                        (Some(b'/'), Some(b'*')) => {
                            depth += 1;
                            i += 2;
                        }
                        (Some(b'*'), Some(b'/')) => {
                            depth -= 1;
                            i += 2;
                        }
                        _ => i += 1,
                    }
                }
                continue;
            }
            b'\'' | b'"' => {
                i += 1;
                loop {
                    match b.get(i) {
                        None => {
                            return Err(Error::InvalidArgument(
                                "unterminated quoted string".into(),
                            ))
                        }
                        Some(&q) if q == c => {
                            if b.get(i + 1) == Some(&c) {
                                i += 2;
                                continue;
                            }
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'$' if i == 0 || !ident(b[i - 1]) && b[i - 1] != b'$' => {
                // `$tag$ … $tag$`; `$1` is a parameter, not a quote.
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] >= 0x80)
                {
                    j += 1;
                }
                let tag_ok = b.get(j) == Some(&b'$')
                    && (j == i + 1 || !b[i + 1].is_ascii_digit());
                if tag_ok {
                    let tag = &sql[i..=j];
                    match sql[j + 1..].find(tag) {
                        Some(p) => {
                            i = j + 1 + p + tag.len();
                            continue;
                        }
                        None => {
                            return Err(Error::InvalidArgument(
                                "unterminated dollar-quoted string".into(),
                            ))
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// `E'…'` literal: independent of `standard_conforming_strings`, quotes and backslashes escaped.
pub fn literal(v: &str) -> Result<String> {
    if v.contains('\0') {
        return Err(Error::InvalidArgument("NUL character in value".into()));
    }
    let mut s = String::with_capacity(v.len() + 3);
    s.push_str("E'");
    for c in v.chars() {
        match c {
            '\'' => s.push_str("''"),
            '\\' => s.push_str("\\\\"),
            c => s.push(c),
        }
    }
    s.push('\'');
    Ok(s)
}

/// Replaces `$1`…`$9` outside quotes with literals; `None` becomes `NULL`.
pub(crate) fn bind(sql: &str, params: &[Option<&str>]) -> Result<String> {
    if params.len() > 9 {
        return Err(Error::InvalidArgument("too many parameters".into()));
    }
    let mut out = String::with_capacity(sql.len() + 32);
    let mut quote: Option<char> = None;
    let mut it = sql.chars().peekable();
    while let Some(c) = it.next() {
        match quote {
            Some(q) => {
                out.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                out.push(c);
            }
            None if c == '$' && it.peek().is_some_and(|d| ('1'..='9').contains(d)) => {
                let n = it.next().and_then(|d| d.to_digit(10)).unwrap_or(0) as usize;
                match params.get(n - 1) {
                    Some(Some(v)) => out.push_str(&literal(v)?),
                    Some(None) => out.push_str("NULL"),
                    None => return Err(Error::InvalidArgument(format!("missing ${n}"))),
                }
            }
            None => out.push(c),
        }
    }
    Ok(out)
}

/// `COPY … FROM STDIN` would read the protocol lines as data, `TO STDOUT` breaks framing.
fn reject_copy(sql: &str) -> Result<()> {
    if guard::lex(sql, Kind::Postgres).words.first().map(String::as_str) == Some("COPY") {
        return Err(Error::Unsupported("COPY over SSH"));
    }
    Ok(())
}

/// One line from the transport.
#[derive(Debug, PartialEq, Eq)]
pub enum Recv {
    Line(String),
    Timeout,
    Eof,
}

/// Byte pipe to a psql process. The real one is [`SshProcess`]; tests use a fake.
pub trait Transport: Send {
    fn send(&mut self, data: &str) -> io::Result<()>;
    fn recv_line(&mut self, timeout: Duration) -> Recv;
    /// Last lines of the process stderr (ssh/docker/psql diagnostics).
    fn stderr_tail(&self) -> String {
        String::new()
    }
}

/// Creates a new transport (initial connect and reconnects).
pub type Spawner = Box<dyn FnMut() -> Result<Box<dyn Transport>> + Send>;

type ChildSlot = Arc<Mutex<Option<Child>>>;

/// `ssh` child process; stdout/stderr read by background threads.
pub struct SshProcess {
    stdin: ChildStdin,
    lines: Receiver<String>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    slot: ChildSlot,
}

const STDERR_KEEP: usize = 20;

impl SshProcess {
    pub fn spawn(argv: &[String], slot: ChildSlot) -> Result<Self> {
        let mut cmd = Command::new("ssh");
        cmd.args(argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Driver(format!("cannot start ssh: {e}")))?;
        let stdin = child.stdin.take().ok_or_else(|| Error::Driver("ssh stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Driver("ssh stdout".into()))?;
        let stderr_pipe = child.stderr.take();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match r.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        while matches!(buf.last(), Some(b'\n' | b'\r')) {
                            buf.pop();
                        }
                        if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(p) = stderr_pipe {
            let keep = stderr.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(p).lines() {
                    let Ok(line) = line else { return };
                    if let Ok(mut q) = keep.lock() {
                        if q.len() >= STDERR_KEEP {
                            q.pop_front();
                        }
                        q.push_back(line);
                    }
                }
            });
        }
        if let Ok(mut s) = slot.lock() {
            *s = Some(child);
        }
        Ok(Self {
            stdin,
            lines,
            stderr,
            slot,
        })
    }
}

impl Drop for SshProcess {
    fn drop(&mut self) {
        if let Ok(mut s) = self.slot.lock() {
            if let Some(mut c) = s.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

impl Transport for SshProcess {
    fn send(&mut self, data: &str) -> io::Result<()> {
        self.stdin.write_all(data.as_bytes())?;
        self.stdin.flush()
    }
    fn recv_line(&mut self, timeout: Duration) -> Recv {
        match self.lines.recv_timeout(timeout) {
            Ok(l) => Recv::Line(l),
            Err(RecvTimeoutError::Timeout) => Recv::Timeout,
            Err(RecvTimeoutError::Disconnected) => Recv::Eof,
        }
    }
    fn stderr_tail(&self) -> String {
        self.stderr
            .lock()
            .map(|q| q.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }
}

/// Timeouts: connect (ssh + handshake) and per request. The server-side `statement_timeout`
/// is set to `query`, the client waits a little longer.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub query: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(30),
            query: Duration::from_secs(300),
        }
    }
}

const GRACE: Duration = Duration::from_secs(10);

fn new_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let local = 0u8;
    let addr = &local as *const u8 as usize as u128;
    let mut x = nanos ^ (u128::from(std::process::id()) << 64) ^ addr.rotate_left(32);
    // splitmix-style scramble so the token is not just the clock
    let mut out = String::from("SAYSDF");
    for _ in 0..2 {
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd_c4ce_b9fe_1a85_ec53);
        x ^= x >> 29;
        out.push_str(&format!("{:016x}", x as u64));
    }
    out
}

/// Script markers derived from the session token.
#[derive(Debug, Clone)]
pub(crate) struct Markers {
    token: String,
}

impl Markers {
    fn new(token: String) -> Self {
        Self { token }
    }
    fn begin(&self) -> String {
        format!("{}B", self.token)
    }
    fn status(&self) -> String {
        format!("{}S", self.token)
    }
    fn end(&self) -> String {
        format!("{}Z", self.token)
    }
    fn null(&self) -> String {
        format!("{}N", self.token)
    }

    /// psql script: `pre` SQL, begin marker, the statement, status + error message, `post`
    /// SQL, end marker. `sql` must already pass [`check_psql_text`].
    pub(crate) fn script(&self, pre: &str, sql: &str, post: &str) -> String {
        let body = sql.trim_end_matches(|c: char| c.is_whitespace() || c == ';');
        let mut s = String::new();
        s.push_str(pre);
        s.push_str(&format!("\\echo {}\n", self.begin()));
        if !body.trim().is_empty() {
            s.push_str(body);
            s.push_str("\n;\n");
        }
        s.push_str(&format!("\\echo {} :ERROR :SQLSTATE\n", self.status()));
        s.push_str("\\echo :LAST_ERROR_MESSAGE\n");
        s.push_str(post);
        s.push_str(&format!("\\echo {}\n", self.end()));
        s
    }
}

#[derive(Debug)]
enum Failure {
    /// The connection is gone or stuck; it was dropped.
    Lost(String),
    Other(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Failure::Other(e)
    }
}

impl Failure {
    fn into_error(self) -> Error {
        match self {
            Failure::Lost(m) => Error::Driver(m),
            Failure::Other(e) => e,
        }
    }
}

/// Is the CSV record complete (even number of `"`)? Escaped quotes come in pairs.
fn record_complete(pending: &str) -> bool {
    pending.bytes().filter(|&b| b == b'"').count() % 2 == 0
}

fn parse_record(text: &str) -> Result<Vec<String>> {
    let mut r = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    match r.records().next() {
        Some(Ok(rec)) => Ok(rec.iter().map(String::from).collect()),
        Some(Err(e)) => Err(Error::Driver(format!("CSV from psql: {e}"))),
        None => Ok(vec![String::new()]),
    }
}

/// Reads one framed response from `t`, feeding CSV records to `sink` (first record = header).
/// Returns rows passed to the sink.
fn read_response(
    t: &mut dyn Transport,
    m: &Markers,
    deadline: Instant,
    sink: &mut dyn RowSink,
) -> std::result::Result<u64, Failure> {
    let null = m.null();
    let (begin, status, end) = (m.begin(), m.status(), m.end());
    let next = |t: &mut dyn Transport| -> std::result::Result<String, Failure> {
        let left = deadline.saturating_duration_since(Instant::now());
        match t.recv_line(left) {
            Recv::Line(l) => Ok(l),
            Recv::Timeout => Err(Failure::Lost(
                "timed out waiting for psql; connection reset".into(),
            )),
            Recv::Eof => {
                let tail = t.stderr_tail();
                Err(Failure::Lost(if tail.trim().is_empty() {
                    "connection closed (ssh/psql exited)".into()
                } else {
                    format!("connection closed: {}", tail.trim())
                }))
            }
        }
    };
    // Skip anything before our begin marker (e.g. output of the prefix SQL).
    loop {
        if next(t)? == begin {
            break;
        }
    }
    let mut header_seen = false;
    let mut want_rows = true;
    let mut sink_err: Option<Error> = None;
    let mut sent = 0u64;
    let mut pending = String::new();
    let status_line = loop {
        let line = next(t)?;
        if pending.is_empty() && line.starts_with(&status) {
            break line;
        }
        if !pending.is_empty() {
            pending.push('\n');
        }
        pending.push_str(&line);
        if !record_complete(&pending) {
            continue;
        }
        let rec = std::mem::take(&mut pending);
        if sink_err.is_some() || (!want_rows && header_seen) {
            continue; // drain
        }
        let fields = match parse_record(&rec) {
            Ok(f) => f,
            Err(e) => {
                sink_err = Some(e);
                continue;
            }
        };
        if !header_seen {
            header_seen = true;
            if let Err(e) = sink.columns(&fields) {
                sink_err = Some(e);
            }
            continue;
        }
        let row = fields
            .into_iter()
            .map(|f| if f == null { Value::Null } else { Value::Text(f) })
            .collect();
        match sink.row(row) {
            Ok(true) => sent += 1,
            Ok(false) => want_rows = false,
            Err(e) => sink_err = Some(e),
        }
    };
    let mut parts = status_line[status.len()..].split_whitespace();
    let failed = parts.next() == Some("true");
    let sqlstate = parts.next().unwrap_or("").to_string();
    let mut message = Vec::new();
    loop {
        let line = next(t)?;
        if line == end {
            break;
        }
        message.push(line);
    }
    if failed {
        let msg = message.join("\n");
        let msg = msg.trim();
        return Err(Failure::Other(Error::Driver(if sqlstate.is_empty() {
            msg.to_string()
        } else {
            format!("{msg} (SQLSTATE {sqlstate})")
        })));
    }
    if let Some(e) = sink_err {
        return Err(Failure::Other(e));
    }
    if !header_seen {
        sink.columns(&[])?;
    }
    Ok(sent)
}

pub struct PgSshBackend {
    access: Access,
    spawn: Spawner,
    conn: Option<Box<dyn Transport>>,
    markers: Markers,
    timeouts: Timeouts,
    slot: ChildSlot,
}

impl PgSshBackend {
    pub fn open(cfg: &ConnectionConfig, access: Access) -> Result<Self> {
        let target = SshTarget::from_config(cfg)?;
        let argv = target.ssh_argv()?;
        let slot: ChildSlot = Arc::new(Mutex::new(None));
        let s2 = slot.clone();
        let spawn: Spawner = Box::new(move || {
            Ok(Box::new(SshProcess::spawn(&argv, s2.clone())?) as Box<dyn Transport>)
        });
        Self::with_spawner(spawn, access, Timeouts::default(), slot)
    }

    /// Opens with a custom transport factory (tests: fake transport, no network).
    pub fn with_spawner(
        spawn: Spawner,
        access: Access,
        timeouts: Timeouts,
        slot: ChildSlot,
    ) -> Result<Self> {
        let mut b = Self {
            access,
            spawn,
            conn: None,
            markers: Markers::new(new_token()),
            timeouts,
            slot,
        };
        b.ensure().map_err(Failure::into_error)?;
        Ok(b)
    }

    fn handshake_script(&self) -> String {
        let ro = if self.access.is_read_only() { "on" } else { "off" };
        let pre = format!(
            "\\encoding UTF8\n\\pset null {}\n\\set ON_ERROR_STOP 0\n\
             SET standard_conforming_strings = on;\n\
             SET statement_timeout = {};\n",
            self.markers.null(),
            self.timeouts.query.as_millis()
        );
        self.markers.script(
            &pre,
            &format!("SET default_transaction_read_only = {ro}"),
            "",
        )
    }

    /// Connects (or reconnects) and runs the handshake.
    fn ensure(&mut self) -> std::result::Result<(), Failure> {
        if self.conn.is_some() {
            return Ok(());
        }
        let mut t = (self.spawn)()?;
        let script = self.handshake_script();
        t.send(&script)
            .map_err(|e| Failure::Lost(format!("cannot write to ssh: {e}")))?;
        let deadline = Instant::now() + self.timeouts.connect;
        let mut sink = Collector::new(0);
        read_response(t.as_mut(), &self.markers, deadline, &mut sink)?;
        self.conn = Some(t);
        Ok(())
    }

    /// Sends one script and reads the framed response. A lost connection is dropped; in
    /// read-only mode the request is retried once on a fresh connection (never in write mode,
    /// where a retry could repeat a write).
    fn exchange(&mut self, script: &str, sink: &mut dyn RowSink, read_only: bool) -> Result<u64> {
        let reused = self.conn.is_some();
        match self.exchange_once(script, sink) {
            Err(Failure::Lost(_)) if reused && read_only => self
                .exchange_once(script, sink)
                .map_err(Failure::into_error),
            r => r.map_err(Failure::into_error),
        }
    }

    fn exchange_once(
        &mut self,
        script: &str,
        sink: &mut dyn RowSink,
    ) -> std::result::Result<u64, Failure> {
        self.ensure()?;
        let deadline = Instant::now() + self.timeouts.query + GRACE;
        let markers = self.markers.clone();
        let Some(t) = self.conn.as_mut() else {
            return Err(Failure::Lost("not connected".into()));
        };
        let r = match t.send(script) {
            Ok(()) => read_response(t.as_mut(), &markers, deadline, sink),
            Err(e) => Err(Failure::Lost(format!("cannot write to ssh: {e}"))),
        };
        if matches!(r, Err(Failure::Lost(_))) {
            self.conn = None; // drop kills the process; next request reconnects
        }
        r
    }

    /// One statement; `read_only` wraps it in `BEGIN READ ONLY` … `ROLLBACK`.
    fn run(&mut self, sql: &str, read_only: bool, sink: &mut dyn RowSink) -> Result<u64> {
        check_psql_text(sql)?;
        let (pre, post) = if read_only {
            ("BEGIN READ ONLY;\n", "ROLLBACK;\n")
        } else {
            ("", "")
        };
        let script = self.markers.script(pre, sql, post);
        self.exchange(&script, sink, read_only)
    }

    fn collect(&mut self, sql: &str, max_rows: usize) -> Result<ResultSet> {
        let ro = self.access.is_read_only();
        let mut c = Collector::new(max_rows);
        self.run(sql, ro, &mut c)?;
        Ok(c.rs)
    }

    fn rows(&mut self, sql: &str, params: &[Option<&str>]) -> Result<intro::Rows> {
        let sql = bind(sql, params)?;
        let mut c = Collector::new(100_000);
        // introspection is always read-only, whatever the session access
        self.run(&sql, true, &mut c)?;
        Ok(c
            .rs
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

struct KillCancel(ChildSlot);

impl CancelHandle for KillCancel {
    /// Kills the ssh process: the running statement is abandoned (the server cancels it when the
    /// connection drops), the next request reconnects.
    fn cancel(&self) -> Result<()> {
        if let Ok(mut s) = self.0.lock() {
            if let Some(c) = s.as_mut() {
                c.kill()
                    .map_err(|e| Error::Driver(format!("cancel: {e}")))?;
            }
        }
        Ok(())
    }
}

impl Backend for PgSshBackend {
    fn kind(&self) -> Kind {
        Kind::PgSsh
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
        Ok(intro::tables(self.rows(&intro::tables_sql(), &[schema])?))
    }
    fn describe(&mut self, t: &TableRef) -> Result<TableDesc> {
        let p = [t.schema.as_deref(), Some(t.name.as_str())];
        let cols = self.rows(&intro::columns_sql(), &p)?;
        let idx = self.rows(&intro::indexes_sql(), &p)?;
        let fks = self.rows(&intro::foreign_keys_sql(), &p)?;
        intro::table_desc(t, cols, idx, fks)
    }
    fn query(&mut self, sql: &str, max_rows: usize) -> Result<ResultSet> {
        guard::check(sql, Kind::Postgres, self.access.is_read_only())?;
        reject_copy(sql)?;
        self.collect(sql, max_rows)
    }
    fn query_stream(&mut self, sql: &str, sink: &mut dyn RowSink) -> Result<u64> {
        guard::check(sql, Kind::Postgres, true)?;
        reject_copy(sql)?;
        self.run(sql, true, sink)
    }
    fn fetch_page(&mut self, req: &PageRequest) -> Result<ResultSet> {
        let (sql, has_param) = page_sql(Kind::Postgres, req, "$1")?;
        let sql = match (&req.filter, has_param) {
            (Some((_, v)), true) => bind(&sql, &[Some(v)])?,
            _ => sql,
        };
        let mut c = Collector::new(req.limit as usize);
        self.run(&sql, self.access.is_read_only(), &mut c)?;
        Ok(c.rs)
    }
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        Some(Box::new(KillCancel(self.slot.clone())))
    }
}

#[cfg(test)]
mod tests;
