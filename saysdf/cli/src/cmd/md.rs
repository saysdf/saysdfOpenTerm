//! `saysdf md <path>`: Markdown preview (change markdown-preview, design D3–D5) on top of the shared pager.
//! Parsing and layout come from `saysdf_render::markdown`; auto-reload is the pager's shared `watch`;
//! images use OSC 1337 for local files only; external links are never opened.

use crate::pager::{self, Content, Nav};
use saysdf_keys::Key;
use clap::Args;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;
use saysdf_render::doc::Doc as ViewDoc;
use saysdf_render::markdown::{self, Layout, LayoutOptions};
use saysdf_render::mermaid::{self, Zoom};
use saysdf_render::sanitize::{visible_line, DEFAULT_TAB_WIDTH};
use saysdf_render::source::DEFAULT_MAX_SIZE;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-view");

#[derive(Args)]
pub struct MdArgs {
    /// Markdown file
    pub path: PathBuf,
    /// Source line (1-based): the preview scrolls to the nearest block
    #[arg(long)]
    pub line: Option<usize>,
    /// Compatibility with `view` (ignored)
    #[arg(long, default_value_t = 1, hide = true)]
    pub col: usize,
    /// ASCII markers and frames instead of Unicode
    #[arg(long)]
    pub ascii: bool,
    /// Do not show images (caption only)
    #[arg(long)]
    pub no_images: bool,
    /// Editor command for the `e` key: a JSON array, `{path}`/`{line}`/`{col}` are substituted
    #[arg(long)]
    pub editor_cmd: Option<String>,
}

/// Whether the terminal supports OSC 1337 (saysdfTerm inherits upstream's `TERM_PROGRAM`).
fn terminal_has_images() -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "WezTerm" || v == "iTerm.app")
}

struct MdContent {
    path: PathBuf,
    model: markdown::Doc,
    opts: LayoutOptions,
    lay: Layout,
    /// Selected link (Tab/Shift+Tab).
    link: Option<usize>,
    /// Navigation history: (file, source line).
    history: Vec<(PathBuf, usize)>,
    /// The last window the images were drawn for: (top, height, x).
    drawn: Option<(usize, usize, usize)>,
    /// Current pager window: (top, hscroll), for zooming mermaid blocks.
    view: (usize, usize),
}

fn read_md(path: &Path) -> io::Result<String> {
    if path.is_dir() {
        return Err(super::view::dir_error());
    }
    let meta = std::fs::metadata(path)?;
    if meta.len() > DEFAULT_MAX_SIZE {
        return Err(io::Error::other(I18N.t("the file is larger than 16 MiB, open it with `saysdf view`").into_owned()));
    }
    Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned())
}

fn title_of(path: &Path) -> String {
    visible_line(&path.to_string_lossy(), DEFAULT_TAB_WIDTH)
}

impl MdContent {
    fn open(path: &Path, opts: LayoutOptions) -> io::Result<Self> {
        let mut c = Self {
            path: path.to_path_buf(),
            model: markdown::Doc::default(),
            opts,
            lay: Layout::default(),
            link: None,
            history: Vec::new(),
            drawn: None,
            view: (0, 0),
        };
        c.load(path)?;
        Ok(c)
    }

    fn load(&mut self, path: &Path) -> io::Result<()> {
        let src = read_md(path)?;
        self.path = path.to_path_buf();
        self.opts.base_dir = path.parent().map(Path::to_path_buf);
        self.model = markdown::parse(&src);
        self.relayout();
        self.link = None;
        Ok(())
    }

    fn relayout(&mut self) {
        self.lay = markdown::layout(&self.model, &self.opts);
        self.drawn = None;
    }

    fn open_link(&mut self, pos: usize) -> Nav {
        let Some(i) = self.link else {
            return Nav::Status(I18N.t("Tab to select a link").into_owned());
        };
        let Some(l) = self.lay.links.get(i).cloned() else { return Nav::None };
        let shown = visible_line(&l.url, DEFAULT_TAB_WIDTH);
        if let Some(anchor) = l.url.strip_prefix('#') {
            return match self.lay.heading_by_slug(anchor) {
                Some(h) => Nav::Goto(h.line),
                None => Nav::Status(I18N.f("no heading #{anchor}", &[("anchor", &visible_line(anchor, DEFAULT_TAB_WIDTH))])),
            };
        }
        match markdown::layout::local_target(&l.url, self.opts.base_dir.as_deref()) {
            Some((path, frag)) if is_markdown(&path) => {
                let back = (self.path.clone(), self.lay.doc.source_line(pos));
                if let Err(e) = self.load(&path) {
                    return Nav::Status(visible_line(&format!("{shown}: {e}"), DEFAULT_TAB_WIDTH));
                }
                self.history.push(back);
                let line = frag
                    .and_then(|f| self.lay.heading_by_slug(&f).map(|h| h.line))
                    .unwrap_or(0);
                Nav::Open { title: title_of(&path), path, line }
            }
            // External URLs and other files are shown only (design D5)
            _ => Nav::Status(I18N.f("link (not opened): {url}", &[("url", &shown)])),
        }
    }

    fn back(&mut self) -> Nav {
        let Some((path, src)) = self.history.pop() else {
            return Nav::Status(I18N.t("history is empty").into_owned());
        };
        match self.load(&path) {
            Ok(()) => Nav::Open { title: title_of(&path), line: self.lay.doc.view_line(src), path },
            Err(e) => Nav::Status(visible_line(&I18N.f("read error: {error}", &[("error", &e.to_string())]), DEFAULT_TAB_WIDTH)),
        }
    }
}

fn is_markdown(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// Base64 (RFC 4648) for OSC 1337, without an external dependency.
pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

impl Content for MdContent {
    fn len(&self) -> usize {
        self.lay.doc.len()
    }

    fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
        let d = &self.lay.doc;
        d.lines[from.min(d.len())..to.min(d.len())].to_vec()
    }

    fn doc(&self) -> Option<&ViewDoc> {
        Some(&self.lay.doc)
    }

    fn reload(&mut self) -> io::Result<Option<String>> {
        if self.path.as_os_str().is_empty() {
            // Built-in text (`saysdf help`): nothing to reload.
            return Ok(None);
        }
        let p = self.path.clone();
        self.load(&p)?;
        Ok(Some(I18N.t("reloaded").into_owned()))
    }

    fn set_width(&mut self, width: usize) {
        if width != self.opts.width {
            self.opts.width = width.max(10);
            self.relayout();
        }
    }

    fn key(&mut self, k: &KeyEvent, phys: Option<Key>, pos: usize) -> io::Result<Nav> {
        let is = |spec| crate::keys::is(k, phys, spec);

        let lay = &self.lay;
        Ok(match k.code {
            _ if is("]")? => match lay.headings.iter().find(|h| h.line > pos) {
                Some(h) => Nav::Goto(h.line),
                None => Nav::Status(I18N.t("last heading").into_owned()),
            },
            _ if is("[")? => match lay.headings.iter().rev().find(|h| h.line < pos) {
                Some(h) => Nav::Goto(h.line),
                None => Nav::Status(I18N.t("first heading").into_owned()),
            },
            KeyCode::Tab | KeyCode::BackTab if !lay.links.is_empty() => {
                let back = k.code == KeyCode::BackTab || k.modifiers.contains(KeyModifiers::SHIFT);
                let n = lay.links.len();
                let i = match (self.link, back) {
                    (Some(i), false) => (i + 1) % n,
                    (Some(i), true) => (i + n - 1) % n,
                    (None, false) => lay.links.iter().position(|l| l.line >= pos).unwrap_or(0),
                    (None, true) => lay.links.iter().rposition(|l| l.line <= pos).unwrap_or(n - 1),
                };
                self.link = Some(i);
                let l = &lay.links[i];
                let status = I18N.f(
                    "link {i}/{n}: {url}  (Enter to open)",
                    &[("i", &(i + 1).to_string()), ("n", &n.to_string()), ("url", &visible_line(&l.url, DEFAULT_TAB_WIDTH))],
                );
                Nav::Select(l.line, status)
            }
            KeyCode::Tab | KeyCode::BackTab => Nav::Status(I18N.t("no links").into_owned()),
            KeyCode::Enter => self.open_link(pos),
            KeyCode::Backspace => self.back(),
            // Shared zoom of the document's mermaid blocks (mermaid-preview D7); the position is kept by source line
            _ if is("+")? || is("=")? || is("-")? || is("z")? => {
                let z = if is("-")? {
                    (self.opts.mermaid_zoom - 1).max(-1)
                } else if is("z")? {
                    0
                } else {
                    (self.opts.mermaid_zoom + 1).min(1)
                };
                let name = Zoom::from_step(z).name();
                if z == self.opts.mermaid_zoom {
                    return Ok(Nav::Status(I18N.f("mermaid zoom: {name}", &[("name", &name)])));
                }
                let src = self.lay.doc.source_line(self.view.0);
                self.opts.mermaid_zoom = z;
                self.relayout();
                Nav::Scroll { top: self.lay.doc.view_line(src), hscroll: self.view.1, status: I18N.f("mermaid zoom: {name}", &[("name", &name)]) }
            }
            _ => Nav::None,
        })
    }

    fn viewport(&mut self, top: usize, hscroll: usize, _height: usize, _width: usize) {
        self.view = (top, hscroll);
    }

    fn overlay(&mut self, out: &mut dyn Write, top: usize, height: usize, x: usize, hscroll: usize) {
        if !self.opts.images || self.lay.images.is_empty() || hscroll > 0 {
            self.drawn = None;
            return;
        }
        let key = (top, height, x);
        if self.drawn == Some(key) {
            return;
        }
        self.drawn = Some(key);
        let mut buf = Vec::new();
        buf.extend_from_slice(b"\x1b7");
        for im in &self.lay.images {
            // Only fully visible images; partially visible ones stay empty space
            if im.line < top || im.line + im.rows > top + height {
                continue;
            }
            let Ok(data) = std::fs::read(&im.path) else { continue };
            let row = im.line - top + 1;
            let col = x + im.col + 1;
            let _ = write!(
                buf,
                "\x1b[{row};{col}H\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=1:{}\x07",
                data.len(),
                im.cols,
                im.rows,
                base64(&data)
            );
        }
        buf.extend_from_slice(b"\x1b8");
        let _ = out.write_all(&buf);
        let _ = out.flush();
    }
}

/// Show built-in Markdown text (no file): `saysdf help`. No auto-reload and no editor;
/// relative file links are not opened (there is no base folder).
pub fn run_text(title: &str, text: &str) -> i32 {
    if !io::stdout().is_terminal() {
        print!("{text}");
        return 0;
    }
    let opts = LayoutOptions {
        width: 80,
        ascii: false,
        images: false,
        base_dir: None,
        mermaid: Some(mermaid::markdown_renderer),
        mermaid_zoom: 0,
    };
    let mut content = MdContent {
        path: PathBuf::new(),
        model: markdown::parse(text),
        opts,
        lay: Layout::default(),
        link: None,
        history: Vec::new(),
        drawn: None,
        view: (0, 0),
    };
    content.relayout();
    let opts = pager::Options {
        path: None,
        title: visible_line(title, DEFAULT_TAB_WIDTH),
        line: None,
        col: 1,
        editor: None,
        watch: false,
        status: String::new(),
    };
    match pager::run(content, opts) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}", I18N.f("saysdf help: terminal: {error}", &[("error", &e.to_string())]));
            1
        }
    }
}

pub fn run(args: MdArgs) -> i32 {
    let abs = std::path::absolute(&args.path).unwrap_or_else(|_| args.path.clone());
    let title = title_of(&abs);
    let editor = match args.editor_cmd.as_deref().map(crate::editor::parse).transpose() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("saysdf md: {}", visible_line(&e, DEFAULT_TAB_WIDTH));
            return 2;
        }
    };
    let images = !args.no_images && terminal_has_images();
    let opts = LayoutOptions {
        width: 80,
        ascii: args.ascii,
        images,
        base_dir: None,
        mermaid: Some(mermaid::markdown_renderer),
        mermaid_zoom: 0,
    };
    let content = match MdContent::open(&abs, opts) {
        Ok(c) => c,
        Err(e) => {
            let msg = super::view::open_error_text(&e);
            eprintln!("saysdf md: {title}: {}", visible_line(&msg, DEFAULT_TAB_WIDTH));
            return 1;
        }
    };
    if !io::stdout().is_terminal() {
        eprintln!("{}", I18N.t("saysdf md: stdout is not a terminal"));
        return 1;
    }
    let status = if images || args.no_images { String::new() } else { I18N.t("images: captions only (terminal without OSC 1337)").into_owned() };
    let opts = pager::Options {
        path: Some(abs),
        title,
        line: args.line.map(|l| l.saturating_sub(1)),
        col: 1,
        editor,
        watch: true,
        status,
    };
    match pager::run(content, opts) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}", I18N.f("saysdf md: terminal: {error}", &[("error", &e.to_string())]));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
