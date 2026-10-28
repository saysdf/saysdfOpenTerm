//! `saysdf view <path>`: file viewer (design D2, D7). Errors show a message screen (exit with `q`),
//! a binary file gets a hex view, a large one is a `LargeFile` without highlighting.

use crate::editor;
use crate::pager::{self, Content};
use clap::Args;
use ratatui::text::Line;
use saysdf_render::doc::Doc;
use saysdf_render::highlight::{plain_line, Highlighter};
use saysdf_render::sanitize::{visible_line, DEFAULT_TAB_WIDTH};
use saysdf_render::source::{self, HexFile, LargeFile, Opened, DEFAULT_MAX_SIZE};
use std::io::{self, IsTerminal};
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
    Text { lines: Vec<String>, doc: Doc, hl: Highlighter },
    Large(LargeFile),
    Hex(HexFile),
    Message(Doc),
}

struct ViewContent {
    path: PathBuf,
    tab_width: usize,
    max_size: u64,
    kind: Kind,
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
            },
            status,
        ))
    }

    fn load(path: &Path, tab_width: usize, max_size: u64) -> io::Result<(Kind, String)> {
        if path.is_dir() {
            return Err(dir_error());
        }
        Ok(match source::open(path, max_size)? {
            Opened::Text(lines) => {
                let first = lines.first().map(String::as_str).unwrap_or("");
                let hl = Highlighter::for_path(path, first, tab_width);
                let doc = Doc::from_lines(lines.iter().map(|l| plain_line(l, tab_width)).collect());
                let status = hl.syntax_name().unwrap_or("").to_owned();
                (Kind::Text { lines, doc, hl }, status)
            }
            Opened::Binary(h) => (Kind::Hex(h), I18N.t("binary file: hex view").into_owned()),
            Opened::Large(l) => (Kind::Large(l), I18N.t("large file: no highlighting or search").into_owned()),
        })
    }
}

impl Content for ViewContent {
    fn len(&self) -> usize {
        match &self.kind {
            Kind::Text { lines, .. } => lines.len(),
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
            Kind::Text { lines, hl, .. } => hl.highlight_window(lines, from, to),
            Kind::Large(l) => read(l.read_lines(from, to)),
            Kind::Hex(h) => read(h.window(from as u64, to.saturating_sub(from))),
            Kind::Message(d) => d.lines[from.min(d.len())..to.min(d.len())].to_vec(),
        }
    }

    fn doc(&self) -> Option<&Doc> {
        match &self.kind {
            Kind::Text { doc, .. } | Kind::Message(doc) => Some(doc),
            _ => None,
        }
    }

    fn reload(&mut self) -> io::Result<Option<String>> {
        if matches!(self.kind, Kind::Message(_)) {
            return Ok(None);
        }
        let (kind, status) = Self::load(&self.path, self.tab_width, self.max_size)?;
        self.kind = kind;
        Ok(Some(if status.is_empty() {
            I18N.t("reloaded").into_owned()
        } else {
            I18N.f("reloaded; {status}", &[("status", &status)])
        }))
    }
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

pub fn run(args: ViewArgs) -> i32 {
    let abs = std::path::absolute(&args.path).unwrap_or_else(|_| args.path.clone());
    let title = visible_line(&abs.to_string_lossy(), DEFAULT_TAB_WIDTH);
    let editor = match args.editor_cmd.as_deref().map(editor::parse).transpose() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("saysdf view: {}", visible_line(&e, DEFAULT_TAB_WIDTH));
            return 2;
        }
    };
    let (content, status) = match ViewContent::open(&abs, args.tab_width, args.max_size) {
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
    let opts = pager::Options {
        path: Some(abs),
        title,
        line: args.line.map(|l| l.saturating_sub(1)),
        col: args.col,
        editor,
        watch: true,
        status,
    };
    match pager::run(content, opts) {
        Ok(()) => 0,
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
}
