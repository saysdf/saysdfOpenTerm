//! `saysdf mermaid <path>`: Mermaid as character graphics (change mermaid-preview, design D7) on top of
//! the shared pager: horizontal/vertical scroll, zoom with `+`/`-`/`z` and Ctrl+wheel keeping an anchor
//! node at the view center, auto-reload through `watch`. Unsupported input shows the source with a note.
//! `--print` (or a non-terminal stdout) prints to stdout without interaction.

use crate::pager::{self, Content, Nav};
use saysdf_keys::Key;
use clap::Args;
use ratatui::crossterm::event::KeyEvent;
use ratatui::text::Line;
use saysdf_render::doc::Doc;
use saysdf_render::mermaid::{self, Anchor, Opts, Zoom};
use saysdf_render::sanitize::{visible_line, DEFAULT_TAB_WIDTH};
use saysdf_render::source::DEFAULT_MAX_SIZE;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-view");

#[derive(Args)]
pub struct MermaidArgs {
    /// A .mmd/.mermaid file
    pub path: PathBuf,
    /// ASCII graphics instead of Unicode box-drawing
    #[arg(long)]
    pub ascii: bool,
    /// Layout density: compact | normal | spacious
    #[arg(long, default_value = "normal", value_parser = parse_zoom)]
    pub zoom: Zoom,
    /// Print the diagram to stdout (no colors) and exit
    #[arg(long)]
    pub print: bool,
    /// Compatibility with `view` (ignored)
    #[arg(long, hide = true)]
    pub line: Option<usize>,
    /// Compatibility with `view` (ignored)
    #[arg(long, default_value_t = 1, hide = true)]
    pub col: usize,
    /// Editor command for the `e` key: a JSON array, `{path}`/`{line}`/`{col}` are substituted
    #[arg(long)]
    pub editor_cmd: Option<String>,
}

fn parse_zoom(s: &str) -> Result<Zoom, String> {
    Zoom::parse(s).ok_or_else(|| I18N.t("expected compact, normal or spacious").into_owned())
}

fn read_src(path: &Path) -> io::Result<String> {
    if path.is_dir() {
        return Err(super::view::dir_error());
    }
    if std::fs::metadata(path)?.len() > DEFAULT_MAX_SIZE {
        return Err(io::Error::other(I18N.t("the file is larger than 16 MiB").into_owned()));
    }
    Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned())
}

/// Diagram lines (or the source with a note), anchors and status.
fn build(src: &str, o: &Opts) -> (Vec<Line<'static>>, Vec<Anchor>, String) {
    match mermaid::render(src, o) {
        Ok(r) => (r.lines, r.anchors, I18N.f("zoom: {name} (+/- , z to reset)", &[("name", &o.zoom.name())])),
        Err(f) => (mermaid::fallback_lines(src, &f, o.ascii), Vec::new(), visible_line(&f.to_string(), DEFAULT_TAB_WIDTH)),
    }
}

struct MermaidContent {
    path: PathBuf,
    src: String,
    opts: Opts,
    doc: Doc,
    anchors: Vec<Anchor>,
    view: (usize, usize, usize, usize),
}

impl MermaidContent {
    fn rebuild(&mut self) -> String {
        let (lines, anchors, status) = build(&self.src, &self.opts);
        self.doc = Doc::from_lines(lines);
        self.anchors = anchors;
        status
    }

    /// Zoom change: the node closest to the view center stays at the center.
    fn set_zoom(&mut self, z: Zoom) -> Nav {
        if z == self.opts.zoom {
            return Nav::Status(I18N.f("zoom: {name} (limit)", &[("name", &z.name())]));
        }
        let (top, hs, h, w) = self.view;
        let (cx, cy) = (hs + w / 2, top + h / 2);
        let anchor = self
            .anchors
            .iter()
            .min_by_key(|a| a.x.abs_diff(cx) + a.y.abs_diff(cy) * 2)
            .map(|a| a.id.clone());
        self.opts.zoom = z;
        let status = self.rebuild();
        match anchor.and_then(|id| self.anchors.iter().find(|a| a.id == id)) {
            Some(a) => Nav::Scroll { top: a.y.saturating_sub(h / 2), hscroll: a.x.saturating_sub(w / 2), status },
            None => Nav::Scroll { top, hscroll: hs, status },
        }
    }
}

impl Content for MermaidContent {
    fn len(&self) -> usize {
        self.doc.len()
    }

    fn window(&mut self, from: usize, to: usize) -> Vec<Line<'static>> {
        let n = self.doc.len();
        self.doc.lines[from.min(n)..to.min(n)].to_vec()
    }

    fn doc(&self) -> Option<&Doc> {
        Some(&self.doc)
    }

    fn reload(&mut self) -> io::Result<Option<String>> {
        self.src = read_src(&self.path)?;
        Ok(Some(I18N.f("reloaded; {status}", &[("status", &self.rebuild())])))
    }

    fn viewport(&mut self, top: usize, hscroll: usize, height: usize, width: usize) {
        self.view = (top, hscroll, height, width);
    }

    fn key(&mut self, k: &KeyEvent, phys: Option<Key>, _pos: usize) -> io::Result<Nav> {
        let is = |spec| crate::keys::is(k, phys, spec);

        Ok(match k.code {
            _ if is("+")? || is("=")? => self.set_zoom(self.opts.zoom.zoom_in()),
            _ if is("-")? => self.set_zoom(self.opts.zoom.zoom_out()),
            _ if is("z")? => self.set_zoom(Zoom::Normal),
            _ => Nav::None,
        })
    }
}

pub fn run(args: MermaidArgs) -> i32 {
    let abs = std::path::absolute(&args.path).unwrap_or_else(|_| args.path.clone());
    let title = visible_line(&abs.to_string_lossy(), DEFAULT_TAB_WIDTH);
    let src = match read_src(&abs) {
        Ok(s) => s,
        Err(e) => {
            let msg = super::view::open_error_text(&e);
            eprintln!("saysdf mermaid: {title}: {}", visible_line(&msg, DEFAULT_TAB_WIDTH));
            return 1;
        }
    };
    let opts = Opts { zoom: args.zoom, ascii: args.ascii };
    if args.print || !io::stdout().is_terminal() {
        let (lines, _, _) = build(&src, &opts);
        let mut out = io::stdout().lock();
        for l in lines {
            if writeln!(out, "{}", l.to_string().trim_end()).is_err() {
                return 1;
            }
        }
        return 0;
    }
    let editor = match args.editor_cmd.as_deref().map(crate::editor::parse).transpose() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("saysdf mermaid: {}", visible_line(&e, DEFAULT_TAB_WIDTH));
            return 2;
        }
    };
    let (lines, anchors, status) = build(&src, &opts);
    let content = MermaidContent { path: abs.clone(), src, opts, doc: Doc::from_lines(lines), anchors, view: (0, 0, 24, 80) };
    let popts = pager::Options { path: Some(abs), title, line: None, col: 1, editor, watch: true, status };
    match pager::run(content, popts) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}", I18N.f("saysdf mermaid: terminal: {error}", &[("error", &e.to_string())]));
            1
        }
    }
}
