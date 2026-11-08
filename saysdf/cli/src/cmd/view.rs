//! `saysdf view <path>`: file viewer (design D2, D7). Errors show a message screen (exit with `q`),
//! a binary file gets a hex view, a large one is a `LargeFile` without highlighting.

use crate::annotate::{Op, Refusal, Session};
use crate::bridge;
use crate::control;
use crate::critic_style::MarkStyles;
use crate::editor;
use crate::pager::{self, Content, MousePhase, Nav};
use crate::selection::{self, Motion, Selection};
use crate::wrap::{Row, WrapMap};
use clap::Args;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use saysdf_render::doc::Doc;
use saysdf_keys::Key;
use saysdf_render::critic::{self, Action, Edit, Mark, Pos};
use saysdf_render::highlight::{plain_line, Highlighter};
use saysdf_render::sanitize::{col_to_grapheme, grapheme_to_col, visible_line, DEFAULT_TAB_WIDTH};
use saysdf_render::source::{self, HexFile, LargeFile, Opened, DEFAULT_MAX_SIZE};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-view");

#[derive(Args)]
pub struct ViewArgs {
    /// File to view
    pub path: PathBuf,
    /// Initial line (1-based), highlighted
    #[arg(long)]
    pub line: Option<usize>,
    /// Initial column (1-based)
    #[arg(long, default_value_t = 1)]
    pub col: usize,
    /// Tab width
    #[arg(long, default_value_t = DEFAULT_TAB_WIDTH)]
    pub tab_width: usize,
    /// Threshold (bytes) above which the file is read in windows and without highlighting
    #[arg(long, default_value_t = DEFAULT_MAX_SIZE)]
    pub max_size: u64,
    /// Editor command for the `e` key: a JSON array, `{path}`/`{line}`/`{col}` are substituted
    #[arg(long)]
    pub editor_cmd: Option<String>,
}

/// Error for a path that is a folder (shared by `view`, `md`, `mermaid`).
pub(crate) fn dir_error() -> io::Error {
    io::Error::other(I18N.t("this is a folder, not a file").into_owned())
}

/// Text for a file that cannot be opened (shared by `view`, `md`, `mermaid`).
pub(crate) fn open_error_text(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => I18N.t("file not found").into_owned(),
        io::ErrorKind::PermissionDenied => I18N.t("no permission to read").into_owned(),
        _ => e.to_string(),
    }
}

enum Kind {
    /// `session` is the refusal when the file cannot be annotated (non-UTF-8 or similar): read-only view.
    Text { lines: Vec<String>, doc: Doc, hl: Highlighter, session: Result<Session, Refusal>, marks: MarkStyles },
    Large(LargeFile),
    Hex(HexFile),
    Message(Doc),
}

struct ViewContent {
    path: PathBuf,
    tab_width: usize,
    max_size: u64,
    kind: Kind,
    /// Keyboard selection (text-annotations D5); only for `Kind::Text` with a session.
    sel: Option<Selection>,
    /// Last pager viewport: top view line, horizontal offset, text height, text width.
    vp: (usize, usize, usize, usize),
    /// Soft wrap wanted (D12): on by default for prose, toggled by Ctrl+W.
    wrap: bool,
    /// Text area width from the pager (`set_width`); 0 until known.
    width: usize,
    /// Visual rows while wrapping a text; `None` means view lines are source lines.
    wrapped: Option<Wrapped>,
    /// Display column kept by Up/Down over visual rows (cleared by other motions).
    want: Option<usize>,
    /// How many times the row map was built (tests: once per load/resize/toggle).
    builds: usize,
    /// Annotation waiting for the input editor or the delete confirmation.
    pending: Option<Pending>,
    /// Input editor request for the pager (`take_input_request`).
    request: Option<(String, String)>,
    /// Ctrl+G hand-off channel (`SAYSDF_VIEWER`, D7); `None` when saysdfTerm did not open this viewer.
    handoff: Option<bridge::Channel>,
}

/// User var of the viewer -> GUI channel (`saysdf.annotations`, D7).
pub const VIEWER_VAR: &str = "SAYSDF_VIEWER";
/// Environment variable with the token of the viewer pane (`saysdf.viewer`, D6).
pub const VIEWER_TOKEN_ENV: &str = "SAYSDF_VIEWER_TOKEN";

/// The hand-off channel of this viewer: the token was taken out of the environment once by the
/// dispatcher ([`control::Session`]) and is kept in memory across subcommand switches. One send
/// per press: every SetUserVar raises `user-var-changed`, and the viewer record of the Lua side does
/// not keep the previous value, so retries would paste the phrase several times.
fn handoff_channel(token: Option<&str>) -> Option<bridge::Channel> {
    Some(bridge::Channel { var: VIEWER_VAR, token: token?.to_owned(), retries: 1 })
}

/// A path that may be handed to the agent: absolute, UTF-8, no control or invisible/bidi
/// characters (the phrase is pasted into the agent's input and must read as what it is).
fn handoff_path(path: &Path) -> Option<&str> {
    let s = path.to_str()?;
    let bad = |c: char| c.is_control() || saysdf_i18n::text::is_invisible_format(c);
    (path.is_absolute() && !s.is_empty() && !s.chars().any(bad)).then_some(s)
}

/// Annotation action of the selection mode (text-annotations D5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Substitute,
    Delete,
    Insert,
    Comment,
    /// Ctrl+E: change the typed text of the mark under the caret.
    Amend,
    /// Ctrl+X: remove the mark under the caret.
    Remove,
}

/// An action started on a selection: byte range in the session text.
struct Pending {
    act: Act,
    start: usize,
    end: usize,
    /// The mark of `Amend` / `Remove`.
    mark: Option<Mark>,
}

/// Wrapped view of a text: the row map and the search document of the rows.
struct Wrapped {
    map: WrapMap,
    doc: Doc,
    /// Search document: the source lines as drawn (tabs expanded), so a phrase split by a wrap
    /// break is one match. Rows are contiguous slices of these lines.
    src: Doc,
    /// Byte offset of every row's text within its source line's text.
    starts: Vec<usize>,
}

impl Wrapped {
    /// Row of source line `line` holding byte `byte` of its text (a row owns its start, so a
    /// match beginning at a break belongs to the next row).
    fn row_of(&self, line: usize, byte: usize) -> usize {
        let from = self.map.first_row(line);
        let to = if line + 1 < self.src.len() { self.map.first_row(line + 1) } else { self.map.len() };
        (from..to).rev().find(|&r| self.starts[r] <= byte).unwrap_or(from)
    }
}

/// Prose files are wrapped by default (`.md`, `.markdown`, `.txt`); code is not.
fn is_prose(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["md", "markdown", "txt"].iter().any(|p| e.eq_ignore_ascii_case(p)))
}

/// Visual row and display cell of `pos` (source line and cell without wrapping).
fn caret_view(wrapped: Option<&Wrapped>, lines: &[String], tab_width: usize, pos: Pos) -> (usize, usize) {
    match wrapped {
        Some(w) => w.map.pos_to_visual(lines, pos),
        None => (pos.line, lines.get(pos.line).map_or(0, |l| grapheme_to_col(l, tab_width, pos.col))),
    }
}

/// Smallest scroll change that shows the caret at view row `row`, cell `col`.
fn follow(vp: (usize, usize, usize, usize), (row, col): (usize, usize), wrapping: bool) -> (usize, usize) {
    let (top, hscroll, height, width) = vp;
    let top = if row < top {
        row
    } else if height > 0 && row >= top + height {
        row + 1 - height
    } else {
        top
    };
    let hscroll = if wrapping {
        0
    } else if col < hscroll {
        col
    } else if width > 0 && col >= hscroll + width {
        col + 1 - width
    } else {
        hscroll
    };
    (top, hscroll)
}

impl ViewContent {
    fn open(path: &Path, tab_width: usize, max_size: u64) -> io::Result<(Self, String)> {
        let (kind, status) = Self::load(path, tab_width, max_size)?;
        Ok((
            Self {
                path: path.to_path_buf(),
                tab_width,
                max_size,
                kind,
                sel: None,
                vp: (0, 0, 0, 0),
                wrap: is_prose(path),
                width: 0,
                wrapped: None,
                want: None,
                builds: 0,
                pending: None,
                request: None,
                handoff: None,
            },
            status,
        ))
    }

    fn load(path: &Path, tab_width: usize, max_size: u64) -> io::Result<(Kind, String)> {
        if path.is_dir() {
            return Err(dir_error());
        }
        Ok(match source::open(path, max_size)? {
            Opened::Text(read_lines) => {
                // The lines come from the session snapshot, so selection positions are the
                // positions a later write uses; without a session the view is read-only.
                let session = Session::open(path, max_size);
                let lines = session.as_ref().map_or(read_lines, |s| selection::split_text(s.text()));
                Self::text_kind(path, tab_width, lines, session)
            }
            Opened::Binary(h) => (Kind::Hex(h), I18N.t("binary file: hex view").into_owned()),
            Opened::Large(l) => (Kind::Large(l), I18N.t("large file: no highlighting or search").into_owned()),
        })
    }

    /// Text view over `lines` (from the session snapshot when there is one).
    fn text_kind(path: &Path, tab_width: usize, lines: Vec<String>, session: Result<Session, Refusal>) -> (Kind, String) {
        let first = lines.first().map(String::as_str).unwrap_or("");
        let hl = Highlighter::for_path(path, first, tab_width);
        let doc = Doc::from_lines(lines.iter().map(|l| plain_line(l, tab_width)).collect());
        let status = hl.syntax_name().unwrap_or("").to_owned();
        // CriticMarkup styles: parsed once per load, applied per frame.
        let marks = session.as_ref().map_or_else(|_| MarkStyles::default(), |s| MarkStyles::build(s.text(), &lines, tab_width));
        (Kind::Text { lines, doc, hl, session, marks }, status)
    }

    /// Ctrl+R/D/N/K on the selection: checks it, then asks for the input editor (or, for Ctrl+D,
    /// for a confirming Enter). Nothing is written here.
    fn begin(&mut self, act: Act) -> Nav {
        let (Kind::Text { session: Ok(s), .. }, Some(sel)) = (&self.kind, &self.sel) else {
            return Nav::None;
        };
        let text = s.text();
        let (a, b) = sel.range();
        let (Some(start), Some(end)) = (critic::byte_offset(text, a), critic::byte_offset(text, b)) else {
            return Nav::Status(critic::Refusal::BadRange.to_string());
        };
        if start == end && matches!(act, Act::Substitute | Act::Delete) {
            return Nav::Status(critic::Refusal::EmptySelection.to_string());
        }
        let prompt = match act {
            Act::Delete => {
                self.pending = Some(Pending { act, start, end, mark: None });
                return Nav::Status(I18N.f("mark the selection as deleted ({markup})?", &[("markup", &"{--text--}")]));
            }
            Act::Substitute => I18N.f("replace the selection with ({markup}):", &[("markup", &"{~~old~>new~~}")]),
            Act::Insert => I18N.f("insert text ({markup}):", &[("markup", &"{++text++}")]),
            Act::Comment if start == end => I18N.f("comment at the caret ({markup}):", &[("markup", &"{>>comment<<}")]),
            Act::Amend | Act::Remove => return Nav::None,
            Act::Comment => I18N.f("comment on the selection ({markup}):", &[("markup", &"{==text==}{>>comment<<}")]),
        };
        // Typed text uses `\n`; the session writes the file's line ending.
        let prefill = if act == Act::Substitute { text[start..end].replace("\r\n", "\n") } else { String::new() };
        self.pending = Some(Pending { act, start, end, mark: None });
        self.request = Some((prompt, prefill));
        Nav::None
    }

    /// Writes the pending annotation through the session. On success the view is rebuilt from
    /// the new text and the selection mode ends with the new mark in view; on a refusal nothing
    /// is written and the selection stays; a file changed on disk is reloaded.
    fn write(&mut self, p: Pending, typed: String) -> io::Result<Nav> {
        let (start, end) = (p.start, p.end);
        let (op, removed) = match (p.act, p.mark) {
            (Act::Remove, Some(m)) => (Op::Remove(m), true),
            (Act::Amend, Some(m)) => (Op::Amend(m, typed), false),
            (act, _) => {
                let action = match act {
                    Act::Substitute => Action::Substitute(typed),
                    Act::Insert => Action::Insert(typed),
                    Act::Comment => Action::Comment(typed),
                    _ => Action::Delete,
                };
                (Op::Annotate(Edit { start, end, action }), false)
            }
        };
        // The new mark starts after the selection for an insertion, at its start otherwise.
        let at = if p.act == Act::Insert { p.end } else { p.start };
        let Kind::Text { session: Ok(s), .. } = &mut self.kind else {
            return Ok(Nav::None);
        };
        match s.apply(&op) {
            Ok(_) => {}
            Err(Refusal::Changed) => {
                let msg = match self.reload() {
                    Ok(_) => Refusal::Changed.to_string(),
                    Err(e) => format!("{}: {e}", Refusal::Changed),
                };
                return Ok(Nav::Scroll { top: self.vp.0, hscroll: self.vp.1, status: msg });
            }
            Err(e) => return Ok(Nav::Status(e.to_string())),
        }
        let old = std::mem::replace(&mut self.kind, Kind::Message(Doc::from_lines(Vec::new())));
        let Kind::Text { session: Ok(s), .. } = old else {
            self.kind = old;
            return Ok(Nav::None);
        };
        let caret = critic::position(s.text(), at).unwrap_or(Pos { line: 0, col: 0 });
        let lines = selection::split_text(s.text());
        self.kind = Self::text_kind(&self.path, self.tab_width, lines, Ok(s)).0;
        self.sel = None;
        self.rewrap();
        let Kind::Text { lines, .. } = &self.kind else {
            return Ok(Nav::None);
        };
        let view = caret_view(self.wrapped.as_ref(), lines, self.tab_width, caret);
        let (top, hscroll) = follow(self.vp, view, self.wrapped.is_some());
        let status = if removed { I18N.t("annotation removed") } else { I18N.t("annotation written") };
        Ok(Nav::Scroll { top, hscroll, status: status.into_owned() })
    }

    /// Parsed marks of the text (empty for a read-only view).
    fn marks(&self) -> &[Mark] {
        match &self.kind {
            Kind::Text { marks, .. } => &marks.list,
            _ => &[],
        }
    }

    /// Byte offset of the selection caret in the session text.
    fn caret_byte(&self) -> Option<usize> {
        let (Kind::Text { session: Ok(s), .. }, Some(sel)) = (&self.kind, &self.sel) else {
            return None;
        };
        critic::byte_offset(s.text(), sel.caret())
    }

    /// Index of the mark whose range holds the caret.
    fn mark_under_caret(&self) -> Option<usize> {
        let at = self.caret_byte()?;
        self.marks().iter().position(|m| m.range.start <= at && at < m.range.end)
    }

    /// Alt+Down / Alt+Up: puts an empty selection (the caret) on the next / previous mark start.
    fn jump(&mut self, forward: bool, pos: usize) -> Nav {
        let Kind::Text { session: Ok(s), lines, .. } = &self.kind else {
            return Nav::None;
        };
        let text = s.text();
        let marks = self.marks();
        if marks.is_empty() {
            return Nav::Status(I18N.t("no annotations").into_owned());
        }
        // From the caret (strictly beyond it) or, without one, from the top of the view line.
        let from = match self.caret_byte() {
            Some(b) => (b, true),
            None => (critic::byte_offset(text, self.view_pos(pos)).unwrap_or(0), false),
        };
        let found = if forward {
            marks.iter().find(|m| m.range.start > from.0 || (!from.1 && m.range.start == from.0))
        } else {
            marks.iter().rev().find(|m| m.range.start < from.0)
        };
        let Some(m) = found else {
            return Nav::Status(I18N.t("no more annotations").into_owned());
        };
        let Some(caret) = critic::position(text, m.range.start) else {
            return Nav::None;
        };
        let at = caret_view(self.wrapped.as_ref(), lines, self.tab_width, caret);
        let (top, hscroll) = follow(self.vp, at, self.wrapped.is_some());
        self.sel = Some(Selection::new(caret));
        self.want = None;
        Nav::Scroll { top, hscroll, status: String::new() }
    }

    /// Ctrl+E / Ctrl+X on the mark under the caret: the amend editor or the removal confirmation.
    fn begin_mark(&mut self, act: Act) -> Nav {
        let Some(i) = self.mark_under_caret() else {
            return Nav::Status(I18N.t("no annotation under the caret").into_owned());
        };
        let mark = self.marks()[i].clone();
        let Kind::Text { session: Ok(s), .. } = &self.kind else {
            return Nav::None;
        };
        let (start, end) = (mark.range.start, mark.range.end);
        if act == Act::Remove {
            self.pending = Some(Pending { act, start, end, mark: Some(mark) });
            return Nav::Status(I18N.t("remove the annotation under the caret?").into_owned());
        }
        let typed = match mark.kind {
            critic::Kind::Delete => return Nav::Status(critic::Refusal::NothingToAmend.to_string()),
            critic::Kind::Insert | critic::Kind::Comment => Some(mark.text.clone()),
            critic::Kind::Substitute => mark.new_text.clone(),
            critic::Kind::Highlight => mark.comment.clone(),
        };
        let prefill = typed.map(|r| s.text()[r].replace("\r\n", "\n")).unwrap_or_default();
        self.pending = Some(Pending { act, start, end, mark: Some(mark) });
        self.request = Some((I18N.t("amend the annotation text:").into_owned(), prefill));
        Nav::None
    }

    /// Where a new selection starts without a caret: the start of view row `pos`.
    fn selection_start(&self, pos: usize) -> Option<Pos> {
        let Kind::Text { lines, .. } = &self.kind else {
            return None;
        };
        if lines.is_empty() {
            return None;
        }
        Some(match self.wrapped.as_ref().and_then(|w| w.map.row(pos)) {
            Some(r) => Pos { line: r.line, col: r.g0 },
            None => Pos { line: pos.min(lines.len() - 1), col: 0 },
        })
    }

    /// Why annotation keys do nothing here (hex, large or non-UTF-8 view).
    fn unavailable(&self) -> Option<String> {
        match &self.kind {
            Kind::Text { session: Err(e), .. } => Some(e.to_string()),
            Kind::Hex(_) => Some(Refusal::Binary.to_string()),
            Kind::Large(_) => Some(Refusal::TooLarge.to_string()),
            _ => None,
        }
    }

    /// Rebuilds the visual rows (load, reload, resize, toggle; never per frame).
    fn rewrap(&mut self) {
        self.wrapped = None;
        self.want = None;
        let Kind::Text { lines, .. } = &self.kind else { return };
        if !self.wrap || self.width == 0 {
            return;
        }
        let map = WrapMap::build(lines, self.tab_width, self.width);
        let plain: Vec<Line<'static>> = lines.iter().map(|l| plain_line(l, self.tab_width)).collect();
        let rows: Vec<Line<'static>> = (0..map.len())
            .filter_map(|i| map.row(i))
            .map(|r| pager::slice_cells(&plain[r.line], r.c0, r.c1 - r.c0))
            .collect();
        let mut starts = Vec::with_capacity(rows.len());
        let mut acc = 0;
        for (i, row) in rows.iter().enumerate() {
            if map.is_first(i) {
                acc = 0;
            }
            starts.push(acc);
            acc += row.spans.iter().map(|s| s.content.len()).sum::<usize>();
        }
        let doc = Doc::with_anchors(rows, map.anchors());
        let src = Doc::from_lines(plain);
        self.wrapped = Some(Wrapped { map, doc, src, starts });
        self.builds += 1;
    }

    /// Source position at the start of view line `view`.
    fn view_pos(&self, view: usize) -> Pos {
        match self.wrapped.as_ref().and_then(|w| w.map.row(view)) {
            Some(r) => Pos { line: r.line, col: r.g0 },
            None => Pos { line: view, col: 0 },
        }
    }

    /// Ctrl+F (D13): `saysdf diff HEAD -- <file>` in the terminal, then redraw; the file is reloaded
    /// when it changed meanwhile.
    fn diff_head(&mut self) -> Nav {
        let dir = self.path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let root = match crate::git::toplevel(&dir) {
            Ok(Some(r)) => PathBuf::from(r),
            Ok(None) => return Nav::Status(I18N.t("not a git repository: no diff against HEAD").into_owned()),
            Err(e) => return Nav::Status(I18N.f("diff: {error}", &[("error", &e.to_string())])),
        };
        let tracked = crate::git::Git::safe()
            .dir(&root)
            .run([std::ffi::OsStr::new("ls-files"), "--error-unmatch".as_ref(), "--".as_ref(), self.path.as_os_str()]);
        if !tracked.is_ok_and(|o| o.success()) {
            return Nav::Status(I18N.t("the file is not tracked by git: no diff against HEAD").into_owned());
        }
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("saysdf"));
        let args = diff_head_args(&root, &self.path);
        let stamp = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified().map(|t| (t, m.len()))).ok();
        let before = stamp(&self.path);
        let res = pager::suspend(|| std::process::Command::new(&exe).args(&args).status());
        let mut msg = match res {
            Err(e) => I18N.f("terminal: {error}", &[("error", &e.to_string())]),
            Ok(Err(e)) => I18N.f("diff: {error}", &[("error", &e.to_string())]),
            Ok(Ok(_)) => String::new(),
        };
        if stamp(&self.path) != before {
            match self.reload() {
                Ok(Some(m)) => msg = m,
                Ok(None) => {}
                Err(e) => msg = visible_line(&e.to_string(), 4),
            }
        }
        Nav::Redraw(msg)
    }

    /// Ctrl+L (D13): the annotation list of the project folder in this pane (`saysdf annotations
    /// --result-file`); the chosen mark opens here, Esc returns to the same place.
    fn annotation_list(&mut self) -> Nav {
        let dir = self.path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let root = match crate::git::toplevel(&dir) {
            Ok(Some(r)) => PathBuf::from(r),
            _ => dir,
        };
        let result = match super::annotations::new_result_file() {
            Ok(f) => f,
            Err(e) => return Nav::Status(I18N.f("annotations: {error}", &[("error", &e.to_string())])),
        };
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("saysdf"));
        let args = super::annotations::list_args(&root, &result);
        let res = pager::suspend(|| std::process::Command::new(&exe).args(&args).status());
        // The file is data from another process: read once, removed, checked against the root.
        let choice = super::annotations::take_choice(&result, &root);
        let failed = match res {
            Err(e) => Some(I18N.f("terminal: {error}", &[("error", &e.to_string())])),
            Ok(Err(e)) => Some(I18N.f("annotations: {error}", &[("error", &e.to_string())])),
            Ok(Ok(_)) => None,
        };
        let Some((path, line)) = choice else {
            return Nav::Redraw(failed.unwrap_or_default());
        };
        match self.switch_to(&path, line) {
            Ok(nav) => nav,
            Err(e) => Nav::Redraw(visible_line(&e.to_string(), DEFAULT_TAB_WIDTH)),
        }
    }

    /// Ctrl+G (D7, D13): the OSC sequence handing the shown file to the agent pane, or the status
    /// message why it cannot be sent.
    fn handoff_seq(&self) -> Result<String, String> {
        let Some(ch) = &self.handoff else {
            return Err(I18N.t("open the viewer from saysdfTerm to hand off").into_owned());
        };
        let Some(path) = handoff_path(&self.path) else {
            return Err(I18N.t("the path contains control or invisible characters").into_owned());
        };
        ch.encode("handoff-file", path).map_err(|e| e.to_string())
    }

    /// Ctrl+G: sends the hand-off to the GUI (`saysdf.annotations` pastes the phrase).
    fn handoff(&self) -> Nav {
        let msg = match self.handoff_seq() {
            Err(m) => m,
            Ok(seq) => {
                let mut out = io::stdout();
                match out.write_all(seq.as_bytes()).and_then(|()| out.flush()) {
                    Ok(()) => I18N.t("handed to the agent").into_owned(),
                    Err(e) => I18N.f("terminal: {error}", &[("error", &e.to_string())]),
                }
            }
        };
        Nav::Status(visible_line(&msg, DEFAULT_TAB_WIDTH))
    }

    /// Shows `path` in this viewer (nothing changes when it cannot be read); `line` is a 0-based
    /// source line.
    fn switch_to(&mut self, path: &Path, line: usize) -> io::Result<Nav> {
        let (kind, _) = Self::load(path, self.tab_width, self.max_size)?;
        self.kind = kind;
        self.path = path.to_path_buf();
        self.wrap = is_prose(path);
        self.sel = None;
        self.pending = None;
        self.rewrap();
        let view = self.wrapped.as_ref().map_or(line, |w| w.map.first_row(line));
        let title = visible_line(&path.to_string_lossy(), DEFAULT_TAB_WIDTH);
        Ok(Nav::Open { path: path.to_path_buf(), title, line: view })
    }

    /// Ctrl+W: toggles wrapping, keeping the top position and the caret in view.
    fn toggle_wrap(&mut self) -> Nav {
        let anchor = self.view_pos(self.vp.0);
        self.wrap = !self.wrap;
        self.rewrap();
        let Kind::Text { lines, .. } = &self.kind else { return Nav::None };
        let top = caret_view(self.wrapped.as_ref(), lines, self.tab_width, anchor).0;
        let vp = (top, if self.wrap { 0 } else { self.vp.1 }, self.vp.2, self.vp.3);
        let (top, hscroll) = match &self.sel {
            Some(sel) => follow(vp, caret_view(self.wrapped.as_ref(), lines, self.tab_width, sel.caret()), self.wrapped.is_some()),
            None => (vp.0, vp.1),
        };
        let status = if self.wrap { I18N.t("soft wrap on") } else { I18N.t("soft wrap off") };
        Nav::Scroll { top, hscroll, status: status.into_owned() }
    }
}

impl Content for ViewContent {
    fn pane_title(&self) -> Option<String> {
        // the file name only: the full path stays out of the pane (D13)
        self.path.file_name().map(|n| n.to_string_lossy().into_owned())
    }

    fn len(&self) -> usize {
        match &self.kind {
            Kind::Text { lines, .. } => self.wrapped.as_ref().map_or(lines.len(), |w| w.map.len()),
            Kind::Large(l) => l.line_count(),
            Kind::Hex(h) => h.rows() as usize,
            Kind::Message(d) => d.len(),
        }
    }

    fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
        let tw = self.tab_width;
        let read = |r: io::Result<Vec<String>>| match r {
            Ok(v) => v.iter().map(|l| plain_line(l, tw)).collect(),
            Err(e) => vec![plain_line(&I18N.f("read error: {error}", &[("error", &e.to_string())]), tw)],
        };
        match &mut self.kind {
            Kind::Text { lines, hl, marks, .. } => {
                // Wrapped: draw the source lines under the rows, then slice them per row, so
                // marks and selection (per source line) need no wrap knowledge.
                let rows: Option<Vec<(Row, bool)>> = self.wrapped.as_ref().map(|w| {
                    (from..to.min(w.map.len())).filter_map(|i| w.map.row(i).map(|r| (r, w.map.is_last(i)))).collect()
                });
                let (from, to) = match &rows {
                    Some(r) if !r.is_empty() => (r[0].0.line, r[r.len() - 1].0.line + 1),
                    Some(_) => (0, 0),
                    None => (from, to),
                };
                let mut out = hl.highlight_window(lines, from, to);
                if !marks.is_empty() {
                    for (line, i) in out.iter_mut().zip(from..) {
                        *line = marks.apply(i, std::mem::take(line));
                    }
                }
                if let Some(sel) = &self.sel {
                    let rev = Style::new().add_modifier(Modifier::REVERSED);
                    for (line, i) in out.iter_mut().zip(from..) {
                        if let Some((a, b)) = lines.get(i).and_then(|l| sel.cells(i, l, tw)) {
                            *line = selection::style_cells(std::mem::take(line), a, b, rev);
                        }
                    }
                }
                match rows {
                    // The last row of a line keeps one more cell: the caret or the line-break
                    // cell of a selection past the line end.
                    Some(rows) => rows
                        .iter()
                        .map(|(r, last)| pager::slice_cells(&out[r.line - from], r.c0, r.c1 - r.c0 + usize::from(*last)))
                        .collect(),
                    None => out,
                }
            }
            Kind::Large(l) => read(l.read_lines(from, to)),
            Kind::Hex(h) => read(h.window(from as u64, to.saturating_sub(from))),
            Kind::Message(d) => d.lines[from.min(d.len())..to.min(d.len())].to_vec(),
        }
    }

    fn doc(&self) -> Option<&Doc> {
        match &self.kind {
            Kind::Text { .. } if self.wrapped.is_some() => self.wrapped.as_ref().map(|w| &w.doc),
            Kind::Text { doc, .. } | Kind::Message(doc) => Some(doc),
            _ => None,
        }
    }

    fn search_doc(&self) -> Option<&Doc> {
        match &self.wrapped {
            Some(w) => Some(&w.src),
            None => self.doc(),
        }
    }

    fn search_anchor(&self, view: usize, end: bool) -> (usize, usize) {
        match &self.wrapped {
            Some(w) => {
                let i = view.min(w.map.len().saturating_sub(1));
                let line = w.map.row(i).map_or(0, |r| r.line);
                let len = w.doc.line_text(i).len();
                (line, w.starts.get(i).map_or(0, |s| if end { s + len } else { *s }))
            }
            None => (view, if end { usize::MAX } else { 0 }),
        }
    }

    fn match_view(&self, m: saysdf_render::doc::Match) -> (usize, usize) {
        match &self.wrapped {
            // Wrapping has no horizontal scroll: the column is irrelevant.
            Some(w) => (w.row_of(m.line, m.start), 0),
            None => (m.line, self.doc().map_or(0, |d| saysdf_render::sanitize::str_width(&d.line_text(m.line)[..m.start]))),
        }
    }

    fn match_ranges(&self, query: &str, view: usize) -> Vec<(usize, usize)> {
        let Some(w) = &self.wrapped else {
            return self.doc().map_or_else(Vec::new, |d| d.matches_in_line(query, view));
        };
        let (Some(row), Some(&start)) = (w.map.row(view), w.starts.get(view)) else { return Vec::new() };
        let end = start + w.doc.line_text(view).len();
        w.src
            .matches_in_line(query, row.line)
            .into_iter()
            .filter(|&(a, b)| a < end && b > start)
            .map(|(a, b)| (a.max(start) - start, b.min(end) - start))
            .collect()
    }

    fn hscroll_enabled(&self) -> bool {
        self.wrapped.is_none()
    }

    fn set_width(&mut self, width: usize) {
        if width != self.width {
            self.width = width;
            self.rewrap();
        }
    }

    fn viewport(&mut self, top: usize, hscroll: usize, height: usize, width: usize) {
        self.vp = (top, hscroll, height, width);
    }

    fn line_count(&self) -> usize {
        match &self.kind {
            Kind::Text { lines, .. } => lines.len(),
            _ => self.len(),
        }
    }

    fn line_number(&self, view: usize) -> Option<usize> {
        match &self.wrapped {
            Some(w) => w.map.is_first(view).then(|| self.source_of(view)),
            None => Some(view),
        }
    }

    fn source_of(&self, view: usize) -> usize {
        self.wrapped.as_ref().and_then(|w| w.map.row(view)).map_or(view, |r| r.line)
    }

    fn view_of(&self, line: usize) -> usize {
        self.wrapped.as_ref().map_or(line, |w| w.map.first_row(line))
    }

    fn captures_keys(&self) -> bool {
        // D13: Shift/Alt+arrows, Ctrl commands and Esc reach the text before the pager scrolls or
        // quits; everything the text does not use is passed back.
        matches!(self.kind, Kind::Text { .. })
    }

    fn status_extra(&self) -> Option<String> {
        if !matches!(self.kind, Kind::Text { session: Ok(_), .. }) {
            return None;
        }
        if self.pending.is_some() {
            return Some([format!("Enter {}", I18N.t("write")), hint_esc()].join(HINT_SEP));
        }
        let n = self.marks().len();
        let under = self.mark_under_caret();
        let counter = match under {
            Some(i) => I18N.f("annotations: {k}/{n}", &[("k", &(i + 1)), ("n", &n)]),
            None if n > 0 => I18N.f("annotations: {n}", &[("n", &n)]),
            None => String::new(),
        };
        let mut parts = Vec::new();
        if !counter.is_empty() {
            parts.push(counter);
        }
        if self.sel.is_none() {
            parts.push(hint_select());
        }
        if n > 0 {
            parts.push(format!("Alt+↓/↑ {}", I18N.t("annotations")));
        }
        parts.push(match (under, self.sel.is_some()) {
            (Some(_), _) => ctrl_group(&[("E", I18N.t("amend")), ("X", I18N.t("remove"))]),
            (None, true) => ctrl_group(&[
                ("R", I18N.t("substitute")),
                ("D", I18N.t("delete")),
                ("N", I18N.t("insert")),
                ("K", I18N.t("comment")),
            ]),
            (None, false) => ctrl_group(&[("G", I18N.t("agent")), ("L", I18N.t("list")), ("F", I18N.t("diff")), ("W", I18N.t("wrap"))]),
        });
        if self.sel.is_some() {
            parts.push(hint_esc());
        }
        Some(parts.join(HINT_SEP))
    }

    /// Keys of D13: every command is a Ctrl combination matched by physical key
    /// (`crate::keys::is`, lowercase specs), selection by Shift+arrows or Ctrl+Space,
    /// annotation jumps by Alt+Down/Up.
    fn key(&mut self, k: &KeyEvent, phys: Option<Key>, pos: usize) -> io::Result<Nav> {
        let is = |spec| crate::keys::is(k, phys, spec);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let page = self.vp.2.saturating_sub(1).max(1);
        let motion = match k.code {
            KeyCode::Left if ctrl => Some(Motion::WordPrev),
            KeyCode::Right if ctrl => Some(Motion::WordNext),
            KeyCode::Left => Some(Motion::Left),
            KeyCode::Right => Some(Motion::Right),
            KeyCode::Up => Some(Motion::Up),
            KeyCode::Down => Some(Motion::Down),
            KeyCode::Home if ctrl => Some(Motion::FileStart),
            KeyCode::End if ctrl => Some(Motion::FileEnd),
            KeyCode::Home => Some(Motion::LineStart),
            KeyCode::End => Some(Motion::LineEnd),
            KeyCode::PageUp => Some(Motion::PageUp(page)),
            KeyCode::PageDown => Some(Motion::PageDown(page)),
            _ => None,
        };
        let jump = alt && !shift && !ctrl && matches!(k.code, KeyCode::Up | KeyCode::Down);
        let toggle = is("Ctrl+Space")?;
        let selecting = toggle || (shift && motion.is_some());
        let mut act = None;
        for (spec, a) in [
            ("Ctrl+r", Act::Substitute),
            ("Ctrl+d", Act::Delete),
            ("Ctrl+n", Act::Insert),
            ("Ctrl+k", Act::Comment),
            ("Ctrl+e", Act::Amend),
            ("Ctrl+x", Act::Remove),
        ] {
            if is(spec)? {
                act = Some(a);
                break;
            }
        }
        if is("Ctrl+l")? {
            return Ok(self.annotation_list());
        }
        if is("Ctrl+g")? && !matches!(self.kind, Kind::Message(_)) {
            return Ok(self.handoff());
        }
        if is("Ctrl+f")? && !matches!(self.kind, Kind::Message(_)) {
            return Ok(self.diff_head());
        }
        // Read-only views (hex, large, non-UTF-8): annotation keys say why they do nothing.
        if !matches!(self.kind, Kind::Text { session: Ok(_), .. }) {
            if matches!(self.kind, Kind::Message(_)) {
                return Ok(Nav::Pass);
            }
            if selecting {
                return Ok(Nav::Status(I18N.t("selection is unavailable: the file is shown read-only").into_owned()));
            }
            if act.is_some() || jump {
                return Ok(Nav::Status(self.unavailable().unwrap_or_default()));
            }
            if matches!(self.kind, Kind::Text { .. }) && is("Ctrl+w")? {
                return Ok(self.toggle_wrap());
            }
            return Ok(Nav::Pass);
        }
        if is("Ctrl+w")? {
            return Ok(self.toggle_wrap());
        }
        // Ctrl+D / Ctrl+X wait for a confirming Enter; any other key cancels.
        if let Some(p) = self.pending.take() {
            if k.code == KeyCode::Enter {
                return self.write(p, String::new());
            }
            return Ok(Nav::Status(I18N.t("annotation cancelled").into_owned()));
        }
        if jump {
            return Ok(self.jump(k.code == KeyCode::Down, pos));
        }
        match act {
            Some(a @ (Act::Amend | Act::Remove)) => return Ok(self.begin_mark(a)),
            Some(_) if self.sel.is_none() => {
                return Ok(Nav::Status(I18N.t("nothing selected: Shift+arrows or Ctrl+Space").into_owned()));
            }
            Some(a) => return Ok(self.begin(a)),
            None => {}
        }
        if (toggle || k.code == KeyCode::Esc) && self.sel.take().is_some() {
            return Ok(Nav::Status(String::new()));
        }
        if self.sel.is_none() {
            if !selecting {
                return Ok(Nav::Pass);
            }
            let Some(start) = self.selection_start(pos) else {
                return Ok(Nav::Status(I18N.t("nothing to select").into_owned()));
            };
            self.sel = Some(Selection::new(start));
            self.want = None;
            if toggle {
                return Ok(Nav::Status(String::new()));
            }
        }
        let Some(motion) = motion else {
            return Ok(Nav::Pass);
        };
        let (Kind::Text { lines, .. }, Some(sel)) = (&self.kind, &mut self.sel) else {
            return Ok(Nav::Pass);
        };
        // Wrapped: vertical motions go by visual row and keep the display column.
        let rows = match motion {
            Motion::Up => Some(-1),
            Motion::Down => Some(1),
            Motion::PageUp(n) => Some(-(n.max(1) as isize)),
            Motion::PageDown(n) => Some(n.max(1) as isize),
            _ => None,
        };
        match (rows, &self.wrapped) {
            (Some(d), Some(w)) => {
                let caret = sel.caret();
                let want = *self.want.get_or_insert_with(|| w.map.pos_to_visual(lines, caret).1);
                sel.set_caret(w.map.move_rows(lines, caret, want, d));
            }
            _ => {
                sel.apply(motion, lines);
                self.want = None;
            }
        }
        // Shift extends the selection; a plain motion moves the caret alone.
        if !shift {
            sel.collapse();
        }
        // Minimal scrolling to keep the caret visible (vertically by visual row when wrapping,
        // horizontally otherwise).
        let at = caret_view(self.wrapped.as_ref(), lines, self.tab_width, sel.caret());
        let (top, hscroll) = follow(self.vp, at, self.wrapped.is_some());
        Ok(Nav::Scroll { top, hscroll, status: String::new() })
    }

    /// Left-button selection (text-annotations D5): press starts a selection at the clicked
    /// grapheme (a click without drag leaves a caret), drag extends it, release keeps it.
    /// Dragging on the first or last text row scrolls one row per event.
    fn mouse(&mut self, line: usize, col: usize, phase: MousePhase) -> io::Result<Nav> {
        let Kind::Text { lines, session: Ok(_), .. } = &self.kind else {
            return Ok(Nav::None);
        };
        if lines.is_empty() || phase != MousePhase::Press && self.sel.is_none() {
            return Ok(Nav::None);
        }
        let (top, hscroll, height, _) = self.vp;
        // Autoscroll: the pager clamps drags outside the text to its edge rows.
        let (mut line, mut new_top) = (line, top);
        if phase == MousePhase::Drag && height > 1 {
            if line <= top && top > 0 {
                (line, new_top) = (top - 1, top - 1);
            } else if line + 1 >= top + height && top + height < self.len() {
                (line, new_top) = (top + height, top + 1);
            }
        }
        let pos = match &self.wrapped {
            Some(w) => w.map.visual_to_pos(lines, line, col),
            None => {
                let l = line.min(lines.len() - 1);
                Pos { line: l, col: col_to_grapheme(&lines[l], self.tab_width, col) }
            }
        };
        match (phase, &mut self.sel) {
            (MousePhase::Press, _) => self.sel = Some(Selection::new(pos)),
            (_, Some(sel)) => sel.set_caret(pos),
            _ => {}
        }
        self.want = None;
        Ok(if new_top != top { Nav::Scroll { top: new_top, hscroll, status: String::new() } } else { Nav::None })
    }

    fn take_input_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    /// Viewer control file (D6): the next file of a reused split.
    fn open_path(&mut self, path: &Path, line: usize) -> Option<io::Result<Nav>> {
        Some(self.switch_to(path, line))
    }

    /// An annotation waiting for its text or a delete confirmation holds control requests.
    fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Enter in the input editor writes the pending annotation; Esc cancels it (selection kept).
    fn input(&mut self, text: Option<String>, _pos: usize) -> io::Result<Nav> {
        let Some(p) = self.pending.take() else {
            return Ok(Nav::None);
        };
        match text {
            Some(t) => self.write(p, t),
            None => Ok(Nav::Status(I18N.t("annotation cancelled").into_owned())),
        }
    }

    fn reload(&mut self) -> io::Result<Option<String>> {
        if matches!(self.kind, Kind::Message(_)) {
            return Ok(None);
        }
        let (kind, status) = Self::load(&self.path, self.tab_width, self.max_size)?;
        self.kind = kind;
        self.sel = None;
        self.pending = None;
        self.rewrap();
        Ok(Some(if status.is_empty() {
            I18N.t("reloaded").into_owned()
        } else {
            I18N.f("reloaded; {status}", &[("status", &status)])
        }))
    }
}

/// Arguments of our own executable for Ctrl+F: the diff of `file` against `HEAD` in the work tree
/// `root`. Passed as OS strings, so spaces and non-ASCII names need no quoting.
fn diff_head_args(root: &Path, file: &Path) -> Vec<std::ffi::OsString> {
    ["diff".as_ref(), "--cwd".as_ref(), root.as_os_str(), "HEAD".as_ref(), "--".as_ref(), file.as_os_str()]
        .iter()
        .map(|a: &&std::ffi::OsStr| (*a).to_owned())
        .collect()
}

/// Separator of the hint line items (D13 hint form).
const HINT_SEP: &str = " · ";

/// `Ctrl:` group of the hint line (D13): one marker, then letter + label pairs.
fn ctrl_group(pairs: &[(&str, std::borrow::Cow<'static, str>)]) -> String {
    let items: Vec<String> = pairs.iter().map(|(key, label)| format!("{key} {label}")).collect();
    format!("Ctrl: {}", items.join(HINT_SEP))
}

/// "Shift+arrows select" item of the hint line.
fn hint_select() -> String {
    format!("{} {}", I18N.t("Shift+arrows"), I18N.t("select"))
}

/// "Esc cancel" item of the hint line.
fn hint_esc() -> String {
    format!("Esc {}", I18N.t("cancel"))
}

/// Error screen: the message and exit with `q`. Without a terminal it goes to stderr.
fn error_screen(title: &str, msg: &str) -> i32 {
    let text = format!("saysdf view: {title}: {msg}");
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        eprintln!("{text}");
        return 1;
    }
    let doc = Doc::from_lines(vec![
        plain_line(&text, DEFAULT_TAB_WIDTH),
        Line::from(""),
        Line::from(I18N.t("q to quit").into_owned()),
    ]);
    let content = ViewContent {
        path: PathBuf::new(),
        tab_width: DEFAULT_TAB_WIDTH,
        max_size: DEFAULT_MAX_SIZE,
        kind: Kind::Message(doc),
        sel: None,
        vp: (0, 0, 0, 0),
        wrap: false,
        width: 0,
        wrapped: None,
        want: None,
        builds: 0,
        pending: None,
        request: None,
        handoff: None,
    };
    let opts = pager::Options {
        path: None,
        title: title.to_owned(),
        line: None,
        col: 1,
        editor: None,
        watch: false,
        status: I18N.t("error").into_owned(),
    };
    if let Err(e) = pager::run(content, opts) {
        eprintln!("{text} ({e})");
    }
    1
}

/// `saysdf view` inside the dispatcher ([`control::dispatch`]): a control request for another
/// subcommand is left in `s.next`.
pub fn run_in(args: ViewArgs, s: &mut control::Session) -> i32 {
    let abs = std::path::absolute(&args.path).unwrap_or_else(|_| args.path.clone());
    let title = visible_line(&abs.to_string_lossy(), DEFAULT_TAB_WIDTH);
    let editor = match args.editor_cmd.as_deref().map(editor::parse).transpose() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("saysdf view: {}", visible_line(&e, DEFAULT_TAB_WIDTH));
            return 2;
        }
    };
    let (mut content, status) = match ViewContent::open(&abs, args.tab_width, args.max_size) {
        Ok(v) => v,
        Err(e) => {
            let msg = open_error_text(&e);
            return error_screen(&title, &visible_line(&msg, DEFAULT_TAB_WIDTH));
        }
    };
    if !io::stdout().is_terminal() {
        eprintln!("{}", I18N.t("saysdf view: stdout is not a terminal"));
        return 1;
    }
    content.handoff = handoff_channel(s.token.as_deref());
    let opts = pager::Options {
        path: Some(abs),
        title,
        line: args.line.map(|l| l.saturating_sub(1)),
        col: args.col,
        editor,
        watch: true,
        status,
    };
    match pager::run_ctl(content, opts, s.control(control::Sub::View)) {
        Ok(pager::Exit::Quit) => 0,
        Ok(pager::Exit::Switch(req)) => {
            s.next = Some(req);
            0
        }
        Err(e) => {
            eprintln!("{}", I18N.f("saysdf view: terminal: {error}", &[("error", &e.to_string())]));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_errors_follow_language() {
        let nf = io::Error::from(io::ErrorKind::NotFound);
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert_eq!(open_error_text(&nf), "file not found");
            assert_eq!(dir_error().to_string(), "this is a folder, not a file");
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert_eq!(open_error_text(&nf), "файл не найден"); // i18n-allow-cyrillic
        assert_eq!(dir_error().to_string(), "это папка, а не файл"); // i18n-allow-cyrillic
    }

    fn open_bytes(bytes: &[u8]) -> (tempfile::TempDir, ViewContent) {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("f.txt");
        std::fs::write(&path, bytes).unwrap();
        let (c, _) = ViewContent::open(&path, 4, DEFAULT_MAX_SIZE).unwrap();
        (d, c)
    }

    fn press(c: &mut ViewContent, code: KeyCode, pos: usize) -> Nav {
        c.key(&KeyEvent::new(code, KeyModifiers::NONE), None, pos).unwrap()
    }

    fn press_mod(c: &mut ViewContent, code: KeyCode, mods: KeyModifiers, pos: usize) -> Nav {
        c.key(&KeyEvent::new(code, mods), None, pos).unwrap()
    }

    /// Shift+`code`: extends the selection.
    fn shift(c: &mut ViewContent, code: KeyCode, pos: usize) -> Nav {
        press_mod(c, code, KeyModifiers::SHIFT, pos)
    }

    /// Alt+Down / Alt+Up: next / previous annotation.
    fn alt(c: &mut ViewContent, forward: bool) -> Nav {
        let code = if forward { KeyCode::Down } else { KeyCode::Up };
        press_mod(c, code, KeyModifiers::ALT, 0)
    }

    /// Ctrl+`typed` with the physical key `base` in `base_layout`, as Windows (VK code) and
    /// kitty (alternate keys) deliver it on any layout.
    fn ctrl_as(c: &mut ViewContent, typed: char, base: char) -> Nav {
        let mut k = KeyEvent::new(KeyCode::Char(typed), KeyModifiers::CONTROL);
        k.base_layout = Some(base);
        let phys = crate::keys::phys(&k);
        c.key(&k, phys, 0).unwrap()
    }

    fn ctrl(c: &mut ViewContent, letter: char) -> Nav {
        ctrl_as(c, letter, letter)
    }

    fn ch(c: char) -> KeyCode {
        KeyCode::Char(c)
    }

    fn range(c: &ViewContent) -> (Pos, Pos) {
        c.sel.expect("selection").range()
    }

    fn at(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn marks_parse_once_and_selection_stays_on_top() {
        let (_d, mut c) = open_bytes(b"a {--b--} c\n");
        for _ in 0..3 {
            c.window(0, 1);
        }
        match &c.kind {
            Kind::Text { marks, .. } => assert_eq!(marks.parses, 1),
            _ => panic!("text"),
        }
        let line = c.window(0, 1).remove(0);
        assert!(line.spans.iter().any(|s| s.style.add_modifier.contains(Modifier::CROSSED_OUT)));
        shift(&mut c, KeyCode::End, 0);
        let line = c.window(0, 1).remove(0);
        assert!(line.spans.iter().any(|s| {
            s.style.add_modifier.contains(Modifier::REVERSED) && s.style.add_modifier.contains(Modifier::CROSSED_OUT)
        }));
    }

    #[test]
    fn shift_arrows_select_and_esc_clears() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (_d, mut c) = open_bytes(b"one\ntwo\n");
        // Without a selection plain keys belong to the pager.
        for code in [ch('x'), KeyCode::Down, KeyCode::Esc] {
            assert!(matches!(press(&mut c, code, 1), Nav::Pass));
        }
        assert!(c.sel.is_none());
        shift(&mut c, KeyCode::Right, 1);
        shift(&mut c, KeyCode::Right, 1);
        assert_eq!(range(&c), (at(1, 0), at(1, 2)));
        press(&mut c, KeyCode::Left, 1); // a plain arrow moves the caret and collapses
        assert_eq!(range(&c), (at(1, 1), at(1, 1)));
        shift(&mut c, KeyCode::Home, 1);
        assert_eq!(range(&c), (at(1, 0), at(1, 1)));
        assert_eq!(status_of(press(&mut c, KeyCode::Esc, 1)), "");
        assert!(c.sel.is_none());
        // Esc without a selection is the pager's again (never swallowed).
        assert!(matches!(press(&mut c, KeyCode::Esc, 1), Nav::Pass));
    }

    #[test]
    fn ctrl_space_starts_and_clears_selection_mode() {
        let (_d, mut c) = open_bytes(b"one\ntwo\n");
        press_mod(&mut c, ch(' '), KeyModifiers::CONTROL, 1);
        assert_eq!(range(&c), (at(1, 0), at(1, 0)));
        press(&mut c, KeyCode::Right, 1); // the caret moves inside selection mode
        assert_eq!(range(&c), (at(1, 1), at(1, 1)));
        shift(&mut c, KeyCode::End, 1);
        assert_eq!(range(&c), (at(1, 1), at(1, 3)));
        ctrl_as(&mut c, ' ', ' ');
        assert!(c.sel.is_none());
    }

    #[test]
    fn ctrl_word_motion_and_ctrl_shift_extends() {
        let (_d, mut c) = open_bytes(b"alpha beta gamma\n");
        press_mod(&mut c, KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT, 0);
        assert_eq!(range(&c), (at(0, 0), at(0, 6)));
        press_mod(&mut c, KeyCode::Right, KeyModifiers::CONTROL, 0);
        assert_eq!(range(&c), (at(0, 11), at(0, 11)));
        press_mod(&mut c, KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::SHIFT, 0);
        assert_eq!(range(&c), (at(0, 6), at(0, 11)));
        // Ctrl+Shift+End / Ctrl+Home: to the end / start of the file.
        press_mod(&mut c, KeyCode::End, KeyModifiers::CONTROL | KeyModifiers::SHIFT, 0);
        assert_eq!(range(&c), (at(0, 11), at(0, 16))); // the anchor stays at 11
        press_mod(&mut c, KeyCode::Home, KeyModifiers::CONTROL, 0);
        assert_eq!(range(&c), (at(0, 0), at(0, 0)));
    }

    #[test]
    fn old_single_letters_do_nothing() {
        let (d, mut c) = open_bytes(MARKED.as_bytes());
        for l in ['v', 'd', 'i', 'c', 'a', 'x', 'W', ']', '['] {
            assert!(matches!(press(&mut c, ch(l), 0), Nav::Pass), "{l}");
            assert!(c.sel.is_none() && c.pending.is_none() && c.take_input_request().is_none());
        }
        // Inside a selection too: letters neither act nor move.
        select(&mut c, at(0, 0), at(0, 2));
        for l in ['v', 'd', 'r', 'i', 'c', 'x', 'l', 'w', '$'] {
            press(&mut c, ch(l), 0);
            assert_eq!(range(&c), (at(0, 0), at(0, 2)), "{l}");
            assert!(c.pending.is_none() && c.take_input_request().is_none(), "{l}");
        }
        assert_eq!(file_of(&d), MARKED.as_bytes());
    }

    /// Ctrl+L on a Cyrillic layout (typed char, physical `l`) is the annotation list key; a plain `l` is not.
    #[test]
    fn ctrl_l_is_matched_by_physical_key() {
        let mut k = KeyEvent::new(KeyCode::Char('д'), KeyModifiers::CONTROL); // i18n-allow-cyrillic
        k.base_layout = Some('l');
        let phys = crate::keys::phys(&k);
        assert!(crate::keys::is(&k, phys, "Ctrl+l").unwrap());
        assert!(!crate::keys::is(&k, phys, "Ctrl+f").unwrap());
        let plain = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
        assert!(!crate::keys::is(&plain, crate::keys::phys(&plain), "Ctrl+l").unwrap());
    }

    /// Ctrl+G on a Cyrillic layout (typed char, physical `g`) is the hand-off key; a plain `g` is not.
    #[test]
    fn ctrl_g_is_matched_by_physical_key() {
        let mut k = KeyEvent::new(KeyCode::Char('п'), KeyModifiers::CONTROL); // i18n-allow-cyrillic
        k.base_layout = Some('g');
        let phys = crate::keys::phys(&k);
        assert!(crate::keys::is(&k, phys, "Ctrl+g").unwrap());
        assert!(!crate::keys::is(&k, phys, "Ctrl+l").unwrap());
        let plain = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        assert!(!crate::keys::is(&plain, crate::keys::phys(&plain), "Ctrl+g").unwrap());
    }

    fn with_token(c: &mut ViewContent) {
        c.handoff = Some(bridge::Channel { var: VIEWER_VAR, token: "0123abcd".into(), retries: 1 });
    }

    /// Ctrl+G encodes `SAYSDF_VIEWER` = `<token>:handoff-file:<absolute path>` in an OSC 1337
    /// SetUserVar; a second press sends the same bytes again (each SetUserVar is an event).
    #[test]
    fn ctrl_g_hands_off_the_file() {
        let (_d, mut c) = open_bytes(b"abc\n");
        with_token(&mut c);
        let want = format!("0123abcd:handoff-file:{}", c.path.to_str().unwrap());
        let seq = c.handoff_seq().unwrap();
        assert_eq!(seq, format!("\x1b]1337;SetUserVar=SAYSDF_VIEWER={}\x07", crate::cmd::md::base64(want.as_bytes())));
        assert_eq!(c.handoff_seq().unwrap(), seq);
    }

    /// Without the token (viewer not opened by saysdfTerm) Ctrl+G only explains; the key goes
    /// through the regular dispatch.
    #[test]
    fn ctrl_g_without_token_explains() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (_d, mut c) = open_bytes(b"abc\n");
        let pos = c.vp.0;
        match press_mod(&mut c, KeyCode::Char('g'), KeyModifiers::CONTROL, pos) {
            Nav::Status(m) => assert_eq!(m, "open the viewer from saysdfTerm to hand off"),
            _ => panic!("expected a status message"),
        }
    }

    /// Paths with control or invisible/bidi characters are not sent (the phrase lands in the
    /// agent's input); relative paths neither.
    #[test]
    fn handoff_path_rejects_control_and_bidi() {
        let base = std::env::temp_dir();
        assert!(handoff_path(&base.join("notes.md")).is_some());
        for bad in ["a\nb.md", "a\x1b[31m.md", "a\u{202E}dm.txt", "a\u{200B}.md", "a\u{2028}.md"] {
            assert!(handoff_path(&base.join(bad)).is_none(), "{bad:?}");
        }
        assert!(handoff_path(Path::new("relative.md")).is_none());
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (_d, mut c) = open_bytes(b"abc\n");
        with_token(&mut c);
        c.path = base.join("x\u{202E}dm.txt");
        assert_eq!(c.handoff_seq().unwrap_err(), "the path contains control or invisible characters");
    }

    /// The chosen annotation replaces the shown file and lands on its source line.
    #[test]
    fn switching_to_a_listed_file() {
        let (d, mut c) = open_bytes(b"abc
");
        let other = d.path().join("заметка один.md"); // i18n-allow-cyrillic
        std::fs::write(&other, "a
b
{++c++}
").unwrap();
        match c.switch_to(&other, 2).unwrap() {
            Nav::Open { path, line, .. } => assert_eq!((path, line), (other.clone(), 2)),
            _ => panic!("expected Nav::Open"),
        }
        assert_eq!(c.path, other);
        assert!(c.switch_to(&d.path().join("missing.md"), 0).is_err());
        assert_eq!(c.path, other);
    }

    /// A control-file request (D6) for this viewer opens the file in place at its line.
    #[test]
    fn control_request_opens_the_file_at_its_line() {
        let (d, mut c) = open_bytes(b"abc\n");
        let other = d.path().join("b.rs");
        std::fs::write(&other, "1\n2\n3\n4\n").unwrap();
        let cf = control::ControlFile::new(d.path(), "0123abcd").unwrap();
        std::fs::create_dir_all(d.path().join("viewer")).unwrap();
        let body = serde_json::json!({ "subcommand": "view", "path": other, "line": 3, "col": 1 });
        std::fs::write(d.path().join("viewer").join("0123abcd.json"), body.to_string()).unwrap();
        let req = cf.poll().unwrap().unwrap();
        assert_eq!(req.sub, control::Sub::View);
        match Content::open_path(&mut c, &req.path, req.line0()).unwrap().unwrap() {
            Nav::Open { path, line, .. } => assert_eq!((path, line), (other.clone(), 2)),
            _ => panic!("expected Nav::Open"),
        }
        assert_eq!(c.path, other);
        assert!(cf.poll().is_none());
    }

    /// Every command arrives as a Cyrillic char + Ctrl with the physical key in `base_layout`
    /// (Windows VK code, kitty alternate keys) and still works.
    #[test]
    fn ctrl_commands_work_on_a_cyrillic_layout() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        // Russian layout: r=к d=в n=т k=л e=у x=ч w=ц // i18n-allow-cyrillic
        let (d, mut c) = open_bytes(b"abc def\n");
        select(&mut c, at(0, 0), at(0, 3));
        assert!(matches!(ctrl_as(&mut c, 'к', 'r'), Nav::None)); // i18n-allow-cyrillic
        assert!(c.take_input_request().is_some());
        c.input(None, 0).unwrap();
        assert!(status_of(ctrl_as(&mut c, 'в', 'd')).contains("{--text--}")); // i18n-allow-cyrillic
        press(&mut c, KeyCode::Esc, 0);
        for (typed, base) in [('т', 'n'), ('л', 'k')] { // i18n-allow-cyrillic
            assert!(matches!(ctrl_as(&mut c, typed, base), Nav::None), "{base}");
            c.take_input_request().expect("input editor");
            c.input(None, 0).unwrap();
        }
        assert_eq!(file_of(&d), b"abc def\n");
        select(&mut c, at(0, 0), at(0, 3));
        ctrl_as(&mut c, 'л', 'k'); // i18n-allow-cyrillic
        c.take_input_request().unwrap();
        c.input(Some("n".into()), 0).unwrap();
        alt(&mut c, true);
        assert!(matches!(ctrl_as(&mut c, 'у', 'e'), Nav::None)); // i18n-allow-cyrillic
        assert!(c.take_input_request().is_some());
        c.input(None, 0).unwrap();
        assert_eq!(status_of(ctrl_as(&mut c, 'ч', 'x')), "remove the annotation under the caret?"); // i18n-allow-cyrillic
        assert_eq!(status_of(press(&mut c, KeyCode::Enter, 0)), "annotation removed");
        assert_eq!(file_of(&d), b"abc def\n");
        let wrapped = c.wrap;
        assert!(matches!(ctrl_as(&mut c, 'ц', 'w'), Nav::Scroll { .. })); // i18n-allow-cyrillic
        assert_ne!(c.wrap, wrapped);
        // Without a physical key a Cyrillic Ctrl char is not a command.
        select(&mut c, at(0, 0), at(0, 3));
        press_mod(&mut c, ch('к'), KeyModifiers::CONTROL, 0); // i18n-allow-cyrillic
        assert!(c.take_input_request().is_none());
    }

    #[test]
    fn motions_work_by_grapheme_over_cyrillic_wide_and_crlf() {
        let (_d, mut c) = open_bytes("привет мир\r\n日本語\r\nend\r\n".as_bytes()); // i18n-allow-cyrillic
        press_mod(&mut c, KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT, 0);
        assert_eq!(range(&c), (at(0, 0), at(0, 7)));
        shift(&mut c, KeyCode::Down, 0); // the wanted column 7 is clamped to the 3 wide chars
        assert_eq!(range(&c), (at(0, 0), at(1, 3)));
        shift(&mut c, KeyCode::Right, 0); // CRLF is not a column: next line start
        assert_eq!(range(&c), (at(0, 0), at(2, 0)));
    }

    #[test]
    fn selection_spans_lines_and_is_normalised_when_moving_up() {
        let (_d, mut c) = open_bytes(b"aaa\nbbb\nccc\n");
        shift(&mut c, KeyCode::Up, 2);
        shift(&mut c, KeyCode::End, 2);
        let (s, e) = range(&c);
        assert_eq!((s, e), (at(1, 3), at(2, 0)));
        assert!(s <= e);
        shift(&mut c, KeyCode::PageUp, 2);
        assert_eq!(range(&c), (at(0, 3), at(2, 0)));
        // The drawn window reverses cells of every selected line.
        let w = c.window(0, 3);
        let rev = |l: &Line| l.spans.iter().any(|s| s.style.add_modifier.contains(Modifier::REVERSED));
        assert!(rev(&w[0]) && rev(&w[1]) && !rev(&w[2]));
    }

    #[test]
    fn binary_file_is_read_only_and_shift_arrows_explain() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (_d, mut c) = open_bytes(&[0, 1, 2, 3, 0xff, 0]);
        match shift(&mut c, KeyCode::Right, 0) {
            Nav::Status(s) => assert!(s.contains("read-only"), "{s}"),
            _ => panic!("status expected"),
        }
        assert!(c.sel.is_none() && !c.captures_keys());
        assert!(matches!(press(&mut c, KeyCode::Down, 0), Nav::Pass));
        assert_eq!(c.status_extra(), None);
        // Invalid UTF-8 text: shown, but not annotatable.
        let (_d2, mut t) = open_bytes(b"abc \xff def\nsecond\n");
        assert!(matches!(shift(&mut t, KeyCode::Right, 0), Nav::Status(_)));
        assert!(t.sel.is_none());
    }

    fn open_named(name: &str, bytes: &[u8], width: usize) -> (tempfile::TempDir, ViewContent) {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let (mut c, _) = ViewContent::open(&path, 4, DEFAULT_MAX_SIZE).unwrap();
        c.set_width(width);
        c.viewport(0, 0, 5, width);
        (d, c)
    }

    /// Presses a key and applies the scroll answer to the viewport, as the pager does.
    fn step(c: &mut ViewContent, code: KeyCode, mods: KeyModifiers) {
        let pos = c.vp.0;
        if let Nav::Scroll { top, hscroll, .. } = press_mod(c, code, mods, pos) {
            let (_, _, h, w) = c.vp;
            c.viewport(top, hscroll, h, w);
        }
    }

    fn text_of(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn reversed(l: &Line) -> String {
        l.spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn prose_wraps_and_code_does_not() {
        let para = "Мама мыла раму, а папа читал газету у окна.\n".as_bytes(); // i18n-allow-cyrillic
        let (_d, mut md) = open_named("ch.md", para, 16);
        assert!(md.len() > 2, "{}", md.len());
        assert_eq!(md.line_count(), 1);
        assert_eq!((md.line_number(0), md.line_number(1)), (Some(0), None));
        assert_eq!(md.source_of(2), 0);
        let rows: Vec<String> = md.window(0, md.len()).iter().map(text_of).collect();
        assert_eq!(rows[0], "Мама мыла раму, "); // i18n-allow-cyrillic
        assert!(rows.iter().all(|r| saysdf_render::sanitize::str_width(r) <= 16));
        assert_eq!(rows.concat().trim_end(), "Мама мыла раму, а папа читал газету у окна."); // i18n-allow-cyrillic
        let (_d2, mut code) = open_named("main.rs", para, 16);
        assert_eq!((code.len(), code.builds), (1, 0));
        assert_eq!(text_of(&code.window(0, 1)[0]), "Мама мыла раму, а папа читал газету у окна."); // i18n-allow-cyrillic
        // Search runs over the rows.
        assert_eq!(md.doc().map(Doc::len), Some(md.len()));
    }

    #[test]
    fn map_is_built_once_per_load_and_ctrl_w_toggles_it() {
        let (_d, mut c) = open_named("a.txt", b"one two three four five six\nshort\nlast line here\n", 10);
        assert_eq!(c.builds, 1);
        for _ in 0..3 {
            c.window(0, 4);
        }
        c.set_width(10);
        shift(&mut c, KeyCode::Down, 0);
        press(&mut c, KeyCode::Esc, 0);
        assert_eq!(c.builds, 1);
        c.set_width(12);
        assert_eq!(c.builds, 2);
        c.reload().unwrap();
        assert_eq!(c.builds, 3);
        // Toggle off at the row of line 2: the top stays on line 2.
        let top = c.view_of(2);
        c.viewport(top, 0, 5, 12);
        assert!(matches!(press(&mut c, ch('W'), top), Nav::Pass)); // the old letter is gone
        match press_mod(&mut c, ch('w'), KeyModifiers::CONTROL, top) {
            Nav::Scroll { top, .. } => assert_eq!(top, 2),
            _ => panic!("scroll expected"),
        }
        assert!(c.wrapped.is_none() && c.len() == 3 && c.builds == 3);
        c.viewport(2, 0, 5, 12);
        match press_mod(&mut c, ch('w'), KeyModifiers::CONTROL, 2) {
            Nav::Scroll { top, hscroll, .. } => assert_eq!((top, hscroll), (c.view_of(2), 0)),
            _ => panic!("scroll expected"),
        }
        assert_eq!(c.builds, 4);
    }

    #[test]
    fn selection_moves_by_visual_row_and_is_styled_across_rows() {
        // width 10: "one two " / "three " / "four"
        let (_d, mut c) = open_named("a.md", b"one two three four\nx\n", 10);
        assert_eq!(c.len(), 4);
        shift(&mut c, KeyCode::Right, 0);
        shift(&mut c, KeyCode::Right, 0);
        shift(&mut c, KeyCode::Down, 0);
        assert_eq!(range(&c), (at(0, 0), at(0, 10)));
        shift(&mut c, KeyCode::Down, 0);
        assert_eq!(range(&c), (at(0, 0), at(0, 16)));
        shift(&mut c, KeyCode::Down, 0); // the next source line keeps the display column, clamped
        assert_eq!(range(&c), (at(0, 0), at(1, 1)));
        shift(&mut c, KeyCode::Up, 0);
        assert_eq!(range(&c), (at(0, 0), at(0, 16)));
        let w = c.window(0, 4);
        assert_eq!(reversed(&w[0]), "one two ");
        assert_eq!(reversed(&w[1]), "three ");
        assert_eq!(reversed(&w[2]), "fo");
        assert_eq!(reversed(&w[3]), "");
    }

    fn mouse(c: &mut ViewContent, line: usize, col: usize, phase: MousePhase) -> Nav {
        c.mouse(line, col, phase).unwrap()
    }

    #[test]
    fn mouse_drag_selects_over_cyrillic_and_wide_chars_with_hscroll() {
        let (_d, mut c) = open_named("a.rs", "a世b привет\nsecond\n".as_bytes(), 40); // i18n-allow-cyrillic
        c.viewport(0, 0, 5, 40);
        // Cells: a=0 世=1..3 b=3 space=4 п=5 р=6 и=7; graphemes a0 世1 b2 space3 п4 р5 и6. // i18n-allow-cyrillic
        mouse(&mut c, 0, 2, MousePhase::Press); // inside the wide char
        assert_eq!(range(&c), (at(0, 1), at(0, 1)));
        mouse(&mut c, 0, 7, MousePhase::Drag);
        assert_eq!(range(&c), (at(0, 1), at(0, 6)));
        mouse(&mut c, 1, 3, MousePhase::Release);
        assert_eq!(range(&c), (at(0, 1), at(1, 3)));
        assert!(c.captures_keys());
        // Horizontal scroll: the pager adds it to the column, so the same cell maps the same way.
        mouse(&mut c, 0, 5 + 2, MousePhase::Press);
        mouse(&mut c, 0, 5 + 3, MousePhase::Drag);
        assert_eq!(range(&c), (at(0, 6), at(0, 7)));
    }

    #[test]
    fn mouse_click_past_line_end_and_without_drag() {
        let (_d, mut c) = open_named("a.rs", "abc
de
".as_bytes(), 40);
        mouse(&mut c, 0, 50, MousePhase::Press);
        mouse(&mut c, 0, 50, MousePhase::Release);
        assert_eq!(range(&c), (at(0, 3), at(0, 3)));
        assert!(c.sel.unwrap().is_empty() && c.captures_keys());
        // Keys continue from the clicked caret.
        shift(&mut c, KeyCode::Left, 0);
        assert_eq!(range(&c), (at(0, 2), at(0, 3))); // the anchor stays
        // Drag without a press does nothing.
        c.sel = None;
        mouse(&mut c, 1, 1, MousePhase::Drag);
        assert!(c.sel.is_none());
    }

    #[test]
    fn mouse_selects_across_wrapped_rows() {
        // width 10: "one two " / "three " / "four"; second source line follows.
        let (_d, mut c) = open_named("a.md", b"one two three four
x
", 10);
        mouse(&mut c, 0, 4, MousePhase::Press);
        mouse(&mut c, 2, 2, MousePhase::Drag);
        mouse(&mut c, 2, 2, MousePhase::Release);
        assert_eq!(range(&c), (at(0, 4), at(0, 16)));
        mouse(&mut c, 3, 0, MousePhase::Press);
        assert_eq!(range(&c), (at(1, 0), at(1, 0)));
    }

    #[test]
    fn mouse_drag_on_edge_rows_scrolls_and_read_only_ignores_mouse() {
        let text: String = (0..20).map(|i| format!("l{i}
")).collect();
        let (_d, mut c) = open_named("a.rs", text.as_bytes(), 40);
        c.viewport(5, 0, 5, 40);
        mouse(&mut c, 6, 0, MousePhase::Press);
        match mouse(&mut c, 9, 0, MousePhase::Drag) {
            Nav::Scroll { top, .. } => assert_eq!(top, 6),
            _ => panic!("scroll expected"),
        }
        assert_eq!(range(&c), (at(6, 0), at(10, 0)));
        c.viewport(6, 0, 5, 40);
        match mouse(&mut c, 6, 0, MousePhase::Drag) {
            Nav::Scroll { top, .. } => assert_eq!(top, 5),
            _ => panic!("scroll expected"),
        }
        assert_eq!(range(&c), (at(5, 0), at(6, 0)));
        let (_d2, mut r) = open_bytes(b"abc \xff def\nsecond\n");
        mouse(&mut r, 0, 1, MousePhase::Press);
        assert!(r.sel.is_none());
        let (_d3, mut h) = open_bytes(&[0, 1, 2, 3, 0xff]);
        mouse(&mut h, 0, 1, MousePhase::Press);
        assert!(h.sel.is_none());
    }

    #[test]
    fn caret_stays_visible_in_both_modes() {
        let text = "aa bb cc dd ee ff gg hh ii jj kk ll\n";
        let (_d, mut c) = open_named("a.md", text.as_bytes(), 7);
        c.viewport(0, 0, 2, 7);
        shift(&mut c, KeyCode::Right, 0);
        for _ in 0..8 {
            step(&mut c, KeyCode::Down, KeyModifiers::NONE);
            let (row, _) = c.wrapped.as_ref().unwrap().map.pos_to_visual(
                match &c.kind {
                    Kind::Text { lines, .. } => lines,
                    _ => unreachable!(),
                },
                c.sel.unwrap().caret(),
            );
            assert!(row >= c.vp.0 && row < c.vp.0 + 2, "row {row} top {}", c.vp.0);
        }
        assert!(c.vp.0 > 0);
        // Without wrap the horizontal scroll follows the caret.
        let (_d2, mut r) = open_named("a.rs", text.as_bytes(), 10);
        step(&mut r, KeyCode::End, KeyModifiers::SHIFT);
        assert_eq!(r.vp.1, 35 + 1 - 10);
        step(&mut r, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(r.vp.1, 0);
    }

    #[test]
    fn normal_hint_mentions_wrap_in_both_languages() {
        let (_d, c) = open_named("a.md", b"x\n", 20);
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert!(c.status_extra().unwrap().contains("Ctrl: G agent · L list · F diff · W wrap"));
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        let h = c.status_extra().unwrap();
        assert!(h.contains("Ctrl: G агент · L список · F diff · W перенос") && h.contains("Shift+стрелки выделить"), "{h}"); // i18n-allow-cyrillic
    }

    #[test]
    fn hint_follows_the_mode() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (_d, mut c) = open_bytes(b"abc\n");
        assert_eq!(c.status_extra().as_deref(), Some("Shift+arrows select · Ctrl: G agent · L list · F diff · W wrap"));
        shift(&mut c, KeyCode::Right, 0);
        assert_eq!(
            c.status_extra().as_deref(),
            Some("Ctrl: R substitute · D delete · N insert · K comment · Esc cancel")
        );
    }

    #[test]
    fn diff_head_arguments_keep_paths_whole() {
        let root = Path::new("C:/Users/me/my repo");
        let file = Path::new("C:/Users/me/my repo/док/файл 1.md"); // i18n-allow-cyrillic
        let args = diff_head_args(root, file);
        let want: Vec<std::ffi::OsString> =
            ["diff", "--cwd", "C:/Users/me/my repo", "HEAD", "--", "C:/Users/me/my repo/док/файл 1.md"] // i18n-allow-cyrillic
                .iter()
                .map(Into::into)
                .collect();
        assert_eq!(args, want);
    }

    #[test]
    fn ctrl_f_outside_a_repository_says_so_on_any_layout() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (_d, mut c) = open_bytes(b"abc\n");
        // Cyrillic letter on the physical F key (base_layout 'f').
        let st = status_of(ctrl_as(&mut c, 'а', 'f')); // i18n-allow-cyrillic
        if crate::git::toplevel(&c.path.parent().unwrap().to_path_buf()).unwrap().is_none() {
            assert_eq!(st, "not a git repository: no diff against HEAD");
        }
        assert!(!st.is_empty());
        assert_eq!(status_of(ctrl(&mut c, 'f')), st);
    }

    // ---- Annotation actions (tasks 3.3) ----

    fn status_of(nav: Nav) -> String {
        match nav {
            Nav::Status(s) | Nav::Scroll { status: s, .. } => s,
            _ => String::new(),
        }
    }

    fn select(c: &mut ViewContent, a: Pos, b: Pos) {
        let mut sel = Selection::new(a);
        sel.set_caret(b);
        c.sel = Some(sel);
    }

    fn file_of(d: &tempfile::TempDir) -> Vec<u8> {
        std::fs::read(d.path().join("f.txt")).unwrap()
    }

    /// Presses Ctrl+`key`, takes the editor request and submits `typed`; returns the prefill and status.
    fn act(c: &mut ViewContent, key: char, typed: &str) -> (String, String) {
        assert!(matches!(ctrl(c, key), Nav::None));
        let (_, prefill) = c.take_input_request().expect("input editor");
        let st = status_of(c.input(Some(typed.into()), 0).unwrap());
        (prefill, st)
    }

    #[test]
    fn substitute_prefills_cyrillic_and_rebuilds_the_view() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes("кот сидит\n".as_bytes()); // i18n-allow-cyrillic
        for _ in 0..3 {
            shift(&mut c, KeyCode::Right, 0);
        }
        let (prefill, st) = act(&mut c, 'r', "пёс"); // i18n-allow-cyrillic
        assert_eq!(prefill, "кот"); // i18n-allow-cyrillic
        assert_eq!(st, "annotation written");
        assert_eq!(file_of(&d), "{~~кот~>пёс~~} сидит\n".as_bytes()); // i18n-allow-cyrillic
        assert!(c.sel.is_none());
        match &c.kind {
            Kind::Text { lines, marks, .. } => {
                assert_eq!(lines[0], "{~~кот~>пёс~~} сидит"); // i18n-allow-cyrillic
                assert!(!marks.is_empty());
            }
            _ => panic!("text"),
        }
    }

    #[test]
    fn substitute_prefill_of_a_multiline_crlf_selection() {
        let (d, mut c) = open_bytes(b"one\r\ntwo\r\n");
        select(&mut c, at(0, 1), at(1, 2));
        let (prefill, _) = act(&mut c, 'r', "X\nY");
        assert_eq!(prefill, "ne\ntw");
        assert_eq!(file_of(&d), b"o{~~ne\r\ntw~>X\r\nY~~}o\r\n");
    }

    #[test]
    fn delete_needs_a_selection_and_a_confirming_enter() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes(b"\xEF\xBB\xBFone two\r\nthree\r\n");
        select(&mut c, at(0, 4), at(0, 4));
        assert_eq!(status_of(ctrl(&mut c, 'd')), "select some text first");
        assert!(c.take_input_request().is_none());
        // A multi-line selection; Ctrl+D asks, another key cancels, Enter writes.
        select(&mut c, at(0, 4), at(1, 3));
        assert!(status_of(ctrl(&mut c, 'd')).contains("{--text--}"));
        assert_eq!(c.status_extra().as_deref(), Some("Enter write · Esc cancel"));
        assert_eq!(status_of(press(&mut c, KeyCode::Esc, 0)), "annotation cancelled");
        assert!(c.sel.is_some());
        assert_eq!(file_of(&d), b"\xEF\xBB\xBFone two\r\nthree\r\n");
        ctrl(&mut c, 'd');
        assert_eq!(status_of(press(&mut c, KeyCode::Enter, 0)), "annotation written");
        assert_eq!(file_of(&d), b"\xEF\xBB\xBFone {--two\r\nthr--}ee\r\n");
    }

    #[test]
    fn insert_after_a_multiline_selection_with_multiline_text() {
        let (d, mut c) = open_bytes(b"a\r\nb\r\n");
        select(&mut c, at(0, 0), at(1, 1));
        let (prefill, _) = act(&mut c, 'n', "x\ny");
        assert_eq!(prefill, "");
        assert_eq!(file_of(&d), b"a\r\nb{++x\r\ny++}\r\n");
    }

    #[test]
    fn comment_at_the_caret_and_on_a_selection() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes("абв где\n".as_bytes()); // i18n-allow-cyrillic
        select(&mut c, at(0, 3), at(0, 3));
        ctrl(&mut c, 'k');
        let (prompt, _) = c.take_input_request().unwrap();
        assert!(prompt.contains("{>>comment<<}") && !prompt.contains("{=="), "{prompt}");
        c.input(Some("уточнить дату".into()), 0).unwrap(); // i18n-allow-cyrillic
        assert_eq!(file_of(&d), "абв{>>уточнить дату<<} где\n".as_bytes()); // i18n-allow-cyrillic
        // The view now shows the new text; select the word after the mark and comment it.
        let col = "абв{>>уточнить дату<<} ".chars().count(); // i18n-allow-cyrillic
        select(&mut c, at(0, col), at(0, col + 3));
        act(&mut c, 'k', "n");
        assert_eq!(file_of(&d), "абв{>>уточнить дату<<} {==где==}{>>n<<}\n".as_bytes()); // i18n-allow-cyrillic
    }

    #[test]
    fn plain_r_is_left_to_the_pager_reload() {
        let (d, mut c) = open_bytes(b"abc\n");
        assert!(matches!(press(&mut c, ch('r'), 0), Nav::Pass));
        select(&mut c, at(0, 0), at(0, 2));
        assert!(matches!(press(&mut c, ch('r'), 0), Nav::Pass));
        assert!(c.take_input_request().is_none());
        assert_eq!(file_of(&d), b"abc\n");
    }

    #[test]
    fn esc_in_the_editor_cancels_without_writing() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes(b"abc\n");
        select(&mut c, at(0, 0), at(0, 2));
        ctrl(&mut c, 'r');
        assert!(c.take_input_request().is_some());
        assert_eq!(status_of(c.input(None, 0).unwrap()), "annotation cancelled");
        assert!(c.sel.is_some());
        assert_eq!(file_of(&d), b"abc\n");
    }

    #[test]
    fn markup_refusal_writes_nothing_and_keeps_the_selection() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes(b"abc\n");
        select(&mut c, at(0, 0), at(0, 2));
        let (_, st) = act(&mut c, 'r', "a ~> b");
        assert!(st.contains("~>"), "{st}");
        assert_eq!(range(&c), (at(0, 0), at(0, 2)));
        assert_eq!(file_of(&d), b"abc\n");
    }

    #[test]
    fn file_changed_on_disk_is_reloaded_and_not_written() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes(b"abc\n");
        select(&mut c, at(0, 1), at(0, 1));
        ctrl(&mut c, 'n');
        c.take_input_request().unwrap();
        std::fs::write(d.path().join("f.txt"), b"zzz\nyyy\n").unwrap();
        let st = status_of(c.input(Some("x".into()), 0).unwrap());
        assert_eq!(st, "file changed on disk; reloaded, annotation not saved");
        assert_eq!(file_of(&d), b"zzz\nyyy\n");
        match &c.kind {
            Kind::Text { lines, .. } => assert_eq!(lines, &["zzz", "yyy"]),
            _ => panic!("text"),
        }
        assert!(c.sel.is_none());
    }

    #[test]
    fn read_only_views_refuse_annotation_keys() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (_d, mut c) = open_bytes(b"a\0b\x01\x02");
        assert!(matches!(c.kind, Kind::Hex(_)));
        assert_eq!(status_of(ctrl(&mut c, 'd')), "binary file: annotations are not available");
        assert_eq!(status_of(alt(&mut c, true)), "binary file: annotations are not available");
        let (_d, mut c) = open_bytes(b"ok \xFF\xFE bad\n");
        assert_eq!(status_of(ctrl(&mut c, 'k')), "not a UTF-8 file: annotations are not available");
        assert!(c.take_input_request().is_none());
    }

    #[test]
    fn read_only_permission_is_refused_with_a_message() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes(b"abc\n");
        let path = d.path().join("f.txt");
        let orig = std::fs::metadata(&path).unwrap().permissions();
        let mut perm = orig.clone();
        perm.set_readonly(true);
        std::fs::set_permissions(&path, perm).unwrap();
        select(&mut c, at(0, 0), at(0, 1));
        let (_, st) = act(&mut c, 'k', "n");
        std::fs::set_permissions(&path, orig).unwrap();
        assert_eq!(st, "the file is read-only; annotation not saved");
        assert_eq!(file_of(&d), b"abc\n");
        assert!(c.sel.is_some());
    }

    // ---- Navigation, amend, remove (tasks 3.5) ----

    const MARKED: &str = "aa {++X++} bb {--Y--} cc {~~o~>n~~} dd {==h==}{>>c<<} ee {>>note<<}\n";

    /// `n` times Alt+Down; returns the last answer.
    fn jumps(c: &mut ViewContent, n: usize) -> Nav {
        let mut last = Nav::None;
        for _ in 0..n {
            last = alt(c, true);
        }
        last
    }

    fn caret_byte_of(c: &ViewContent) -> usize {
        c.caret_byte().expect("caret")
    }

    #[test]
    fn next_and_previous_annotation_move_the_caret_and_stop_at_the_ends() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (_d, mut c) = open_bytes(MARKED.as_bytes());
        let starts: Vec<usize> = c.marks().iter().map(|m| m.range.start).collect();
        assert_eq!(starts.len(), 5);
        for want in &starts {
            alt(&mut c, true);
            assert_eq!(caret_byte_of(&c), *want);
            assert!(c.sel.as_ref().unwrap().is_empty());
        }
        let st = status_of(alt(&mut c, true));
        assert_eq!(st, "no more annotations");
        assert_eq!(caret_byte_of(&c), starts[4]);
        for want in starts.iter().rev().skip(1) {
            alt(&mut c, false);
            assert_eq!(caret_byte_of(&c), *want);
        }
        assert_eq!(status_of(alt(&mut c, false)), "no more annotations");
        assert_eq!(caret_byte_of(&c), starts[0]);
    }

    #[test]
    fn no_marks_and_shifted_alt_arrows_do_nothing() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes(MARKED.as_bytes());
        // Alt+Shift+Down is not a jump (and not a selection either: Alt is not a selection key).
        press_mod(&mut c, KeyCode::Down, KeyModifiers::ALT | KeyModifiers::CONTROL, 0);
        assert!(c.sel.is_none());
        assert_eq!(file_of(&d), MARKED.as_bytes());
        let (_d, mut c) = open_bytes(b"plain\n");
        assert_eq!(status_of(alt(&mut c, true)), "no annotations");
        assert_eq!(status_of(alt(&mut c, false)), "no annotations");
    }

    #[test]
    fn jump_scrolls_wrapped_rows_into_view() {
        let (_d, mut c) = open_named("a.txt", b"aaaa bbbb cccc dddd eeee ffff {++X++} gggg\n", 10);
        c.viewport(0, 0, 2, 10);
        match alt(&mut c, true) {
            Nav::Scroll { top, .. } => assert!(top > 0, "top {top}"),
            _ => panic!("scroll expected"),
        }
    }

    #[test]
    fn counter_shows_total_and_position() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (_d, mut c) = open_bytes(MARKED.as_bytes());
        let h = c.status_extra().unwrap();
        assert_eq!(h, "annotations: 5 · Shift+arrows select · Alt+↓/↑ annotations · Ctrl: G agent · L list · F diff · W wrap");
        jumps(&mut c, 2);
        let h = c.status_extra().unwrap();
        assert_eq!(h, "annotations: 2/5 · Alt+↓/↑ annotations · Ctrl: E amend · X remove · Esc cancel");
        let (_d, c) = open_bytes(b"plain\n");
        assert_eq!(c.status_extra().as_deref(), Some("Shift+arrows select · Ctrl: G agent · L list · F diff · W wrap"));
    }

    #[test]
    fn amend_each_kind_writes_the_expected_bytes() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes(MARKED.as_bytes());
        let want = [
            ("X", "aa {++Q++} bb"),
            ("n", "{~~o~>Q~~}"),
            ("c", "{==h==}{>>Q<<}"),
            ("note", "{>>Q<<}\n"),
        ];
        // A write ends the selection mode, so each jump starts again from the top.
        alt(&mut c, true);
        let (prefill, st) = act(&mut c, 'e', "Q");
        assert_eq!((prefill.as_str(), st.as_str()), (want[0].0, "annotation written"));
        jumps(&mut c, 2); // the second mark is a deletion: nothing to amend
        assert_eq!(status_of(ctrl(&mut c, 'e')), critic::Refusal::NothingToAmend.to_string());
        assert!(c.take_input_request().is_none());
        for (i, w) in want[1..].iter().enumerate() {
            press(&mut c, KeyCode::Esc, 0);
            jumps(&mut c, i + 3);
            let (prefill, _) = act(&mut c, 'e', "Q");
            assert_eq!(prefill, w.0);
        }
        let out = String::from_utf8(file_of(&d)).unwrap();
        for w in &want {
            assert!(out.contains(w.1), "{out}");
        }
        assert_eq!(out, "aa {++Q++} bb {--Y--} cc {~~o~>Q~~} dd {==h==}{>>Q<<} ee {>>Q<<}\n");
    }

    #[test]
    fn amend_prefills_cyrillic_and_keeps_crlf() {
        let (d, mut c) = open_bytes("кот {>>проверить<<}\r\n".as_bytes()); // i18n-allow-cyrillic
        alt(&mut c, true);
        let (prefill, _) = act(&mut c, 'e', "проверить\nдаты"); // i18n-allow-cyrillic
        assert_eq!(prefill, "проверить"); // i18n-allow-cyrillic
        assert_eq!(file_of(&d), "кот {>>проверить\r\nдаты<<}\r\n".as_bytes()); // i18n-allow-cyrillic
    }

    #[test]
    fn remove_after_substitute_restores_the_original_bytes() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let orig = "\u{feff}кот сидит\r\nдома\r\n"; // i18n-allow-cyrillic
        let (d, mut c) = open_bytes(orig.as_bytes());
        select(&mut c, at(0, 0), at(0, 3));
        act(&mut c, 'r', "пёс"); // i18n-allow-cyrillic
        assert_ne!(file_of(&d), orig.as_bytes());
        alt(&mut c, true);
        // Esc keeps the annotation.
        assert_eq!(status_of(ctrl(&mut c, 'x')), "remove the annotation under the caret?");
        assert_eq!(c.status_extra().as_deref(), Some("Enter write · Esc cancel"));
        assert_eq!(status_of(press(&mut c, KeyCode::Esc, 0)), "annotation cancelled");
        assert_ne!(file_of(&d), orig.as_bytes());
        ctrl(&mut c, 'x');
        assert_eq!(status_of(press(&mut c, KeyCode::Enter, 0)), "annotation removed");
        assert_eq!(file_of(&d), orig.as_bytes());
        assert!(c.marks().is_empty());
    }

    #[test]
    fn amend_and_remove_off_a_mark_do_nothing() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let (d, mut c) = open_bytes(MARKED.as_bytes());
        for k in ['e', 'x'] {
            assert_eq!(status_of(ctrl(&mut c, k)), "no annotation under the caret");
        }
        assert!(c.pending.is_none() && c.take_input_request().is_none());
        assert_eq!(file_of(&d), MARKED.as_bytes());
        // Plain `x` belongs to the pager.
        assert!(matches!(press(&mut c, ch('x'), 0), Nav::Pass));
    }

    /// Search runs over source lines: a phrase split by a wrap break is found, the view goes to
    /// the row where the match starts and the highlight covers both rows.
    #[test]
    fn search_finds_a_phrase_split_by_a_wrap() {
        let text = "один два папа читал газету\nпапа читал снова\n"; // i18n-allow-cyrillic
        let (_d, c) = open_named("ch.md", text.as_bytes(), 16);
        let rows: Vec<String> = (0..c.len()).map(|i| c.doc().unwrap().line_text(i)).collect();
        let q = "папа читал"; // i18n-allow-cyrillic
        let at = rows.iter().position(|r| r.ends_with("папа ")).expect("break inside the phrase"); // i18n-allow-cyrillic
        assert!(!rows[at].contains("читал"), "{rows:?}"); // i18n-allow-cyrillic
        let sd = c.search_doc().unwrap();
        let m = sd.search(q, None, saysdf_render::doc::Direction::Forward).unwrap();
        assert_eq!(m.line, 0);
        assert_eq!(c.match_view(m).0, at); // the row where the match starts
        let (a, b) = (c.match_ranges(q, at), c.match_ranges(q, at + 1));
        assert_eq!(a.len(), 1);
        assert_eq!(&rows[at][a[0].0..a[0].1], "папа "); // i18n-allow-cyrillic
        assert_eq!(b.len(), 1);
        assert!(rows[at + 1][b[0].0..b[0].1].starts_with("читал")); // i18n-allow-cyrillic
        // n/N order: second match is on the next source line, and wraps back to the first.
        let m2 = sd.search(q, Some((m.line, m.start)), saysdf_render::doc::Direction::Forward).unwrap();
        assert_eq!(m2.line, 1);
        assert_eq!(c.match_view(m2).0, c.view_of(1));
        let back = sd.search(q, Some((m2.line, m2.start)), saysdf_render::doc::Direction::Backward).unwrap();
        assert_eq!(back, m);
        // Without wrap the hooks are the document's own and h/l stay enabled.
        assert!(!c.hscroll_enabled());
        let (_d2, code) = open_named("a.rs", text.as_bytes(), 16);
        assert!(code.hscroll_enabled());
        assert_eq!(code.match_view(m).0, 0);
    }
}
