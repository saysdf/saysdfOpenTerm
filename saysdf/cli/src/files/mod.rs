//! `saysdf files [<dir>]` — file browser (change file-browser). Model — [`tree`], operations —
//! [`ops`], terminal — the shared form session (`form.rs`: tree, input and confirmation forms in
//! one alt-screen). GUI actions (open in the viewer, folder as the cwd of a new pane, copy the
//! path) go through the sidebar channel (`crate::sidebar`, kind `action`); git marks are computed
//! on a background thread by the shared `saysdf git-status` code (`status_map`); the root follows
//! the cwd of the active pane through a file written by the GUI (`--cwd-file`, watched by `watch.rs`).
//!
//! Its own loop rather than `pager::run`: the pager takes left/right (horizontal scroll), `n`/`N`/`r`
//! and gives no tick for background results (see the report: a proposal to extend `pager::Content`).

pub mod ops;
pub mod tree;

use crate::cmd::git_status::{self, Mark};
use crate::form::{Field, Session};
use crate::sidebar;
use crate::watch::FileWatch;
use saysdf_keys::Key;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use saysdf_render::sanitize::{str_width, visible_line};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;
use tree::{Kind, Tree};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-files");

/// Limit on the number of roots under which the GUI accepts actions (same in `saysdf.files`).
const MAX_ROOTS: usize = 32;

/// Key hints in the status line.
fn help() -> std::borrow::Cow<'static, str> {
    I18N.t("Enter open  ←→ collapse/expand  Bksp up  / filter  . hidden  o cwd  y path  n/N create  r rename  d delete  p pin  q quit")
}

type GitResult = (u64, Option<(PathBuf, git_status::Marks)>);

pub struct App {
    tree: Tree,
    /// Position in the list of visible nodes.
    cursor: usize,
    top: usize,
    height: usize,
    filter_mode: bool,
    status: String,
    pinned: bool,
    /// Roots known to the GUI (opening and cwd sync): no actions are sent outside them.
    roots: Vec<PathBuf>,
    token: Option<String>,
    seq: u64,
    git_tx: Sender<GitResult>,
    git_rx: Receiver<GitResult>,
    git_gen: u64,
    git_busy: bool,
    git_again: bool,
    git_watch: Option<FileWatch>,
    cwd_watch: Option<(PathBuf, FileWatch)>,
}

enum Flow {
    Go,
    Quit,
}

fn path_bytes(p: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        p.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        p.to_string_lossy().into_owned().into_bytes()
    }
}

/// Argument of a `files.*` action: `<seq>:<base64 of the path>` (seq tells repeated identical actions apart).
pub fn action_arg(seq: u64, path: &Path) -> String {
    format!("{seq}:{}", crate::cmd::md::base64(&path_bytes(path)))
}

/// First line of the cwd file (the GUI writes the whole path); limit 4 KB.
fn read_cwd_file(p: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(p).ok()?;
    if meta.len() > 4096 {
        return None;
    }
    let s = std::fs::read_to_string(p).ok()?;
    let s = s.lines().next()?.trim();
    if s.is_empty() {
        return None;
    }
    let pb = PathBuf::from(s);
    (pb.is_absolute() && pb.is_dir()).then_some(pb)
}

impl App {
    pub fn new(root: &Path, token: Option<String>, cwd_file: Option<PathBuf>) -> io::Result<Self> {
        let root = ops::canonical(root)?;
        let tree = Tree::new(&root, false)?;
        let (git_tx, git_rx) = channel();
        let cwd_watch = cwd_file.map(|p| {
            let w = FileWatch::new(&p);
            (p, w)
        });
        let mut app = Self {
            tree,
            cursor: 0,
            top: 0,
            height: 1,
            filter_mode: false,
            status: String::new(),
            pinned: false,
            roots: vec![root],
            token,
            seq: 0,
            git_tx,
            git_rx,
            git_gen: 0,
            git_busy: false,
            git_again: false,
            git_watch: None,
            cwd_watch,
        };
        app.spawn_git();
        Ok(app)
    }

    fn spawn_git(&mut self) {
        if self.git_busy {
            self.git_again = true;
            return;
        }
        self.git_busy = true;
        let gen = self.git_gen;
        let tx = self.git_tx.clone();
        let root = self.tree.root.clone();
        std::thread::spawn(move || {
            let r = git_status::status_map(&root).ok().flatten();
            let _ = tx.send((gen, r));
        });
    }

    /// Background results and watching (every ~100 ms).
    fn tick(&mut self) {
        while let Ok((gen, r)) = self.git_rx.try_recv() {
            self.git_busy = false;
            if gen != self.git_gen {
                self.git_again = true;
                continue;
            }
            match r {
                Some((repo, map)) => {
                    self.tree.git = map;
                    let index = repo.join(".git").join("index");
                    if self.git_watch.is_none() && index.is_file() {
                        self.git_watch = Some(FileWatch::new(&index));
                    }
                }
                None => {
                    self.tree.git.clear();
                    self.git_watch = None;
                }
            }
        }
        if std::mem::take(&mut self.git_again) {
            self.spawn_git();
        }
        if self.git_watch.as_mut().is_some_and(|w| w.poll()) {
            self.spawn_git();
        }
        let changed = match &mut self.cwd_watch {
            Some((p, w)) => {
                if w.poll() {
                    read_cwd_file(p)
                } else {
                    None
                }
            }
            None => None,
        };
        if let Some(dir) = changed {
            self.add_root(&dir);
            if !self.pinned && dir != self.tree.root {
                self.change_root(&dir);
            }
        }
    }

    fn add_root(&mut self, dir: &Path) {
        if !self.roots.iter().any(|r| r == dir) {
            if self.roots.len() >= MAX_ROOTS {
                self.roots.remove(1); // the first one is the root the browser was opened with
            }
            self.roots.push(dir.to_path_buf());
        }
    }

    fn change_root(&mut self, dir: &Path) {
        let dir = ops::canonical(dir).unwrap_or_else(|_| dir.to_path_buf());
        match self.tree.set_root(&dir) {
            Ok(()) => {
                self.cursor = 0;
                self.top = 0;
                self.tree.filter.clear();
                self.git_gen += 1;
                self.git_watch = None;
                self.spawn_git();
                self.status.clear();
            }
            Err(e) => self.status = format!("{}: {e}", dir.display()),
        }
    }

    /// Re-read the tree; if the root is gone, use the nearest existing ancestor.
    fn refresh(&mut self) {
        let keep = self.current().map(|i| self.tree.nodes[i].path.clone());
        if self.tree.refresh().is_err() {
            let mut p = self.tree.root.clone();
            while let Some(par) = p.parent().map(Path::to_path_buf) {
                if self.tree.set_root(&par).is_ok() {
                    break;
                }
                p = par;
            }
        }
        if let Some(k) = keep {
            self.select(&k);
        }
        self.clamp();
        self.spawn_git();
    }

    fn visible(&self) -> Vec<usize> {
        self.tree.visible()
    }

    /// Index of the node under the cursor.
    fn current(&self) -> Option<usize> {
        self.visible().get(self.cursor).copied()
    }

    fn select(&mut self, path: &Path) {
        if let Some(pos) = self.visible().iter().position(|&i| self.tree.nodes[i].path == path) {
            self.cursor = pos;
        }
    }

    fn clamp(&mut self) {
        let n = self.visible().len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
        if self.cursor < self.top {
            self.top = self.cursor;
        }
        if self.cursor >= self.top + self.height {
            self.top = self.cursor + 1 - self.height;
        }
    }

    fn move_by(&mut self, d: isize) {
        self.cursor = self.cursor.saturating_add_signed(d);
        self.clamp();
    }

    fn send_action(&mut self, id: &str, path: &Path) {
        let Some(token) = self.token.clone() else {
            self.status = I18N.t("GUI actions are only available in the saysdfTerm sidebar").into_owned();
            return;
        };
        let path = match ops::under_root(&self.roots, path) {
            Ok(p) => p,
            Err(e) => {
                self.status = visible_line(&e, 4);
                return;
            }
        };
        self.seq += 1;
        let payload = format!("{id}:{}", action_arg(self.seq, &path));
        self.status = match sidebar::send(&token, "action", &payload) {
            Ok(()) => match id {
                "files.copy_path" => I18N.t("path copied").into_owned(),
                "files.cd_pane" => I18N.t("opening pane…").into_owned(),
                _ => I18N.t("opening viewer…").into_owned(),
            },
            Err(e) => visible_line(&e.to_string(), 4),
        };
    }

    fn key(&mut self, k: KeyEvent, phys: Option<Key>, s: &mut Session) -> io::Result<Flow> {
        if self.filter_mode {
            match k.code {
                KeyCode::Esc => {
                    self.filter_mode = false;
                    self.tree.filter.clear();
                }
                KeyCode::Enter | KeyCode::Down | KeyCode::Up => self.filter_mode = false,
                KeyCode::Backspace => {
                    self.tree.filter.pop();
                }
                KeyCode::Char(c) if !c.is_control() && !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.tree.filter.push(c)
                }
                _ => {}
            }
            self.cursor = 0;
            self.top = 0;
            return Ok(Flow::Go);
        }
        let is = |spec| crate::keys::is(&k, phys, spec);
        if is("Ctrl+c")? {
            return Ok(Flow::Quit);
        }
        let page = self.height.max(2) as isize - 1;
        let cur = self.current();
        match k.code {
            _ if is("q")? => return Ok(Flow::Quit),
            KeyCode::Esc if !self.tree.filter.is_empty() => self.tree.filter.clear(),
            KeyCode::Esc => return Ok(Flow::Quit),
            _ if k.code == KeyCode::Down || is("j")? => self.move_by(1),
            _ if k.code == KeyCode::Up || is("k")? => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            _ if k.code == KeyCode::Home || is("g")? => self.move_by(isize::MIN / 2),
            _ if k.code == KeyCode::End || is("G")? => self.move_by(isize::MAX / 2),
            _ if k.code == KeyCode::Right || is("l")? => {
                if let Some(i) = cur {
                    if !self.tree.expand(i) && self.tree.nodes[i].expanded {
                        self.move_by(1);
                    }
                    self.denied_status(i);
                }
            }
            _ if k.code == KeyCode::Left || is("h")? => {
                if let Some(i) = cur {
                    if !self.tree.collapse(i) {
                        if let Some(p) = self.tree.parent(i) {
                            let path = self.tree.nodes[p].path.clone();
                            self.select(&path);
                        }
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(i) = cur {
                    let n = self.tree.nodes[i].clone();
                    match n.kind {
                        Kind::Dir => {
                            if !self.tree.collapse(i) {
                                self.tree.expand(i);
                                self.denied_status(i);
                            }
                        }
                        Kind::File | Kind::Link { dir: false } => self.send_action("files.open", &n.path),
                        Kind::Link { dir: true } => {
                            self.status = I18N.t("a link to a folder is not expanded (o — open as cwd)").into_owned()
                        }
                        Kind::Truncated => {}
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(par) = self.tree.root.parent().map(Path::to_path_buf) {
                    let old = self.tree.root.clone();
                    self.change_root(&par);
                    self.select(&old);
                    self.clamp();
                }
            }
            _ if is("/")? => {
                self.filter_mode = true;
                self.tree.filter.clear();
            }
            _ if is(".")? => {
                if let Err(e) = self.tree.toggle_hidden() {
                    self.status = visible_line(&e.to_string(), 4);
                }
                self.clamp();
            }
            _ if is("p")? => {
                self.pinned = !self.pinned;
                self.status = if self.pinned {
                    I18N.t("root pinned")
                } else {
                    I18N.t("root follows the pane")
                }
                .into_owned();
            }
            KeyCode::F(5) => {
                self.refresh();
                self.status = I18N.t("reloaded").into_owned();
            }
            _ if is("o")? => {
                if let Some(i) = cur {
                    let n = &self.tree.nodes[i];
                    let dir = match n.kind {
                        Kind::Dir | Kind::Link { dir: true } => n.path.clone(),
                        _ => n.path.parent().map_or_else(|| self.tree.root.clone(), Path::to_path_buf),
                    };
                    self.send_action("files.cd_pane", &dir);
                } else {
                    let root = self.tree.root.clone();
                    self.send_action("files.cd_pane", &root);
                }
            }
            _ if is("y")? => {
                if let Some(i) = cur {
                    let p = self.tree.nodes[i].path.clone();
                    if self.tree.nodes[i].kind != Kind::Truncated {
                        self.send_action("files.copy_path", &p);
                    }
                }
            }
            _ if is("n")? => self.create(s, false)?,
            _ if is("N")? => self.create(s, true)?,
            _ if is("r")? => self.rename(s)?,
            _ if k.code == KeyCode::Delete || is("d")? => self.delete(s)?,
            _ => {}
        }
        Ok(Flow::Go)
    }

    fn denied_status(&mut self, i: usize) {
        if self.tree.nodes.get(i).is_some_and(|n| n.denied) {
            self.status = I18N.t("no access to the folder").into_owned();
        }
    }

    /// Folder to create in: the expanded folder under the cursor, otherwise the parent folder of the item under the cursor.
    fn target_dir(&self) -> PathBuf {
        match self.current().map(|i| &self.tree.nodes[i]) {
            Some(n) if n.kind == Kind::Dir && n.expanded => n.path.clone(),
            Some(n) => n.path.parent().map_or_else(|| self.tree.root.clone(), Path::to_path_buf),
            None => self.tree.root.clone(),
        }
    }

    fn create(&mut self, s: &mut Session, is_dir: bool) -> io::Result<()> {
        let dir = self.target_dir();
        let mut fields = [Field::new(&I18N.t("Name"), "", "")];
        let mut made = None;
        let shown_dir = visible_line(&dir.to_string_lossy(), 4);
        let title = if is_dir {
            I18N.f("Create folder in {dir}", &[("dir", &shown_dir)])
        } else {
            I18N.f("Create file in {dir}", &[("dir", &shown_dir)])
        };
        let ok = s.edit(&title, &mut fields, |f| match ops::create(&dir, &f[0].value, is_dir) {
            Ok(p) => {
                made = Some(p);
                true
            }
            Err(e) => {
                f[0].error = Some(e);
                false
            }
        })?;
        if ok {
            self.refresh();
            if let Some(p) = made {
                self.expand_to(&p);
                self.status = I18N.f("created: {path}", &[("path", &visible_line(&p.to_string_lossy(), 4))]);
            }
        }
        Ok(())
    }

    /// Expand the ancestors of the path and put the cursor on it.
    fn expand_to(&mut self, p: &Path) {
        let mut chain: Vec<PathBuf> = p.ancestors().skip(1).take_while(|a| a.starts_with(&self.tree.root) && *a != self.tree.root).map(Path::to_path_buf).collect();
        chain.reverse();
        for a in chain {
            if let Some(i) = self.tree.find(&a) {
                self.tree.expand(i);
            }
        }
        self.select(p);
        self.clamp();
    }

    fn rename(&mut self, s: &mut Session) -> io::Result<()> {
        let Some(i) = self.current() else { return Ok(()) };
        let n = self.tree.nodes[i].clone();
        if n.kind == Kind::Truncated {
            return Ok(());
        }
        let mut fields = [Field::new(&I18N.t("New name"), "", &n.name)];
        let mut done = None;
        let title = I18N.f("Rename {path}", &[("path", &visible_line(&n.path.to_string_lossy(), 4))]);
        let ok = s.edit(&title, &mut fields, |f| match ops::rename(&n.path, &f[0].value) {
            Ok(p) => {
                done = Some(p);
                true
            }
            Err(e) => {
                f[0].error = Some(e);
                false
            }
        })?;
        if ok {
            self.refresh();
            if let Some(p) = done {
                self.select(&p);
                self.status = I18N.t("renamed").into_owned();
            }
        }
        Ok(())
    }

    fn delete(&mut self, s: &mut Session) -> io::Result<()> {
        let Some(i) = self.current() else { return Ok(()) };
        let n = self.tree.nodes[i].clone();
        if n.kind == Kind::Truncated {
            return Ok(());
        }
        let shown = visible_line(&n.path.to_string_lossy(), 4);
        let kind = match ops::delete_kind(&n.path) {
            Ok(k) => k,
            Err(e) => {
                self.status = visible_line(&e.to_string(), 4);
                return Ok(());
            }
        };
        let args: [(&str, &dyn std::fmt::Display); 1] = [("path", &shown)];
        let question = match kind {
            ops::DeleteKind::Link => I18N.f("Permanently delete the link (the target is not touched)?\n{path}", &args),
            ops::DeleteKind::File => I18N.f("Permanently delete the file?\n{path}", &args),
            ops::DeleteKind::EmptyDir => I18N.f("Permanently delete the empty folder?\n{path}", &args),
            ops::DeleteKind::NonEmptyDir => I18N.f("Permanently delete the folder WITH ALL ITS CONTENTS?\n{path}", &args),
        };
        if !s.confirm(&question)? {
            self.status = I18N.t("deletion cancelled").into_owned();
            return Ok(());
        }
        let typed = if kind == ops::DeleteKind::NonEmptyDir {
            let mut fields = [Field::new(&I18N.t("Folder name"), &n.name, "")];
            let title = I18N.f("The folder is not empty. Type its name to delete it: {name}", &[("name", &visible_line(&n.name, 4))]);
            if !s.edit(&title, &mut fields, |_| true)? {
                self.status = I18N.t("deletion cancelled").into_owned();
                return Ok(());
            }
            Some(fields[0].value.clone())
        } else {
            None
        };
        self.status = match ops::delete(&n.path, kind, typed.as_deref()) {
            Ok(()) => I18N.f("deleted: {path}", &[("path", &shown)]),
            Err(e) => visible_line(&e, 4),
        };
        self.refresh();
        Ok(())
    }

    fn draw(&mut self, f: &mut ratatui::Frame) {
        let area = f.area();
        let width = area.width as usize;
        self.height = (area.height as usize).saturating_sub(2).max(1);
        self.clamp();
        let vis = self.visible();
        let mut lines = Vec::with_capacity(self.height + 2);
        let mut head = visible_line(&self.tree.root.to_string_lossy(), 4);
        let flags = format!(
            "{}{}",
            if self.pinned { I18N.t(" [pinned]") } else { "".into() },
            if self.tree.show_hidden { " [.]" } else { "" }
        );
        let room = width.saturating_sub(str_width(&flags) + 1);
        if str_width(&head) > room {
            let mut cut: String = head.chars().rev().take(room.saturating_sub(1)).collect::<Vec<_>>().into_iter().rev().collect();
            while str_width(&cut) > room.saturating_sub(1) && !cut.is_empty() {
                cut.remove(0);
            }
            head = format!("\u{2026}{cut}");
        }
        lines.push(Line::from(vec![
            Span::styled(format!(" {head}"), Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(flags, Style::new().fg(Color::Yellow)),
        ]));
        for (row, &i) in vis.iter().enumerate().skip(self.top).take(self.height) {
            let n = &self.tree.nodes[i];
            let mark = self.tree.mark(i);
            let (msym, mstyle) = match mark {
                Some(Mark::Modified) => ('M', Style::new().fg(Color::Yellow)),
                Some(Mark::Added) => ('A', Style::new().fg(Color::Green)),
                Some(Mark::Untracked) => ('?', Style::new().fg(Color::Magenta)),
                Some(Mark::Deleted) => ('D', Style::new().fg(Color::Red)),
                Some(Mark::Conflict) => ('!', Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)),
                None => (' ', Style::new()),
            };
            let indent = (n.depth * 2).min(width / 2);
            let (icon, nstyle) = match n.kind {
                Kind::Dir if n.expanded => ("\u{25be} ", Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                Kind::Dir => ("\u{25b8} ", Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                Kind::Link { .. } => ("\u{21aa} ", Style::new().fg(Color::Cyan)),
                Kind::File => ("  ", Style::new()),
                Kind::Truncated => ("  ", Style::new().add_modifier(Modifier::DIM)),
            };
            let nstyle = if mark.is_some() && n.kind == Kind::File { mstyle } else { nstyle };
            let mut spans = vec![
                Span::styled(format!("{msym} "), mstyle),
                Span::raw(" ".repeat(indent)),
                Span::styled(icon, nstyle),
                Span::styled(visible_line(&n.name, 4), nstyle),
            ];
            if n.denied {
                spans.push(Span::styled(I18N.t(" (no access)"), Style::new().fg(Color::Red)));
            }
            if row == self.cursor {
                // REVERSED keeps the cursor visible in any theme (light or dark)
                for sp in &mut spans {
                    sp.style = sp.style.add_modifier(Modifier::REVERSED);
                }
                let used: usize = spans.iter().map(|s| str_width(&s.content)).sum();
                spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), Style::new().add_modifier(Modifier::REVERSED)));
            }
            lines.push(Line::from(spans));
        }
        if vis.is_empty() {
            let t = if self.tree.filter.is_empty() { I18N.t("  (empty)") } else { I18N.t("  (no matches)") };
            lines.push(Line::styled(t, Style::new().add_modifier(Modifier::DIM)));
        }
        while lines.len() < self.height + 1 {
            lines.push(Line::raw(""));
        }
        let status = if self.filter_mode {
            format!("/{}", visible_line(&self.tree.filter, 4))
        } else if !self.tree.filter.is_empty() && self.status.is_empty() {
            I18N.f(" filter: {text}  (Esc to clear)", &[("text", &visible_line(&self.tree.filter, 4))])
        } else if self.status.is_empty() {
            format!(" {}", help())
        } else {
            format!(" {}", self.status)
        };
        lines.push(Line::styled(status, Style::new().add_modifier(Modifier::REVERSED)));
        f.render_widget(Paragraph::new(lines), area);
    }
}

/// Event loop until quit.
pub fn run(root: &Path, token: Option<String>, cwd_file: Option<PathBuf>) -> io::Result<()> {
    let mut app = App::new(root, token, cwd_file)?;
    let mut session = Session::enter()?;
    loop {
        session.draw(|f| app.draw(f))?;
        if event::poll(Duration::from_millis(100))? {
            let (ev, phys) = crate::keys::read()?;
            match ev {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    // the message lives until the next key
                    app.status.clear();
                    if let Flow::Quit = app.key(k, phys, &mut session)? {
                        return Ok(());
                    }
                }
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::ScrollDown => app.move_by(3),
                    MouseEventKind::ScrollUp => app.move_by(-3),
                    _ => {}
                },
                _ => {}
            }
        }
        app.tick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_follow_language() {
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert!(help().starts_with("Enter open"));
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert!(help().starts_with("Enter открыть")); // i18n-allow-cyrillic
    }

    #[test]
    fn action_arg_format() {
        let a = action_arg(7, Path::new("/a:b"));
        assert_eq!(a, format!("7:{}", crate::cmd::md::base64(b"/a:b")));
    }

    #[test]
    fn cwd_file_parsing() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("files-x.cwd");
        std::fs::write(&f, format!("{}\n", d.path().display())).unwrap();
        assert_eq!(read_cwd_file(&f), Some(d.path().to_path_buf()));
        std::fs::write(&f, "relative/dir").unwrap();
        assert_eq!(read_cwd_file(&f), None);
        std::fs::write(&f, "x".repeat(5000)).unwrap();
        assert_eq!(read_cwd_file(&f), None);
    }
}
