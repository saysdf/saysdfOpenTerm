//! `saysdf diff` (design diff-merge-tool D3–D5, D7): git diff of the working tree / index / commits,
//! diff of two files or folders; side-by-side and inline in the shared pager, stage/unstage of a hunk.

use crate::git::Git;
use crate::pager::{self, Content, Nav};
use saysdf_keys::Key;
use clap::Args;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use saysdf_render::diff::{self, FileDiff, Kind, Layout, RenderOpts, Row};
use saysdf_render::doc::Doc;
use saysdf_render::highlight::Highlighter;
use saysdf_render::sanitize::{visible_line, DEFAULT_TAB_WIDTH};
use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-diff");

#[derive(Args)]
pub struct DiffArgs {
    /// Two files or two folders; otherwise git revisions (`HEAD~1`, `A B`); paths after `--`
    pub args: Vec<String>,
    /// Index <-> HEAD (`git diff --cached`)
    #[arg(long, alias = "cached")]
    pub staged: bool,
    /// Working directory (git mode)
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    /// Inline view instead of side-by-side
    #[arg(long)]
    pub inline: bool,
    /// Do not fold unchanged regions
    #[arg(long)]
    pub no_fold: bool,
    /// Non-interactive: unified diff to stdout (exit code 1 means there are differences)
    #[arg(long)]
    pub print: bool,
    /// Write the `saysdf` difftool to git config (after a y/N confirmation)
    #[arg(long)]
    pub install_git_tool: bool,
    /// With `--install-git-tool`: write to the repository config instead of `--global`
    #[arg(long)]
    pub local: bool,
    /// Tab width
    #[arg(long, default_value_t = DEFAULT_TAB_WIDTH)]
    pub tab_width: usize,
    /// Paths (git mode)
    #[arg(last = true)]
    pub paths: Vec<String>,
}

/// Where the diff comes from.
#[derive(Clone)]
enum Source {
    Files { a: PathBuf, b: PathBuf },
    Dirs { a: PathBuf, b: PathBuf },
    Git { root: PathBuf, staged: bool, revs: Vec<String>, paths: Vec<String> },
}

impl Source {
    fn load(&self) -> Result<Vec<FileDiff>, String> {
        match self {
            Source::Files { a, b } => {
                let ra = std::fs::read(a).map_err(|e| format!("{}: {e}", a.display()))?;
                let rb = std::fs::read(b).map_err(|e| format!("{}: {e}", b.display()))?;
                let (pa, pb) = (a.to_string_lossy(), b.to_string_lossy());
                Ok(vec![diff::diff_bytes(&ra, &rb, Some(&pa), Some(&pb))])
            }
            Source::Dirs { a, b } => diff_dirs(a, b),
            Source::Git { root, staged, revs, paths } => git_diff(root, *staged, revs, paths),
        }
    }

    /// Stage/unstage is possible: git mode without revisions.
    fn stage_mode(&self) -> Option<(&Path, bool)> {
        match self {
            Source::Git { root, staged, revs, .. } if revs.is_empty() => Some((root, *staged)),
            _ => None,
        }
    }
}

/// `git diff -U3 --no-ext-diff --no-textconv` + untracked (working tree).
fn git_diff(root: &Path, staged: bool, revs: &[String], paths: &[String]) -> Result<Vec<FileDiff>, String> {
    let mut args: Vec<String> = ["diff", "-U3", "--no-ext-diff", "--no-textconv", "--no-color", "--no-renames"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if staged {
        args.push("--cached".into());
    }
    args.extend(revs.iter().cloned());
    args.push("--".into());
    args.extend(paths.iter().cloned());
    let out = Git::new().dir(root).run(&args).map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(out.stderr_str());
    }
    let mut files = diff::parse_unified(&out.stdout_str());
    if !staged && revs.is_empty() {
        let mut a = vec!["ls-files".to_string(), "--others".into(), "--exclude-standard".into(), "-z".into(), "--".into()];
        a.extend(paths.iter().cloned());
        if let Ok(o) = Git::new().dir(root).run(&a) {
            for p in o.stdout.split(|&b| b == 0).filter(|p| !p.is_empty()) {
                let p = String::from_utf8_lossy(p).into_owned();
                let data = std::fs::read(root.join(&p)).unwrap_or_default();
                let mut f = diff::diff_bytes(b"", &data, None, Some(&p));
                f.new_file_mode = Some("100644".into());
                files.push(f);
            }
        }
    }
    Ok(files)
}

fn walk(root: &Path, rel: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root.join(rel)) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        if name == ".git" {
            continue;
        }
        let r = rel.join(&name);
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(root, &r, out),
            Ok(_) => {
                out.insert(r);
            }
            Err(_) => {}
        }
    }
}

/// Diff of folders: recursive (without `.git`), only files that differ.
fn diff_dirs(a: &Path, b: &Path) -> Result<Vec<FileDiff>, String> {
    let (mut sa, mut sb) = (BTreeSet::new(), BTreeSet::new());
    walk(a, Path::new(""), &mut sa);
    walk(b, Path::new(""), &mut sb);
    let mut files = Vec::new();
    for rel in sa.union(&sb) {
        let p = rel.to_string_lossy().replace('\\', "/");
        let ra = sa.contains(rel).then(|| std::fs::read(a.join(rel)).unwrap_or_default());
        let rb = sb.contains(rel).then(|| std::fs::read(b.join(rel)).unwrap_or_default());
        if ra == rb {
            continue;
        }
        let f = diff::diff_bytes(
            ra.as_deref().unwrap_or(b""),
            rb.as_deref().unwrap_or(b""),
            ra.as_ref().map(|_| p.as_str()),
            rb.as_ref().map(|_| p.as_str()),
        );
        files.push(f);
    }
    Ok(files)
}

/// `git apply --cached [-R] --check`, then the real apply (patch through stdin).
fn git_apply(root: &Path, patch: &str, reverse: bool) -> Result<(), String> {
    for check in [true, false] {
        let mut args = vec!["apply", "--cached", "--whitespace=nowarn"];
        if reverse {
            args.push("-R");
        }
        if check {
            args.push("--check");
        }
        args.push("-");
        let mut child = Git::new()
            .dir(root)
            .command(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(mut si) = child.stdin.take() {
            si.write_all(patch.as_bytes()).map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }
    }
    Ok(())
}

struct DiffView {
    src: Source,
    files: Vec<FileDiff>,
    layout: Layout,
    fold: bool,
    tab_width: usize,
    width: usize,
    hscroll: usize,
    /// Render cache: (hscroll, width) for side-by-side.
    key: Option<(usize, usize)>,
    lines: Vec<Line<'static>>,
    /// View line -> (file, render row of the file).
    rows: Vec<(usize, Row)>,
    file_rows: Vec<usize>,
    hunk_rows: Vec<(usize, usize, usize)>,
    /// Selected lines: (file, hunk, line).
    selected: BTreeSet<(usize, usize, usize)>,
    doc: Doc,
}

impl DiffView {
    fn new(src: Source, files: Vec<FileDiff>, layout: Layout, fold: bool, tab_width: usize) -> Self {
        let mut v = Self {
            src,
            files,
            layout,
            fold,
            tab_width,
            width: 120,
            hscroll: 0,
            key: None,
            lines: Vec::new(),
            rows: Vec::new(),
            file_rows: Vec::new(),
            hunk_rows: Vec::new(),
            selected: BTreeSet::new(),
            doc: Doc::default(),
        };
        v.rebuild();
        v
    }

    fn rebuild(&mut self) {
        let sbs = self.layout == Layout::SideBySide;
        let o = RenderOpts {
            layout: self.layout,
            fold: self.fold,
            side_width: self.width.saturating_sub(1) / 2,
            hscroll: if sbs { self.hscroll } else { 0 },
            tab_width: self.tab_width,
        };
        self.lines.clear();
        self.rows.clear();
        self.file_rows.clear();
        self.hunk_rows.clear();
        if self.files.is_empty() {
            self.lines.push(Line::raw(diff::no_differences()));
            self.rows.push((0, Row::Message));
        }
        for (fi, f) in self.files.iter().enumerate() {
            let path = Path::new(f.path());
            let mut hl = Highlighter::for_path(path, "", self.tab_width);
            let r = diff::render(f, Some(&mut hl), &o);
            let base = self.lines.len();
            self.file_rows.push(base);
            for (k, h) in r.hunk_rows.iter().enumerate() {
                self.hunk_rows.push((base + h, fi, k));
            }
            for (k, l) in r.lines.into_iter().enumerate() {
                // Side-by-side: the shift is already applied inside the sides; the pager will cut as many cells on the left
                let l = if sbs && self.hscroll > 0 {
                    let mut spans = vec![Span::raw(" ".repeat(self.hscroll))];
                    spans.extend(l.spans);
                    Line::from(spans)
                } else {
                    l
                };
                self.lines.push(l);
                self.rows.push((fi, r.rows[k]));
            }
            self.lines.push(Line::raw(""));
            self.rows.push((fi, Row::Message));
        }
        self.doc = Doc::from_lines(self.lines.clone());
        self.key = Some((if sbs { self.hscroll } else { 0 }, self.width));
    }

    fn hunk_at(&self, pos: usize) -> Option<(usize, usize)> {
        match self.rows.get(pos)? {
            (f, Row::HunkHeader(h) | Row::Line(h, _) | Row::Pair(h, _, _) | Row::Fold(h, _, _)) => Some((*f, *h)),
            _ => None,
        }
    }

    fn row_lines(&self, pos: usize) -> Vec<(usize, usize, usize)> {
        match self.rows.get(pos) {
            Some((f, Row::Line(h, i))) => vec![(*f, *h, *i)],
            Some((f, Row::Pair(h, a, b))) => a.iter().chain(b.iter()).map(|i| (*f, *h, *i)).collect::<BTreeSet<_>>().into_iter().collect(),
            _ => vec![],
        }
    }

    fn stage(&mut self, unstage: bool, pos: usize) -> Nav {
        let Some((root, staged)) = self.src.stage_mode().map(|(r, s)| (r.to_path_buf(), s)) else {
            return Nav::Status(I18N.t("stage/unstage is only available in git mode without revisions").into_owned());
        };
        if unstage != staged {
            return Nav::Status(if unstage { I18N.t("unstage works in the --staged view") } else { I18N.t("stage works in the working tree view (without --staged)") }.into_owned());
        }
        let Some((fi, hi)) = self.hunk_at(pos) else {
            return Nav::Status(I18N.t("the cursor is not on a hunk").into_owned());
        };
        let f = &self.files[fi];
        let sel: Vec<bool> = (0..f.hunks[hi].lines.len()).map(|i| self.selected.contains(&(fi, hi, i))).collect();
        let sel = sel.iter().any(|&b| b).then_some(sel);
        let Some(patch) = diff::hunk_patch(f, hi, sel.as_deref()) else {
            return Nav::Status(I18N.t("no changes in the selection").into_owned());
        };
        match git_apply(&root, &patch, unstage) {
            Ok(()) => {
                self.selected.clear();
                let msg = match self.src.load() {
                    Ok(files) => {
                        self.files = files;
                        self.rebuild();
                        if unstage { I18N.t("hunk removed from the index") } else { I18N.t("hunk added to the index") }.into_owned()
                    }
                    Err(e) => I18N.f("reload failed: {error}", &[("error", &e)]),
                };
                let pos = pos.min(self.lines.len().saturating_sub(1));
                Nav::Select(pos, msg)
            }
            Err(e) => Nav::Status(visible_line(&format!("git apply: {e}"), 4)),
        }
    }
}

impl Content for DiffView {
    fn len(&self) -> usize {
        self.lines.len()
    }

    fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
        let to = to.min(self.lines.len());
        let from = from.min(to);
        (from..to)
            .map(|i| {
                let l = self.lines[i].clone();
                if self.row_lines(i).iter().any(|k| self.selected.contains(k)) {
                    let mut spans = vec![Span::styled("▌", Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD))];
                    spans.extend(l.spans);
                    Line::from(spans)
                } else {
                    l
                }
            })
            .collect()
    }

    fn doc(&self) -> Option<&Doc> {
        Some(&self.doc)
    }

    fn reload(&mut self) -> io::Result<Option<String>> {
        let files = self.src.load().map_err(io::Error::other)?;
        self.files = files;
        self.selected.clear();
        self.rebuild();
        Ok(Some(I18N.t("reloaded").into_owned()))
    }

    fn set_width(&mut self, width: usize) {
        if width != self.width {
            self.width = width;
            self.rebuild();
        }
    }

    fn viewport(&mut self, _top: usize, hscroll: usize, _height: usize, width: usize) {
        self.hscroll = hscroll;
        self.width = width;
        let want = (if self.layout == Layout::SideBySide { hscroll } else { 0 }, width);
        if self.key != Some(want) {
            self.rebuild();
        }
    }

    fn key(&mut self, k: &KeyEvent, phys: Option<Key>, pos: usize) -> io::Result<Nav> {
        let is = |spec| crate::keys::is(k, phys, spec);

        let next = |rows: &mut dyn Iterator<Item = usize>, what: std::borrow::Cow<'static, str>| match rows.next() {
            Some(r) => Nav::Goto(r),
            None => Nav::Status(what.into_owned()),
        };
        Ok(match k.code {
            KeyCode::Tab => {
                self.layout = match self.layout {
                    Layout::SideBySide => Layout::Inline,
                    Layout::Inline => Layout::SideBySide,
                };
                self.rebuild();
                Nav::Status(match self.layout {
                    Layout::SideBySide => "side-by-side".into(),
                    Layout::Inline => "inline".into(),
                })
            }
            _ if is("z")? => {
                self.fold = !self.fold;
                self.rebuild();
                Nav::Status(if self.fold { I18N.t("folding on") } else { I18N.t("folding off") }.into_owned())
            }
            _ if is("]")? => next(&mut self.hunk_rows.iter().map(|h| h.0).filter(|&r| r > pos), I18N.t("no more hunks")),
            _ if is("[")? => next(&mut self.hunk_rows.iter().rev().map(|h| h.0).filter(|&r| r < pos), I18N.t("no more hunks")),
            _ if is("}")? => next(&mut self.file_rows.iter().copied().filter(|&r| r > pos), I18N.t("no more files")),
            _ if is("{")? => next(&mut self.file_rows.iter().rev().copied().filter(|&r| r < pos), I18N.t("no more files")),
            _ if is("v")? => {
                let ks: Vec<_> = self
                    .row_lines(pos)
                    .into_iter()
                    .filter(|&(f, h, i)| self.files[f].hunks[h].lines[i].kind != Kind::Context)
                    .collect();
                if ks.is_empty() {
                    return Ok(Nav::Status(I18N.t("only +/- lines can be selected").into_owned()));
                }
                for k in ks {
                    if !self.selected.remove(&k) {
                        self.selected.insert(k);
                    }
                }
                Nav::Goto((pos + 1).min(self.lines.len().saturating_sub(1)))
            }
            _ if is("s")? => self.stage(false, pos),
            _ if is("u")? => self.stage(true, pos),
            _ => Nav::None,
        })
    }
}

/// A value for git config: a command for sh (Git for Windows uses sh too), the path is absolute, with `/`.
pub fn tool_cmd(exe: &Path, sub: &str, vars: &[&str]) -> String {
    let p = exe.to_string_lossy().replace('\\', "/");
    let mut q = String::new();
    for c in p.chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            q.push('\\');
        }
        q.push(c);
    }
    let mut s = format!("\"{q}\" {sub}");
    for v in vars {
        s.push_str(&format!(" \"${v}\""));
    }
    s
}

/// Prints the keys, asks `y/N`, writes `git config` (design D7).
pub fn install_git_tool(kind: &str, local: bool) -> i32 {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("saysdf"));
    let mut keys: Vec<(String, String)> = Vec::new();
    if kind == "diff" {
        keys.push(("difftool.saysdf.cmd".into(), tool_cmd(&exe, "diff", &["LOCAL", "REMOTE"])));
    } else {
        keys.push(("mergetool.saysdf.cmd".into(), tool_cmd(&exe, "merge", &["BASE", "LOCAL", "REMOTE", "MERGED"])));
        keys.push(("mergetool.saysdf.trustExitCode".into(), "true".into()));
    }
    let scope = if local { "--local" } else { "--global" };
    println!("{}", I18N.f("These keys will be written (git config {scope}):", &[("scope", &scope)]));
    for (k, v) in &keys {
        println!("  {k} = {v}");
    }
    let confirm = |q: &str| {
        ask(q).map_err(|e| eprintln!("saysdf: install-git-tool: confirmation from terminal: {e}"))
    };
    match confirm(&I18N.t("Write them?")) {
        Ok(true) => {}
        Ok(false) => {
            println!("{}", I18N.t("cancelled"));
            return 1;
        }
        Err(()) => return 1,
    }
    let default_key = if kind == "diff" { "diff.tool" } else { "merge.tool" };
    let make_default = match confirm(&I18N.f("Make saysdf the default tool ({default_key})?", &[("default_key", &default_key)])) {
        Ok(yes) => yes,
        Err(()) => return 1,
    };
    if make_default {
        keys.push((default_key.into(), "saysdf".into()));
    }
    for (k, v) in &keys {
        match Git::new().run(["config", scope, k.as_str(), v.as_str()]) {
            Ok(o) if o.success() => {}
            Ok(o) => {
                eprintln!("git config {k}: {}", o.stderr_str());
                return 1;
            }
            Err(e) => {
                eprintln!("git config {k}: {e}");
                return 1;
            }
        }
    }
    println!("{}", I18N.t("done"));
    0
}

fn ask(q: &str) -> io::Result<bool> {
    print!("{q} {} ", saysdf_keys::CONFIRM_HINT);
    io::stdout().flush()?;
    ratatui::crossterm::terminal::enable_raw_mode()?;
    let answer = crate::keys::push_flags().and_then(|()| loop {
        if let (ratatui::crossterm::event::Event::Key(k), phys) = crate::keys::read()? {
            if k.kind == ratatui::crossterm::event::KeyEventKind::Press {
                break Ok(crate::keys::confirmed(&k, phys));
            }
        }
    });
    let restored = crate::keys::pop_flags().and(ratatui::crossterm::terminal::disable_raw_mode());
    println!();
    let answer = answer?;
    restored?;
    Ok(answer)
}

pub fn run(args: DiffArgs) -> i32 {
    if args.install_git_tool {
        return install_git_tool("diff", args.local);
    }
    let cwd = args.cwd.clone().unwrap_or_else(|| PathBuf::from("."));
    let src = if args.args.len() == 2 && args.args.iter().all(|a| Path::new(a).exists()) {
        let (a, b) = (PathBuf::from(&args.args[0]), PathBuf::from(&args.args[1]));
        if a.is_dir() && b.is_dir() {
            Source::Dirs { a, b }
        } else if a.is_dir() || b.is_dir() {
            eprintln!("{}", I18N.t("saysdf diff: two files or two folders are required"));
            return 2;
        } else {
            Source::Files { a, b }
        }
    } else {
        let root = match crate::git::toplevel(&cwd) {
            Ok(Some(r)) => PathBuf::from(r),
            Ok(None) => {
                eprintln!("{}", I18N.t("saysdf diff: not a git repository (or give two files/folders)"));
                return 2;
            }
            Err(e) => {
                eprintln!("saysdf diff: {e}");
                return 2;
            }
        };
        Source::Git { root, staged: args.staged, revs: args.args.clone(), paths: args.paths.clone() }
    };
    let files = match src.load() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("saysdf diff: {}", visible_line(&e, 4));
            return 2;
        }
    };
    let differs = files.iter().any(|f| f.binary || !f.hunks.is_empty());
    if args.print || !io::stdout().is_terminal() {
        let mut out = io::stdout().lock();
        for f in &files {
            let _ = out.write_all(diff::to_unified(f).as_bytes());
        }
        return i32::from(differs);
    }
    let title = match &src {
        Source::Files { a, b } | Source::Dirs { a, b } => format!("{} ↔ {}", a.display(), b.display()),
        Source::Git { root, staged, revs, .. } => {
            format!("{}{}{}", root.display(), if *staged { " [staged]" } else { "" }, if revs.is_empty() { String::new() } else { format!(" {}", revs.join(" ")) })
        }
    };
    let n = files.len();
    let layout = if args.inline { Layout::Inline } else { Layout::SideBySide };
    let view = DiffView::new(src, files, layout, !args.no_fold, args.tab_width);
    let opts = pager::Options {
        path: None,
        title: visible_line(&title, 4),
        line: None,
        col: 1,
        editor: None,
        watch: false,
        status: I18N.f("files: {n}; Tab layout, ]/[ hunk, }}/{{ file, z fold, v select, s/u stage/unstage", &[("n", &n.to_string())]),
    };
    match pager::run(view, opts) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}", I18N.f("saysdf diff: terminal: {error}", &[("error", &e.to_string())]));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_follows_language() {
        let text = |v: &mut DiffView| match v.stage(false, 0) {
            Nav::Status(m) => m,
            _ => panic!("expected a status"),
        };
        let mut v = DiffView::new(Source::Files { a: "a".into(), b: "b".into() }, Vec::new(), Layout::Inline, true, 4);
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert_eq!(text(&mut v), "stage/unstage is only available in git mode without revisions");
            assert_eq!(v.lines[0].to_string(), "no differences");
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert_eq!(text(&mut v), "stage/unstage — только в git-режиме без ревизий"); // i18n-allow-cyrillic
        v.rebuild();
        assert_eq!(v.lines[0].to_string(), "нет различий"); // i18n-allow-cyrillic
    }

    #[test]
    fn tool_cmd_quotes_windows_path_with_spaces() {
        let s = tool_cmd(Path::new(r"C:\Program Files\saysdf $x\saysdf.exe"), "diff", &["LOCAL", "REMOTE"]);
        assert_eq!(s, r#""C:/Program Files/saysdf \$x/saysdf.exe" diff "$LOCAL" "$REMOTE""#);
    }
}
