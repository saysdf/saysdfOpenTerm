//! Viewer control file (text-annotations D6): `saysdf.viewer` reuses a viewer split by writing
//! `<runtime_dir>/viewer/<token>.json` = `{subcommand, path, line, col}` and activating the pane.
//! The pager polls that one path with a single `metadata` call per tick; a present file is read
//! (size-capped), deleted and validated as untrusted data. The same subcommand
//! opens the file in place, another one ends the pager with a switch request that [`dispatch`]
//! turns into that subcommand in the same process and pane.

use crate::bridge;
use crate::cmd;
use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Upper bound of a request file; a bigger one is not read.
pub const MAX_SIZE: u64 = 4096;
/// Sanity bounds of `line` / `col` (1-based).
const MAX_LINE: u64 = 100_000_000;
const MAX_COL: u64 = 1_000_000;

/// Subcommands a request may name (the extension -> subcommand table stays in Lua).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sub {
    View,
    Md,
    Mermaid,
}

impl Sub {
    pub fn name(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Md => "md",
            Self::Mermaid => "mermaid",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::View, Self::Md, Self::Mermaid].into_iter().find(|v| v.name() == s)
    }
}

/// A validated request: `line`/`col` are 1-based as written by Lua.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub sub: Sub,
    pub path: PathBuf,
    pub line: Option<usize>,
    pub col: Option<usize>,
}

impl Request {
    /// 0-based source line for the pager.
    pub fn line0(&self) -> usize {
        self.line.unwrap_or(1).saturating_sub(1)
    }
}

/// Why a request file was ignored (logged; the user sees one status line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
    TooLarge,
    Read(String),
    Json,
    Subcommand,
    Path,
    NotAFile,
    Position,
}

/// Parses and validates the request bytes. The path must be absolute, plain (no control or
/// invisible characters) and an existing regular file.
pub fn parse(bytes: &[u8]) -> Result<Request, Invalid> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Invalid::Json)?;
    let obj = v.as_object().ok_or(Invalid::Json)?;
    let sub = obj.get("subcommand").and_then(|s| s.as_str()).ok_or(Invalid::Json)?;
    let sub = Sub::parse(sub).ok_or(Invalid::Subcommand)?;
    let path = obj.get("path").and_then(|s| s.as_str()).ok_or(Invalid::Json)?;
    let bad = |c: char| c.is_control() || saysdf_i18n::text::is_invisible_format(c);
    let path = PathBuf::from(path);
    if path.as_os_str().is_empty() || !path.is_absolute() || path.to_string_lossy().chars().any(bad) {
        return Err(Invalid::Path);
    }
    let pos = |key: &str, max: u64| -> Result<Option<usize>, Invalid> {
        match obj.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(n) => match n.as_u64() {
                Some(n) if (1..=max).contains(&n) => Ok(Some(n as usize)),
                _ => Err(Invalid::Position),
            },
        }
    };
    let line = pos("line", MAX_LINE)?;
    let col = pos("col", MAX_COL)?;
    if !fs::metadata(&path).is_ok_and(|m| m.is_file()) {
        return Err(Invalid::NotAFile);
    }
    Ok(Request { sub, path, line, col })
}

/// Identity of a request file version: size and modification time.
pub type Stamp = (u64, Option<std::time::SystemTime>);

/// The control file of one viewer process.
pub struct ControlFile {
    path: PathBuf,
}

impl ControlFile {
    /// `<runtime>/viewer/<token>.json`; `None` for a token that is not a valid bridge token.
    pub fn new(runtime: &Path, token: &str) -> Option<Self> {
        bridge::valid_token(token).then(|| Self { path: runtime.join("viewer").join(format!("{token}.json")) })
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// One tick: `None` when there is no file (a single `symlink_metadata` call). A present
    /// entry is always removed, whatever it holds.
    pub fn poll(&self) -> Option<Result<Request, Invalid>> {
        let meta = fs::symlink_metadata(&self.path).ok()?;
        let res = if !meta.is_file() {
            Err(Invalid::NotAFile)
        } else if meta.len() > MAX_SIZE {
            Err(Invalid::TooLarge)
        } else {
            self.read().and_then(|b| parse(&b))
        };
        if meta.is_dir() {
            let _ = fs::remove_dir(&self.path);
        } else {
            let _ = fs::remove_file(&self.path);
        }
        if let Err(e) = &res {
            crate::log::slog!("control: ignored {:?}: {e:?}", self.path);
        }
        Some(res)
    }

    /// Size and mtime of a waiting request (one `symlink_metadata` call); `None` when absent.
    pub fn stamp(&self) -> Option<Stamp> {
        let m = fs::symlink_metadata(&self.path).ok()?;
        Some((m.len(), m.modified().ok()))
    }

    /// Reads a waiting request without removing it (the viewer is busy: the request is held
    /// until the user's input or confirmation ends; a newer write by Lua replaces it).
    pub fn peek(&self) -> Option<Result<Request, Invalid>> {
        let meta = fs::symlink_metadata(&self.path).ok()?;
        Some(if !meta.is_file() {
            Err(Invalid::NotAFile)
        } else if meta.len() > MAX_SIZE {
            Err(Invalid::TooLarge)
        } else {
            self.read().and_then(|b| parse(&b))
        })
    }

    fn read(&self) -> Result<Vec<u8>, Invalid> {
        let f = fs::File::open(&self.path).map_err(|e| Invalid::Read(e.to_string()))?;
        let mut buf = Vec::new();
        f.take(MAX_SIZE + 1).read_to_end(&mut buf).map_err(|e| Invalid::Read(e.to_string()))?;
        if buf.len() as u64 > MAX_SIZE {
            return Err(Invalid::TooLarge);
        }
        Ok(buf)
    }
}

/// Control of one pager run: the file and the subcommand that is showing.
pub struct Control {
    pub file: ControlFile,
    pub sub: Sub,
}

/// State kept across subcommand switches in one process.
pub struct Session {
    /// Viewer token, taken out of the environment once (children do not inherit it).
    pub token: Option<String>,
    runtime: Option<PathBuf>,
    /// Set by a subcommand whose pager ended with a switch request.
    pub next: Option<Request>,
    /// `--editor-cmd` of the first launch, carried to the next subcommand.
    editor_cmd: Option<String>,
}

impl Session {
    pub fn from_env() -> Self {
        Self {
            token: bridge::take_token(cmd::view::VIEWER_TOKEN_ENV),
            runtime: saysdf_identity::runtime_dir(),
            next: None,
            editor_cmd: None,
        }
    }

    #[cfg(test)]
    pub fn for_test(token: Option<&str>, runtime: Option<&Path>) -> Self {
        Self { token: token.map(Into::into), runtime: runtime.map(Into::into), next: None, editor_cmd: None }
    }

    /// Control for a pager showing `sub`; `None` when saysdfTerm did not open this viewer.
    pub fn control(&self, sub: Sub) -> Option<Control> {
        let file = ControlFile::new(self.runtime.as_deref()?, self.token.as_deref()?)?;
        Some(Control { file, sub })
    }
}

/// The subcommands that can take part in a switch.
#[derive(Subcommand)]
pub enum Launch {
    View(cmd::view::ViewArgs),
    Md(cmd::md::MdArgs),
    Mermaid(cmd::mermaid::MermaidArgs),
}

#[derive(Parser)]
#[command(name = "saysdf")]
struct One {
    #[command(subcommand)]
    launch: Launch,
}

impl Launch {
    fn editor_cmd(&self) -> Option<&String> {
        match self {
            Self::View(a) => a.editor_cmd.as_ref(),
            Self::Md(a) => a.editor_cmd.as_ref(),
            Self::Mermaid(a) => a.editor_cmd.as_ref(),
        }
    }

    /// Arguments of the next subcommand: the request plus the carried `--editor-cmd`; the rest
    /// takes the subcommand's defaults (clap fills them, so they stay in one place).
    pub fn from_request(req: &Request, editor_cmd: Option<&str>) -> Option<Self> {
        let mut argv: Vec<OsString> = vec!["saysdf".into(), req.sub.name().into()];
        if let Some(l) = req.line {
            argv.extend(["--line".into(), l.to_string().into()]);
        }
        if let Some(c) = req.col {
            argv.extend(["--col".into(), c.to_string().into()]);
        }
        if let Some(e) = editor_cmd {
            argv.extend(["--editor-cmd".into(), e.into()]);
        }
        argv.extend(["--".into(), req.path.clone().into_os_string()]);
        One::try_parse_from(argv).ok().map(|o| o.launch)
    }
}

/// Runs `launch` and every subcommand a pager switches to, in this process and pane.
pub fn dispatch(launch: Launch) -> i32 {
    run_loop(launch, Session::from_env(), |l, s| match l {
        Launch::View(a) => cmd::view::run_in(a, s),
        Launch::Md(a) => cmd::md::run_in(a, s),
        Launch::Mermaid(a) => cmd::mermaid::run_in(a, s),
    })
}

fn run_loop(mut launch: Launch, mut s: Session, mut run: impl FnMut(Launch, &mut Session) -> i32) -> i32 {
    s.editor_cmd = launch.editor_cmd().cloned();
    loop {
        let code = run(launch, &mut s);
        let Some(req) = s.next.take() else { return code };
        match Launch::from_request(&req, s.editor_cmd.as_deref()) {
            Some(l) => launch = l,
            None => return code,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123abcd-ef";

    fn setup() -> (tempfile::TempDir, ControlFile, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let cf = ControlFile::new(dir.path(), TOKEN).unwrap();
        fs::create_dir_all(cf.path().parent().unwrap()).unwrap();
        let target = dir.path().join("a b.rs");
        fs::write(&target, "x\ny\nz\n").unwrap();
        (dir, cf, target)
    }

    fn request(sub: &str, path: &Path, line: u64) -> String {
        serde_json::json!({ "subcommand": sub, "path": path, "line": line, "col": 2 }).to_string()
    }

    #[test]
    fn absent_file_is_nothing() {
        let (_d, cf, _) = setup();
        assert!(cf.poll().is_none());
        assert!(cf.poll().is_none());
    }

    #[test]
    fn valid_request_is_read_once_and_deleted() {
        let (_d, cf, target) = setup();
        assert_eq!(cf.path().file_name().unwrap(), "0123abcd-ef.json");
        fs::write(cf.path(), request("view", &target, 3)).unwrap();
        let req = cf.poll().unwrap().unwrap();
        assert_eq!(req, Request { sub: Sub::View, path: target, line: Some(3), col: Some(2) });
        assert_eq!(req.line0(), 2);
        assert!(!cf.path().exists());
        assert!(cf.poll().is_none());
    }

    #[test]
    fn invalid_requests_are_ignored_and_deleted() {
        let (d, cf, target) = setup();
        let missing = d.path().join("missing.rs");
        let big = format!("{{\"subcommand\":\"view\",\"path\":{:?},\"pad\":\"{}\"}}", target, "x".repeat(5000));
        let cases: Vec<(String, Invalid)> = vec![
            (big, Invalid::TooLarge),
            ("{not json".into(), Invalid::Json),
            ("[1,2]".into(), Invalid::Json),
            (request("rm", &target, 1), Invalid::Subcommand),
            (request("view", Path::new("rel/a.rs"), 1), Invalid::Path),
            (request("view", &missing, 1), Invalid::NotAFile),
            (request("view", d.path(), 1), Invalid::NotAFile),
            (request("view", &target, 0), Invalid::Position),
            (request("view", &target.with_file_name("a\u{202e}b.rs"), 1), Invalid::Path),
        ];
        for (body, want) in cases {
            fs::write(cf.path(), &body).unwrap();
            assert_eq!(cf.poll(), Some(Err(want)), "{body:.60}");
            assert!(!cf.path().exists());
        }
        // a directory in place of the file is removed too
        fs::create_dir(cf.path()).unwrap();
        assert_eq!(cf.poll(), Some(Err(Invalid::NotAFile)));
        assert!(!cf.path().exists());
    }

    #[test]
    fn bad_token_has_no_control_file() {
        let d = tempfile::tempdir().unwrap();
        assert!(ControlFile::new(d.path(), "../x").is_none());
        assert!(Session::for_test(None, Some(d.path())).control(Sub::View).is_none());
        assert!(Session::for_test(Some(TOKEN), None).control(Sub::View).is_none());
        assert!(Session::for_test(Some(TOKEN), Some(d.path())).control(Sub::Md).is_some());
    }

    #[test]
    fn switch_runs_the_next_subcommand_with_carried_editor() {
        let (_d, _cf, target) = setup();
        let first = Launch::from_request(
            &Request { sub: Sub::View, path: target.clone(), line: None, col: None },
            Some("[\"ed\",\"{path}\"]"),
        )
        .unwrap();
        let mut seen = Vec::new();
        let code = run_loop(first, Session::for_test(Some(TOKEN), None), |l, s| {
            match &l {
                Launch::View(a) => {
                    seen.push(("view", a.line, a.editor_cmd.clone()));
                    s.next = Some(Request { sub: Sub::Md, path: target.clone(), line: Some(7), col: None });
                }
                Launch::Md(a) => seen.push(("md", a.line, a.editor_cmd.clone())),
                Launch::Mermaid(_) => seen.push(("mermaid", None, None)),
            }
            5
        });
        assert_eq!(code, 5);
        let ed = Some("[\"ed\",\"{path}\"]".to_string());
        assert_eq!(seen, vec![("view", None, ed.clone()), ("md", Some(7), ed)]);
    }
}
