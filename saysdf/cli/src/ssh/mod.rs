//! Writing the ssh config for `saysdf ssh add` (change ssh-connections, design D4/D5).
//!
//! User files are never rewritten: new `Host` blocks go only to our own file
//! (`~/.ssh/saysdfterm.conf`); `~/.ssh/config` gets a single `Include` line as its first line,
//! after confirmation, with a backup, preserving the BOM and line endings; a symlink is left alone.
//! Passwords and passphrases are not stored (keys/agent). If that is ever needed, use `saysdf-secrets`
//! (secret-storage, shared with database-viewer); the integration point is [`HostEntry`].

pub mod meta;

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-ssh");

/// Name of our own file in `~/.ssh`.
pub const OWN_FILE: &str = "saysdfterm.conf";
/// The line inserted into `~/.ssh/config`.
pub const INCLUDE_LINE: &str = "Include saysdfterm.conf";
const HEADER: &str = "# managed by saysdfTerm (saysdf ssh add): only Host blocks added by the form";

/// Fields of a `Host` block (no secrets).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostEntry {
    pub alias: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity: Option<String>,
}

#[derive(Debug)]
pub enum Error {
    /// Field, error text.
    Invalid(&'static str, String),
    Duplicate(String),
    /// `~/.ssh/config` is a symlink: the line has to be added by hand.
    Symlink(PathBuf),
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(field, msg) => write!(f, "{field}: {msg}"),
            Error::Duplicate(a) => {
                write!(f, "{}", I18N.f("alias \"{alias}\" is already in ssh config", &[("alias", a)]))
            }
            Error::Symlink(p) => write!(
                f,
                "{}",
                I18N.f(
                    "{path} is a symbolic link, not modified; add this as the first line by hand: {line}",
                    &[("path", &p.display()), ("line", &INCLUDE_LINE)]
                )
            ),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// No control characters (line breaks, tabs, ESC…).
pub fn check_text(field: &'static str, s: &str) -> Result<()> {
    if s.chars().any(char::is_control) {
        return Err(Error::Invalid(field, I18N.t("line breaks and control characters are not allowed").into_owned()));
    }
    Ok(())
}

pub fn validate_alias(a: &str) -> Result<()> {
    if a.is_empty() {
        return Err(Error::Invalid("alias", I18N.t("required field").into_owned()));
    }
    if !a.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(Error::Invalid(
            "alias",
            I18N.t("only Latin letters, digits and . _ - (no spaces or *?!)").into_owned(),
        ));
    }
    Ok(())
}

pub fn parse_port(s: &str) -> Result<Option<u16>> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    match s.parse::<u16>() {
        Ok(p) if p >= 1 => Ok(Some(p)),
        _ => Err(Error::Invalid("port", I18N.t("port must be a number from 1 to 65535").into_owned())),
    }
}

/// Host: a name, IPv4 or IPv6 (no `user@`, spaces, `$` or quotes).
pub fn validate_hostname(h: &str) -> Result<()> {
    let bad = |m: &str| Err(Error::Invalid("host", m.into()));
    check_text("host", h)?;
    if h.is_empty() {
        return bad(&I18N.t("required field"));
    }
    if h.contains('@') {
        return bad(&I18N.t(
            "HostName must not contain \"@\" — enter the login in the User field (user@host is split automatically)",
        ));
    }
    if h.chars().any(char::is_whitespace) {
        return bad(&I18N.t("HostName must not contain spaces — enter only the address, not the rest of the ssh command"));
    }
    if h.contains(['$', '%', '"', '\'', '`', '\\']) {
        return bad(&I18N.t("HostName may only be a host name, IPv4 or IPv6 (no $, %, quotes or backslash)"));
    }
    if !h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')) {
        return bad(&I18N.t("HostName may only contain Latin letters, digits, . - _ and \":\" for IPv6"));
    }
    Ok(())
}

pub fn validate_user(u: &str) -> Result<()> {
    let bad = |m: &str| Err(Error::Invalid("user", m.into()));
    check_text("user", u)?;
    if u.contains('@') {
        return bad(&I18N.t("User must not contain \"@\""));
    }
    if u.contains(['$', '%']) {
        return bad(&I18N.t(
            "User must not contain $ or % — it is the login name on the server (for example deploy), not a variable or a path",
        ));
    }
    if u.chars().any(char::is_whitespace) || u.contains(['"', '\'', '`', '\\']) {
        return bad(&I18N.t("User must not contain spaces or quotes"));
    }
    Ok(())
}

/// `user@host` -> (user, host); `None` without `@`. Splits at the last `@`.
pub fn split_user_host(s: &str) -> Option<(String, String)> {
    let (u, h) = s.trim().rsplit_once('@')?;
    (!u.is_empty() && !h.is_empty()).then(|| (u.to_string(), h.to_string()))
}

/// Expand `~`, `$env:NAME`, `%NAME%`, `$NAME`, `${NAME}`; `get` is the variable source.
pub fn expand_path_with(s: &str, get: &dyn Fn(&str) -> Option<String>) -> String {
    let mut rest = s.trim().to_string();
    if rest == "~" || rest.starts_with("~/") || rest.starts_with("~\\") {
        if let Some(h) = get("USERPROFILE").or_else(|| get("HOME")) {
            rest = format!("{h}{}", &rest[1..]);
        }
    }
    let chars: Vec<char> = rest.chars().collect();
    let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '%' {
            if let Some(len) = chars[i + 1..].iter().position(|&x| x == '%') {
                let name: String = chars[i + 1..i + 1 + len].iter().collect();
                if !name.is_empty() && name.chars().all(is_name) {
                    if let Some(v) = get(&name) {
                        out.push_str(&v);
                        i += len + 2;
                        continue;
                    }
                }
            }
        } else if c == '$' {
            let mut j = i + 1;
            let pre: String = chars[j..chars.len().min(j + 4)].iter().collect();
            if pre.eq_ignore_ascii_case("env:") {
                j += 4;
            }
            let braced = chars.get(j) == Some(&'{');
            if braced {
                j += 1;
            }
            let st = j;
            while j < chars.len() && is_name(chars[j]) {
                j += 1;
            }
            let name: String = chars[st..j].iter().collect();
            let closed = !braced || chars.get(j) == Some(&'}');
            if braced && closed {
                j += 1;
            }
            if closed && !name.is_empty() {
                if let Some(v) = get(&name) {
                    out.push_str(&v);
                    i = j;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn env_get(n: &str) -> Option<String> {
    std::env::var(n).ok().filter(|v| !v.is_empty())
}

/// Result of parsing a pasted `ssh ...` command (the command is NOT executed).
#[derive(Debug, Default, PartialEq)]
pub struct ParsedCommand {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    pub identity: Option<String>,
}

/// Words honouring `"..."` and `'...'`; a backslash does not escape (Windows paths).
fn shell_words(s: &str) -> std::result::Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut q: Option<char> = None;
    for c in s.chars() {
        match q {
            Some(qc) if c == qc => q = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                q = Some(c);
                has = true;
            }
            None if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            None => cur.push(c),
        }
    }
    if q.is_some() {
        return Err(I18N.t("unclosed quote in the ssh command").into_owned());
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Parse `ssh -i "<path>" [-p N] [-l user] user@host`. Text only, nothing is run.
pub fn parse_ssh_command(cmd: &str) -> Result<ParsedCommand> {
    let bad = |m: String| Error::Invalid("command", m);
    let mut it = shell_words(cmd).map_err(bad)?.into_iter().peekable();
    let is_ssh = it
        .peek()
        .is_some_and(|w| w.rsplit(['/', '\\']).next().unwrap_or("").trim_end_matches(".exe") == "ssh");
    if is_ssh {
        it.next();
    }
    let mut r = ParsedCommand::default();
    let mut target: Option<String> = None;
    while let Some(w) = it.next() {
        let flag_val = |flag: &str, it: &mut std::iter::Peekable<std::vec::IntoIter<String>>| -> Result<String> {
            if w.len() > 2 {
                return Ok(w[2..].to_string());
            }
            it.next().ok_or_else(|| bad(I18N.f("no value after {flag}", &[("flag", &flag)])))
        };
        if w.starts_with("-i") {
            r.identity = Some(flag_val("-i", &mut it)?);
        } else if w.starts_with("-p") {
            let v = flag_val("-p", &mut it)?;
            r.port = parse_port(&v).map_err(|_| bad(I18N.f("port \"{port}\" must be a number from 1 to 65535", &[("port", &v)])))?;
        } else if w.starts_with("-l") {
            r.user = Some(flag_val("-l", &mut it)?);
        } else if w.starts_with('-') {
            if matches!(w.as_str(), "-o" | "-F" | "-J" | "-L" | "-R" | "-D" | "-b" | "-c" | "-E" | "-m") {
                it.next();
            }
        } else if target.is_none() {
            target = Some(w);
        } else {
            return Err(bad(I18N.t("extra words after the server address (a remote command is not supported)").into_owned()));
        }
    }
    let t = target
        .ok_or_else(|| bad(I18N.t("no server address in the command (expected ssh -i <key> user@host)").into_owned()))?;
    match split_user_host(&t) {
        Some((u, h)) => {
            r.user = Some(u);
            r.host = h;
        }
        None => r.host = t,
    }
    Ok(r)
}

impl HostEntry {
    /// Normalize the input: `user@host` -> User+HostName, `~`/`$env:`/`%VAR%` in
    /// IdentityFile. Returns notes about the replacements made.
    pub fn normalize(&mut self) -> Vec<String> {
        self.normalize_with(&env_get)
    }

    pub fn normalize_with(&mut self, get: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
        let mut notes = Vec::new();
        self.hostname = self.hostname.trim().to_string();
        if let Some((u, h)) = split_user_host(&self.hostname) {
            match self.user.as_deref().filter(|x| !x.is_empty()) {
                Some(old) if old != u => notes.push(I18N.f(
                    "HostName contained the login \"{login}\"; the User field (\"{old}\") is kept, the login from HostName is dropped",
                    &[("login", &u), ("old", &old)],
                )),
                _ => {
                    notes.push(I18N.f(
                        "\"{value}\" was split: User = {user}, HostName = {host}",
                        &[("value", &self.hostname), ("user", &u), ("host", &h)],
                    ));
                    self.user = Some(u);
                }
            }
            self.hostname = h;
        }
        if let Some(i) = self.identity.clone().filter(|i| !i.trim().is_empty()) {
            let e = expand_path_with(&i, get).replace('\\', "/");
            if e != i.trim() {
                notes.push(I18N.f("IdentityFile expanded: {path}", &[("path", &e)]));
            }
            self.identity = Some(e);
        }
        notes
    }

    /// Warnings (not errors): the key file is not on disk.
    pub fn warnings(&self) -> Vec<String> {
        match self.identity.as_deref().filter(|i| !i.is_empty()) {
            Some(i) if !Path::new(i).is_file() => {
                vec![I18N.f("warning: key file not found: {path} — check the path (the entry was saved)", &[("path", &i)])]
            }
            _ => Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_alias(&self.alias)?;
        validate_hostname(&self.hostname)?;
        if let Some(u) = &self.user {
            validate_user(u)?;
        }
        if self.port == Some(0) {
            return Err(Error::Invalid("port", I18N.t("a number from 1 to 65535").into_owned()));
        }
        if let Some(i) = &self.identity {
            check_text("identity", i)?;
            if i.contains('"') {
                return Err(Error::Invalid("identity", I18N.t("no quotes").into_owned()));
            }
        }
        Ok(())
    }
}

/// A `Host` block (line endings are `nl`).
pub fn render_host_block(e: &HostEntry, nl: &str) -> String {
    let mut s = format!("Host {}{nl}    HostName {}{nl}", e.alias, e.hostname);
    if let Some(u) = e.user.as_deref().filter(|u| !u.is_empty()) {
        s.push_str(&format!("    User {u}{nl}"));
    }
    if let Some(p) = e.port {
        s.push_str(&format!("    Port {p}{nl}"));
    }
    if let Some(i) = e.identity.as_deref().filter(|i| !i.is_empty()) {
        if i.contains(' ') {
            s.push_str(&format!("    IdentityFile \"{i}\"{nl}"));
        } else {
            s.push_str(&format!("    IdentityFile {i}{nl}"));
        }
    }
    s
}

/// Literal aliases from `Host ...` lines (no patterns or negations).
pub fn literal_hosts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let (kw, rest) = split_kw(line);
        if kw.eq_ignore_ascii_case("host") {
            for a in rest.split_whitespace() {
                let a = a.trim_matches('"');
                if !a.is_empty() && !a.contains(['*', '?', '!']) {
                    out.push(a.to_string());
                }
            }
        }
    }
    out
}

/// `Keyword value` / `Keyword=value` → (keyword, value).
fn split_kw(line: &str) -> (&str, &str) {
    let t = line.trim_start_matches('\u{feff}').trim();
    if t.starts_with('#') {
        return ("", "");
    }
    let end = t.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(t.len());
    let rest = t[end..].trim_start().trim_start_matches('=').trim();
    (&t[..end], rest)
}

fn newline_of(bytes: &[u8]) -> &'static str {
    if bytes.windows(2).any(|w| w == b"\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Write via a temporary file in the same folder plus rename; `private` means 0600 on Unix.
/// The permissions of an existing file are preserved.
pub fn write_atomic(path: &Path, data: &[u8], private: bool) -> io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-saysdf-{}", std::process::id()));
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        if let Ok(m) = fs::metadata(path) {
            fs::set_permissions(&tmp, m.permissions())?;
        } else if private {
            set_private(&tmp)?;
        }
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

#[cfg(unix)]
fn set_private(p: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o600))
}

/// Windows: the inherited ACLs of the user profile (design D4).
#[cfg(not(unix))]
fn set_private(_p: &Path) -> io::Result<()> {
    Ok(())
}

fn ensure_dir(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() || dir.exists() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Append a `Host` block to our own file. `known` are the aliases ssh already sees (from `--existing-file`).
/// A duplicate is an error with nothing written; the existing bytes of the file are not changed.
pub fn append_host(path: &Path, entry: &HostEntry, known: &[String]) -> Result<()> {
    entry.validate()?;
    let dup = |a: &String| a.eq_ignore_ascii_case(&entry.alias);
    if known.iter().any(dup) {
        return Err(Error::Duplicate(entry.alias.clone()));
    }
    let old = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    // re-check right before writing (two forms open at once)
    if literal_hosts(&String::from_utf8_lossy(&old)).iter().any(dup) {
        return Err(Error::Duplicate(entry.alias.clone()));
    }
    let nl = if old.is_empty() { "\n" } else { newline_of(&old) };
    let mut data = old.clone();
    if data.is_empty() {
        data.extend_from_slice(HEADER.as_bytes());
        data.extend_from_slice(nl.as_bytes());
    } else if !data.ends_with(b"\n") {
        data.extend_from_slice(nl.as_bytes());
    }
    data.extend_from_slice(nl.as_bytes());
    data.extend_from_slice(render_host_block(entry, nl).as_bytes());
    if let Some(dir) = path.parent() {
        ensure_dir(dir)?;
    }
    write_atomic(path, &data, true)?;
    Ok(())
}

/// Whether `config` includes our file (an `Include` with any path ending in `saysdfterm.conf`).
pub fn has_include(text: &str) -> bool {
    text.lines().any(|line| {
        let (kw, rest) = split_kw(line);
        kw.eq_ignore_ascii_case("include")
            && rest.split_whitespace().any(|p| {
                let p = p.trim_matches('"').replace('\\', "/");
                p == OWN_FILE || p.ends_with(&format!("/{OWN_FILE}"))
            })
    })
}

/// State of the `Include` in the user config.
pub fn include_present(config: &Path) -> bool {
    fs::read(config).map(|b| has_include(&String::from_utf8_lossy(&b))).unwrap_or(false)
}

/// Insert `line` as the first line of `config`. `Ok(None)` means already included (no-op),
/// `Ok(Some(backup))` means inserted (with a backup if the file existed). A symlink gives [`Error::Symlink`].
pub fn ensure_include(config: &Path, line: &str) -> Result<Option<Option<PathBuf>>> {
    match fs::symlink_metadata(config) {
        Ok(m) if m.file_type().is_symlink() => return Err(Error::Symlink(config.to_path_buf())),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(dir) = config.parent() {
                ensure_dir(dir)?;
            }
            write_atomic(config, format!("{line}\n").as_bytes(), true)?;
            return Ok(Some(None));
        }
        Err(e) => return Err(e.into()),
    }
    let old = fs::read(config)?;
    if has_include(&String::from_utf8_lossy(&old)) {
        return Ok(None);
    }
    const BOM: &[u8] = b"\xEF\xBB\xBF";
    let (bom, body) = match old.strip_prefix(BOM) {
        Some(rest) => (BOM, rest),
        None => (&[][..], &old[..]),
    };
    let nl = newline_of(body);
    let backup = backup_path(config);
    fs::copy(config, &backup)?;
    let mut data = Vec::with_capacity(old.len() + line.len() + 2);
    data.extend_from_slice(bom);
    data.extend_from_slice(line.as_bytes());
    data.extend_from_slice(nl.as_bytes());
    data.extend_from_slice(body);
    write_atomic(config, &data, true)?;
    Ok(Some(Some(backup)))
}

fn backup_path(config: &Path) -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86400) as i64);
    let t = secs % 86400;
    let name = config.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let base = format!(
        "{name}.bak-saysdf-{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        t / 3600,
        t / 60 % 60,
        t % 60
    );
    let mut p = config.with_file_name(&base);
    let mut n = 1;
    while p.exists() {
        p = config.with_file_name(format!("{base}-{n}"));
        n += 1;
    }
    p
}

/// Days since 1970-01-01 -> (year, month, day), UTC (H. Hinnant).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Home folder: on Windows `USERPROFILE`, then `HOME`; otherwise `HOME`.
pub fn home_dir() -> Option<PathBuf> {
    saysdf_identity::home_dir()
}

#[cfg(test)]
mod tests;
