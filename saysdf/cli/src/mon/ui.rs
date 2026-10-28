//! TUI `saysdf mon`: braille graphs, process table, context menu (design D2, D3).
use super::actions;
use super::metrics::{self, braille_graph, fmt_bytes, fmt_pct, Ring, Sample, Sampler};
use super::procs::{self, Action, Me, ProcRow, SortKey};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::crossterm::terminal::enable_raw_mode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::{Frame, Terminal};
use saysdf_keys::Key;
use std::collections::HashSet;
use std::io::{self, Write};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-mon");

const HIST: usize = 300;

struct History {
    cores: Vec<Ring>,
    cpu: Ring,
    mem: Ring,
    swap: Ring,
    rx: Ring,
    tx: Ring,
    disk: Ring,
    gpu: Ring,
}

impl History {
    fn new() -> Self {
        let r = || Ring::new(HIST);
        Self { cores: Vec::new(), cpu: r(), mem: r(), swap: r(), rx: r(), tx: r(), disk: r(), gpu: r() }
    }
    fn push(&mut self, s: &Sample) {
        if self.cores.len() != s.cores.len() {
            self.cores = (0..s.cores.len()).map(|_| Ring::new(HIST)).collect();
        }
        for (r, v) in self.cores.iter_mut().zip(&s.cores) {
            r.push(f64::from(*v));
        }
        self.cpu.push(f64::from(s.cpu));
        let pct = |u: u64, t: u64| if t == 0 { 0.0 } else { u as f64 * 100.0 / t as f64 };
        self.mem.push(pct(s.mem_used, s.mem_total));
        self.swap.push(pct(s.swap_used, s.swap_total));
        self.rx.push(s.net_rx as f64);
        self.tx.push(s.net_tx as f64);
        self.disk.push((s.disk_read + s.disk_write) as f64);
        if let Some(g) = s.gpu {
            self.gpu.push(f64::from(g));
        }
    }
}

enum Mode {
    Normal,
    Search,
    Menu { sel: usize },
    Confirm { action: Action, row: ProcRow },
}

struct App {
    sampler: Sampler,
    psys: System,
    hist: History,
    last: Sample,
    rows: Vec<ProcRow>,
    view: Vec<(usize, usize)>,
    sel: usize,
    top: usize,
    sort: SortKey,
    query: String,
    tree: bool,
    focus: Option<u32>,
    highlight: Option<u32>,
    hl_set: HashSet<u32>,
    paused: HashSet<u32>,
    me: Me,
    mode: Mode,
    msg: String,
    table_area: Rect,
    menu_area: Rect,
}

impl App {
    fn new(highlight: Option<u32>) -> Self {
        let sampler = Sampler::new();
        let mut a = App {
            sampler,
            psys: System::new(),
            hist: History::new(),
            last: Sample::default(),
            rows: Vec::new(),
            view: Vec::new(),
            sel: 0,
            top: 0,
            sort: SortKey::Cpu,
            query: String::new(),
            tree: false,
            focus: None,
            highlight,
            hl_set: HashSet::new(),
            paused: HashSet::new(),
            me: Me::default(),
            mode: Mode::Normal,
            msg: String::new(),
            table_area: Rect::default(),
            menu_area: Rect::default(),
        };
        a.tick();
        a
    }

    fn tick(&mut self) {
        self.last = self.sampler.sample();
        self.hist.push(&self.last);
        procs::refresh(&mut self.psys);
        let sel_pid = self.selected().map(|r| r.pid);
        self.rows = procs::collect(&self.psys);
        let me = std::process::id();
        let mut protected = procs::ancestors(&self.rows, me);
        protected.insert(me);
        if let Some(h) = self.highlight {
            protected.insert(h);
        }
        let user = self.rows.iter().find(|r| r.pid == me).and_then(|r| r.user.clone());
        self.me = Me { user, protected };
        self.hl_set = self.highlight.map(|h| procs::descendants(&self.rows, h)).unwrap_or_default();
        self.rebuild(sel_pid);
    }

    fn rebuild(&mut self, keep: Option<u32>) {
        procs::sort_rows(&mut self.rows, self.sort);
        let focus = self.focus.map(|f| procs::descendants(&self.rows, f));
        let keep_row = |r: &ProcRow| procs::matches(r, &self.query) && focus.as_ref().is_none_or(|f| f.contains(&r.pid));
        self.view = if self.tree {
            procs::build_tree(&self.rows).into_iter().filter(|&(i, _)| keep_row(&self.rows[i])).collect()
        } else {
            (0..self.rows.len()).filter(|&i| keep_row(&self.rows[i])).map(|i| (i, 0)).collect()
        };
        if let Some(pid) = keep {
            if let Some(p) = self.view.iter().position(|&(i, _)| self.rows[i].pid == pid) {
                self.sel = p;
            }
        }
        self.sel = self.sel.min(self.view.len().saturating_sub(1));
    }

    fn selected(&self) -> Option<&ProcRow> {
        self.view.get(self.sel).map(|&(i, _)| &self.rows[i])
    }

    /// PID + start_time: it is the same process that was shown (protects against PID reuse).
    fn still_same(&mut self, row: &ProcRow) -> bool {
        let p = Pid::from_u32(row.pid);
        self.psys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), true, ProcessRefreshKind::nothing());
        self.psys.process(p).is_some_and(|x| x.start_time() == row.start)
    }

    fn run_action(&mut self, action: Action, row: ProcRow) {
        match action {
            Action::CopyCmd => {
                let text = if row.cmd.is_empty() { row.exe.as_ref().map(|e| e.display().to_string()).unwrap_or_default() } else { row.cmd.clone() };
                let mut out = io::stdout();
                let _ = out.write_all(actions::osc52(&text).as_bytes());
                let _ = out.flush();
                self.msg = I18N.n(
                    "copied: {n} character",
                    "copied: {n} characters",
                    text.chars().count() as u64,
                    &[],
                );
            }
            Action::OpenDir => {
                self.msg = match &row.exe {
                    Some(e) => match actions::open_dir(e) {
                        Ok(()) => I18N.f("opened: {path}", &[("path", &e.display())]),
                        Err(err) => err,
                    },
                    None => I18N.t("exe path unavailable (permission denied)").into_owned(),
                }
            }
            Action::Tree => {
                self.focus = Some(row.pid);
                self.tree = true;
                self.rebuild(Some(row.pid));
                self.msg = I18N.f("descendants of PID {pid} (Esc for all processes)", &[("pid", &row.pid)]);
            }
            _ => {
                if !self.still_same(&row) {
                    self.msg = I18N.f("PID {pid} is now a different process or has exited", &[("pid", &row.pid)]);
                    return;
                }
                self.msg = match actions::apply(row.pid, action) {
                    Ok(m) => {
                        match action {
                            Action::Suspend => {
                                self.paused.insert(row.pid);
                            }
                            Action::Resume => {
                                self.paused.remove(&row.pid);
                            }
                            _ => {}
                        }
                        format!("{} (PID {}): {m}", row.name, row.pid)
                    }
                    Err(e) => format!("{} (PID {}): {e}", row.name, row.pid),
                };
            }
        }
    }

    fn choose(&mut self, action: Action) {
        let Some(row) = self.selected().cloned() else {
            self.mode = Mode::Normal;
            return;
        };
        if procs::needs_confirm(&row, action, &self.me) {
            self.mode = Mode::Confirm { action, row };
        } else {
            self.mode = Mode::Normal;
            self.run_action(action, row);
        }
    }

    fn move_sel(&mut self, d: isize) {
        let n = self.view.len();
        if n == 0 {
            return;
        }
        self.sel = (self.sel as isize + d).clamp(0, n as isize - 1) as usize;
    }

    /// true means quit.
    fn key(&mut self, k: KeyEvent, phys: Option<Key>) -> io::Result<bool> {
        let code = k.code;
        let is = |spec| crate::keys::is(&k, phys, spec);
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Search => {
                match code {
                    KeyCode::Esc => {
                        self.query.clear();
                    }
                    KeyCode::Enter => {}
                    KeyCode::Backspace => {
                        self.query.pop();
                        self.mode = Mode::Search;
                    }
                    KeyCode::Char(c) => {
                        self.query.push(c);
                        self.mode = Mode::Search;
                    }
                    _ => self.mode = Mode::Search,
                }
                let keep = self.selected().map(|r| r.pid);
                self.rebuild(keep);
            }
            Mode::Menu { sel } => match code {
                KeyCode::Esc => {}
                _ if is("q")? => {}
                KeyCode::Up => self.mode = Mode::Menu { sel: sel.saturating_sub(1) },
                KeyCode::Down => self.mode = Mode::Menu { sel: (sel + 1).min(Action::MENU.len() - 1) },
                KeyCode::Enter => self.choose(Action::MENU[sel]),
                KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                    let i = c as usize - '1' as usize;
                    if i < Action::MENU.len() {
                        self.choose(Action::MENU[i]);
                    }
                }
                _ => self.mode = Mode::Menu { sel },
            },
            Mode::Confirm { action, row } => {
                if crate::keys::confirmed(&k, phys) {
                    self.run_action(action, row);
                } else {
                    self.msg = I18N.t("cancelled").into_owned();
                }
            }
            Mode::Normal => match code {
                _ if is("q")? || is("Ctrl+c")? => return Ok(true),
                KeyCode::Esc => {
                    if self.focus.take().is_some() || !self.query.is_empty() {
                        self.query.clear();
                        let keep = self.selected().map(|r| r.pid);
                        self.rebuild(keep);
                    } else {
                        return Ok(true);
                    }
                }
                _ if code == KeyCode::Up || is("k")? => self.move_sel(-1),
                _ if code == KeyCode::Down || is("j")? => self.move_sel(1),
                KeyCode::PageUp => self.move_sel(-(self.table_area.height as isize - 2).max(1)),
                KeyCode::PageDown => self.move_sel((self.table_area.height as isize - 2).max(1)),
                KeyCode::Home => self.sel = 0,
                KeyCode::End => self.sel = self.view.len().saturating_sub(1),
                _ if is("/")? => self.mode = Mode::Search,
                _ if is("s")? => {
                    self.sort = self.sort.next();
                    let keep = self.selected().map(|r| r.pid);
                    self.rebuild(keep);
                }
                _ if is("t")? => {
                    self.tree = !self.tree;
                    let keep = self.selected().map(|r| r.pid);
                    self.rebuild(keep);
                }
                _ if self.selected().is_some()
                    && (matches!(code, KeyCode::Enter | KeyCode::Menu) || is("m")?) =>
                {
                    self.mode = Mode::Menu { sel: 0 };
                }
                _ => {}
            },
        }
        Ok(false)
    }

    fn mouse(&mut self, kind: MouseEventKind, x: u16, y: u16) {
        let inside = |a: Rect| x >= a.x && x < a.x + a.width && y >= a.y && y < a.y + a.height;
        match kind {
            MouseEventKind::Down(btn) => {
                if let Mode::Menu { .. } = self.mode {
                    let m = self.menu_area;
                    if inside(m) && y > m.y && ((y - m.y - 1) as usize) < Action::MENU.len() && btn == MouseButton::Left {
                        self.choose(Action::MENU[(y - m.y - 1) as usize]);
                    } else {
                        self.mode = Mode::Normal;
                    }
                    return;
                }
                let t = self.table_area;
                // row 0 is the border, 1 is the header
                if inside(t) && y >= t.y + 2 {
                    let i = self.top + (y - t.y - 2) as usize;
                    if i < self.view.len() {
                        self.sel = i;
                        if btn == MouseButton::Right {
                            self.mode = Mode::Menu { sel: 0 };
                        }
                    }
                }
            }
            MouseEventKind::ScrollDown => self.move_sel(3),
            MouseEventKind::ScrollUp => self.move_sel(-3),
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let graph_h = (area.height * 2 / 5).clamp(4, 16);
        let [top, table, status] =
            Layout::vertical([Constraint::Length(graph_h), Constraint::Min(3), Constraint::Length(1)]).areas(area);
        self.draw_graphs(f, top);
        self.draw_table(f, table);
        let line = match &self.mode {
            Mode::Search => I18N.f("/{query}▏  Enter — done, Esc — clear", &[("query", &self.query)]),
            Mode::Confirm { action, row } => procs::confirm_text(row, *action),
            _ if !self.msg.is_empty() => self.msg.clone(),
            _ => I18N.f(
                "{status} │ q quit  / search  s sort  t tree  m/right-click menu",
                &[("status", &metrics::status_line(&self.last))],
            ),
        };
        let style = if matches!(self.mode, Mode::Confirm { .. }) { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) };
        f.render_widget(Paragraph::new(Line::styled(line, style)), status);
        if let Mode::Menu { sel } = self.mode {
            self.draw_menu(f, table, sel);
        }
    }

    fn draw_graphs(&self, f: &mut Frame, area: Rect) {
        let [left, right] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
        // CPU per core: one row per core (height 1); if they do not fit, one overall graph.
        let s = &self.last;
        let block = Block::default().borders(Borders::ALL).title(format!(" CPU {} ", fmt_pct(s.cpu)));
        let inner = block.inner(left);
        f.render_widget(block, left);
        let mut lines = Vec::new();
        let label_w = 10usize;
        let gw = (inner.width as usize).saturating_sub(label_w);
        if (inner.height as usize) >= self.hist.cores.len() && !self.hist.cores.is_empty() {
            for (i, r) in self.hist.cores.iter().enumerate() {
                let g = braille_graph(&r.values(), 100.0, gw, 1);
                let v = s.cores.get(i).copied().unwrap_or(0.0);
                lines.push(Line::from(vec![
                    Span::raw(format!("{:<4}{:>5} ", format!("c{i}"), fmt_pct(v))),
                    Span::styled(g.into_iter().next().unwrap_or_default(), Style::default().fg(heat(v))),
                ]));
            }
        } else {
            for g in braille_graph(&self.hist.cpu.values(), 100.0, inner.width as usize, inner.height as usize) {
                lines.push(Line::styled(g, Style::default().fg(heat(s.cpu))));
            }
        }
        f.render_widget(Paragraph::new(lines), inner);

        // Right side: memory/swap, network, disk, GPU.
        let gpu_txt = match s.gpu {
            Some(g) => format!("GPU {}", fmt_pct(g)),
            None => I18N.t("GPU: unavailable").into_owned(),
        };
        let items: Vec<(String, &Ring, f64, Color)> = vec![
            (format!("RAM {} ({:.0}%)", metrics::fmt_mem(s.mem_used, s.mem_total), self.hist.mem.values().last().copied().unwrap_or(0.0)), &self.hist.mem, 100.0, Color::Green),
            (format!("Swap {}", metrics::fmt_mem(s.swap_used, s.swap_total)), &self.hist.swap, 100.0, Color::Magenta),
            (format!("Net ↓{}/s", fmt_bytes(s.net_rx)), &self.hist.rx, self.hist.rx.max().max(1.0), Color::Cyan),
            (format!("Net ↑{}/s", fmt_bytes(s.net_tx)), &self.hist.tx, self.hist.tx.max().max(1.0), Color::Blue),
            (format!("Disk {}/s", fmt_bytes(s.disk_read + s.disk_write)), &self.hist.disk, self.hist.disk.max().max(1.0), Color::Yellow),
            (gpu_txt, &self.hist.gpu, 100.0, Color::Red),
        ];
        let block = Block::default().borders(Borders::ALL).title(I18N.t(" Memory / network / disk / GPU "));
        let inner = block.inner(right);
        f.render_widget(block, right);
        let label_w = 24usize;
        let gw = (inner.width as usize).saturating_sub(label_w);
        let mut lines = Vec::new();
        for (label, ring, max, color) in items.into_iter().take(inner.height as usize) {
            let show_graph = !(s.gpu.is_none() && label.starts_with("GPU"));
            let g = if show_graph { braille_graph(&ring.values(), max, gw, 1).into_iter().next().unwrap_or_default() } else { String::new() };
            lines.push(Line::from(vec![
                Span::raw(format!("{:<w$}", truncate(&label, label_w - 1), w = label_w)),
                Span::styled(g, Style::default().fg(color)),
            ]));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_table(&mut self, f: &mut Frame, area: Rect) {
        self.table_area = area;
        let title = I18N.f(
            " Processes: {count} │ sort {sort} {tree}{query}{focus} ",
            &[
                ("count", &self.view.len()),
                ("sort", &self.sort.label()),
                ("tree", &if self.tree { I18N.t("│ tree ") } else { "".into() }),
                ("query", &if self.query.is_empty() { String::new() } else { format!("│ /{} ", self.query) }),
                (
                    "focus",
                    &self.focus.map(|p| I18N.f("│ descendants {pid} ", &[("pid", &p)])).unwrap_or_default(),
                ),
            ],
        );
        let block = Block::default().borders(Borders::ALL).title(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let h = (inner.height as usize).saturating_sub(1);
        if self.sel < self.top {
            self.top = self.sel;
        } else if h > 0 && self.sel >= self.top + h {
            self.top = self.sel + 1 - h;
        }
        let w = inner.width as usize;
        let mut lines = vec![Line::styled(
            format!("{:>7} {:>6} {:>7}  {}", "PID", "CPU", "MEM", I18N.t("NAME / COMMAND")),
            Style::default().add_modifier(Modifier::BOLD),
        )];
        for (vi, &(i, depth)) in self.view.iter().enumerate().skip(self.top).take(h) {
            let r = &self.rows[i];
            let foreign = r.user.is_none() || r.user != self.me.user;
            let mark = if self.paused.contains(&r.pid) { " [paused]" } else { "" };
            let indent = if self.tree { format!("{}{}", "  ".repeat(depth.min(20)), if depth > 0 { "└ " } else { "" }) } else { String::new() };
            let text = format!(
                "{:>7} {:>6} {:>7}  {indent}{}{mark}  {}",
                r.pid,
                format!("{:.1}", r.cpu),
                fmt_bytes(r.mem),
                r.name,
                r.cmd
            );
            let mut style = Style::default();
            if foreign {
                style = style.fg(Color::DarkGray);
            }
            if self.hl_set.contains(&r.pid) {
                style = style.fg(Color::Yellow).add_modifier(Modifier::BOLD);
            }
            if vi == self.sel {
                style = style.add_modifier(Modifier::REVERSED);
            }
            lines.push(Line::styled(truncate(&text, w), style));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_menu(&mut self, f: &mut Frame, table: Rect, sel: usize) {
        let w = 34u16.min(table.width);
        let h = (Action::MENU.len() as u16 + 2).min(table.height);
        let row_y = table.y + 2 + (self.sel.saturating_sub(self.top)) as u16;
        let y = row_y.min(table.y + table.height.saturating_sub(h));
        let x = table.x + table.width.saturating_sub(w) / 2;
        let area = Rect { x, y, width: w, height: h };
        self.menu_area = area;
        let name = self.selected().map(|r| format!(" {} ({}) ", r.name, r.pid)).unwrap_or_default();
        let lines: Vec<Line> = Action::MENU
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let st = if i == sel { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
                Line::styled(format!("{} {}", i + 1, a.label()), st)
            })
            .collect();
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(truncate(&name, w as usize - 2))), area);
    }
}

fn heat(v: f32) -> Color {
    if v >= 85.0 {
        Color::Red
    } else if v >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn truncate(s: &str, w: usize) -> String {
    s.chars().take(w).collect()
}

pub fn run(interval: Duration, highlight: Option<u32>) -> io::Result<()> {
    let mut app = App::new(highlight);
    enable_raw_mode()?;
    crate::pager::enter_screen()?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        crate::pager::restore_terminal();
        hook(info);
    }));
    let res = (|| -> io::Result<()> {
        // with NO_COLOR crossterm writes `\e[;m` and drops bold/reverse (see pager::init_colors)
        crate::pager::init_colors();
        let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        let mut next = Instant::now() + interval;
        loop {
            term.draw(|f| app.draw(f))?;
            let wait = next.saturating_duration_since(Instant::now());
            if event::poll(wait)? {
                match crate::keys::read()? {
                    (Event::Key(k), phys) if k.kind != KeyEventKind::Release => {
                        if app.key(k, phys)? {
                            return Ok(());
                        }
                    }
                    (Event::Mouse(m), _) => app.mouse(m.kind, m.column, m.row),
                    _ => {}
                }
            }
            if Instant::now() >= next {
                app.tick();
                next = Instant::now() + interval;
            }
        }
    })();
    crate::pager::restore_terminal();
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn render_small_does_not_panic() {
        let mut app = App::new(Some(std::process::id()));
        for (w, h) in [(20u16, 10u16), (120, 40), (5, 3)] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| app.draw(f)).unwrap();
            app.mode = Mode::Menu { sel: 0 };
            t.draw(|f| app.draw(f)).unwrap();
            app.mode = Mode::Normal;
        }
        assert!(!app.rows.is_empty());
        assert!(app.hl_set.contains(&std::process::id()));
    }

    fn screen_text(t: &Terminal<TestBackend>) -> String {
        t.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn screen_follows_language() {
        let mut app = App::new(None);
        let mut t = Terminal::new(TestBackend::new(140, 40)).unwrap();
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            t.draw(|f| app.draw(f)).unwrap();
            let s = screen_text(&t);
            assert!(s.contains(" Processes:") && s.contains("NAME / COMMAND") && s.contains("q quit"), "{s}");
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        t.draw(|f| app.draw(f)).unwrap();
        let s = screen_text(&t);
        assert!(s.contains(" Процессы:") && s.contains("ИМЯ / КОМАНДА") && s.contains("q выход"), "{s}"); // i18n-allow-cyrillic
    }
}
