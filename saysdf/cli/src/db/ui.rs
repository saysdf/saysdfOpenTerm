//! `saysdf db` UI on ratatui: connections -> schema tree | structure / data / query.
//! In dangerous mode the first line is a red banner outside any scrolling (see [`super::state`]).

use super::conns;
use super::state::{self, Effect, Focus, UiAction};
use super::worker::{self, Req, Resp, Worker};
use crate::form::{Field, Session};
use crate::pager::Guard;
use saysdf_keys::Key;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use saysdf_db::{
    Access, ConnectionConfig, Kind, ObjectKind, PageRequest, ResultSet, SortDir, TableDesc,
    TableInfo, TableRef, Value, WriteMode,
};
use saysdf_render::highlight::Highlighter;
use saysdf_render::sanitize::{char_width, str_width, visible_line};
use saysdf_secrets::{Domain, Secrets};
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-db");

const PAGE: u64 = 200;
const MAX_COL: usize = 40;

pub struct Options {
    pub main_path: Option<PathBuf>,
    pub saved_ids: Vec<String>,
    pub initial: Option<ConnectionConfig>,
    pub query: Option<String>,
    pub max_rows: usize,
    pub save_history: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Conns,
    Browser,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Structure,
    Data,
    Query,
}

enum Modal {
    Confirm {
        typed: String,
        db: String,
    },
    Password {
        value: String,
    },
    Filter {
        value: String,
    },
    Export {
        value: String,
        query: bool,
    },
    Overwrite {
        path: PathBuf,
        query: bool,
    },
    /// Dolt: pick a branch (`log: false`) or a commit from the history (`log: true`); an item is
    /// (ref, label), an empty ref is the connection's current branch.
    Refs {
        items: Vec<(String, String)>,
        sel: usize,
        log: bool,
    },
    Delete {
        idx: usize,
    },
}

enum Cmd {
    None,
    Quit,
    Form(Option<usize>),
}

/// Cell of width `w`: cut with an ellipsis and padded with spaces.
fn fit(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    let cut = str_width(s) > w;
    let limit = if cut { w.saturating_sub(1) } else { w };
    for c in s.chars() {
        let cw = char_width(c);
        if used + cw > limit {
            break;
        }
        out.push(c);
        used += cw;
    }
    if cut && w > 0 {
        out.push('…');
        used += 1;
    }
    out.extend(std::iter::repeat_n(' ', w.saturating_sub(used)));
    out
}

fn cell(v: &Value) -> String {
    visible_line(&v.to_string().replace(['\n', '\r'], " "), 4)
}

struct Grid {
    rs: ResultSet,
    row: usize,
    col: usize,
    top: usize,
    left: usize,
    widths: Vec<usize>,
}

impl Grid {
    fn new(rs: ResultSet) -> Self {
        let widths = (0..rs.columns.len())
            .map(|i| {
                let h = str_width(&visible_line(&rs.columns[i], 4)) + 2;
                let m = rs
                    .rows
                    .iter()
                    .map(|r| r.get(i).map(|v| str_width(&cell(v))).unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                h.max(m).clamp(3, MAX_COL)
            })
            .collect();
        Self {
            rs,
            row: 0,
            col: 0,
            top: 0,
            left: 0,
            widths,
        }
    }

    fn key(&mut self, code: KeyCode, h: usize) -> bool {
        let n = self.rs.rows.len();
        let nc = self.rs.columns.len();
        match code {
            KeyCode::Up => self.row = self.row.saturating_sub(1),
            KeyCode::Down => self.row = (self.row + 1).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.row = self.row.saturating_sub(h.max(1)),
            KeyCode::PageDown => self.row = (self.row + h.max(1)).min(n.saturating_sub(1)),
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = nc.saturating_sub(1),
            KeyCode::Left => self.col = self.col.saturating_sub(1),
            KeyCode::Right => self.col = (self.col + 1).min(nc.saturating_sub(1)),
            _ => return false,
        }
        true
    }

    fn lines(
        &mut self,
        width: usize,
        height: usize,
        sort: Option<&(String, SortDir)>,
    ) -> Vec<Line<'static>> {
        if self.rs.columns.is_empty() {
            let msg = match self.rs.affected {
                Some(a) => format!(" {}", I18N.f("rows affected: {n}", &[("n", &a)])),
                None => format!(" ({})", I18N.t("no columns")),
            };
            return vec![Line::raw(msg)];
        }
        // Horizontal scrolling by column: the selected column is always visible.
        if self.col < self.left {
            self.left = self.col;
        }
        loop {
            let mut w = 0;
            let mut last = self.left;
            for i in self.left..self.widths.len() {
                if w + self.widths[i] + 1 > width && i > self.left {
                    break;
                }
                w += self.widths[i] + 1;
                last = i;
            }
            if self.col <= last || self.left >= self.col {
                break;
            }
            self.left += 1;
        }
        let body_h = height.saturating_sub(1);
        if self.row < self.top {
            self.top = self.row;
        } else if body_h > 0 && self.row >= self.top + body_h {
            self.top = self.row + 1 - body_h;
        }
        let mut header = Vec::new();
        let mut w = 0;
        let visible: Vec<usize> = (self.left..self.widths.len())
            .take_while(|&i| {
                let ok = w == 0 || w + self.widths[i] < width;
                w += self.widths[i] + 1;
                ok
            })
            .collect();
        for &i in &visible {
            let name = visible_line(&self.rs.columns[i], 4);
            let mark = match sort {
                Some((c, SortDir::Asc)) if *c == self.rs.columns[i] => " ▲",
                Some((c, SortDir::Desc)) if *c == self.rs.columns[i] => " ▼",
                _ => "",
            };
            let mut st = Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            if i == self.col {
                st = st.fg(Color::Cyan);
            }
            header.push(Span::styled(
                fit(&format!("{name}{mark}"), self.widths[i]),
                st,
            ));
            header.push(Span::raw("│"));
        }
        let mut out = vec![Line::from(header)];
        for r in self.top..(self.top + body_h).min(self.rs.rows.len()) {
            let mut spans = Vec::new();
            for &i in &visible {
                let v = self.rs.rows[r].get(i).cloned().unwrap_or(Value::Null);
                let mut st = if v == Value::Null {
                    Style::new().add_modifier(Modifier::DIM)
                } else {
                    Style::new()
                };
                if r == self.row {
                    st = st.bg(Color::DarkGray);
                    if i == self.col {
                        st = st.add_modifier(Modifier::REVERSED);
                    }
                }
                spans.push(Span::styled(fit(&cell(&v), self.widths[i]), st));
                spans.push(Span::raw("│"));
            }
            out.push(Line::from(spans));
        }
        out
    }
}

enum Node {
    Schema {
        name: Option<String>,
        expanded: bool,
    },
    Table(TableInfo),
}

#[derive(Default)]
struct Editor {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

fn bidx(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
}

impl Editor {
    fn set(&mut self, text: &str) {
        self.lines = text.split('\n').map(str::to_string).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].chars().count();
    }
    fn text(&self) -> String {
        self.lines.join("\n")
    }
    fn key(&mut self, k: KeyEvent) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let len = |s: &String| s.chars().count();
        match k.code {
            KeyCode::Char(c)
                if !k
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let i = bidx(&self.lines[self.row], self.col);
                self.lines[self.row].insert(i, c);
                self.col += 1;
            }
            KeyCode::Enter => {
                let i = bidx(&self.lines[self.row], self.col);
                let rest = self.lines[self.row].split_off(i);
                self.row += 1;
                self.lines.insert(self.row, rest);
                self.col = 0;
            }
            KeyCode::Backspace => {
                if self.col > 0 {
                    let i = bidx(&self.lines[self.row], self.col - 1);
                    self.lines[self.row].remove(i);
                    self.col -= 1;
                } else if self.row > 0 {
                    let cur = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = len(&self.lines[self.row]);
                    self.lines[self.row].push_str(&cur);
                }
            }
            KeyCode::Delete => {
                if self.col < len(&self.lines[self.row]) {
                    let i = bidx(&self.lines[self.row], self.col);
                    self.lines[self.row].remove(i);
                } else if self.row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&next);
                }
            }
            KeyCode::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = len(&self.lines[self.row]);
                }
            }
            KeyCode::Right => {
                if self.col < len(&self.lines[self.row]) {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            KeyCode::Up if self.row > 0 => {
                self.row -= 1;
                self.col = self.col.min(len(&self.lines[self.row]));
            }
            KeyCode::Down if self.row + 1 < self.lines.len() => {
                self.row += 1;
                self.col = self.col.min(len(&self.lines[self.row]));
            }
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = len(&self.lines[self.row]),
            _ => return false,
        }
        true
    }
}

struct App {
    conns: Vec<ConnectionConfig>,
    saved: HashSet<String>,
    main_path: Option<PathBuf>,
    max_rows: usize,
    save_history: Option<PathBuf>,
    screen: Screen,
    sel: usize,
    cfg: Option<ConnectionConfig>,
    pending: Option<ConnectionConfig>,
    password: Option<String>,
    session_pw: HashMap<String, String>,
    worker: Option<Worker>,
    mode: WriteMode,
    focus: Focus,
    tab: Tab,
    tree: Vec<Node>,
    tree_sel: usize,
    tree_top: usize,
    desc: Option<(TableRef, TableDesc)>,
    struct_top: usize,
    page: Option<PageRequest>,
    data: Option<Grid>,
    editor: Editor,
    history: Vec<String>,
    hist_pos: Option<usize>,
    results: Option<Grid>,
    hl: Highlighter,
    modal: Option<Modal>,
    status: String,
    prev_esc: bool,
    button: Option<Rect>,
    banner: Option<Rect>,
    grid_h: usize,
    /// Dolt: branch/commit the data is viewed at (`AS OF`), read-only.
    as_of: Option<String>,
    /// Query whose result is shown (for exporting all rows).
    last_sql: Option<String>,
}

fn err(s: &str) -> String {
    visible_line(&s.replace(['\n', '\r'], " "), 4)
}

impl App {
    fn db_name(&self) -> String {
        self.cfg
            .as_ref()
            .map(|c| c.database_name())
            .unwrap_or_default()
    }

    fn connect(&mut self, cfg: ConnectionConfig) {
        state::apply(&mut self.mode, "", UiAction::SwitchConnection, "");
        let pw = self
            .session_pw
            .get(&cfg.id)
            .cloned()
            .or_else(|| worker::stored_password(&cfg));
        if cfg.kind.needs_password() && pw.is_none() {
            self.pending = Some(cfg);
            self.modal = Some(Modal::Password {
                value: String::new(),
            });
            return;
        }
        self.tree.clear();
        self.tree_sel = 0;
        self.desc = None;
        self.page = None;
        self.data = None;
        self.results = None;
        self.as_of = None;
        self.last_sql = None;
        self.start(cfg, pw, Access::ReadOnly);
    }

    fn start(&mut self, cfg: ConnectionConfig, pw: Option<String>, access: Access) {
        let write = !access.is_read_only();
        set_user_var("SAYSDF_DB_MODE", if write { "write" } else { "read" });
        let mut w = Worker::spawn(cfg.clone(), pw.clone(), access);
        if self.tree.is_empty() {
            w.send(Req::Schemas);
        }
        if let Some(p) = &self.page {
            w.send(Req::Page(p.clone()));
        }
        self.worker = Some(w);
        self.cfg = Some(cfg);
        self.password = pw;
        self.screen = Screen::Browser;
        self.status = if write {
            I18N.t("reconnecting with write access…").into_owned()
        } else {
            I18N.t("connecting (read-only)…").into_owned()
        };
    }

    fn reopen(&mut self, access: Access) {
        if let Some(cfg) = self.cfg.clone() {
            let pw = self.password.clone();
            self.start(cfg, pw, access);
        }
    }

    fn ui_action(&mut self, a: UiAction) {
        let db = self.db_name();
        match state::apply(&mut self.mode, &db, a, &I18N.t("enabled by the toolbar button"))
        {
            Effect::AskConfirm(db) => {
                self.modal = Some(Modal::Confirm {
                    typed: String::new(),
                    db,
                })
            }
            Effect::ReopenWrite(g) => self.reopen(Access::Write(g)),
            Effect::ReopenReadOnly => self.reopen(Access::ReadOnly),
            Effect::WrongName => {
                self.status = I18N
                    .t("database name does not match — writes not enabled")
                    .into_owned()
            }
            Effect::None => {}
        }
    }

    fn pump(&mut self) {
        loop {
            let Some(w) = self.worker.as_mut() else {
                return;
            };
            match w.poll() {
                Ok(Some(r)) => self.on_resp(r),
                Ok(None) => return,
                Err(()) => {
                    self.worker = None;
                    return;
                }
            }
        }
    }

    fn on_resp(&mut self, r: Resp) {
        match r {
            Resp::Opened(Ok(c)) => {
                let write = self.worker.as_ref().is_some_and(|w| w.write);
                if let Some(w) = self.worker.as_mut() {
                    w.set_cancel(c);
                }
                self.status = if write {
                    I18N.t("connected: WRITES ALLOWED").into_owned()
                } else {
                    I18N.t("connected (read-only)").into_owned()
                };
            }
            Resp::Opened(Err(e)) => {
                if self.mode.is_write() {
                    self.mode.exit_write();
                }
                self.status = I18N.f("connection error: {error}", &[("error", &err(&e))]);
            }
            Resp::Schemas(Ok(v)) => {
                self.tree = v
                    .into_iter()
                    .map(|s| Node::Schema {
                        name: Some(s),
                        expanded: false,
                    })
                    .collect();
                if self.tree.is_empty() {
                    self.tree.push(Node::Schema {
                        name: None,
                        expanded: false,
                    });
                }
                self.tree_sel = 0;
                self.toggle(0);
            }
            Resp::Tables(sc, Ok(v)) => {
                let Some(i) = self
                    .tree
                    .iter()
                    .position(|n| matches!(n, Node::Schema { name, .. } if *name == sc))
                else {
                    return;
                };
                self.collapse(i);
                if let Node::Schema { expanded, .. } = &mut self.tree[i] {
                    *expanded = true;
                }
                for (k, t) in v.into_iter().enumerate() {
                    self.tree.insert(i + 1 + k, Node::Table(t));
                }
            }
            Resp::Describe(t, Ok(d)) => {
                self.desc = Some((t, d));
                self.struct_top = 0;
            }
            Resp::Page(p, Ok(rs)) => {
                let same = self
                    .data
                    .as_ref()
                    .is_some_and(|g| g.rs.columns == rs.columns);
                let mut g = Grid::new(rs);
                if same {
                    if let Some(old) = &self.data {
                        g.col = old.col.min(g.widths.len().saturating_sub(1));
                        g.left = old.left;
                    }
                }
                self.status = format!(
                    "{}{}{}",
                    I18N.f(
                        "rows {from}–{to}",
                        &[("from", &(p.offset + 1)), ("to", &(p.offset + g.rs.rows.len() as u64))]
                    ),
                    p.as_of
                        .as_ref()
                        .map(|r| format!(" · AS OF {}", visible_line(r, 4)))
                        .unwrap_or_default(),
                    p.filter
                        .as_ref()
                        .map(|(c, v)| format!(
                            " · {}",
                            I18N.f(
                                "filter {column} LIKE {value}",
                                &[("column", c), ("value", &visible_line(v, 4))]
                            )
                        ))
                        .unwrap_or_default()
                );
                self.data = Some(g);
                self.page = Some(p);
            }
            Resp::Query(Ok(rs), dt) => {
                self.status = format!(
                    "{}{}{}",
                    I18N.n(
                        "{n} row in {ms} ms",
                        "{n} rows in {ms} ms",
                        rs.rows.len() as u64,
                        &[("ms", &dt.as_millis())]
                    ),
                    if rs.truncated {
                        format!(
                            " ({})",
                            I18N.f("truncated to {max}", &[("max", &self.max_rows)])
                        )
                    } else {
                        String::new()
                    },
                    rs.affected
                        .map(|a| format!(", {}", I18N.f("affected {n}", &[("n", &a)])))
                        .unwrap_or_default()
                );
                self.results = Some(Grid::new(rs));
            }
            Resp::Schemas(Err(e))
            | Resp::Tables(_, Err(e))
            | Resp::Describe(_, Err(e))
            | Resp::Page(_, Err(e)) => {
                self.status = I18N.f("error: {error}", &[("error", &err(&e))]);
            }
            Resp::Query(Err(e), _) => {
                self.status = I18N.f("query error: {error}", &[("error", &err(&e))])
            }
            Resp::Branches(Ok(v)) => {
                let mut items = vec![(String::new(), I18N.t("(connection branch)").into_owned())];
                items.extend(v.into_iter().map(|b| {
                    let label = format!(
                        "{}  {}  {}",
                        b.name,
                        b.hash.chars().take(8).collect::<String>(),
                        b.latest_commit_message
                    );
                    (b.name, label)
                }));
                let sel = self
                    .as_of
                    .as_ref()
                    .and_then(|r| items.iter().position(|(n, _)| n == r))
                    .unwrap_or(0);
                self.modal = Some(Modal::Refs {
                    items,
                    sel,
                    log: false,
                });
            }
            Resp::Log(at, Ok(v)) => {
                let items: Vec<(String, String)> = v
                    .into_iter()
                    .map(|c| {
                        let label = format!(
                            "{}  {}  {}  {}",
                            c.hash.chars().take(8).collect::<String>(),
                            c.date,
                            c.committer,
                            c.message
                        );
                        (c.hash, label)
                    })
                    .collect();
                if items.is_empty() {
                    self.status = I18N.t("history is empty").into_owned();
                } else {
                    self.status = match at.as_deref() {
                        Some(r) => I18N.f("history {ref}", &[("ref", &visible_line(r, 4))]),
                        None => I18N.t("history of the current branch").into_owned(),
                    };
                    self.modal = Some(Modal::Refs {
                        items,
                        sel: 0,
                        log: true,
                    });
                }
            }
            Resp::Branches(Err(e)) | Resp::Log(_, Err(e)) => {
                self.status = format!("Dolt: {}", err(&e));
            }
            Resp::Exported(path, r) => {
                self.status = match r {
                    Ok(n) => I18N.n(
                        "saved {n} row: {path}",
                        "saved {n} rows: {path}",
                        n as u64,
                        &[("path", &visible_line(&path.display().to_string(), 4))],
                    ),
                    Err(e) => I18N.f("export: {error}", &[("error", &err(&e))]),
                };
            }
        }
    }

    fn collapse(&mut self, i: usize) {
        let mut j = i + 1;
        while j < self.tree.len() && matches!(self.tree[j], Node::Table(_)) {
            j += 1;
        }
        self.tree.drain(i + 1..j);
        if let Node::Schema { expanded, .. } = &mut self.tree[i] {
            *expanded = false;
        }
    }

    fn toggle(&mut self, i: usize) {
        let Some(Node::Schema { name, expanded }) = self.tree.get(i) else {
            return;
        };
        if *expanded {
            self.collapse(i);
        } else {
            let name = name.clone();
            if let Some(w) = self.worker.as_mut() {
                w.send(Req::Tables(name));
            }
        }
    }

    fn open_table(&mut self, t: &TableInfo) {
        let tr = TableRef {
            schema: t.schema.clone(),
            name: t.name.clone(),
        };
        let p = PageRequest {
            table: tr.clone(),
            order_by: None,
            filter: None,
            offset: 0,
            limit: PAGE,
            as_of: self.as_of.clone(),
        };
        self.data = None;
        if let Some(w) = self.worker.as_mut() {
            w.send(Req::Describe(tr));
            w.send(Req::Page(p));
        }
        if self.tab == Tab::Query {
            self.tab = Tab::Data;
        }
    }

    fn refetch(&mut self, p: PageRequest) {
        if let Some(w) = self.worker.as_mut() {
            w.send(Req::Page(p));
        }
    }

    fn run_query(&mut self) {
        let sql = self.editor.text().trim().to_string();
        if sql.is_empty() {
            return;
        }
        if self.history.last() != Some(&sql) {
            self.history.push(sql.clone());
        }
        self.hist_pos = None;
        if let (Some(p), false) = (&self.save_history, self.mode.is_write()) {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
            {
                let _ = writeln!(f, "{sql};");
            }
        }
        let max = self.max_rows;
        self.last_sql = Some(sql.clone());
        if let Some(w) = self.worker.as_mut() {
            w.send(Req::Query(sql, max));
            self.status = I18N.t("running… (Esc to cancel)").into_owned();
        } else {
            self.status = I18N.t("not connected").into_owned();
        }
    }

    fn history_step(&mut self, back: bool) {
        if self.history.is_empty() {
            return;
        }
        let n = self.history.len();
        let pos = match (self.hist_pos, back) {
            (None, true) => n - 1,
            (None, false) => return,
            (Some(p), true) => p.saturating_sub(1),
            (Some(p), false) if p + 1 >= n => {
                self.hist_pos = None;
                self.editor.set("");
                return;
            }
            (Some(p), false) => p + 1,
        };
        self.hist_pos = Some(pos);
        let t = self.history[pos].clone();
        self.editor.set(&t);
    }

    /// Export the whole table (with the current sort, filter and AS OF) or all rows of the
    /// last query, streamed on the worker thread.
    fn export(&mut self, path: PathBuf, query: bool) {
        let req = if query {
            self.last_sql
                .clone()
                .filter(|_| self.results.is_some())
                .map(|sql| Req::ExportQuery(sql, path))
        } else {
            self.page.clone().map(|p| Req::ExportTable(p, path))
        };
        let Some(req) = req else {
            self.status = I18N.t("nothing to export").into_owned();
            return;
        };
        match self.worker.as_mut() {
            Some(w) => {
                w.send(req);
                self.status = I18N.t("exporting…").into_owned();
            }
            None => self.status = I18N.t("not connected").into_owned(),
        }
    }

    fn is_dolt(&self) -> bool {
        self.cfg.as_ref().is_some_and(|c| c.kind == Kind::Dolt)
    }

    /// Dolt: view data at a branch/commit (`None` is the connection's branch).
    fn set_as_of(&mut self, r: Option<String>) {
        self.as_of = r;
        if let Some(p) = self.page.clone() {
            self.refetch(PageRequest {
                as_of: self.as_of.clone(),
                offset: 0,
                ..p
            });
        } else {
            self.status = match &self.as_of {
                Some(r) => I18N.f("AS OF {ref} — open a table", &[("ref", &visible_line(r, 4))]),
                None => I18N.t("connection branch").into_owned(),
            };
        }
    }

    fn save_conns(&mut self) {
        let Some(p) = self.main_path.clone() else {
            self.status = I18N.t("config directory not found").into_owned();
            return;
        };
        let list: Vec<ConnectionConfig> = self
            .conns
            .iter()
            .filter(|c| self.saved.contains(&c.id))
            .cloned()
            .collect();
        if let Err(e) = conns::save(&p, &list) {
            self.status = I18N.f("not saved: {error}", &[("error", &err(&e))]);
        }
    }

    // ---- keys ----

    fn modal_key(&mut self, k: KeyEvent, phys: Option<Key>) -> io::Result<()> {
        let Some(mut m) = self.modal.take() else {
            return Ok(());
        };
        let text = match &mut m {
            Modal::Confirm { typed, .. } => Some(typed),
            Modal::Password { value } | Modal::Filter { value } | Modal::Export { value, .. } => {
                Some(value)
            }
            _ => None,
        };
        if let Some(t) = text {
            match k.code {
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    t.push(c);
                    self.modal = Some(m);
                    return Ok(());
                }
                KeyCode::Backspace => {
                    t.pop();
                    self.modal = Some(m);
                    return Ok(());
                }
                _ => {}
            }
        }
        let enter = k.code == KeyCode::Enter;
        let yes = crate::keys::confirmed(&k, phys);
        let is = |spec| crate::keys::is(&k, phys, spec);
        if let Modal::Refs { items, sel, log } = m {
            let n = items.len();
            match k.code {
                KeyCode::Up => {
                    self.modal = Some(Modal::Refs {
                        items,
                        sel: sel.saturating_sub(1),
                        log,
                    })
                }
                KeyCode::Down => {
                    let sel = (sel + 1).min(n.saturating_sub(1));
                    self.modal = Some(Modal::Refs { items, sel, log })
                }
                KeyCode::Enter => {
                    let r = items.get(sel).map(|(r, _)| r.clone()).unwrap_or_default();
                    self.set_as_of((!r.is_empty()).then_some(r));
                }
                // history of the selected branch
                _ if !log && is("h")? => {
                    let r = items.get(sel).map(|(r, _)| r.clone()).unwrap_or_default();
                    if let Some(w) = self.worker.as_mut() {
                        w.send(Req::Log((!r.is_empty()).then_some(r)));
                    }
                }
                // put an AS OF query template for the selected ref into the editor
                _ if is("a")? => {
                    let r = items.get(sel).map(|(r, _)| r.clone()).unwrap_or_default();
                    let table = self
                        .page
                        .as_ref()
                        .map(|p| format!("`{}`", p.table.name.replace('`', "``")))
                        .unwrap_or_else(|| I18N.t("<table>").into_owned());
                    let as_of = if r.is_empty() {
                        Ok(String::new())
                    } else {
                        saysdf_db::dolt::as_of_clause(&r)
                    };
                    let sql = match as_of {
                        Ok(c) => format!("SELECT * FROM {table}{c} LIMIT 100"),
                        Err(e) => {
                            self.status = err(&e.to_string());
                            return Ok(());
                        }
                    };
                    self.editor.set(&sql);
                    self.tab = Tab::Query;
                    self.focus = Focus::Right;
                }
                KeyCode::Esc => {}
                _ => self.modal = Some(Modal::Refs { items, sel, log }),
            }
            return Ok(());
        }
        match m {
            Modal::Confirm { typed, .. } => {
                if enter {
                    self.ui_action(UiAction::ConfirmSubmit(typed));
                } else if k.code == KeyCode::Esc {
                    self.ui_action(UiAction::ConfirmCancel);
                } else {
                    self.modal = Some(Modal::Confirm {
                        typed,
                        db: self.db_name(),
                    });
                }
            }
            Modal::Password { value } => {
                if enter {
                    if let Some(cfg) = self.pending.take() {
                        self.session_pw.insert(cfg.id.clone(), value.clone());
                        self.tree.clear();
                        self.page = None;
                        self.data = None;
                        self.desc = None;
                        self.start(cfg, Some(value), Access::ReadOnly);
                    }
                } else if k.code == KeyCode::Esc {
                    self.pending = None;
                } else {
                    self.modal = Some(Modal::Password { value });
                }
            }
            Modal::Filter { value } => {
                if enter {
                    if let (Some(mut p), Some(g)) = (self.page.clone(), self.data.as_ref()) {
                        let col = g.rs.columns.get(g.col).cloned();
                        p.offset = 0;
                        p.filter = match (col, value.is_empty()) {
                            (Some(c), false) => {
                                let v = if value.contains('%') || value.contains('_') {
                                    value
                                } else {
                                    format!("%{value}%")
                                };
                                Some((c, v))
                            }
                            _ => None,
                        };
                        self.refetch(p);
                    }
                } else if k.code != KeyCode::Esc {
                    self.modal = Some(Modal::Filter { value });
                }
            }
            Modal::Export { value, query } => {
                if enter && !value.trim().is_empty() {
                    let path = PathBuf::from(value.trim());
                    if path.exists() {
                        self.modal = Some(Modal::Overwrite { path, query });
                    } else {
                        self.export(path, query);
                    }
                } else if k.code != KeyCode::Esc {
                    self.modal = Some(Modal::Export { value, query });
                }
            }
            Modal::Overwrite { path, query } => {
                if yes {
                    self.export(path, query);
                } else {
                    self.status = I18N.t("export cancelled").into_owned();
                }
            }
            Modal::Refs { .. } => {}
            Modal::Delete { idx } => {
                if yes && idx < self.conns.len() {
                    let c = self.conns.remove(idx);
                    if self.saved.remove(&c.id) {
                        self.save_conns();
                    }
                    let _ = Secrets::os().delete(Domain::Db, &c.id);
                    self.sel = self.sel.min(self.conns.len().saturating_sub(1));
                }
            }
        }
        Ok(())
    }

    fn key(&mut self, k: KeyEvent, phys: Option<Key>) -> io::Result<Cmd> {
        if self.modal.is_some() {
            self.modal_key(k, phys)?;
            self.prev_esc = false;
            return Ok(Cmd::None);
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            return Ok(Cmd::Quit);
        }
        if self.screen == Screen::Conns {
            return self.conns_key(k, phys);
        }
        let prev_esc = std::mem::replace(&mut self.prev_esc, k.code == KeyCode::Esc);
        if let Some(a) = state::key_action(self.focus, k.code, k.modifiers, prev_esc) {
            if a != UiAction::ExitKey || self.mode.is_write() {
                self.prev_esc = false;
                self.ui_action(a);
                return Ok(Cmd::None);
            }
        }
        if k.code == KeyCode::Esc && self.worker.as_ref().is_some_and(|w| w.busy > 0) {
            if let Some(w) = &self.worker {
                self.status = match w.cancel() {
                    Ok(()) => I18N.t("cancelling query…").into_owned(),
                    Err(e) => err(&e),
                };
            }
            return Ok(Cmd::None);
        }
        match k.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::WriteButton => Focus::Tree,
                    Focus::Tree => Focus::Right,
                    Focus::Right => Focus::WriteButton,
                };
                return Ok(Cmd::None);
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::WriteButton => Focus::Right,
                    Focus::Tree => Focus::WriteButton,
                    Focus::Right => Focus::Tree,
                };
                return Ok(Cmd::None);
            }
            KeyCode::F(2) => self.tab = Tab::Structure,
            KeyCode::F(3) => self.tab = Tab::Data,
            KeyCode::F(4) => {
                self.tab = Tab::Query;
                self.focus = Focus::Right;
            }
            _ => {}
        }
        let editing = self.focus == Focus::Right && self.tab == Tab::Query;
        let is = |spec| crate::keys::is(&k, phys, spec);
        if !editing {
            match k.code {
                _ if is("q")? => return Ok(Cmd::Quit),
                _ if is("c")? => {
                    state::apply(&mut self.mode, "", UiAction::SwitchConnection, "");
                    self.worker = None;
                    self.screen = Screen::Conns;
                    return Ok(Cmd::None);
                }
                _ if is("r")? => {
                    self.ui_action(UiAction::Reconnect);
                    return Ok(Cmd::None);
                }
                _ if self.is_dolt() && (is("b")? || is("h")?) => {
                    let req = if is("b")? {
                        Req::Branches
                    } else {
                        Req::Log(self.as_of.clone())
                    };
                    if let Some(w) = self.worker.as_mut() {
                        w.send(req);
                    }
                    return Ok(Cmd::None);
                }
                _ if is("1")? => self.tab = Tab::Structure,
                _ if is("2")? => self.tab = Tab::Data,
                _ if is("3")? => {
                    self.tab = Tab::Query;
                    self.focus = Focus::Right;
                }
                _ => {}
            }
        }
        match self.focus {
            Focus::WriteButton => {}
            Focus::Tree => self.tree_key(k),
            Focus::Right => match self.tab {
                Tab::Structure => match k.code {
                    KeyCode::Up => self.struct_top = self.struct_top.saturating_sub(1),
                    KeyCode::Down => self.struct_top += 1,
                    _ => {}
                },
                Tab::Data => self.data_key(k, phys)?,
                Tab::Query => self.query_key(k),
            },
        }
        Ok(Cmd::None)
    }

    fn conns_key(&mut self, k: KeyEvent, phys: Option<Key>) -> io::Result<Cmd> {
        let is = |spec| crate::keys::is(&k, phys, spec);
        let n = self.conns.len();
        match k.code {
            KeyCode::Esc => return Ok(Cmd::Quit),
            _ if is("q")? => return Ok(Cmd::Quit),
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Insert => return Ok(Cmd::Form(None)),
            _ if is("a")? => return Ok(Cmd::Form(None)),
            KeyCode::Enter if n == 0 => return Ok(Cmd::Form(None)),
            _ if n > 0 && is("e")? => {
                if self.saved.contains(&self.conns[self.sel].id) {
                    return Ok(Cmd::Form(Some(self.sel)));
                }
                self.status = I18N
                    .t("connection from --connections — edit the source file")
                    .into_owned();
            }
            _ if n > 0 && is("d")? => {
                if self.saved.contains(&self.conns[self.sel].id) {
                    self.modal = Some(Modal::Delete { idx: self.sel });
                } else {
                    self.status = I18N
                        .t("connection from --connections — edit the source file")
                        .into_owned();
                }
            }
            KeyCode::Enter if n > 0 => {
                let c = self.conns[self.sel].clone();
                self.connect(c);
            }
            _ => {}
        }
        Ok(Cmd::None)
    }

    fn tree_key(&mut self, k: KeyEvent) {
        let n = self.tree.len();
        match k.code {
            KeyCode::Up => self.tree_sel = self.tree_sel.saturating_sub(1),
            KeyCode::Down => self.tree_sel = (self.tree_sel + 1).min(n.saturating_sub(1)),
            KeyCode::Left => {
                if let Some(Node::Schema { expanded: true, .. }) = self.tree.get(self.tree_sel) {
                    self.collapse(self.tree_sel);
                } else if let Some(i) = (0..self.tree_sel)
                    .rev()
                    .find(|&i| matches!(self.tree[i], Node::Schema { .. }))
                {
                    self.tree_sel = i;
                }
            }
            KeyCode::Enter | KeyCode::Right => match self.tree.get(self.tree_sel) {
                Some(Node::Schema { expanded, .. }) => {
                    if !(*expanded && k.code == KeyCode::Right) {
                        self.toggle(self.tree_sel);
                    }
                }
                Some(Node::Table(t)) => {
                    let t = t.clone();
                    self.open_table(&t);
                }
                None => {}
            },
            _ => {}
        }
    }

    fn data_key(&mut self, k: KeyEvent, phys: Option<Key>) -> io::Result<()> {
        let is = |spec| crate::keys::is(&k, phys, spec);
        let h = self.grid_h;
        let Some(p) = self.page.clone() else {
            return Ok(());
        };
        match k.code {
            _ if is("]")? || is("n")? => {
                if self
                    .data
                    .as_ref()
                    .is_some_and(|g| g.rs.rows.len() as u64 == p.limit)
                {
                    self.refetch(PageRequest {
                        offset: p.offset + p.limit,
                        ..p
                    });
                }
            }
            _ if is("[")? || is("p")? => {
                if p.offset > 0 {
                    self.refetch(PageRequest {
                        offset: p.offset.saturating_sub(p.limit),
                        ..p
                    });
                }
            }
            _ if is("s")? => {
                let Some(col) = self
                    .data
                    .as_ref()
                    .and_then(|g| g.rs.columns.get(g.col).cloned())
                else {
                    return Ok(());
                };
                let order_by = match &p.order_by {
                    Some((c, SortDir::Asc)) if *c == col => Some((col, SortDir::Desc)),
                    Some((c, SortDir::Desc)) if *c == col => None,
                    _ => Some((col, SortDir::Asc)),
                };
                self.refetch(PageRequest {
                    order_by,
                    offset: 0,
                    ..p
                });
            }
            _ if is("/")? || is("f")? => {
                let value = p
                    .filter
                    .as_ref()
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                self.modal = Some(Modal::Filter { value });
            }
            _ if is("e")? => {
                self.modal = Some(Modal::Export {
                    value: format!("{}.csv", p.table.name),
                    query: false,
                });
            }
            code => {
                if let Some(g) = self.data.as_mut() {
                    g.key(code, h);
                }
            }
        }
        Ok(())
    }

    fn query_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::F(5) => self.run_query(),
            KeyCode::Enter if ctrl => self.run_query(),
            KeyCode::Char('j') if ctrl => self.run_query(),
            KeyCode::Up if ctrl || alt => self.history_step(true),
            KeyCode::Down if ctrl || alt => self.history_step(false),
            KeyCode::Char('e') if ctrl => {
                self.modal = Some(Modal::Export {
                    value: "result.csv".into(),
                    query: true,
                });
            }
            KeyCode::PageUp | KeyCode::PageDown => {
                let h = self.grid_h;
                if let Some(g) = self.results.as_mut() {
                    g.key(k.code, h);
                }
            }
            KeyCode::Left if ctrl => {
                if let Some(g) = self.results.as_mut() {
                    g.key(KeyCode::Left, 0);
                }
            }
            KeyCode::Right if ctrl => {
                if let Some(g) = self.results.as_mut() {
                    g.key(KeyCode::Right, 0);
                }
            }
            _ => {
                self.editor.key(k);
            }
        }
    }

    fn mouse(&mut self, m: MouseEvent) {
        if self.modal.is_some() || m.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let p = Position::new(m.column, m.row);
        if self.banner.is_some_and(|r| r.contains(p)) {
            self.ui_action(UiAction::BannerClick);
        } else if self.screen == Screen::Browser && self.button.is_some_and(|r| r.contains(p)) {
            self.focus = Focus::WriteButton;
            self.ui_action(UiAction::WriteButton);
        }
    }

    // ---- connection form ----

    fn run_form(&mut self, idx: Option<usize>) -> io::Result<()> {
        let base = idx.and_then(|i| self.conns.get(i).cloned());
        let g = |f: fn(&ConnectionConfig) -> Option<String>| {
            base.as_ref().and_then(f).unwrap_or_default()
        };
        let mut pw = Field::new(
            &I18N.t("Password"),
            &I18N.t("empty — keep as is / ask when connecting"),
            "",
        );
        pw.mask = true;
        let mut fields = vec![
            Field::new(
                "id",
                &I18N.t("Latin letters, digits, - _ ."),
                &g(|c| Some(c.id.clone())),
            ),
            Field::new(&I18N.t("Title"), "", &g(|c| c.title.clone())),
            Field::new(
                &I18N.t("Description"),
                &I18N.t("for example: a role with SELECT only"),
                &g(|c| c.description.clone()),
            ),
            Field::new(
                &I18N.t("Type"),
                "sqlite | postgres | mysql | dolt | postgres-ssh",
                base.as_ref()
                    .map(|c| conns::kind_str(c.kind))
                    .unwrap_or("sqlite"),
            ),
            Field::new(
                &I18N.t("SQLite file"),
                &I18N.t("file path (for sqlite)"),
                &g(|c| c.path.clone()),
            ),
            Field::new(&I18N.t("Host"), "localhost", &g(|c| c.host.clone())),
            Field::new(
                &I18N.t("Port"),
                &I18N.t("driver default"),
                &g(|c| c.port.map(|p| p.to_string())),
            ),
            Field::new(&I18N.t("Database"), "", &g(|c| c.database.clone())),
            Field::new(
                &I18N.t("User"),
                &I18N.t("Postgres: prefer a role with SELECT only"),
                &g(|c| c.user.clone()),
            ),
            Field::new(
                "TLS",
                "require | verify-full | disable",
                base.as_ref()
                    .map(|c| conns::tls_str(c.tls))
                    .unwrap_or("require"),
            ),
            pw,
            Field::new(
                &I18N.t("Remember password"),
                &I18N.t("y/n — in the OS secret store (saysdfTerm)"),
                if base.as_ref().is_some_and(|c| c.remember_password) {
                    "y"
                } else {
                    "n"
                },
            ),
            Field::new(
                &I18N.t("SSH alias"),
                &I18N.t("postgres-ssh: Host from ~/.ssh/config (key login)"),
                &g(|c| c.ssh_alias.clone()),
            ),
            Field::new(
                &I18N.t("Container"),
                &I18N.t("postgres-ssh: docker container with psql; empty — psql on the host"),
                &g(|c| c.container.clone()),
            ),
        ];
        let old_id = base.as_ref().map(|c| c.id.clone());
        let taken: Vec<String> = self
            .conns
            .iter()
            .map(|c| c.id.clone())
            .filter(|i| Some(i) != old_id.as_ref())
            .collect();
        let mut out: Option<(ConnectionConfig, String)> = None;
        let title = if idx.is_some() {
            I18N.t("Edit connection")
        } else {
            I18N.t("New connection")
        };
        let mut sess = Session::enter()?;
        let ok = sess.edit(&title, &mut fields, |fs| {
            let opt = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
            let mut good = true;
            let mut bad = |f: &mut Field, m: &str| {
                f.error = Some(m.into());
                good = false;
            };
            let id = fs[0].value.trim().to_string();
            if let Err(e) = conns::validate_id(&id) {
                bad(&mut fs[0], &e);
            } else if taken.contains(&id) {
                bad(&mut fs[0], &I18N.t("this id already exists"));
            }
            let kind = conns::parse_kind(&fs[3].value);
            if kind.is_none() {
                bad(&mut fs[3], &I18N.t("sqlite, postgres, mysql, dolt or postgres-ssh"));
            }
            if kind == Some(Kind::PgSsh) {
                // same rules as when connecting: the values end up in a remote command
                let checks = [
                    (12, I18N.t("SSH alias")),
                    (7, I18N.t("database")),
                    (8, I18N.t("user")),
                ];
                for (i, name) in checks {
                    if let Err(e) = saysdf_db::pg_ssh::validate_name(&name, fs[i].value.trim()) {
                        bad(&mut fs[i], &e.to_string());
                    }
                }
                let ct = fs[13].value.trim().to_string();
                if !ct.is_empty() {
                    if let Err(e) = saysdf_db::pg_ssh::validate_name(&I18N.t("container"), &ct) {
                        bad(&mut fs[13], &e.to_string());
                    }
                }
            }
            if kind == Some(Kind::Sqlite) && fs[4].value.trim().is_empty() {
                bad(&mut fs[4], &I18N.t("specify a file"));
            }
            let port = match fs[6].value.trim() {
                "" => None,
                s => match s.parse::<u16>() {
                    Ok(p) if p > 0 => Some(p),
                    _ => {
                        bad(&mut fs[6], "1–65535");
                        None
                    }
                },
            };
            let tls = conns::parse_tls(&fs[9].value);
            if tls.is_none() {
                bad(&mut fs[9], &I18N.t("require, verify-full or disable"));
            }
            // the Russian "yes" answers are still accepted next to the English ones
            let remember = matches!(fs[11].value.trim(), "y" | "Y" | "д" | "да" | "yes"); // i18n-allow-cyrillic
            if !good {
                return false;
            }
            let mut c = ConnectionConfig::sqlite(&id, fs[4].value.trim());
            c.path = opt(&fs[4].value);
            c.title = opt(&fs[1].value);
            c.description = opt(&fs[2].value);
            c.kind = kind.unwrap_or(Kind::Sqlite);
            c.host = opt(&fs[5].value);
            c.port = port;
            c.database = opt(&fs[7].value);
            c.user = opt(&fs[8].value);
            c.tls = tls.unwrap_or_default();
            c.remember_password = remember && kind != Some(Kind::PgSsh);
            c.ssh_alias = opt(&fs[12].value);
            c.container = opt(&fs[13].value);
            out = Some((c, fs[10].value.clone()));
            true
        })?;
        drop(sess);
        let Some((c, password)) = out.filter(|_| ok) else {
            return Ok(());
        };
        let secrets = Secrets::os();
        if let Some(old) = old_id.as_ref().filter(|o| **o != c.id) {
            self.conns.retain(|x| x.id != *old);
            self.saved.remove(old);
            let _ = secrets.delete(Domain::Db, old);
        }
        if !password.is_empty() {
            self.session_pw.insert(c.id.clone(), password.clone());
        }
        if c.remember_password && !password.is_empty() {
            if secrets.set(Domain::Db, &c.id, &password).is_err() {
                self.status = I18N
                    .t("secret store unavailable — password kept for this session only")
                    .into_owned();
            }
        } else if !c.remember_password {
            let _ = secrets.delete(Domain::Db, &c.id);
        }
        self.saved.insert(c.id.clone());
        let id = c.id.clone();
        conns::upsert(&mut self.conns, c);
        self.sel = self.conns.iter().position(|x| x.id == id).unwrap_or(0);
        self.save_conns();
        Ok(())
    }

    // ---- drawing ----

    fn draw(&mut self, f: &mut Frame) {
        let a = state::areas(&self.mode, f.area());
        self.banner = a.banner;
        if let (Some(b), Some((main, explain))) =
            (a.banner, state::banner_text(&self.mode, f.area().width))
        {
            let line = Line::from(vec![
                Span::styled(
                    main,
                    Style::new()
                        .fg(Color::White)
                        .bg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(explain, Style::new().fg(Color::White).bg(Color::Red)),
            ]);
            f.render_widget(Paragraph::new(line).style(Style::new().bg(Color::Red)), b);
        }
        self.draw_toolbar(f, a.toolbar);
        match self.screen {
            Screen::Conns => self.draw_conns(f, a.body),
            Screen::Browser => self.draw_browser(f, a.body),
        }
        let hints = match (self.screen, self.focus, self.tab) {
            (Screen::Conns, ..) if self.conns.is_empty() => {
                I18N.t("a/Enter — add · q/Esc — quit")
            }
            (Screen::Conns, ..) => I18N.t(
                "Enter — connect · a — add · e — edit · d — delete · q — quit",
            ),
            (_, Focus::WriteButton, _) => I18N.t("Enter — toggle writes · Tab — focus"),
            (_, Focus::Tree, _) if self.is_dolt() => I18N.t(
                "Enter — open · b — branches · h — history · 1/2/3 — tabs · r — reconnect · c — connections · q — quit",
            ),
            (_, Focus::Tree, _) => I18N.t(
                "Enter — open · Tab — focus · 1/2/3 — tabs · r — reconnect · c — connections · q — quit",
            ),
            (_, _, Tab::Data) if self.is_dolt() => I18N.t(
                "←→↑↓ · [ ] — page · s — sort · / — filter · e — export · b/h — branches/history",
            ),
            (_, _, Tab::Data) => I18N.t(
                "←→↑↓ · [ ] — page · s — sort · / — filter · e — export · Tab — focus",
            ),
            (_, _, Tab::Query) => I18N.t(
                "F5/Ctrl+Enter — run · Ctrl+↑↓ — history · Esc — cancel · Ctrl+E — export · F2/F3 — tabs",
            ),
            _ => I18N.t("↑↓ — scroll · Tab — focus"),
        };
        let st = Line::from(vec![
            Span::raw(format!(" {} ", visible_line(&self.status, 4))),
            Span::styled(
                format!(" {hints}"),
                Style::new().add_modifier(Modifier::DIM),
            ),
        ]);
        f.render_widget(Paragraph::new(st), a.status);
        if self.modal.is_some() {
            self.draw_modal(f, a.body);
        }
    }

    fn draw_toolbar(&mut self, f: &mut Frame, r: Rect) {
        if self.screen == Screen::Conns {
            self.button = None;
            f.render_widget(
                Paragraph::new(Line::styled(
                    format!(" saysdf db — {}", I18N.t("connections")),
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                r,
            );
            return;
        }
        let label = if self.mode.is_write() {
            I18N.t("[ Leave dangerous mode ]")
        } else {
            I18N.t("[ Allow writes… ]")
        };
        let mut st = if self.mode.is_write() {
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        if self.focus == Focus::WriteButton {
            st = st.add_modifier(Modifier::REVERSED);
        }
        let bw = str_width(&label) as u16;
        self.button = Some(Rect::new(
            r.x + 1,
            r.y,
            bw.min(r.width.saturating_sub(1)),
            r.height,
        ));
        let title = self
            .cfg
            .as_ref()
            .map(|c| {
                format!(
                    "{} · {} · {}",
                    c.title.clone().unwrap_or_else(|| c.id.clone()),
                    conns::kind_str(c.kind),
                    c.database_name()
                )
            })
            .unwrap_or_default();
        let access = if self.mode.is_write() {
            Span::styled(
                format!(" {} ", I18N.t("WRITE")),
                Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" {} ", I18N.t("read-only")), Style::new().fg(Color::Green))
        };
        let busy = if self.worker.as_ref().is_some_and(|w| w.busy > 0) {
            " ⏳"
        } else {
            ""
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(label.into_owned(), st),
                Span::raw("  "),
                Span::styled(
                    visible_line(&title, 4),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                access,
                Span::raw(busy),
            ])),
            r,
        );
    }

    fn draw_conns(&mut self, f: &mut Frame, r: Rect) {
        let mut lines = Vec::new();
        if self.conns.is_empty() {
            lines.extend(empty_conns_lines());
        }
        for (i, c) in self.conns.iter().enumerate() {
            let sel = i == self.sel;
            let mut st = Style::new().add_modifier(Modifier::BOLD);
            if sel {
                st = st.fg(Color::Cyan).add_modifier(Modifier::REVERSED);
            }
            let title = c.title.clone().unwrap_or_else(|| c.id.clone());
            let target = match c.kind {
                Kind::Sqlite => c.path.clone().unwrap_or_default(),
                Kind::PgSsh => format!(
                    "{}@ssh:{}{}/{}",
                    c.user.clone().unwrap_or_default(),
                    c.ssh_alias.clone().unwrap_or_default(),
                    c.container
                        .as_deref()
                        .map(|x| format!(" docker:{x}"))
                        .unwrap_or_default(),
                    c.database.clone().unwrap_or_default()
                ),
                _ => format!(
                    "{}@{}{}/{}",
                    c.user.clone().unwrap_or_default(),
                    c.host_or_default(),
                    c.port.map(|p| format!(":{p}")).unwrap_or_default(),
                    c.database.clone().unwrap_or_default()
                ),
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {} ", visible_line(&title, 4)), st),
                Span::raw(format!(
                    " {} · {}",
                    conns::kind_str(c.kind),
                    visible_line(&target, 4)
                )),
                Span::styled(
                    if self.saved.contains(&c.id) {
                        String::new()
                    } else {
                        format!("  ({})", I18N.t("from --connections"))
                    },
                    Style::new().add_modifier(Modifier::DIM),
                ),
            ]));
            if let Some(d) = &c.description {
                lines.push(Line::styled(
                    format!("    {}", visible_line(d, 4)),
                    Style::new().add_modifier(Modifier::DIM),
                ));
            }
        }
        f.render_widget(Paragraph::new(lines), r);
    }

    fn draw_browser(&mut self, f: &mut Frame, r: Rect) {
        let tw = (r.width / 4).clamp(16.min(r.width), 40);
        let tree_r = Rect::new(r.x, r.y, tw, r.height);
        let right = Rect::new(r.x + tw, r.y, r.width - tw, r.height);
        // tree
        let h = tree_r.height.saturating_sub(2) as usize;
        if self.tree_sel < self.tree_top {
            self.tree_top = self.tree_sel;
        } else if h > 0 && self.tree_sel >= self.tree_top + h {
            self.tree_top = self.tree_sel + 1 - h;
        }
        let win = state::body_window(
            Rect::new(0, 0, tree_r.width, h as u16),
            self.tree_top,
            self.tree.len(),
        );
        let lines: Vec<Line> = win
            .map(|i| (i, &self.tree[i]))
            .map(|(i, n)| {
                let text = match n {
                    Node::Schema { name, expanded } => format!(
                        "{} {}",
                        if *expanded { "▾" } else { "▸" },
                        visible_line(name.as_deref().unwrap_or(&I18N.t("(schema)")), 4)
                    ),
                    Node::Table(t) => format!(
                        "  {} {}",
                        if t.kind == ObjectKind::View {
                            "◇"
                        } else {
                            "▦"
                        },
                        visible_line(&t.name, 4)
                    ),
                };
                let st = if i == self.tree_sel && self.focus == Focus::Tree {
                    Style::new().add_modifier(Modifier::REVERSED)
                } else if i == self.tree_sel {
                    Style::new().add_modifier(Modifier::BOLD)
                } else {
                    Style::new()
                };
                Line::styled(text, st)
            })
            .collect();
        let bst = |on: bool| {
            if on {
                Style::new().fg(Color::Cyan)
            } else {
                Style::new().add_modifier(Modifier::DIM)
            }
        };
        f.render_widget(
            Paragraph::new(lines).block(
                Block::new()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", I18N.t("Schema")))
                    .border_style(bst(self.focus == Focus::Tree)),
            ),
            tree_r,
        );
        // right panel
        let tabs = [
            (Tab::Structure, I18N.t("1 Structure")),
            (Tab::Data, I18N.t("2 Data")),
            (Tab::Query, I18N.t("3 Query")),
        ];
        let mut title = vec![Span::raw(" ")];
        for (t, name) in tabs {
            let st = if t == self.tab {
                Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::new()
            };
            title.push(Span::styled(format!(" {name} "), st));
            title.push(Span::raw(" "));
        }
        let block = Block::new()
            .borders(Borders::ALL)
            .title(Line::from(title))
            .border_style(bst(self.focus == Focus::Right));
        let inner = block.inner(right);
        f.render_widget(block, right);
        let w = inner.width as usize;
        match self.tab {
            Tab::Structure => {
                let lines = self.structure_lines();
                self.struct_top = self.struct_top.min(lines.len().saturating_sub(1));
                f.render_widget(
                    Paragraph::new(lines).scroll((self.struct_top as u16, 0)),
                    inner,
                );
            }
            Tab::Data => {
                self.grid_h = inner.height.saturating_sub(1) as usize;
                let sort = self.page.as_ref().and_then(|p| p.order_by.clone());
                let lines = match self.data.as_mut() {
                    Some(g) => g.lines(w, inner.height as usize, sort.as_ref()),
                    None => vec![Line::raw(format!(" {}", I18N.t("Select a table in the tree (Enter).")))],
                };
                f.render_widget(Paragraph::new(lines), inner);
            }
            Tab::Query => {
                let eh = (inner.height / 3).max(3).min(inner.height);
                let er = Rect::new(inner.x, inner.y, inner.width, eh);
                let rr = Rect::new(inner.x, inner.y + eh, inner.width, inner.height - eh);
                if self.editor.lines.is_empty() {
                    self.editor.lines.push(String::new());
                }
                self.hl.invalidate();
                let mut lines = self.hl.highlight_all(&self.editor.lines);
                let top = self
                    .editor
                    .row
                    .saturating_sub(eh.saturating_sub(1) as usize);
                lines = lines.into_iter().skip(top).take(eh as usize).collect();
                f.render_widget(Paragraph::new(lines), er);
                if rr.height > 0 {
                    let sep = Rect::new(rr.x, rr.y, rr.width, 1);
                    f.render_widget(
                        Paragraph::new(Line::styled(
                            "─".repeat(rr.width as usize),
                            Style::new().add_modifier(Modifier::DIM),
                        )),
                        sep,
                    );
                    let gr = Rect::new(rr.x, rr.y + 1, rr.width, rr.height - 1);
                    self.grid_h = gr.height.saturating_sub(1) as usize;
                    if let Some(g) = self.results.as_mut() {
                        f.render_widget(Paragraph::new(g.lines(w, gr.height as usize, None)), gr);
                    }
                }
                if self.focus == Focus::Right && self.modal.is_none() {
                    let line = &self.editor.lines[self.editor.row];
                    let cx = str_width(&visible_line(
                        &line.chars().take(self.editor.col).collect::<String>(),
                        4,
                    ));
                    let cy = self.editor.row - top;
                    f.set_cursor_position(Position::new(
                        er.x + (cx as u16).min(er.width.saturating_sub(1)),
                        er.y + cy as u16,
                    ));
                }
            }
        }
    }

    fn structure_lines(&self) -> Vec<Line<'static>> {
        let Some((t, d)) = &self.desc else {
            return vec![Line::raw(format!(" {}", I18N.t("Select a table in the tree (Enter).")))];
        };
        let b = Style::new().add_modifier(Modifier::BOLD);
        let dim = Style::new().add_modifier(Modifier::DIM);
        let mut out = vec![
            Line::styled(format!(" {}", visible_line(&t.name, 4)), b),
            Line::styled(format!(" {}", I18N.t("Columns")), b),
        ];
        for c in &d.columns {
            let mut extra = String::new();
            if c.primary_key {
                extra.push_str(" PK");
            }
            if !c.nullable {
                extra.push_str(" NOT NULL");
            }
            if let Some(df) = &c.default {
                extra.push_str(&format!(" DEFAULT {}", visible_line(df, 4)));
            }
            out.push(Line::from(vec![
                Span::raw(format!("   {:<24} ", visible_line(&c.name, 4))),
                Span::styled(
                    format!("{:<16}", visible_line(&c.data_type, 4)),
                    Style::new().fg(Color::Yellow),
                ),
                Span::styled(extra, dim),
            ]));
        }
        out.push(Line::styled(format!(" {}", I18N.t("Indexes")), b));
        if d.indexes.is_empty() {
            out.push(Line::styled("   —", dim));
        }
        for i in &d.indexes {
            out.push(Line::raw(format!(
                "   {} ({}){}",
                visible_line(&i.name, 4),
                visible_line(&i.columns.join(", "), 4),
                if i.unique { " UNIQUE" } else { "" }
            )));
        }
        out.push(Line::styled(format!(" {}", I18N.t("Foreign keys")), b));
        if d.foreign_keys.is_empty() {
            out.push(Line::styled("   —", dim));
        }
        for k in &d.foreign_keys {
            out.push(Line::raw(format!(
                "   ({}) → {}({})",
                visible_line(&k.columns.join(", "), 4),
                visible_line(&k.ref_table, 4),
                visible_line(&k.ref_columns.join(", "), 4)
            )));
        }
        out
    }

    fn draw_modal(&self, f: &mut Frame, body: Rect) {
        let Some(m) = &self.modal else { return };
        let (title, text, input, masked): (String, String, Option<&str>, bool) = match m {
            Modal::Confirm { typed, db } => (
                format!(" {} ", I18N.t("DANGEROUS MODE")),
                I18N.f(
                    "Queries will be able to modify data in “{db}”.\nTo enable, type the database name and press Enter (Esc to cancel).",
                    &[("db", &visible_line(db, 4))],
                ),
                Some(typed.as_str()),
                false,
            ),
            Modal::Password { value } => (
                format!(" {} ", I18N.t("Password")),
                I18N.t("The password is not in the secret store — enter it (Esc to cancel).").into_owned(),
                Some(value.as_str()),
                true,
            ),
            Modal::Filter { value } => (
                format!(" {} ", I18N.t("Filter")),
                I18N.t("LIKE on the current column (without % — substring; empty — reset).").into_owned(),
                Some(value.as_str()),
                false,
            ),
            Modal::Export { value, query } => (
                format!(" {} ", I18N.t("Export")),
                I18N.f(
                    "{what}: file path (.csv or .json).",
                    &[(
                        "what",
                        &if *query {
                            I18N.t("All query rows (re-run read-only)")
                        } else {
                            I18N.t("Whole table (with sort and filter)")
                        },
                    )],
                ),
                Some(value.as_str()),
                false,
            ),
            Modal::Overwrite { path, .. } => (
                format!(" {} ", I18N.t("Export")),
                I18N.f(
                    "File {path} exists. Overwrite? {hint}",
                    &[
                        ("path", &visible_line(&path.display().to_string(), 4)),
                        ("hint", &crate::keys::CONFIRM_HINT),
                    ],
                ),
                None,
                false,
            ),
            Modal::Refs { items, sel, log } => {
                const ROWS: usize = 15;
                let top = sel.saturating_sub(ROWS - 1);
                let mut t = if *log {
                    I18N.t("Enter — view at commit · a — AS OF query · Esc — close").into_owned()
                } else {
                    I18N.t("Enter — view branch · h — history · a — AS OF query · Esc — close").into_owned()
                };
                for (i, (_, label)) in items.iter().enumerate().skip(top).take(ROWS) {
                    t.push('\n');
                    t.push_str(if i == *sel { "> " } else { "  " });
                    t.push_str(&visible_line(label, 4));
                }
                let title = if *log { I18N.t("Dolt history") } else { I18N.t("Dolt branches") };
                (format!(" {title} "), t, None, false)
            }
            Modal::Delete { idx } => (
                format!(" {} ", I18N.t("Delete")),
                I18N.f(
                    "Delete connection “{id}” and its password from the secret store? {hint}",
                    &[
                        ("id", &self.conns.get(*idx).map(|c| visible_line(&c.id, 4)).unwrap_or_default()),
                        ("hint", &crate::keys::CONFIRM_HINT),
                    ],
                ),
                None,
                false,
            ),
        };
        let w = body.width.saturating_sub(4).min(80);
        let h = (text.lines().count() as u16 + 2 + u16::from(input.is_some())).min(body.height);
        let r = Rect::new(
            body.x + (body.width - w) / 2,
            body.y + body.height.saturating_sub(h) / 2,
            w,
            h,
        );
        let mut lines: Vec<Line> = text.lines().map(|l| Line::raw(l.to_string())).collect();
        let border = if matches!(m, Modal::Confirm { .. }) {
            Style::new().fg(Color::Red)
        } else {
            Style::new()
        };
        if let Some(v) = input {
            let shown = if masked {
                "*".repeat(v.chars().count())
            } else {
                visible_line(v, 4)
            };
            lines.push(Line::styled(
                format!("> {shown}"),
                Style::new().add_modifier(Modifier::BOLD),
            ));
        }
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(lines).block(
                Block::new()
                    .borders(Borders::ALL)
                    .title(title.as_str())
                    .border_style(border),
            ),
            r,
        );
        if let Some(v) = input {
            let shown = if masked {
                v.chars().count()
            } else {
                str_width(&visible_line(v, 4))
            };
            let y = r.y + h.saturating_sub(2);
            f.set_cursor_position(Position::new(
                (r.x + 3 + shown as u16).min(r.right().saturating_sub(2)),
                y,
            ));
        }
    }
}

/// Empty connection list: what to press, short, one line per action (fits a narrow pane).
fn empty_conns_lines() -> Vec<Line<'static>> {
    let key = Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan);
    let or = format!(" {} ", I18N.t("or"));
    vec![
        Line::raw(format!(" {}", I18N.t("No saved connections."))),
        Line::raw(""),
        Line::from(vec![Span::styled("  a", key), Span::raw(or.clone()), Span::styled("Enter", key), Span::raw(format!(" — {}", I18N.t("add a connection")))]),
        Line::from(vec![Span::styled("  q", key), Span::raw(or), Span::styled("Esc", key), Span::raw(format!(" — {}", I18N.t("quit")))]),
        Line::raw(""),
        Line::styled(
            format!(" {}", I18N.t("SQLite file: saysdf db <file.sqlite>")),
            Style::new().add_modifier(Modifier::DIM),
        ),
    ]
}

/// Terminal user var (OSC 1337 SetUserVar) for the saysdfTerm status bar (hotkey-statusbar 1.4):
/// `SAYSDF_TOOL=db`, `SAYSDF_DB_MODE=read|write`; an empty value resets it on exit.
fn set_user_var(name: &str, value: &str) {
    let mut out = io::stdout();
    let _ = write!(out, "]1337;SetUserVar={}={}", name, crate::cmd::md::base64(value.as_bytes()));
    let _ = out.flush();
}

pub fn run(conns: Vec<ConnectionConfig>, o: Options) -> io::Result<()> {
    set_user_var("SAYSDF_TOOL", "db");
    set_user_var("SAYSDF_DB_MODE", "read");
    let r = run_inner(conns, o);
    set_user_var("SAYSDF_DB_MODE", "");
    set_user_var("SAYSDF_TOOL", "");
    r
}

/// Presses only: crossterm on Windows sends both Press and Release (two events per key).
fn is_press(k: &KeyEvent) -> bool {
    k.kind == KeyEventKind::Press
}

fn run_inner(conns: Vec<ConnectionConfig>, mut o: Options) -> io::Result<()> {
    let mut app = App {
        saved: o.saved_ids.iter().cloned().collect(),
        conns,
        main_path: o.main_path.take(),
        max_rows: o.max_rows,
        save_history: o.save_history.take(),
        screen: Screen::Conns,
        sel: 0,
        cfg: None,
        pending: None,
        password: None,
        session_pw: HashMap::new(),
        worker: None,
        mode: WriteMode::default(),
        focus: Focus::Tree,
        tab: Tab::Structure,
        tree: Vec::new(),
        tree_sel: 0,
        tree_top: 0,
        desc: None,
        struct_top: 0,
        page: None,
        data: None,
        editor: Editor::default(),
        history: Vec::new(),
        hist_pos: None,
        results: None,
        hl: Highlighter::for_token("sql", 4),
        modal: None,
        status: String::new(),
        prev_esc: false,
        button: None,
        banner: None,
        grid_h: 10,
        as_of: None,
        last_sql: None,
    };
    if let Some(q) = o.query.take() {
        app.editor.set(&q);
        app.tab = Tab::Query;
    }
    crate::log::slog!("db: start, connections={}", app.conns.len());
    let mut guard = Some(Guard::enter()?);
    if let Some(cfg) = o.initial.take() {
        app.connect(cfg);
    }
    loop {
        app.pump();
        if let Some(g) = guard.as_mut() {
            g.term.draw(|f| app.draw(f))?;
        }
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let (ev, phys) = crate::keys::read()?;
        crate::log::slog!("db: event {ev:?} screen={:?}", app.screen);
        match ev {
            Event::Key(k) if is_press(&k) => match app.key(k, phys)? {
                Cmd::None => {}
                Cmd::Quit => break,
                Cmd::Form(idx) => {
                    drop(guard.take());
                    let r = app.run_form(idx);
                    guard = Some(Guard::enter()?);
                    if let Err(e) = r {
                        app.status = I18N.f("form: {error}", &[("error", &e)]);
                    }
                }
            },
            Event::Mouse(m) => app.mouse(m),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind)
    }

    #[test]
    fn only_press_is_handled() {
        assert!(is_press(&ev(KeyCode::Char('a'), KeyEventKind::Press)));
        assert!(!is_press(&ev(KeyCode::Char('a'), KeyEventKind::Release)));
        assert!(!is_press(&ev(KeyCode::Char('a'), KeyEventKind::Repeat)));
    }

    #[test]
    fn empty_state_says_what_to_press() {
        let text: String = empty_conns_lines()
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(text.contains("No saved"), "{text}");
        assert!(text.contains("a|") && text.contains("add"), "{text}");
        assert!(text.contains("q|") && text.contains("quit"), "{text}");
        // one line per action: fits a narrow split pane
        assert!(empty_conns_lines().iter().all(|l| l.width() <= 40));
    }

    fn joined(lines: Vec<Line<'static>>) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn empty_state_in_russian() {
        let _lang = saysdf_i18n::testing::use_lang("ru");
        let text = joined(empty_conns_lines());
        assert!(text.contains("Сохранённых подключений нет"), "{text}"); // i18n-allow-cyrillic
        assert!(text.contains("добавить") && text.contains("выход"), "{text}"); // i18n-allow-cyrillic
        assert!(empty_conns_lines().iter().all(|l| l.width() <= 40));
    }

    #[test]
    fn write_button_and_confirmation_follow_language() {
        // The write-mode texts are safety-critical: the English ones are fixed by the
        // database-write-mode spec, the Russian ones must not change.
        assert_eq!(I18N.t("[ Allow writes… ]"), "[ Allow writes… ]");
        assert_eq!(I18N.t("DANGEROUS MODE"), "DANGEROUS MODE");
        let en = I18N.f(
            "Queries will be able to modify data in “{db}”.\nTo enable, type the database name and press Enter (Esc to cancel).",
            &[("db", &"app.db")],
        );
        assert!(en.contains("modify data in “app.db”"), "{en}");
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert_eq!(I18N.t("[ Allow writes… ]"), "[ Разрешить запись… ]"); // i18n-allow-cyrillic
        assert_eq!(I18N.t("[ Leave dangerous mode ]"), "[ Выйти из опасного режима ]"); // i18n-allow-cyrillic
        assert_eq!(I18N.t("DANGEROUS MODE"), "ОПАСНЫЙ РЕЖИМ"); // i18n-allow-cyrillic
        let ru = I18N.f(
            "Queries will be able to modify data in “{db}”.\nTo enable, type the database name and press Enter (Esc to cancel).",
            &[("db", &"app.db")],
        );
        assert!(ru.contains("изменять данные в «app.db»"), "{ru}"); // i18n-allow-cyrillic
    }
}
