//! `saysdf merge` (design diff-merge-tool D6, D7): `BASE LOCAL REMOTE MERGED` (git mergetool) or
//! a single file with conflict markers. Blocks ours | base | theirs + result, a resolution per block,
//! block editing in an external editor, atomic save; exit code 0 means saved with nothing unresolved.

use crate::editor;
use crate::pager::{self, Content, Nav};
use saysdf_keys::Key;
use clap::Args;
use ratatui::crossterm::event::KeyEvent;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use saysdf_render::doc::Doc;
use saysdf_render::merge::{self, Chunk, ChunkKind, Eol, Resolution};
use saysdf_render::sanitize::{str_width, visible_line, DEFAULT_TAB_WIDTH};
use std::cell::Cell;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-diff");

#[derive(Args)]
pub struct MergeArgs {
    /// `BASE LOCAL REMOTE MERGED` (as git mergetool passes them) or a single file with conflict markers
    pub files: Vec<PathBuf>,
    /// Non-interactive: the auto-merge result (unresolved parts as markers) to stdout; exit code 1 means conflicts
    #[arg(long)]
    pub print: bool,
    /// Write the `saysdf` mergetool (cmd, trustExitCode) to git config after a y/N confirmation
    #[arg(long)]
    pub install_git_tool: bool,
    /// With `--install-git-tool`: write to the repository config instead of `--global`
    #[arg(long)]
    pub local: bool,
    /// Block editor (`E`): a JSON array, `{path}` is substituted; defaults to $VISUAL/$EDITOR
    #[arg(long)]
    pub editor_cmd: Option<String>,
}

fn load(files: &[PathBuf]) -> Result<(Vec<Chunk>, Eol, PathBuf), String> {
    let read = |p: &Path| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| format!("{}: {e}", p.display()));
    match files {
        [one] => {
            let t = read(one)?;
            let (c, eol) = merge::parse_conflicts(&t).map_err(|e| format!("{}: {e}", one.display()))?;
            Ok((c, eol, one.clone()))
        }
        [base, local, remote, merged] => {
            let b = if base.exists() { read(base)? } else { String::new() };
            let (c, eol) = merge::merge3(&b, &read(local)?, &read(remote)?);
            Ok((c, eol, merged.clone()))
        }
        _ => Err(I18N.t("expected 1 file with markers or 4: BASE LOCAL REMOTE MERGED").into_owned()),
    }
}

/// Atomic write: a temporary file next to the target, then rename.
fn save_atomic(path: &Path, text: &str) -> io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.saysdf-merge.{}", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn get(v: &[String], i: usize) -> &str {
    v.get(i).map(String::as_str).unwrap_or("")
}

fn default_editor() -> Vec<String> {
    for v in ["VISUAL", "EDITOR"] {
        if let Ok(e) = std::env::var(v) {
            let mut parts: Vec<String> = e.split_whitespace().map(str::to_owned).collect();
            if !parts.is_empty() {
                parts.push("{path}".into());
                return parts;
            }
        }
    }
    if cfg!(windows) {
        vec!["notepad".into(), "{path}".into()]
    } else {
        vec!["vi".into(), "{path}".into()]
    }
}

struct MergeView {
    chunks: Vec<Chunk>,
    eol: Eol,
    out: PathBuf,
    editor: Vec<String>,
    width: usize,
    lines: Vec<Line<'static>>,
    /// View line -> block index.
    row_chunk: Vec<usize>,
    /// Start lines of changed blocks (block, line).
    change_rows: Vec<(usize, usize)>,
    doc: Doc,
    confirm_save: bool,
    exit: Rc<Cell<i32>>,
}

impl MergeView {
    fn unresolved(&self) -> usize {
        self.chunks.iter().filter(|c| c.is_unresolved()).count()
    }

    fn rebuild(&mut self) {
        let dim = Style::new().fg(Color::DarkGray);
        let col = self.width.saturating_sub(2) / 3;
        let cell = |s: &str| -> String {
            let v = visible_line(s, DEFAULT_TAB_WIDTH);
            let mut out = String::new();
            let mut w = 0;
            for c in v.chars() {
                let cw = saysdf_render::sanitize::char_width(c);
                if w + cw > col {
                    break;
                }
                w += cw;
                out.push(c);
            }
            out.push_str(&" ".repeat(col.saturating_sub(str_width(&out))));
            out
        };
        self.lines.clear();
        self.row_chunk.clear();
        self.change_rows.clear();
        for (ci, c) in self.chunks.iter().enumerate() {
            if c.kind == ChunkKind::Stable {
                for l in &c.ours {
                    self.lines.push(Line::raw(visible_line(l, DEFAULT_TAB_WIDTH)));
                    self.row_chunk.push(ci);
                }
                continue;
            }
            self.change_rows.push((ci, self.lines.len()));
            let (label, color) = match (&c.kind, &c.resolution) {
                (ChunkKind::Conflict, None) => (I18N.t("CONFLICT"), Color::Red),
                (_, Some(Resolution::Ours)) => (I18N.t("resolved: ours"), Color::Green),
                (_, Some(Resolution::Theirs)) => (I18N.t("resolved: theirs"), Color::Green),
                (_, Some(Resolution::OursThenTheirs)) => (I18N.t("resolved: ours+theirs"), Color::Green),
                (_, Some(Resolution::TheirsThenOurs)) => (I18N.t("resolved: theirs+ours"), Color::Green),
                (_, Some(Resolution::Base)) => (I18N.t("resolved: base"), Color::Green),
                (_, Some(Resolution::Edited(_))) => (I18N.t("resolved: edited"), Color::Green),
                (ChunkKind::OursOnly, None) => (I18N.t("auto: ours"), Color::Cyan),
                (ChunkKind::TheirsOnly, None) => (I18N.t("auto: theirs"), Color::Cyan),
                _ => (I18N.t("auto: identical"), Color::Cyan),
            };
            let hdr = Style::new().fg(color).add_modifier(Modifier::BOLD);
            self.lines.push(Line::from(vec![
                Span::styled(format!("┌ {} ", label), hdr),
                Span::styled(format!("{}│{}│{}", cell(" ours"), cell(" base"), cell(" theirs")), dim),
            ]));
            self.row_chunk.push(ci);
            let base = c.base.clone();
            let n = c.ours.len().max(c.theirs.len()).max(base.as_ref().map_or(0, Vec::len));
            for i in 0..n {
                let b = base.as_ref().map_or("", |b| get(b, i));
                self.lines.push(Line::from(vec![
                    Span::styled(cell(get(&c.ours, i)), Style::new().bg(saysdf_render::diff::add_bg())),
                    Span::styled("│", dim),
                    Span::raw(cell(b)),
                    Span::styled("│", dim),
                    Span::styled(cell(get(&c.theirs, i)), Style::new().bg(saysdf_render::diff::del_bg())),
                ]));
                self.row_chunk.push(ci);
            }
            self.lines.push(Line::styled(I18N.t("├ result:"), dim));
            self.row_chunk.push(ci);
            match c.result() {
                Some(r) => {
                    for l in r {
                        self.lines.push(Line::styled(visible_line(&l, DEFAULT_TAB_WIDTH), Style::new().fg(Color::Green)));
                        self.row_chunk.push(ci);
                    }
                }
                None => {
                    self.lines.push(Line::styled(I18N.t("  (unresolved: o ours, t theirs, b ours+theirs, B theirs+ours, a base, E edit)"), Style::new().fg(Color::Red)));
                    self.row_chunk.push(ci);
                }
            }
            self.lines.push(Line::styled("└", dim));
            self.row_chunk.push(ci);
        }
        self.doc = Doc::from_lines(self.lines.clone());
    }

    fn status(&self) -> String {
        I18N.f("unresolved: {n}", &[("n", &self.unresolved().to_string())])
    }

    fn chunk_at(&self, pos: usize) -> Option<usize> {
        let ci = *self.row_chunk.get(pos)?;
        (self.chunks[ci].kind != ChunkKind::Stable).then_some(ci)
    }

    fn resolve(&mut self, pos: usize, r: Option<Resolution>) -> Nav {
        let Some(ci) = self.chunk_at(pos) else {
            return Nav::Status(I18N.t("the cursor is not on a changed block (]/[ to jump)").into_owned());
        };
        self.chunks[ci].resolution = r;
        self.confirm_save = false;
        self.rebuild();
        let row = self.change_rows.iter().find(|(c, _)| *c == ci).map_or(pos, |r| r.1);
        Nav::Select(row, self.status())
    }

    fn edit(&mut self, pos: usize) -> Nav {
        let Some(ci) = self.chunk_at(pos) else {
            return Nav::Status(I18N.t("the cursor is not on a changed block").into_owned());
        };
        let c = &self.chunks[ci];
        let text = match c.result() {
            Some(r) => merge::join(&r, Eol { crlf: self.eol.crlf, trailing: true }),
            None => merge::build(std::slice::from_ref(c), Eol { crlf: self.eol.crlf, trailing: true }).0,
        };
        let tmp = std::env::temp_dir().join(format!("saysdf-merge-{}-{ci}.txt", std::process::id()));
        if let Err(e) = std::fs::write(&tmp, &text) {
            return Nav::Status(I18N.f("temporary file: {error}", &[("error", &e.to_string())]));
        }
        let editor = self.editor.clone();
        let res = pager::suspend(|| editor::run(&editor, &tmp, 1, 1));
        let msg = match res {
            Err(e) => I18N.f("terminal: {error}", &[("error", &e.to_string())]),
            Ok(Err(e)) => visible_line(&e, 4),
            Ok(Ok(())) => match std::fs::read_to_string(&tmp) {
                Ok(t) => {
                    let (lines, _) = merge::split(&t);
                    self.chunks[ci].resolution = Some(Resolution::Edited(lines));
                    self.rebuild();
                    self.status()
                }
                Err(e) => I18N.f("reading the edit: {error}", &[("error", &e.to_string())]),
            },
        };
        let _ = std::fs::remove_file(&tmp);
        Nav::Redraw(msg)
    }

    fn save(&mut self) -> Nav {
        let n = self.unresolved();
        if n > 0 && !self.confirm_save {
            self.confirm_save = true;
            return Nav::Status(I18N.f("unresolved: {n}; press w again to save with markers", &[("n", &n.to_string())]));
        }
        let (text, n) = merge::build(&self.chunks, self.eol);
        match save_atomic(&self.out, &text) {
            Ok(()) => {
                self.exit.set(if n == 0 { 0 } else { 1 });
                self.confirm_save = false;
                Nav::Status(if n == 0 {
                    I18N.f("saved: {path}", &[("path", &self.out.display().to_string())])
                } else {
                    I18N.f("saved: {path} (with markers: {n})", &[("path", &self.out.display().to_string()), ("n", &n.to_string())])
                })
            }
            Err(e) => Nav::Status(I18N.f("save failed: {error}", &[("error", &e.to_string())])),
        }
    }
}

impl Content for MergeView {
    fn len(&self) -> usize {
        self.lines.len()
    }

    fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
        let to = to.min(self.lines.len());
        self.lines[from.min(to)..to].to_vec()
    }

    fn doc(&self) -> Option<&Doc> {
        Some(&self.doc)
    }

    fn set_width(&mut self, width: usize) {
        if width != self.width {
            self.width = width;
            self.rebuild();
        }
    }

    fn key(&mut self, k: &KeyEvent, phys: Option<Key>, pos: usize) -> io::Result<Nav> {
        let is = |spec| crate::keys::is(k, phys, spec);

        let conflict_rows = |me: &Self| -> Vec<usize> {
            me.change_rows.iter().filter(|(c, _)| me.chunks[*c].is_unresolved()).map(|r| r.1).collect()
        };
        Ok(match k.code {
            _ if is("]")? => match conflict_rows(self).into_iter().find(|&r| r > pos) {
                Some(r) => Nav::Select(r, self.status()),
                None => Nav::Status(I18N.f("no more conflicts below; {status}", &[("status", &self.status())])),
            },
            _ if is("[")? => match conflict_rows(self).into_iter().rev().find(|&r| r < pos) {
                Some(r) => Nav::Select(r, self.status()),
                None => Nav::Status(I18N.f("no more conflicts above; {status}", &[("status", &self.status())])),
            },
            _ if is("o")? => self.resolve(pos, Some(Resolution::Ours)),
            _ if is("t")? => self.resolve(pos, Some(Resolution::Theirs)),
            _ if is("b")? => self.resolve(pos, Some(Resolution::OursThenTheirs)),
            _ if is("B")? => self.resolve(pos, Some(Resolution::TheirsThenOurs)),
            _ if is("a")? => self.resolve(pos, Some(Resolution::Base)),
            _ if is("x")? => self.resolve(pos, None),
            _ if is("E")? => self.edit(pos),
            _ if is("w")? => self.save(),
            _ => Nav::None,
        })
    }
}

pub fn run(args: MergeArgs) -> i32 {
    if args.install_git_tool {
        return super::diff::install_git_tool("merge", args.local);
    }
    let (chunks, eol, out) = match load(&args.files) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("saysdf merge: {}", visible_line(&e, 4));
            return 2;
        }
    };
    if args.print || !io::stdout().is_terminal() {
        let (text, n) = merge::build(&chunks, eol);
        print!("{text}");
        return i32::from(n > 0);
    }
    let editor = match args.editor_cmd.as_deref().map(editor::parse).transpose() {
        Ok(e) => e.unwrap_or_else(default_editor),
        Err(e) => {
            eprintln!("saysdf merge: {}", visible_line(&e, 4));
            return 2;
        }
    };
    let exit = Rc::new(Cell::new(1));
    let mut view = MergeView {
        chunks,
        eol,
        out: out.clone(),
        editor,
        width: 120,
        lines: Vec::new(),
        row_chunk: Vec::new(),
        change_rows: Vec::new(),
        doc: Doc::default(),
        confirm_save: false,
        exit: exit.clone(),
    };
    view.rebuild();
    let first = view.change_rows.iter().find(|(c, _)| view.chunks[*c].is_unresolved()).map(|r| r.1);
    let status = I18N.f("{status}; ]/[ conflict, o/t/b/B/a accept, x reset, E edit, w save, q quit", &[("status", &view.status())]);
    let opts = pager::Options {
        path: None,
        title: visible_line(&out.display().to_string(), 4),
        line: first,
        col: 1,
        editor: None,
        watch: false,
        status,
    };
    if let Err(e) = pager::run(view, opts) {
        eprintln!("{}", I18N.f("saysdf merge: terminal: {error}", &[("error", &e.to_string())]));
        return 1;
    }
    exit.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_error_follows_language() {
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert_eq!(load(&[]).unwrap_err(), "expected 1 file with markers or 4: BASE LOCAL REMOTE MERGED");
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert_eq!(load(&[]).unwrap_err(), "ожидается 1 файл с маркерами или 4: BASE LOCAL REMOTE MERGED"); // i18n-allow-cyrillic
    }
}
