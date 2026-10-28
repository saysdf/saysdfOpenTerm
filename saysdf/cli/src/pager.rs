//! Shared pager (design D2, D9): scrolling, go to line, search, status, mouse.
//! The terminal is raw + alt-screen; a guard (Drop) and a panic hook restore it.

use crate::editor;
use crate::watch::FileWatch;
use saysdf_keys::Key;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEventKind,
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
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;

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
    /// A key the pager does not use (`pos` is the current view line).
    fn key(&mut self, _k: &KeyEvent, _phys: Option<Key>, _pos: usize) -> io::Result<Nav> {
        Ok(Nav::None)
    }
    /// Output over the ratatui frame (OSC 1337 images): window `[top, top+height)`, text column `x`.
    fn overlay(&mut self, _out: &mut dyn io::Write, _top: usize, _height: usize, _x: usize, _hscroll: usize) {}
    /// Picker list (`saysdf pick`): arrows/PgUp/PgDn/Home/End move the cursor, not the scroll;
    /// `/` search is fuzzy ([`fuzzy_score`]) over the `doc()` lines.
    fn picker(&self) -> bool {
        false
    }
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
    Search(String),
    Goto(String),
}

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

/// Run the pager until exit by `q`/Esc.
pub fn run<C: Content>(content: C, opts: Options) -> io::Result<()> {
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
                Action::Quit => return Ok(()),
                Action::Editor => p.open_editor(&mut guard)?,
                Action::None => {}
            }
            if std::mem::take(&mut p.clear) {
                guard.term.clear()?;
            }
        }
        if p.watch.as_mut().is_some_and(|w| w.poll()) {
            p.reload();
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
        self.content.len().max(1).to_string().len() + 1
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

    fn key(&mut self, k: KeyEvent, phys: Option<Key>) -> io::Result<Action> {
        match &mut self.mode {
            Mode::Search(s) | Mode::Goto(s) => {
                match k.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        s.pop();
                    }
                    KeyCode::Char(c) => s.push(c),
                    KeyCode::Enter => {
                        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
                        match mode {
                            Mode::Search(q) if !q.is_empty() => {
                                self.query = q;
                                self.last = None;
                                self.fuzzy_started = false;
                                self.find(Direction::Forward);
                            }
                            Mode::Goto(n) => match n.trim().parse::<usize>() {
                                Ok(n) => self.goto(n.saturating_sub(1)),
                                Err(_) => self.opts.status = I18N.t("a line number is expected").into_owned(),
                            },
                            _ => {}
                        }
                    }
                    _ => {}
                }
                return Ok(Action::None);
            }
            Mode::Normal => {}
        }
        let is = |spec| crate::keys::is(&k, phys, spec);
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
            _ if k.code == KeyCode::Left || is("h")? => self.hscroll = self.hscroll.saturating_sub(4),
            _ if k.code == KeyCode::Right || is("l")? => self.hscroll += 4,
            _ if is("0")? => self.hscroll = 0,
            _ if is("/")? => self.mode = Mode::Search(String::new()),
            _ if is(":")? => self.mode = Mode::Goto(String::new()),
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
        {
            {
                let pos = self.cursor.unwrap_or(self.top);
                self.content.viewport(self.top, self.hscroll, self.height, self.width);
                match self.content.key(&k, phys, pos)? {
                    Nav::None => {}
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
        Ok(())
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
        let Some(doc) = self.content.doc() else {
            self.opts.status = I18N.t("search is unavailable for this view").into_owned();
            return;
        };
        let from = match (self.last, dir) {
            (Some(m), _) => Some((m.line, m.start)),
            (None, Direction::Forward) if self.top > 0 => Some((self.top - 1, usize::MAX)),
            (None, Direction::Forward) => None,
            (None, Direction::Backward) => Some((self.top, 0)),
        };
        match doc.search(&self.query, from, dir) {
            Some(m) => {
                let col = str_width(&doc.line_text(m.line)[..m.start]);
                self.last = Some(m);
                self.cursor = Some(m.line);
                if m.line < self.top || m.line >= self.top + self.height {
                    self.top = m.line.saturating_sub(self.height / 3).min(self.max_top());
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
        let line = self.cursor.unwrap_or(self.top) + 1;
        let col = self.opts.col.max(1);
        let res = guard.suspend(|| editor::run(&cmd, &path, line, col))?;
        if let Err(e) = res {
            self.opts.status = visible_line(&e, 4);
        }
        Ok(())
    }

    fn draw(&mut self, f: &mut ratatui::Frame) {
        let area = f.area();
        self.height = area.height.saturating_sub(1).max(1) as usize;
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
                if let Some(doc) = self.content.doc() {
                    let ranges = doc.matches_in_line(&self.query, i);
                    if !ranges.is_empty() {
                        line = apply_ranges(line, &ranges, match_style);
                    }
                }
            }
            let mut spans = vec![Span::styled(
                format!("{:>w$} ", i + 1, w = gutter - 1),
                Style::new().fg(Color::DarkGray),
            )];
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
        out.push(self.status_line(n));
        f.render_widget(Paragraph::new(out), area);
    }

    fn status_line(&self, n: usize) -> Line<'static> {
        let text = match &self.mode {
            Mode::Search(s) => format!("/{}", visible_line(s, 4)),
            Mode::Goto(s) => format!(":{}", visible_line(s, 4)),
            Mode::Normal => {
                let pos = self.cursor.unwrap_or(self.top) + 1;
                let mut t = format!(" {}  {}/{}", self.opts.title, pos.min(n.max(1)), n);
                if self.hscroll > 0 {
                    t.push_str(&format!("  >{}", self.hscroll));
                }
                if !self.opts.status.is_empty() {
                    t.push_str("  | ");
                    t.push_str(&self.opts.status);
                }
                t
            }
        };
        Line::from(Span::styled(text, Style::new().add_modifier(Modifier::REVERSED)))
    }
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
}
