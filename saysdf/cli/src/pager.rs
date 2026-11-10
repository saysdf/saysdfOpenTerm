//! Shared pager (design D2, D9): scrolling, go to line, search, status, mouse.
//! The terminal is raw + alt-screen; a guard (Drop) and a panic hook restore it.

use crate::editor;
use crate::lineedit::{self, LineEdit, Outcome};
use crate::watch::FileWatch;
use saysdf_keys::Key;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Terminal;
use saysdf_render::doc::{Direction, Doc, Match};
use saysdf_render::sanitize::{char_width, str_width, visible_line};
use std::io::{self, Stdout, Write};
use crate::control::{self, Control};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, Instant};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-pager");

/// Source of lines for the pager.
pub trait Content {
    fn len(&self) -> usize;
    /// Ready (escaped) lines `[from, to)`.
    fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>>;
    /// Document for search; `None` means search is unavailable (large file, hex).
    fn doc(&self) -> Option<&Doc>;
    /// Reload the file; `Ok(Some(msg))` is a message for the status line.
    fn reload(&mut self) -> io::Result<Option<String>> {
        Ok(None)
    }
    /// The text area width changed (Markdown re-layouts). The pager keeps the position
    /// through the `doc()` anchors (source line).
    fn set_width(&mut self, _width: usize) {}
    /// Current viewport (called before [`Content::key`]): top line, horizontal
    /// offset, text area size. Needed for zoom anchored at the center (mermaid).
    fn viewport(&mut self, _top: usize, _hscroll: usize, _height: usize, _width: usize) {}
    /// A key the pager does not use (`pos` is the current view line); while
    /// [`Content::captures_keys`], every key. [`Nav::Pass`] means "not mine".
    fn key(&mut self, _k: &KeyEvent, _phys: Option<Key>, _pos: usize) -> io::Result<Nav> {
        Ok(Nav::Pass)
    }
    /// Output over the ratatui frame (OSC 1337 images): window `[top, top+height)`, text column `x`.
    fn overlay(&mut self, _out: &mut dyn io::Write, _top: usize, _height: usize, _x: usize, _hscroll: usize) {}
    /// The cursor/current line (`pos`) after a key or search handled by the pager, and on every
    /// pager tick (about 10 times a second). Lets a content react to cursor changes (debounced
    /// hover messages of `saysdf pick --hover`). Default: nothing.
    fn cursor_moved(&mut self, _pos: usize) {}
    /// Picker list (`saysdf pick`): arrows/PgUp/PgDn/Home/End move the cursor, not the scroll;
    /// `/` search is fuzzy ([`fuzzy_score`]) over the `doc()` lines.
    fn picker(&self) -> bool {
        false
    }
    /// The content takes keys before the pager's own handling (selection mode, a pending
    /// prefix key such as `]`/`[`): while true every key except Ctrl+C goes to
    /// [`Content::key`] first, Esc included; the content answers [`Nav::Pass`] for a key it
    /// leaves to the pager (scrolling, search).
    fn captures_keys(&self) -> bool {
        false
    }
    /// Extra segment of the status line (mark count, context-dependent key hint). The pager
    /// escapes it and shortens the whole line to the pane width; it never wraps.
    fn status_extra(&self) -> Option<String> {
        None
    }
    /// File name shown in the pane header (text-annotations D13): the pager sets it as the
    /// terminal title while it runs, restores the previous one on exit, and leaves it out of the
    /// status line. `None` keeps `Options::title` in the status line.
    fn pane_title(&self) -> Option<String> {
        None
    }
    /// Number of source lines (gutter width, status line); differs from [`Content::len`] when
    /// the view lines are soft-wrapped rows (text-annotations D12).
    fn line_count(&self) -> usize {
        self.len()
    }
    /// Source line numbered in the gutter at view line `view`; `None` for a continuation row.
    fn line_number(&self, view: usize) -> Option<usize> {
        Some(view)
    }
    /// Source line of view line `view` (status line position, editor line).
    fn source_of(&self, view: usize) -> usize {
        view
    }
    /// View line of source line `line` (`:N` go to line).
    fn view_of(&self, line: usize) -> usize {
        line
    }
    /// Document that `/`, `n` and `N` search; matches are `(line, byte)` in its lines. The
    /// default is [`Content::doc`] (match line = view line); a wrapped text returns its source
    /// lines so a phrase split by a wrap break is found.
    fn search_doc(&self) -> Option<&Doc> {
        self.doc()
    }
    /// Search position `(line, byte)` of the start (`end == false`) or the end of view line `view`.
    fn search_anchor(&self, view: usize, end: bool) -> (usize, usize) {
        (view, if end { usize::MAX } else { 0 })
    }
    /// View line and display cell where match `m` starts.
    fn match_view(&self, m: Match) -> (usize, usize) {
        let col = self.doc().map_or(0, |d| str_width(&d.line_text(m.line)[..m.start]));
        (m.line, col)
    }
    /// Byte ranges of `query` to highlight in the text of view line `view`.
    fn match_ranges(&self, query: &str, view: usize) -> Vec<(usize, usize)> {
        self.doc().map_or_else(Vec::new, |d| d.matches_in_line(query, view))
    }
    /// Whether `h`/`l`/arrows and the wheel shift the view horizontally (off while wrapping).
    fn hscroll_enabled(&self) -> bool {
        true
    }
    /// Left-button press/drag/release over the text area: `line` is the view line, `col` the
    /// display cell within the line (gutter removed, horizontal scroll added). Drag and release
    /// outside the text area are clamped to its edge.
    fn mouse(&mut self, _line: usize, _col: usize, _phase: MousePhase) -> io::Result<Nav> {
        Ok(Nav::None)
    }
    /// The content asks for the multi-line input editor at the bottom: `(prompt, prefill)`.
    /// Polled after every key and mouse answer; the prompt is shown in the status line next
    /// to the key hint. The result goes to [`Content::input`].
    fn take_input_request(&mut self) -> Option<(String, String)> {
        None
    }
    /// Result of the input editor: `Some(text)` on Enter, `None` on Esc (`pos` is the current view line).
    fn input(&mut self, _text: Option<String>, _pos: usize) -> io::Result<Nav> {
        Ok(Nav::None)
    }
    /// Shows another file of the same kind in place (viewer control file, D6); `line` is a
    /// 0-based source line. `None`: the content cannot, the subcommand is restarted instead.
    fn open_path(&mut self, _path: &Path, _line: usize) -> Option<io::Result<Nav>> {
        None
    }
    /// The content holds unfinished user work (a pending confirmation): control requests wait.
    fn busy(&self) -> bool {
        false
    }
}

/// Phase of a left-button mouse gesture for [`Content::mouse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MousePhase {
    Press,
    Drag,
    Release,
}

/// Fuzzy match: all query characters in order (case-insensitive). Higher is better:
/// bonus for consecutive characters and word starts, penalty for the first match position.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i64> {
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return None;
    }
    let mut qi = 0;
    let mut score = 0i64;
    let mut prev: Option<usize> = None;
    let mut first = None;
    let mut before = ' ';
    for (i, c) in text.chars().enumerate() {
        if qi < q.len() && c.to_lowercase().eq(std::iter::once(q[qi])) {
            score += 1;
            if prev == Some(i.wrapping_sub(1)) {
                score += 5;
            }
            if !before.is_alphanumeric() {
                score += 3;
            }
            first.get_or_insert(i);
            prev = Some(i);
            qi += 1;
        }
        before = c;
    }
    (qi == q.len()).then(|| score * 100 - first.unwrap_or(0) as i64)
}

/// The content's answer to a key.
pub enum Nav {
    None,
    /// Go to a view line (with highlight).
    Goto(usize),
    /// Go to a line and show a message (link selection).
    Select(usize, String),
    Status(String),
    /// Redraw with a new viewport (content recomputed, e.g. zoom): top line and offset.
    Scroll { top: usize, hscroll: usize, status: String },
    /// The terminal was handed to an external program ([`suspend`]): full redraw and a message.
    Redraw(String),
    /// The content was replaced by another file (link followed / history back).
    Open { path: PathBuf, title: String, line: usize },
    /// The key is not the content's: the pager handles it (only meaningful while
    /// [`Content::captures_keys`]; elsewhere it is the same as [`Nav::None`]).
    Pass,
}

pub struct Options {
    pub path: Option<PathBuf>,
    /// Title in the status line (already escaped).
    pub title: String,
    /// Initial line (0-based), highlighted.
    pub line: Option<usize>,
    pub col: usize,
    pub editor: Option<Vec<String>>,
    pub watch: bool,
    pub status: String,
}

type Term = Terminal<CrosstermBackend<Out>>;

/// Terminal stdout; with `SAYSDF_LOG` every flush writes the sent bytes to the log
/// (escape sequences visible) so one can see which styles actually reached the terminal.
pub(crate) struct Out {
    out: Stdout,
    sent: Vec<u8>,
}

impl Out {
    fn new() -> Self {
        Self { out: io::stdout(), sent: Vec::new() }
    }
}

impl io::Write for Out {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.out.write(buf)?;
        if crate::log::enabled() && self.sent.len() < 64 * 1024 {
            self.sent.extend_from_slice(&buf[..n]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.sent.is_empty() {
            let text = String::from_utf8_lossy(&self.sent).replace('\u{1b}', "\\e");
            let cut: String = text.chars().take(4000).collect();
            crate::log::write(&format!("pager: out {} bytes: {cut}", self.sent.len()));
            self.sent.clear();
        }
        self.out.flush()
    }
}

/// TUI colors are always on. With `NO_COLOR` crossterm writes an empty SGR `\e[;m` instead of a color,
/// which resets ALL attributes: colors, bold and reverse (picker row, status) vanished,
/// and an interface without highlighting is unusable. Found via the `SAYSDF_LOG` log (NO_COLOR=1 came
/// from the environment the terminal was started in). Every ratatui+crossterm TUI calls this before the first draw.
/// Picker row in [`Content::picker`] mode: accent background + bold white.
const PICK_BG: Color = Color::Rgb(0x1f, 0x5f, 0xd0);
const PICK_FG: Color = Color::Rgb(0xff, 0xff, 0xff);

pub(crate) fn init_colors() {
    ratatui::crossterm::style::force_color_output(true);
}

/// Terminal info for the log (with `SAYSDF_LOG`): colors can be turned off by NO_COLOR/TERM;
/// on Windows without VT mode crossterm falls back to WinAPI, where attributes (reverse, bold) do not work.
fn log_env() {
    if !crate::log::enabled() {
        return;
    }
    use std::io::IsTerminal;
    let var = |k: &str| std::env::var(k).ok();
    #[cfg(windows)]
    let ansi = ratatui::crossterm::ansi_support::supports_ansi();
    #[cfg(not(windows))]
    let ansi = true;
    crate::log::write(&format!(
        "pager: env TERM={:?} COLORTERM={:?} NO_COLOR={:?} stdout_tty={} ansi={ansi} size={:?}",
        var("TERM"),
        var("COLORTERM"),
        var("NO_COLOR"),
        io::stdout().is_terminal(),
        ratatui::crossterm::terminal::size().ok(),
    ));
}

/// Temporarily hand the terminal to an external program from [`Content::key`] (block editor in merge);
/// after returning the content answers [`Nav::Redraw`].
pub fn suspend<R>(f: impl FnOnce() -> R) -> io::Result<R> {
    restore_terminal();
    let r = f();
    enable_raw_mode()?;
    enter_screen()?;
    Ok(r)
}

pub(crate) fn enter_screen() -> io::Result<()> {
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    crate::keys::push_flags()
}

pub(crate) fn restore_terminal() {
    let _ = crate::keys::pop_flags();
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
}

/// Guard: the terminal is restored on any exit from the scope.
pub(crate) struct Guard {
    pub(crate) term: Term,
}

impl Guard {
    pub(crate) fn enter() -> io::Result<Self> {
        static HOOK: Once = Once::new();
        HOOK.call_once(|| {
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                prev(info);
            }));
        });
        enable_raw_mode()?;
        let res = (|| {
            enter_screen()?;
            init_colors();
            log_env();
            Terminal::new(CrosstermBackend::new(Out::new()))
        })();
        match res {
            Ok(term) => Ok(Self { term }),
            Err(e) => {
                restore_terminal();
                Err(e)
            }
        }
    }

    /// Temporarily hand the terminal over (external editor).
    fn suspend<R>(&mut self, f: impl FnOnce() -> R) -> io::Result<R> {
        restore_terminal();
        let r = f();
        enable_raw_mode()?;
        enter_screen()?;
        self.term.clear()?;
        Ok(r)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        restore_terminal();
        let _ = self.term.show_cursor();
    }
}

enum Mode {
    Normal,
    Search(LineEdit),
    Goto(LineEdit),
    /// Multi-line input editor requested by [`Content::take_input_request`].
    Input { prompt: String, edit: LineEdit },
}

/// Tab width of the inputs (the viewer's tab width).
const TAB: usize = 4;

struct Pager<C: Content> {
    content: C,
    opts: Options,
    top: usize,
    /// Text area width (without line numbers).
    width: usize,
    hscroll: usize,
    cursor: Option<usize>,
    height: usize,
    mode: Mode,
    query: String,
    last: Option<Match>,
    watch: Option<FileWatch>,
    /// A full redraw is needed (after [`Nav::Redraw`]).
    clear: bool,
    /// Fuzzy search already jumped to the best line; n/N go in order.
    fuzzy_started: bool,
}

/// Escape sequences of the pane title (D13): push the current title (`CSI 22;0t`) and set the new
/// one with OSC 2. The name is reduced to one line of plain text: control, bidi and invisible
/// format characters (`saysdf_i18n::text::single_line`) cannot end the sequence early or disguise
/// the name.
fn title_push(name: &str) -> String {
    let clean: String = saysdf_i18n::text::single_line(name).trim().chars().take(200).collect();
    format!("[22;0t]2;{clean}\\")
}

/// Pops the title saved by [`title_push`] (`CSI 23;0t`).
const TITLE_POP: &str = "[23;0t";

/// Sets the pane title for the scope; the previous title comes back on drop.
struct TitleGuard(bool);

impl TitleGuard {
    fn set(name: Option<&str>, out: &mut impl io::Write) -> Self {
        let Some(name) = name.filter(|n| !n.is_empty()) else { return Self(false) };
        let _ = out.write_all(title_push(name).as_bytes());
        let _ = out.flush();
        Self(true)
    }
}

impl Drop for TitleGuard {
    fn drop(&mut self) {
        if self.0 {
            let mut out = io::stdout();
            let _ = out.write_all(TITLE_POP.as_bytes());
            let _ = out.flush();
        }
    }
}

/// How a pager run with a [`Control`] ended.
#[derive(Debug)]
pub enum Exit {
    Quit,
    /// The control file asked for another subcommand (or a content without [`Content::open_path`]).
    Switch(control::Request),
}

/// Interval of the control file poll (a single `metadata` call).
const CONTROL_POLL: Duration = Duration::from_millis(100);

/// Run the pager until exit by `q`/Esc.
pub fn run<C: Content>(content: C, opts: Options) -> io::Result<()> {
    run_ctl(content, opts, None).map(|_| ())
}

/// [`run`] that also polls the viewer control file of `ctl` (D6).
pub fn run_ctl<C: Content>(content: C, opts: Options, ctl: Option<Control>) -> io::Result<Exit> {
    let mut polled = Instant::now();
    let mut held = None;
    let watch = match (&opts.path, opts.watch) {
        (Some(p), true) => Some(FileWatch::new(p)),
        _ => None,
    };
    let mut p = Pager {
        content,
        top: 0,
        width: 0,
        hscroll: 0,
        cursor: None,
        height: 1,
        mode: Mode::Normal,
        query: String::new(),
        last: None,
        watch,
        opts,
        clear: false,
        fuzzy_started: false,
    };
    let mut guard = Guard::enter()?;
    let _title = TitleGuard::set(p.content.pane_title().as_deref(), &mut io::stdout());
    let size = guard.term.size()?;
    p.height = size.height.saturating_sub(1).max(1) as usize;
    p.width = (size.width as usize).saturating_sub(p.gutter());
    p.content.set_width(p.width);
    if let Some(l) = p.opts.line {
        // opts.line is a source line; map it to a view line through the document anchors
        let l = p.content.doc().map_or(l, |d| d.view_line(l));
        p.goto(l);
        p.hscroll = p.opts.col.saturating_sub(1).saturating_sub(10);
    }
    if p.watch.as_ref().is_some_and(|w| w.is_polling()) {
        p.opts.status = I18N.t("watch unavailable: polling every 2 s").into_owned();
    }
    loop {
        guard.term.draw(|f| p.draw(f))?;
        crate::log::slog!("pager: draw cursor={:?} top={} hscroll={} height={}", p.cursor, p.top, p.hscroll, p.height);
        {
            let mut out = io::stdout();
            let gutter = p.gutter();
            p.content.overlay(&mut out, p.top, p.height, gutter, p.hscroll);
        }
        if event::poll(Duration::from_millis(100))? {
            let (ev, phys) = crate::keys::read()?;
            crate::log::slog!("pager: event {ev:?} cursor={:?} top={} hscroll={}", p.cursor, p.top, p.hscroll);
            match p.event(ev, phys)? {
                Action::Quit => return Ok(Exit::Quit),
                Action::Editor => p.open_editor(&mut guard)?,
                Action::None => {}
            }
            if std::mem::take(&mut p.clear) {
                guard.term.clear()?;
            }
        }
        let pos = p.cursor.unwrap_or(p.top);
        p.content.cursor_moved(pos);
        if p.watch.as_mut().is_some_and(|w| w.poll()) {
            p.reload();
        }
        if let Some(c) = ctl.as_ref().filter(|_| polled.elapsed() >= CONTROL_POLL) {
            polled = Instant::now();
            if let Some(next) = p.tick_control(c, &mut held) {
                return Ok(Exit::Switch(next));
            }
        }
    }
}

enum Action {
    None,
    Quit,
    Editor,
}

impl<C: Content> Pager<C> {
    /// One terminal event. Resize/focus/mouse motion do not touch the selection: at sidebar
    /// start several of them arrive (split, width fit, mouse cursor over the pane).
    fn event(&mut self, ev: Event, phys: Option<Key>) -> io::Result<Action> {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => return self.key(k, phys),
            Event::Mouse(m) => {
                let shift = m.modifiers.contains(KeyModifiers::SHIFT);
                let ctrl = m.modifiers.contains(KeyModifiers::CONTROL);
                match m.kind {
                    // Ctrl+wheel zooms the content (like `+`/`-`) if the terminal passes the modifier
                    MouseEventKind::ScrollUp if ctrl => self.content_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE), None)?,
                    MouseEventKind::ScrollDown if ctrl => self.content_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE), None)?,
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                        if !self.content.hscroll_enabled() && (shift || matches!(m.kind, MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight)) => {}
                    MouseEventKind::ScrollDown if shift => self.hscroll += 4,
                    MouseEventKind::ScrollUp if shift => self.hscroll = self.hscroll.saturating_sub(4),
                    MouseEventKind::ScrollRight => self.hscroll += 4,
                    MouseEventKind::ScrollLeft => self.hscroll = self.hscroll.saturating_sub(4),
                    // in picker mode the wheel moves the selection (otherwise the selected row scrolls off the edge)
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if self.content.picker() => {
                        let last = self.content.len().saturating_sub(1);
                        let cur = self.cursor.unwrap_or(self.top);
                        let to = if m.kind == MouseEventKind::ScrollDown { (cur + 3).min(last) } else { cur.saturating_sub(3) };
                        self.select(to);
                    }
                    MouseEventKind::ScrollDown => self.scroll(3),
                    MouseEventKind::ScrollUp => self.scroll(-3),
                    MouseEventKind::Down(MouseButton::Left) => self.mouse(MousePhase::Press, m.column, m.row)?,
                    MouseEventKind::Drag(MouseButton::Left) => self.mouse(MousePhase::Drag, m.column, m.row)?,
                    MouseEventKind::Up(MouseButton::Left) => self.mouse(MousePhase::Release, m.column, m.row)?,
                    _ => {}
                }
            }
            Event::Resize(_, h) => {
                self.height = h.saturating_sub(1).max(1) as usize;
                self.clamp();
            }
            _ => {}
        }
        Ok(Action::None)
    }

    fn gutter(&self) -> usize {
        self.content.line_count().max(1).to_string().len() + 1
    }

    /// Source lines for the current top/cursor, to restore the position after a re-layout.
    fn anchors(&self) -> (usize, Option<usize>) {
        match self.content.doc() {
            Some(d) => (d.source_line(self.top), self.cursor.map(|c| d.source_line(c))),
            None => (self.top, self.cursor),
        }
    }

    fn restore(&mut self, (top, cursor): (usize, Option<usize>)) {
        if let Some(d) = self.content.doc() {
            self.top = d.view_line(top);
            self.cursor = cursor.map(|c| d.view_line(c));
        }
    }

    fn max_top(&self) -> usize {
        self.content.len().saturating_sub(self.height)
    }

    fn clamp(&mut self) {
        self.top = self.top.min(self.max_top());
        // in picker mode the cursor is always in the visible area (after resize/height change)
        if self.content.picker() {
            if let Some(c) = self.cursor {
                self.select(c.min(self.content.len().saturating_sub(1)));
            }
        }
    }

    fn scroll(&mut self, d: isize) {
        self.top = self.top.saturating_add_signed(d).min(self.max_top());
    }

    /// Move the picker cursor to line `l` with minimal scrolling ([`Content::picker`] mode).
    fn select(&mut self, l: usize) {
        self.cursor = Some(l);
        if l < self.top {
            self.top = l;
        } else if l >= self.top + self.height {
            self.top = (l + 1 - self.height).min(self.max_top());
        }
    }

    /// Fuzzy search ([`Content::picker`]): the first time the best line, n/N the next/previous
    /// matching line in order from the cursor.
    fn find_fuzzy(&mut self, dir: Direction) -> Option<usize> {
        let doc = self.content.doc()?;
        let hits: Vec<(usize, i64)> =
            (0..doc.len()).filter_map(|i| fuzzy_score(&self.query, &doc.line_text(i)).map(|s| (i, s))).collect();
        if !self.fuzzy_started {
            return hits.iter().max_by_key(|(i, s)| (*s, std::cmp::Reverse(*i))).map(|h| h.0);
        }
        let cur = self.cursor.unwrap_or(self.top);
        match dir {
            Direction::Forward => hits.iter().find(|h| h.0 > cur).or(hits.first()).map(|h| h.0),
            Direction::Backward => hits.iter().rev().find(|h| h.0 < cur).or(hits.last()).map(|h| h.0),
        }
    }

    /// Go to a line (0-based) with highlight; past the end it clamps and shows a message.
    fn goto(&mut self, line: usize) {
        let n = self.content.len();
        let l = if n == 0 {
            0
        } else if line >= n {
            self.opts.status = I18N.f("line {line} is beyond the end of the file ({n} lines)", &[("line", &(line + 1).to_string()), ("n", &n.to_string())]);
            n - 1
        } else {
            line
        };
        self.cursor = Some(l);
        self.top = l.saturating_sub(self.height / 3).min(self.max_top());
    }

    /// `:N`: source line `line`, mapped to its view line ([`Content::view_of`]).
    fn goto_source(&mut self, line: usize) {
        let n = self.content.line_count();
        if n > 0 && line >= n {
            self.goto(self.content.view_of(n - 1));
            self.opts.status = I18N.f("line {line} is beyond the end of the file ({n} lines)", &[("line", &(line + 1).to_string()), ("n", &n.to_string())]);
        } else {
            self.goto(self.content.view_of(line));
        }
    }

    fn key(&mut self, k: KeyEvent, phys: Option<Key>) -> io::Result<Action> {
        match &mut self.mode {
            Mode::Search(s) | Mode::Goto(s) => {
                match s.key(&k) {
                    Outcome::Cancel => self.mode = Mode::Normal,
                    Outcome::Handled | Outcome::Ignored => {}
                    Outcome::Submit => {
                        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
                        match mode {
                            Mode::Search(q) if !q.is_empty() => {
                                self.query = q.into_text();
                                self.last = None;
                                self.fuzzy_started = false;
                                self.find(Direction::Forward);
                            }
                            Mode::Goto(n) => match n.text().trim().parse::<usize>() {
                                Ok(n) => self.goto_source(n.saturating_sub(1)),
                                Err(_) => self.opts.status = I18N.t("a line number is expected").into_owned(),
                            },
                            _ => {}
                        }
                    }
                }
                return Ok(Action::None);
            }
            Mode::Input { edit, .. } => {
                let text = match edit.key(&k) {
                    Outcome::Cancel => None,
                    Outcome::Submit => Some(std::mem::replace(edit, LineEdit::new()).into_text()),
                    Outcome::Handled | Outcome::Ignored => return Ok(Action::None),
                };
                self.mode = Mode::Normal;
                let pos = self.cursor.unwrap_or(self.top);
                let nav = self.content.input(text, pos)?;
                self.apply_nav(nav);
                return Ok(Action::None);
            }
            Mode::Normal => {}
        }
        let is = |spec| crate::keys::is(&k, phys, spec);
        if self.content.captures_keys() && !is("Ctrl+c")? {
            let pos = self.cursor.unwrap_or(self.top);
            self.content.viewport(self.top, self.hscroll, self.height, self.width);
            match self.content.key(&k, phys, pos)? {
                Nav::Pass => {}
                nav => {
                    self.apply_nav(nav);
                    return Ok(Action::None);
                }
            }
        }
        let page = self.height.max(2) as isize - 1;
        if self.content.picker() {
            let last = self.content.len().saturating_sub(1) as isize;
            let cur = self.cursor.unwrap_or(self.top) as isize;
            let to = match k.code {
                _ if k.code == KeyCode::Down || is("j")? => Some(cur + 1),
                _ if k.code == KeyCode::Up || is("k")? => Some(cur - 1),
                KeyCode::PageDown => Some(cur + page),
                KeyCode::PageUp => Some(cur - page),
                _ if k.code == KeyCode::Home || is("g")? => Some(0),
                _ if k.code == KeyCode::End || is("G")? => Some(last),
                _ => None,
            };
            if let Some(to) = to {
                self.select(to.clamp(0, last.max(0)) as usize);
                return Ok(Action::None);
            }
        }
        match k.code {
            KeyCode::Esc => return Ok(Action::Quit),
            _ if is("q")? || is("Ctrl+c")? => return Ok(Action::Quit),
            _ if k.code == KeyCode::Down || is("j")? => self.scroll(1),
            _ if k.code == KeyCode::Up || is("k")? => self.scroll(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll(page),
            KeyCode::PageUp => self.scroll(-page),
            _ if k.code == KeyCode::Home || is("g")? => self.top = 0,
            _ if k.code == KeyCode::End || is("G")? => self.top = self.max_top(),
            // saysdfTerminal: no horizontal shift while the content wraps
            _ if !self.content.hscroll_enabled() && (matches!(k.code, KeyCode::Left | KeyCode::Right) || is("h")? || is("l")?) => {}
            _ if k.code == KeyCode::Left || is("h")? => self.hscroll = self.hscroll.saturating_sub(4),
            _ if k.code == KeyCode::Right || is("l")? => self.hscroll += 4,
            _ if is("0")? => self.hscroll = 0,
            _ if is("/")? => self.mode = Mode::Search(LineEdit::new()),
            _ if is(":")? => self.mode = Mode::Goto(LineEdit::new()),
            _ if is("n")? => self.find(Direction::Forward),
            _ if is("N")? => self.find(Direction::Backward),
            _ if is("r")? => self.reload(),
            _ if is("e")? => return Ok(Action::Editor),
            _ => self.content_key(k, phys)?,
        }
        Ok(Action::None)
    }

    /// Key for the content.
    fn content_key(&mut self, k: KeyEvent, phys: Option<Key>) -> io::Result<()> {
        let pos = self.cursor.unwrap_or(self.top);
        self.content.viewport(self.top, self.hscroll, self.height, self.width);
        let nav = self.content.key(&k, phys, pos)?;
        self.apply_nav(nav);
        Ok(())
    }

    /// Left-button gesture over the text area, mapped to content coordinates.
    fn mouse(&mut self, phase: MousePhase, column: u16, row: u16) -> io::Result<()> {
        if matches!(self.mode, Mode::Input { .. }) {
            return Ok(());
        }
        let n = self.content.len();
        let (row, column) = (row as usize, column as usize);
        if n == 0 || phase == MousePhase::Press && row >= self.height {
            return Ok(());
        }
        let line = (self.top + row.min(self.height - 1)).min(n - 1);
        let col = column.saturating_sub(self.gutter()) + self.hscroll;
        let nav = self.content.mouse(line, col, phase)?;
        self.apply_nav(nav);
        Ok(())
    }

    /// Apply the content's answer to a key, mouse gesture or input; then open the input
    /// editor if the content asked for it.
    fn apply_nav(&mut self, nav: Nav) {
        self.apply_nav_only(nav);
        if let Some((prompt, text)) = self.content.take_input_request() {
            self.mode = Mode::Input { prompt, edit: LineEdit::multiline(&text) };
        }
    }

    /// One control tick. While the user's work is open (input editor, a confirmation of the
    /// content) the request is held: the file stays (a newer write replaces it) and the status
    /// says which file waits; `held` remembers the version already announced. Otherwise the
    /// request is consumed and applied by [`Self::control`].
    fn tick_control(&mut self, c: &Control, held: &mut Option<control::Stamp>) -> Option<control::Request> {
        if matches!(self.mode, Mode::Input { .. }) || self.content.busy() {
            let stamp = c.file.stamp()?;
            if held.as_ref() != Some(&stamp) {
                *held = Some(stamp);
                if let Some(Ok(r)) = c.file.peek() {
                    let name = r.path.file_name().map_or_else(|| r.path.to_string_lossy(), |n| n.to_string_lossy());
                    self.opts.status = I18N.f("file waiting: {name}", &[("name", &visible_line(&name, TAB))]);
                }
            }
            return None;
        }
        *held = None;
        let req = c.file.poll()?;
        self.control(req, c.sub)
    }

    /// A polled control request while `sub` is showing: the same subcommand opens the file in
    /// place (column as for `--col`); otherwise the request is returned for a switch. An
    /// invalid request only leaves a status line.
    fn control(&mut self, req: Result<control::Request, control::Invalid>, sub: control::Sub) -> Option<control::Request> {
        let req = match req {
            Ok(r) => r,
            Err(_) => {
                self.opts.status = I18N.t("ignored an invalid open request").into_owned();
                return None;
            }
        };
        if req.sub != sub {
            return Some(req);
        }
        let Some(res) = self.content.open_path(&req.path, req.line0()) else { return Some(req) };
        match res {
            Ok(nav) => {
                self.mode = Mode::Normal;
                self.apply_nav_only(nav);
                if req.line.is_none() {
                    self.cursor = None;
                }
                self.hscroll = req.col.unwrap_or(1).saturating_sub(1).saturating_sub(10);
            }
            Err(e) => {
                let shown = visible_line(&req.path.to_string_lossy(), TAB);
                self.opts.status = I18N.f("cannot open {path}: {error}", &[("path", &shown), ("error", &e.to_string())]);
            }
        }
        None
    }

    fn apply_nav_only(&mut self, nav: Nav) {
        {
            {
                match nav {
                    Nav::None | Nav::Pass => {}
                    Nav::Goto(l) => self.goto(l),
                    Nav::Select(l, s) => {
                        self.goto(l);
                        self.opts.status = s;
                    }
                    Nav::Status(s) => self.opts.status = s,
                    Nav::Redraw(s) => {
                        self.opts.status = s;
                        self.clear = true;
                        self.clamp();
                    }
                    Nav::Scroll { top, hscroll, status } => {
                        self.top = top;
                        self.hscroll = hscroll;
                        self.cursor = None;
                        self.opts.status = status;
                        self.clamp();
                    }
                    Nav::Open { path, title, line } => {
                        if self.watch.is_some() {
                            self.watch = Some(FileWatch::new(&path));
                        }
                        self.opts.path = Some(path);
                        self.opts.title = title;
                        self.query.clear();
                        self.last = None;
                        self.hscroll = 0;
                        self.opts.status.clear();
                        self.goto(line);
                    }
                }
            }
        }
    }

    fn find(&mut self, dir: Direction) {
        if self.query.is_empty() {
            self.opts.status = I18N.t("no query: / to search").into_owned();
            return;
        }
        if self.content.picker() {
            match self.find_fuzzy(dir) {
                Some(l) => {
                    self.fuzzy_started = true;
                    self.select(l);
                    self.opts.status.clear();
                }
                None => self.opts.status = I18N.f("not found: {query}", &[("query", &visible_line(&self.query, 4))]),
            }
            return;
        }
        let Some(doc) = self.content.search_doc() else {
            self.opts.status = I18N.t("search is unavailable for this view").into_owned();
            return;
        };
        let from = match (self.last, dir) {
            (Some(m), _) => Some((m.line, m.start)),
            (None, Direction::Forward) if self.top > 0 => Some(self.content.search_anchor(self.top - 1, true)),
            (None, Direction::Forward) => None,
            (None, Direction::Backward) => Some(self.content.search_anchor(self.top, false)),
        };
        match doc.search(&self.query, from, dir) {
            Some(m) => {
                let (line, col) = self.content.match_view(m);
                self.last = Some(m);
                self.cursor = Some(line);
                if line < self.top || line >= self.top + self.height {
                    self.top = line.saturating_sub(self.height / 3).min(self.max_top());
                }
                if col < self.hscroll || col > self.hscroll + 60 {
                    self.hscroll = col.saturating_sub(10);
                }
                self.opts.status.clear();
            }
            None => self.opts.status = I18N.f("not found: {query}", &[("query", &visible_line(&self.query, 4))]),
        }
    }

    fn reload(&mut self) {
        let pos = self.anchors();
        match self.content.reload() {
            Ok(msg) => {
                self.opts.status = msg.unwrap_or_else(|| I18N.t("reloaded").into_owned());
                self.last = None;
                self.restore(pos);
                if let Some(c) = self.cursor {
                    self.cursor = Some(c.min(self.content.len().saturating_sub(1)));
                }
                self.clamp();
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.opts.status = I18N.t("file deleted").into_owned();
            }
            Err(e) => self.opts.status = visible_line(&I18N.f("read error: {error}", &[("error", &e.to_string())]), 4),
        }
    }

    fn open_editor(&mut self, guard: &mut Guard) -> io::Result<()> {
        let (Some(cmd), Some(path)) = (self.opts.editor.clone(), self.opts.path.clone()) else {
            self.opts.status = I18N.t("no editor set (--editor-cmd)").into_owned();
            return Ok(());
        };
        let line = self.content.source_of(self.cursor.unwrap_or(self.top)) + 1;
        let col = self.opts.col.max(1);
        let res = guard.suspend(|| editor::run(&cmd, &path, line, col))?;
        if let Err(e) = res {
            self.opts.status = visible_line(&e, 4);
        }
        Ok(())
    }

    fn draw(&mut self, f: &mut ratatui::Frame) {
        let area = f.area();
        let edit_rows = self.edit_rows(area.width as usize, area.height as usize);
        let eh = edit_rows.as_ref().map_or(0, |r| r.rows.len());
        // the hints take a second row when they do not fit in one (D13); the text area shrinks
        let mut status = self.status_rows(self.content.line_count(), area.width as usize);
        status.truncate((area.height as usize).saturating_sub(1).max(1));
        self.height = (area.height as usize).saturating_sub(status.len() + eh).max(1);
        self.clamp();
        let width = (area.width as usize).saturating_sub(self.gutter());
        if width != self.width {
            let pos = self.anchors();
            self.width = width;
            self.content.set_width(width);
            self.restore(pos);
            self.clamp();
        }
        let n = self.content.len();
        let gutter = self.gutter();
        let width = (area.width as usize).saturating_sub(gutter);
        let to = (self.top + self.height).min(n);
        self.content.viewport(self.top, self.hscroll, self.height, width);
        let window = self.content.window(self.top, to);
        let match_style = Style::new().add_modifier(Modifier::REVERSED);
        let mut out = Vec::with_capacity(self.height + 1);
        for (k, line) in window.into_iter().enumerate() {
            let i = self.top + k;
            let mut line = line;
            if !self.query.is_empty() && !self.content.picker() {
                let ranges = self.content.match_ranges(&self.query, i);
                if !ranges.is_empty() {
                    line = apply_ranges(line, &ranges, match_style);
                }
            }
            let number = match self.content.line_number(i) {
                Some(n) => format!("{:>w$} ", n + 1, w = gutter - 1),
                None => " ".repeat(gutter),
            };
            let mut spans = vec![Span::styled(number, Style::new().fg(Color::DarkGray))];
            let mut body = slice_cells(&line, self.hscroll, width);
            if self.cursor == Some(i) {
                spans[0].style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
                let picker = self.content.picker();
                for s in &mut body.spans {
                    // RGB, not palette indices: in Modus schemes "bright white" (15) is gray, so text on blue
                    // blended in; white on the accent blue is readable in light and dark schemes
                    s.style = if picker { s.style.bg(PICK_BG).fg(PICK_FG).add_modifier(Modifier::BOLD) } else { s.style.bg(Color::DarkGray) };
                }
            }
            spans.extend(body.spans);
            out.push(Line::from(spans));
        }
        while out.len() < self.height {
            out.push(Line::from(Span::styled("~", Style::new().fg(Color::DarkGray))));
        }
        if let Some(r) = &edit_rows {
            let style = Style::new().fg(Color::Yellow);
            for (i, row) in r.rows.iter().enumerate() {
                let mark = if i == 0 { "> " } else { "  " };
                out.push(Line::from(vec![Span::styled(mark, style), Span::raw(row.clone())]));
            }
        }
        out.extend(status);
        f.render_widget(Paragraph::new(out), area);
        if let Some(r) = &edit_rows {
            // two cells for the "> " mark; the editor rows sit between the text and the status line
            let (row, col) = r.caret;
            let pos = ratatui::layout::Position::new((col + 2) as u16, area.y + (self.height + row) as u16);
            f.set_cursor_position(pos);
        } else if let Some((_, col)) = self.input_caret(area.width as usize) {
            // the status line is the row under the text area; one cell for the `/` or `:` prompt
            f.set_cursor_position(ratatui::layout::Position::new((col + 1) as u16, area.y + self.height as u16));
        }
    }

    /// Escaped input text and caret cell of the open search/goto input (after the prompt).
    fn input_caret(&self, width: usize) -> Option<(String, usize)> {
        match &self.mode {
            Mode::Search(s) | Mode::Goto(s) => Some(s.view(TAB, width.saturating_sub(2))),
            Mode::Normal | Mode::Input { .. } => None,
        }
    }

    /// Rows of the open input editor: it grows with the text up to a third of the pane
    /// (at least one row), leaving room for one text row and the status line.
    fn edit_rows(&self, width: usize, height: usize) -> Option<lineedit::Rows> {
        let Mode::Input { edit, .. } = &self.mode else { return None };
        let max = (height / 3).min(height.saturating_sub(2)).max(1);
        Some(edit.rows(TAB, width.saturating_sub(2), max))
    }

    /// Status rows: one, or two in normal mode when the text (position, hints, message) does not
    /// fit in one; beyond two rows the text is shortened with `…`, never wrapped over content.
    fn status_rows(&self, n: usize, width: usize) -> Vec<Line<'static>> {
        let style = Style::new().add_modifier(Modifier::REVERSED);
        let row = |t: String| Line::from(Span::styled(t, style));
        let text = match (&self.mode, self.input_caret(width)) {
            (Mode::Search(_), Some((s, _))) => format!("/{s}"),
            (Mode::Goto(_), Some((s, _))) => format!(":{s}"),
            (Mode::Input { prompt, .. }, _) => format!(" {}  {}", visible_line(prompt, TAB), lineedit::hint()),
            _ => {
                let pos = self.content.source_of(self.cursor.unwrap_or(self.top)) + 1;
                let mut t = if self.content.pane_title().is_some() {
                    format!(" {}/{}", pos.min(n.max(1)), n)
                } else {
                    format!(" {}  {}/{}", self.opts.title, pos.min(n.max(1)), n)
                };
                if self.hscroll > 0 {
                    t.push_str(&format!("  >{}", self.hscroll));
                }
                if let Some(extra) = self.content.status_extra().filter(|e| !e.is_empty()) {
                    t.push_str("  ");
                    t.push_str(&visible_line(&extra, TAB));
                }
                if !self.opts.status.is_empty() {
                    t.push_str("  | ");
                    t.push_str(&self.opts.status);
                }
                let (first, rest) = split_rows(&t, width);
                let mut rows = vec![row(first)];
                rows.extend(rest.map(|r| row(shorten(&r, width))));
                return rows;
            }
        };
        vec![row(shorten(&text, width))]
    }
}

/// Split an escaped one-line text for two rows. Breaks are only at the segment separators (two
/// spaces) and the hint pair separator ` · `, so a key is never parted from its label; the first
/// row ends at the last separator that fits `width`, the second starts with the next segment.
/// `None` when it fits one row; with no separator that fits, the single row is cut with `…`.
fn split_rows(text: &str, width: usize) -> (String, Option<String>) {
    if str_width(text) <= width {
        return (text.to_owned(), None);
    }
    let mut best: Option<(usize, usize)> = None; // (end of the first row, start of the second)
    for (sep, len) in [("  ", 2), (" \u{b7} ", 4)] {
        for (i, _) in text.match_indices(sep) {
            let head = text[..i].trim_end();
            if !head.is_empty() && str_width(head) <= width && best.is_none_or(|(e, _)| head.len() > e) {
                best = Some((head.len(), i + len));
            }
        }
    }
    match best {
        Some((end, start)) if !text[start..].trim_start().is_empty() => {
            (text[..end].to_owned(), Some(text[start..].trim_start().to_owned()))
        }
        _ => (shorten(text, width), None),
    }
}

/// Cut an escaped one-line text to `width` cells, ending with `…` when cut.
fn shorten(text: &str, width: usize) -> String {
    if str_width(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = char_width(c);
        if used + w + 1 > width {
            break;
        }
        used += w;
        out.push(c);
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// Applies `style` to byte ranges of the line text (concatenation of spans).
pub fn apply_ranges(line: Line<'static>, ranges: &[(usize, usize)], style: Style) -> Line<'static> {
    let mut out = Vec::new();
    let mut base = 0;
    for span in line.spans {
        let text = span.content.as_ref();
        let end = base + text.len();
        let mut cuts = vec![0, text.len()];
        for &(s, e) in ranges {
            for b in [s, e] {
                if b > base && b < end && text.is_char_boundary(b - base) {
                    cuts.push(b - base);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let hit = ranges.iter().any(|&(s, e)| base + a >= s && base + a < e);
            let st = if hit { span.style.patch(style) } else { span.style };
            out.push(Span::styled(text[a..b].to_owned(), st));
        }
        base = end;
    }
    Line::from(out)
}

/// Slice a line by cells `[skip, skip + width)`; a wide character cut by the edge becomes spaces.
pub fn slice_cells(line: &Line<'_>, skip: usize, width: usize) -> Line<'static> {
    let mut out = Vec::new();
    let mut col = 0;
    let end = skip + width;
    for span in &line.spans {
        let mut s = String::new();
        for c in span.content.chars() {
            let w = char_width(c);
            let (a, b) = (col, col + w);
            col = b;
            if b <= skip || w == 0 && a < skip {
                continue;
            }
            if a >= end {
                break;
            }
            if a < skip || b > end {
                // Half a character at the edge
                s.extend(std::iter::repeat_n(' ', b.min(end) - a.max(skip)));
            } else {
                s.push(c);
            }
        }
        if !s.is_empty() {
            out.push(Span::styled(s, span.style));
        }
        if col >= end {
            break;
        }
    }
    Line::from(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn fuzzy_prefers_word_starts_and_runs() {
        assert!(fuzzy_score("gst", "git status").is_some());
        assert!(fuzzy_score("xyz", "git status").is_none());
        assert!(fuzzy_score("", "git").is_none());
        assert!(fuzzy_score("ТЕМ", "Тема тёмная").is_some()); // i18n-allow-cyrillic
        let a = fuzzy_score("dev", "Ветка dev").unwrap(); // i18n-allow-cyrillic
        let b = fuzzy_score("dev", "d-e-v").unwrap();
        assert!(a > b);
    }

    #[test]
    fn picker_arrows_move_cursor_and_fuzzy_search() {
        struct L(Doc);
        impl Content for L {
            fn len(&self) -> usize {
                self.0.len()
            }
            fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
                (from..to).map(|i| Line::from(self.0.line_text(i))).collect()
            }
            fn doc(&self) -> Option<&Doc> {
                Some(&self.0)
            }
            fn picker(&self) -> bool {
                true
            }
        }
        let lines = (0..10).map(|i| Line::from(format!("item {i}"))).chain([Line::from("git status")]);
        let mut p = Pager {
            content: L(Doc::from_lines(lines.collect())),
            opts: Options {
                path: None,
                title: String::new(),
                line: None,
                col: 1,
                editor: None,
                watch: false,
                status: String::new(),
            },
            top: 0,
            width: 40,
            hscroll: 0,
            cursor: Some(0),
            height: 4,
            mode: Mode::Normal,
            query: String::new(),
            last: None,
            watch: None,
            clear: false,
            fuzzy_started: false,
        };
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        p.key(key(KeyCode::Down), None).unwrap();
        assert_eq!(p.cursor, Some(1));
        p.key(key(KeyCode::Char('о')), Some(Key::new(wezterm_input_types::PhysKeyCode::J, wezterm_input_types::Modifiers::NONE))).unwrap(); // i18n-allow-cyrillic
        assert_eq!(p.cursor, Some(2));
        p.key(key(KeyCode::End), None).unwrap();
        assert_eq!((p.cursor, p.top), (Some(10), 7));
        p.key(key(KeyCode::Home), None).unwrap();
        assert_eq!((p.cursor, p.top), (Some(0), 0));
        p.key(key(KeyCode::PageDown), None).unwrap();
        assert_eq!(p.cursor, Some(3));
        p.key(key(KeyCode::Up), None).unwrap();
        assert_eq!(p.cursor, Some(2));
        for c in "/gst".chars() {
            p.key(key(KeyCode::Char(c)), None).unwrap();
        }
        p.key(key(KeyCode::Enter), None).unwrap();
        assert_eq!(p.cursor, Some(10));
        assert!(matches!(p.key(key(KeyCode::Esc), None).unwrap(), Action::Quit));
    }

    #[test]
    fn control_request_opens_in_place_or_switches() {
        use crate::control::{Invalid, Request, Sub};
        struct L(Doc, Option<PathBuf>, bool);
        impl Content for L {
            fn len(&self) -> usize {
                self.0.len()
            }
            fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
                (from..to).map(|i| Line::from(self.0.line_text(i))).collect()
            }
            fn doc(&self) -> Option<&Doc> {
                Some(&self.0)
            }
            fn open_path(&mut self, path: &Path, line: usize) -> Option<io::Result<Nav>> {
                if !self.2 {
                    return None;
                }
                self.1 = Some(path.to_path_buf());
                Some(Ok(Nav::Open { path: path.to_path_buf(), title: "N".into(), line }))
            }
        }
        let lines = (0..50).map(|i| Line::from(format!("line {i}")));
        let mut p = Pager {
            content: L(Doc::from_lines(lines.collect()), None, true),
            opts: Options { path: None, title: "T".into(), line: None, col: 1, editor: None, watch: false, status: String::new() },
            top: 0,
            width: 40,
            hscroll: 0,
            cursor: None,
            height: 10,
            mode: Mode::Normal,
            query: String::new(),
            last: None,
            watch: None,
            clear: false,
            fuzzy_started: false,
        };
        let path = PathBuf::from("/x/b.rs");
        let req = |sub, line| Request { sub, path: path.clone(), line, col: Some(3) };
        assert!(p.control(Ok(req(Sub::View, Some(20))), Sub::View).is_none());
        assert_eq!(p.content.1.as_deref(), Some(path.as_path()));
        assert_eq!((p.cursor, p.opts.title.as_str(), p.opts.path.as_deref()), (Some(19), "N", Some(path.as_path())));
        assert_eq!(p.control(Ok(req(Sub::Md, Some(2))), Sub::View), Some(req(Sub::Md, Some(2))));
        assert_eq!(p.cursor, Some(19));
        assert!(p.control(Err(Invalid::Json), Sub::View).is_none());
        assert_eq!(p.opts.status, "ignored an invalid open request");
        // a content that cannot open in place restarts the same subcommand
        p.content.2 = false;
        assert_eq!(p.control(Ok(req(Sub::View, None)), Sub::View), Some(req(Sub::View, None)));
    }

    /// A request arriving while the user types waits (text intact, file kept, status names it)
    /// and is applied after Esc.
    #[test]
    fn control_request_waits_for_the_input_editor() {
        use crate::control::{Control, ControlFile, Sub};
        struct L(Doc, Option<PathBuf>);
        impl Content for L {
            fn len(&self) -> usize {
                self.0.len()
            }
            fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
                (from..to).map(|i| Line::from(self.0.line_text(i))).collect()
            }
            fn doc(&self) -> Option<&Doc> {
                Some(&self.0)
            }
            fn open_path(&mut self, path: &Path, line: usize) -> Option<io::Result<Nav>> {
                self.1 = Some(path.to_path_buf());
                Some(Ok(Nav::Open { path: path.to_path_buf(), title: "N".into(), line }))
            }
        }
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("b.rs");
        std::fs::write(&target, "1\n2\n3\n").unwrap();
        std::fs::create_dir_all(d.path().join("viewer")).unwrap();
        let file = d.path().join("viewer").join("0123abcd.json");
        let ctl = Control { file: ControlFile::new(d.path(), "0123abcd").unwrap(), sub: Sub::View };
        let lines = (0..10).map(|i| Line::from(format!("line {i}")));
        let mut p = Pager {
            content: L(Doc::from_lines(lines.collect()), None),
            opts: Options { path: None, title: "T".into(), line: None, col: 1, editor: None, watch: false, status: String::new() },
            top: 0,
            width: 40,
            hscroll: 0,
            cursor: None,
            height: 5,
            mode: Mode::Input { prompt: "comment".into(), edit: LineEdit::multiline("draft") },
            query: String::new(),
            last: None,
            watch: None,
            clear: false,
            fuzzy_started: false,
        };
        let mut held = None;
        assert!(p.tick_control(&ctl, &mut held).is_none());
        let body = serde_json::json!({ "subcommand": "view", "path": target, "line": 2 });
        std::fs::write(&file, body.to_string()).unwrap();
        assert!(p.tick_control(&ctl, &mut held).is_none());
        assert!(p.tick_control(&ctl, &mut held).is_none());
        assert!(file.exists());
        assert!(p.content.1.is_none());
        assert_eq!(p.opts.status, "file waiting: b.rs");
        match &p.mode {
            Mode::Input { edit, .. } => assert_eq!(edit.text(), "draft"),
            _ => panic!("the input editor must stay open"),
        }
        p.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), None).unwrap();
        assert!(matches!(p.mode, Mode::Normal));
        assert!(p.tick_control(&ctl, &mut held).is_none());
        assert_eq!(p.content.1.as_deref(), Some(target.as_path()));
        assert_eq!(p.cursor, Some(1));
        assert!(!file.exists());
    }

    #[test]
    fn search_and_goto_inputs_edit_through_line_edit() {
        struct L(Doc);
        impl Content for L {
            fn len(&self) -> usize {
                self.0.len()
            }
            fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
                (from..to).map(|i| Line::from(self.0.line_text(i))).collect()
            }
            fn doc(&self) -> Option<&Doc> {
                Some(&self.0)
            }
        }
        let lines = ["alpha", "beta", "кот", "gamma", "delta", "omega"].map(Line::from); // i18n-allow-cyrillic
        let mut p = Pager {
            content: L(Doc::from_lines(lines.to_vec())),
            opts: Options { path: None, title: String::new(), line: None, col: 1, editor: None, watch: false, status: String::new() },
            top: 0,
            width: 40,
            hscroll: 0,
            cursor: None,
            height: 3,
            mode: Mode::Normal,
            query: String::new(),
            last: None,
            watch: None,
            clear: false,
            fuzzy_started: false,
        };
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        // "/кxт", caret back over "т", Backspace removes "x", "о" typed in the middle // i18n-allow-cyrillic
        for c in "/кxт".chars() { // i18n-allow-cyrillic
            p.key(key(KeyCode::Char(c)), None).unwrap();
        }
        p.key(key(KeyCode::Left), None).unwrap();
        p.key(key(KeyCode::Backspace), None).unwrap();
        p.key(key(KeyCode::Char('о')), None).unwrap(); // i18n-allow-cyrillic
        assert!(matches!(&p.mode, Mode::Search(e) if e.text() == "кот" && e.caret() == 2)); // i18n-allow-cyrillic
        assert_eq!(p.input_caret(40).map(|v| v.1), Some(2));
        p.key(key(KeyCode::Enter), None).unwrap();
        assert!(matches!(p.mode, Mode::Normal));
        assert_eq!(p.query, "кот"); // i18n-allow-cyrillic
        assert_eq!(p.last.map(|m| m.line), Some(2));
        // ":45", Home, Delete removes "4": goes to line 5
        for c in ":45".chars() {
            p.key(key(KeyCode::Char(c)), None).unwrap();
        }
        p.key(key(KeyCode::Home), None).unwrap();
        p.key(key(KeyCode::Delete), None).unwrap();
        p.key(key(KeyCode::Enter), None).unwrap();
        assert_eq!(p.cursor, Some(4));
        // Esc in an input closes it without quitting
        p.key(key(KeyCode::Char('/')), None).unwrap();
        assert!(matches!(p.key(key(KeyCode::Esc), None).unwrap(), Action::None));
        assert!(matches!(p.mode, Mode::Normal));
    }

    #[test]
    fn picker_cursor_row_is_highlighted() {
        struct L(Doc);
        impl Content for L {
            fn len(&self) -> usize {
                self.0.len()
            }
            fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
                (from..to).map(|i| Line::from(self.0.line_text(i))).collect()
            }
            fn doc(&self) -> Option<&Doc> {
                Some(&self.0)
            }
            fn picker(&self) -> bool {
                true
            }
        }
        let mut p = Pager {
            content: L(Doc::from_lines(vec![Line::from("Diff"), Line::from("Pull")])),
            opts: Options { path: None, title: "T".into(), line: None, col: 1, editor: None, watch: false, status: String::new() },
            top: 0,
            width: 0,
            hscroll: 0,
            cursor: Some(1),
            height: 4,
            mode: Mode::Normal,
            query: String::new(),
            last: None,
            watch: None,
            clear: false,
            fuzzy_started: false,
        };
        let mut t = Terminal::new(ratatui::backend::TestBackend::new(20, 5)).unwrap();
        t.draw(|f| p.draw(f)).unwrap();
        let buf = t.backend().buffer();
        assert_eq!(buf[(2, 1)].bg, PICK_BG, "{:?}", buf[(2, 1)]);
        assert_eq!(buf[(2, 1)].fg, PICK_FG);
        assert!(buf[(2, 1)].modifier.contains(Modifier::BOLD));
        assert_ne!(buf[(2, 0)].bg, PICK_BG);
    }

    struct PickL(Doc);
    impl Content for PickL {
        fn len(&self) -> usize {
            self.0.len()
        }
        fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
            (from..to).map(|i| Line::from(self.0.line_text(i))).collect()
        }
        fn doc(&self) -> Option<&Doc> {
            Some(&self.0)
        }
        fn picker(&self) -> bool {
            true
        }
    }

    fn picker_pager(n: usize, height: usize) -> Pager<PickL> {
        Pager {
            content: PickL(Doc::from_lines((1..=n).map(|i| Line::from(format!("item {i}"))).collect())),
            opts: Options { path: None, title: "T".into(), line: None, col: 1, editor: None, watch: false, status: String::new() },
            top: 0,
            width: 20,
            hscroll: 0,
            cursor: Some(0),
            height,
            mode: Mode::Normal,
            query: String::new(),
            last: None,
            watch: None,
            clear: false,
            fuzzy_started: false,
        }
    }

    /// Events that arrive in `saysdf pick` when the sidebar opens (from the SAYSDF_LOG
    /// log): Resize on split and width fit, mouse motion, focus. They do not move the selection.
    #[test]
    fn picker_startup_events_keep_selection() {
        use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
        let mut p = picker_pager(10, 12);
        let moved = Event::Mouse(MouseEvent { kind: MouseEventKind::Moved, column: 39, row: 8, modifiers: KeyModifiers::NONE });
        for ev in [Event::Resize(40, 13), Event::Resize(40, 12), moved, Event::FocusGained, Event::Resize(20, 12), Event::Resize(60, 12), Event::FocusLost] {
            assert!(matches!(p.event(ev, None).unwrap(), Action::None));
        }
        assert_eq!((p.cursor, p.top, p.hscroll), (Some(0), 0, 0));
        let down = Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        p.event(down.clone(), None).unwrap();
        p.event(down, None).unwrap();
        assert_eq!(p.cursor, Some(2));
        // height shrinks: the selected row stays visible
        p.select(9);
        p.event(Event::Resize(20, 4), None).unwrap();
        assert!(p.top <= 9 && 9 < p.top + p.height, "top={} height={}", p.top, p.height);
        // the wheel in picker mode moves the selection, not just the window
        let wheel = Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: 1, row: 1, modifiers: KeyModifiers::NONE });
        p.event(wheel, None).unwrap();
        assert_eq!(p.cursor, Some(6));
        assert!(p.top <= 6 && 6 < p.top + p.height);
    }

    /// Real bytes to the terminal: with NO_COLOR crossterm writes `\e[;m` instead of a color (reset of all
    /// attributes), so the picker row and status lost color, bold and reverse. After
    /// [`init_colors`] the output has the background and bold of the selected row.
    #[test]
    fn picker_row_bytes_keep_style_with_no_color() {
        use ratatui::crossterm::style::Colored;
        fn render(p: &mut Pager<PickL>) -> String {
            let mut buf = Vec::new();
            {
                let mut t = Terminal::with_options(
                    CrosstermBackend::new(&mut buf),
                    ratatui::TerminalOptions { viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 20, 4)) },
                )
                .unwrap();
                t.draw(|f| p.draw(f)).unwrap();
            }
            String::from_utf8_lossy(&buf).into_owned()
        }
        let mut p = picker_pager(3, 3);
        Colored::set_ansi_color_disabled(true); // as with NO_COLOR=1
        let bad = render(&mut p);
        assert!(bad.contains("\x1b[;m"), "{bad:?}");
        init_colors();
        let good = render(&mut p);
        assert!(!good.contains("\x1b[;m"), "{good:?}");
        assert!(good.contains("\x1b[1m"), "{good:?}");
        assert!(good.contains("38;2;255;255;255;48;2;31;95;208mitem 1"),"no picker row background: {good:?}");
        assert!(good.contains("\x1b[7m"), "no status reverse: {good:?}");
    }

    use super::*;

    fn text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Fake content for the B-4 hooks: records what reaches it.
    #[derive(Default)]
    struct Hooks {
        lines: Vec<String>,
        capture: bool,
        keys: Vec<KeyCode>,
        extra: Option<String>,
        title: Option<String>,
        mouse: Vec<(usize, usize, MousePhase)>,
        ask: Option<(String, String)>,
        got: Vec<Option<String>>,
    }
    impl Content for Hooks {
        fn len(&self) -> usize {
            self.lines.len()
        }
        fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
            self.lines[from..to].iter().map(|l| Line::from(l.clone())).collect()
        }
        fn doc(&self) -> Option<&Doc> {
            None
        }
        fn captures_keys(&self) -> bool {
            self.capture
        }
        fn key(&mut self, k: &KeyEvent, _phys: Option<Key>, _pos: usize) -> io::Result<Nav> {
            self.keys.push(k.code);
            match k.code {
                KeyCode::Esc if self.capture => self.capture = false,
                KeyCode::Char(']') => self.capture = true,
                KeyCode::PageDown => return Ok(Nav::Pass),
                KeyCode::Char('i') => self.ask = Some(("insert".into(), "ab".into())),
                _ if self.capture => self.capture = false,
                _ => return Ok(Nav::Pass),
            }
            Ok(Nav::None)
        }
        fn status_extra(&self) -> Option<String> {
            self.extra.clone()
        }
        fn pane_title(&self) -> Option<String> {
            self.title.clone()
        }
        fn mouse(&mut self, line: usize, col: usize, phase: MousePhase) -> io::Result<Nav> {
            self.mouse.push((line, col, phase));
            Ok(Nav::None)
        }
        fn take_input_request(&mut self) -> Option<(String, String)> {
            self.ask.take()
        }
        fn input(&mut self, text: Option<String>, _pos: usize) -> io::Result<Nav> {
            self.got.push(text);
            Ok(Nav::Status("saved".into()))
        }
    }

    fn hooks_pager(n: usize, height: usize) -> Pager<Hooks> {
        Pager {
            content: Hooks { lines: (1..=n).map(|i| format!("line {i}")).collect(), ..Default::default() },
            opts: Options { path: None, title: "T".into(), line: None, col: 1, editor: None, watch: false, status: String::new() },
            top: 0,
            width: 20,
            hscroll: 0,
            cursor: None,
            height,
            mode: Mode::Normal,
            query: String::new(),
            last: None,
            watch: None,
            clear: false,
            fuzzy_started: false,
        }
    }

    fn screen(p: &mut Pager<Hooks>, w: u16, h: u16) -> Vec<String> {
        let mut t = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        t.draw(|f| p.draw(f)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_owned()).collect::<String>()).collect()
    }

    #[test]
    fn capturing_content_sees_keys_before_pager() {
        let mut p = hooks_pager(50, 10);
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        // not capturing: `j` scrolls in the pager, the content never sees it
        p.key(key(KeyCode::Char('j')), None).unwrap();
        assert_eq!(p.top, 1);
        assert!(p.content.keys.is_empty());
        // `]` reaches the content (unused by the pager) and starts a prefix
        p.key(key(KeyCode::Char(']')), None).unwrap();
        assert!(p.content.capture);
        // the next `j` and `q` go to the content, not to scroll/quit
        p.key(key(KeyCode::Char('j')), None).unwrap();
        assert_eq!(p.top, 1);
        p.content.capture = true;
        assert!(matches!(p.key(key(KeyCode::Char('q')), None).unwrap(), Action::None));
        // Esc in capture mode goes to the content and does not quit
        p.content.capture = true;
        assert!(matches!(p.key(key(KeyCode::Esc), None).unwrap(), Action::None));
        assert!(!p.content.capture);
        assert_eq!(p.content.keys, [KeyCode::Char(']'), KeyCode::Char('j'), KeyCode::Char('q'), KeyCode::Esc]);
        // Nav::Pass hands the key back to the pager
        p.content.capture = true;
        p.key(key(KeyCode::PageDown), None).unwrap();
        assert_eq!(p.top, 10);
        // Ctrl+C always quits
        assert!(matches!(p.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), None).unwrap(), Action::Quit));
        // outside capture Esc still quits
        p.content.capture = false;
        assert!(matches!(p.key(key(KeyCode::Esc), None).unwrap(), Action::Quit));
    }

    #[test]
    fn status_extra_is_shown_and_shortened() {
        let mut p = hooks_pager(3, 4);
        p.content.extra = Some("\u{270e} 3  v select".into());
        p.opts.status = "a long message for the status line".into();
        let wide = screen(&mut p, 80, 5);
        assert!(wide[4].contains("1/3  \u{270e} 3  v select  | a long"), "{:?}", wide[4]);
        // a narrow pane: two rows (the text area shrinks by one), the second cut with an ellipsis
        let narrow = screen(&mut p, 16, 6);
        assert_eq!(narrow[4].trim_end(), " T  1/3  \u{270e} 3", "{narrow:?}");
        assert_eq!(narrow[5].trim_end(), "v select  | a l\u{2026}", "{narrow:?}");
        assert_eq!(narrow[0].trim_end(), "1 line 1");
        assert_eq!(shorten("abc", 3), "abc");
        assert_eq!(shorten("a\u{4E2D}bc", 3), "a\u{2026}");
    }

    #[test]
    fn hints_take_one_row_when_wide_two_when_narrow_and_are_shortened() {
        let mut p = hooks_pager(3, 4);
        p.content.title = Some("a.txt".into());
        p.content.extra = Some("Ctrl: R substitute \u{b7} D delete \u{b7} N insert".into());
        let wide = screen(&mut p, 80, 6);
        assert_eq!(p.height, 5);
        assert!(wide[5].starts_with(" 1/3  Ctrl: R substitute"), "{:?}", wide[5]);
        // narrow: two rows, the text area is one row shorter
        let mid = screen(&mut p, 30, 6);
        assert_eq!(p.height, 4);
        assert_eq!(mid[4].trim_end(), " 1/3  Ctrl: R substitute", "{mid:?}");
        assert_eq!(mid[5].trim_end(), "D delete \u{b7} N insert", "{mid:?}");
        // very narrow: still two rows, never more; the second is cut with an ellipsis
        let tiny = screen(&mut p, 12, 6);
        assert_eq!(p.height, 4);
        assert!(tiny[5].trim_end().ends_with('\u{2026}'), "{tiny:?}");
    }

    #[test]
    fn status_line_has_no_path_when_the_content_has_a_pane_title() {
        let mut p = hooks_pager(3, 4);
        p.opts.title = "/home/me/secret/dir/a.txt".into();
        p.content.title = Some("a.txt".into());
        let s = screen(&mut p, 80, 5);
        assert!(!s[4].contains("secret"), "{:?}", s[4]);
        assert_eq!(s[4].trim_end(), " 1/3");
    }

    #[test]
    fn pane_title_is_pushed_sanitized_and_popped() {
        let mut out = Vec::new();
        let g = TitleGuard::set(Some("a\u{202e}t\u{1b}]52;c;x\u{7}.txt"), &mut out);
        assert!(g.0);
        std::mem::forget(g); // Drop writes to the real stdout
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s, "\x1b[22;0t\x1b]2;a t ]52;c;x .txt\x1b\\");
        // the only ESC/BEL bytes are the sequence's own
        assert_eq!(s.matches('\x1b').count(), 3);
        assert!(!s.contains('\u{7}') && !s.contains('\u{202e}'));
        assert_eq!(TITLE_POP, "\x1b[23;0t");
        let mut none = Vec::new();
        assert!(!TitleGuard::set(None, &mut none).0 && none.is_empty());
    }

    #[test]
    fn split_rows_cuts_at_a_space() {
        assert_eq!(split_rows("abc def", 20), ("abc def".into(), None));
        // never between a key and its label: the break is at the separator
        assert_eq!(
            split_rows(" 1/3  Ctrl: R substitute \u{b7} D delete", 28),
            (" 1/3  Ctrl: R substitute".into(), Some("D delete".into()))
        );
        assert_eq!(
            split_rows("Ctrl: R substitute \u{b7} D delete \u{b7} N insert", 22),
            ("Ctrl: R substitute".into(), Some("D delete \u{b7} N insert".into()))
        );
        // no separator fits: one row, cut with an ellipsis
        assert_eq!(split_rows("abcdefghij", 4), ("abc\u{2026}".into(), None));
    }

    #[test]
    fn mouse_is_content_relative_under_scroll() {
        use ratatui::crossterm::event::MouseEvent;
        let mut p = hooks_pager(200, 10);
        p.top = 40;
        p.hscroll = 8;
        assert_eq!(p.gutter(), 4);
        let ev = |kind, column, row| Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE });
        p.event(ev(MouseEventKind::Down(MouseButton::Left), 4 + 3, 2), None).unwrap();
        p.event(ev(MouseEventKind::Drag(MouseButton::Left), 1, 5), None).unwrap(); // over the gutter
        p.event(ev(MouseEventKind::Drag(MouseButton::Left), 10, 30), None).unwrap(); // below the text area
        p.event(ev(MouseEventKind::Up(MouseButton::Left), 4, 9), None).unwrap();
        // a press on the status line and other buttons do not reach the content
        p.event(ev(MouseEventKind::Down(MouseButton::Left), 5, 10), None).unwrap();
        p.event(ev(MouseEventKind::Down(MouseButton::Right), 5, 1), None).unwrap();
        assert_eq!(
            p.content.mouse,
            [(42, 11, MousePhase::Press), (45, 8, MousePhase::Drag), (49, 14, MousePhase::Drag), (49, 8, MousePhase::Release)]
        );
        // the last line clamps to the content
        p.top = 195;
        p.event(ev(MouseEventKind::Drag(MouseButton::Left), 4, 9), None).unwrap();
        assert_eq!(p.content.mouse.last(), Some(&(199, 8, MousePhase::Drag)));
    }

    #[test]
    fn input_editor_grows_caps_and_returns_text() {
        let mut p = hooks_pager(50, 10);
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        p.key(key(KeyCode::Char('i')), None).unwrap();
        assert!(matches!(&p.mode, Mode::Input { prompt, edit } if prompt == "insert" && edit.text() == "ab"));
        let s = screen(&mut p, 30, 12);
        assert_eq!(p.height, 10);
        assert_eq!(s[10].trim_end(), "> ab");
        assert!(s[11].starts_with(" insert  Enter save"), "{:?}", s[11]);
        // Ctrl+J adds rows; the editor grows up to a third of the pane, then scrolls
        let ctrl_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        for c in ['c', 'd', 'e', 'f', 'g', 'h'] {
            p.key(ctrl_j, None).unwrap();
            p.key(key(KeyCode::Char(c)), None).unwrap();
        }
        let s = screen(&mut p, 30, 12);
        assert_eq!(p.height, 7, "editor height capped at 12 / 3 = 4 rows");
        assert_eq!(s[7..11].iter().map(|r| r.trim_end()).collect::<Vec<_>>(), ["> e", "  f", "  g", "  h"]);
        // keys go to the editor, not to the pager or the content
        p.key(key(KeyCode::Char('q')), None).unwrap();
        assert_eq!(p.top, 0);
        p.key(key(KeyCode::Enter), None).unwrap();
        assert!(matches!(p.mode, Mode::Normal));
        assert_eq!(p.content.got, [Some("ab\nc\nd\ne\nf\ng\nhq".to_owned())]);
        assert_eq!(p.opts.status, "saved");
        // Esc cancels: the content gets None, the pager does not quit
        p.key(key(KeyCode::Char('i')), None).unwrap();
        assert!(matches!(p.key(key(KeyCode::Esc), None).unwrap(), Action::None));
        assert_eq!(p.content.got.last(), Some(&None));
        let s = screen(&mut p, 30, 12);
        assert_eq!(p.height, 11);
        assert!(s[11].starts_with(" T  1/50"), "{:?}", s[11]);
    }

    #[test]
    fn slice_cuts_wide_char_with_space() {
        let l = Line::from("a中b");
        assert_eq!(text(&slice_cells(&l, 0, 4)), "a中b");
        assert_eq!(text(&slice_cells(&l, 2, 2)), " b");
        assert_eq!(text(&slice_cells(&l, 0, 2)), "a ");
    }

    #[test]
    fn ranges_split_spans() {
        let l = Line::from(vec![Span::raw("fn ma"), Span::raw("in()")]);
        let r = apply_ranges(l, &[(3, 7)], Style::new().add_modifier(Modifier::REVERSED));
        assert_eq!(text(&r), "fn main()");
        let hit: String = r
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(hit, "main");
    }

    /// `h`/`l`/arrows shift the view only when the content allows it (off while wrapping).
    #[test]
    fn horizontal_shift_follows_content_permission() {
        struct H(Doc, bool);
        impl Content for H {
            fn len(&self) -> usize {
                self.0.len()
            }
            fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
                (from..to).map(|i| Line::from(self.0.line_text(i))).collect()
            }
            fn doc(&self) -> Option<&Doc> {
                Some(&self.0)
            }
            fn hscroll_enabled(&self) -> bool {
                self.1
            }
        }
        for enabled in [true, false] {
            let mut p = Pager {
                content: H(Doc::from_lines(vec![Line::from("a long line")]), enabled),
                opts: Options { path: None, title: "T".into(), line: None, col: 1, editor: None, watch: false, status: String::new() },
                top: 0,
                width: 20,
                hscroll: 0,
                cursor: None,
                height: 4,
                mode: Mode::Normal,
                query: String::new(),
                last: None,
                watch: None,
                clear: false,
                fuzzy_started: false,
            };
            for code in [KeyCode::Char('l'), KeyCode::Right, KeyCode::Char('l')] {
                p.key(KeyEvent::new(code, KeyModifiers::NONE), None).unwrap();
            }
            assert_eq!(p.hscroll, if enabled { 12 } else { 0 });
            p.key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE), None).unwrap();
            assert_eq!(p.hscroll, if enabled { 8 } else { 0 });
        }
    }
}
