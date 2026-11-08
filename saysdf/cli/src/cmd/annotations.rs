//! `saysdf annotations [dir]` — CriticMarkup marks of a folder as a pick list (change
//! text-annotations, design D10). The list is the shared pick content (`cmd::pick`); Enter sends the
//! sidebar action `viewer.open_at:<absolute path>:<line>` (see `saysdf.annotations`).

use super::pick::PickContent;
use crate::pager;
use crate::sidebar::{self, Item};
use clap::Args;
use saysdf_render::critic::{self, Kind, Mark};
use saysdf_render::sanitize::{char_width, visible_line};
use std::fs;
use std::path::{Path, PathBuf};
use unicode_segmentation::UnicodeSegmentation;
use walkdir::WalkDir;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-annotations");

/// Sidebar action id handled by `saysdf.annotations`.
const OPEN_AT: &str = "viewer.open_at";
/// Text files examined at most.
pub(crate) const MAX_FILES: usize = 10_000;
/// Directory entries visited at most (a guard for huge trees without text files).
const MAX_ENTRIES: usize = 200_000;
/// Rows at most.
pub(crate) const MAX_ROWS: usize = 10_000;
/// Larger files are skipped: `critic::parse` is quadratic on many unclosed openers.
pub(crate) const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024;
/// Width of the mark text in a row, in cells.
const TEXT_CELLS: usize = 60;
/// Directories never entered (besides hidden ones).
const SKIP_DIRS: [&str; 3] = [".git", "node_modules", "target"];

#[derive(Args)]
pub struct AnnotationsArgs {
    /// Root folder (the current one by default)
    pub dir: Option<PathBuf>,
    /// Write the chosen `path:line` to this file instead of sending a sidebar action (the viewer's
    /// Ctrl+L uses it: the list owns the terminal, so the choice cannot come back on stdout)
    #[arg(long, value_name = "FILE")]
    pub result_file: Option<PathBuf>,
}

/// Arguments of our own executable for the viewer's Ctrl+L. OS strings: no quoting needed.
pub(crate) fn list_args(root: &Path, result_file: &Path) -> Vec<std::ffi::OsString> {
    vec!["annotations".into(), "--result-file".into(), result_file.into(), "--".into(), root.into()]
}

/// Creates the empty result file (exclusively, private on Unix) in the temp folder; the viewer owns it
/// and removes it after reading.
pub(crate) fn new_result_file() -> std::io::Result<PathBuf> {
    use std::io::ErrorKind;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    for n in 0..16u32 {
        let path = std::env::temp_dir().join(format!("saysdf-annotations-{}-{nanos}-{n}.txt", std::process::id()));
        let mut o = fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        match o.open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::from(ErrorKind::AlreadyExists))
}

/// Largest result file read, in bytes (a path and a line number).
const MAX_RESULT: u64 = 8 * 1024;

/// Reads and removes the result file. The content is data from another process: the answer is
/// accepted only as `path:line` of an existing text file under `root` (see [`parse_choice`]).
pub(crate) fn take_choice(result_file: &Path, root: &Path) -> Option<(PathBuf, usize)> {
    use std::io::Read;
    let mut text = String::new();
    let read = fs::File::open(result_file).and_then(|f| f.take(MAX_RESULT).read_to_string(&mut text));
    let _ = fs::remove_file(result_file);
    read.ok()?;
    parse_choice(&text, root)
}

/// `path:line` (line from 1) -> the canonical path and the 0-based line, or `None` for anything else.
pub(crate) fn parse_choice(text: &str, root: &Path) -> Option<(PathBuf, usize)> {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.chars().any(char::is_control) {
        return None;
    }
    let (path, line) = text.rsplit_once(':')?;
    let line: usize = line.parse().ok().filter(|&l| l >= 1)?;
    let path = fs::canonicalize(path).ok()?;
    let root = fs::canonicalize(root).ok()?;
    (path.is_file() && path.starts_with(&root) && is_text(&path)).then_some((path, line - 1))
}

/// Result of a scan.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub rows: Vec<Item>,
    /// Stopped at a limit (files, entries or rows).
    pub cut: bool,
}

fn is_text(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("md" | "markdown" | "txt")
    )
}

fn skipped_dir(name: &str) -> bool {
    name.starts_with('.') || SKIP_DIRS.contains(&name)
}

fn kind_tag(k: Kind) -> &'static str {
    match k {
        Kind::Insert => "{++}",
        Kind::Delete => "{--}",
        Kind::Substitute => "{~~}",
        Kind::Highlight => "{==}",
        Kind::Comment => "{>>}",
    }
}

/// First line of the mark's text (for a substitution `old → new`, for a highlight with a comment
/// `text: comment`), escaped and cut to [`TEXT_CELLS`] cells on a grapheme boundary.
fn mark_text(text: &str, m: &Mark) -> String {
    let part = |r: &std::ops::Range<usize>| text[r.clone()].lines().next().unwrap_or("").trim().to_string();
    let raw = match (m.kind, &m.new_text, &m.comment) {
        (Kind::Substitute, Some(n), _) => format!("{} → {}", part(&m.text), part(n)),
        (Kind::Highlight, _, Some(c)) => format!("{}: {}", part(&m.text), part(c)),
        _ => part(&m.text),
    };
    let escaped = visible_line(&raw, 4);
    let mut out = String::new();
    let mut w = 0;
    for g in escaped.graphemes(true) {
        let gw: usize = g.chars().map(char_width).sum();
        if w + gw > TEXT_CELLS {
            out.push('…');
            break;
        }
        w += gw;
        out.push_str(g);
    }
    out
}

/// Relative path with `/` on every OS.
fn rel_string(root: &Path, p: &Path) -> String {
    let rel = p.strip_prefix(root).unwrap_or(p);
    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// Walks `root` and lists the marks of text files. Item id = `<absolute path>:<line>`.
pub(crate) fn scan(root: &Path) -> Scan {
    let mut out = Scan::default();
    let abs_root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let walker = WalkDir::new(&abs_root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !(e.file_type().is_dir() && skipped_dir(&e.file_name().to_string_lossy())));
    let (mut files, mut entries) = (0usize, 0usize);
    for entry in walker.filter_map(Result::ok) {
        entries += 1;
        if entries > MAX_ENTRIES {
            out.cut = true;
            break;
        }
        if !entry.file_type().is_file() || !is_text(entry.path()) {
            continue;
        }
        files += 1;
        if files > MAX_FILES {
            out.cut = true;
            break;
        }
        if entry.metadata().map(|m| m.len() > MAX_FILE_SIZE).unwrap_or(true) {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path()) else { continue };
        let Ok(text) = String::from_utf8(bytes) else { continue };
        let rel = rel_string(&abs_root, entry.path());
        for m in critic::parse(&text) {
            if out.rows.len() >= MAX_ROWS {
                out.cut = true;
                return out;
            }
            let line = text.as_bytes()[..m.range.start].iter().filter(|&&b| b == b'\n').count() + 1;
            let label = format!("{rel}:{line}  {}  {}", kind_tag(m.kind), mark_text(&text, &m));
            out.rows.push(Item { id: format!("{}:{line}", entry.path().display()), label: visible_line(&label, 4) });
        }
    }
    out
}

pub fn run(args: AnnotationsArgs) -> i32 {
    // The token is taken before any child process (tool-sidebar D3).
    let token = sidebar::take_token();
    let root = args.dir.unwrap_or_else(|| PathBuf::from("."));
    if !root.is_dir() {
        eprintln!("saysdf annotations: {}: {}", root.display(), I18N.t("not a folder"));
        return 1;
    }
    let found = scan(&root);
    let status = if found.rows.is_empty() {
        I18N.t("No annotations found").into_owned()
    } else if found.cut {
        I18N.t("Cut at the limit; ↑↓ item, Enter open, / search, Esc cancel").into_owned()
    } else {
        I18N.t("↑↓ item, Enter open, / search, Esc cancel").into_owned()
    };
    let n = found.rows.len();
    let content = match args.result_file {
        // The viewer runs us: the choice goes to its file, never to a sidebar.
        Some(f) => PickContent::new(found.rows, None).with_result_file(f),
        None => PickContent::new(found.rows, token).with_action(OPEN_AT),
    };
    let opts = pager::Options {
        path: None,
        title: I18N.t("Annotations").into_owned(),
        line: (n > 0).then_some(0),
        col: 1,
        editor: None,
        watch: false,
        status,
    };
    // Enter exits the process in PickContent::finish(); here only cancel returns.
    match pager::run(content, opts) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("saysdf annotations: {}", I18N.f("terminal: {error}", &[("error", &e)]));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let w = |p: &str, t: &str| {
            let f = d.path().join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, t).unwrap();
        };
        w("a.md", "one\ntwo {++added++} x\n");
        w("sub/deep/b.markdown", "{>>Проверить<<}\n"); // i18n-allow-cyrillic
        w("n.txt", "{--gone--}");
        w(".git/c.md", "{++git++}");
        w("node_modules/d.md", "{++nm++}");
        w("target/e.md", "{++t++}");
        w(".hidden/f.md", "{++h++}");
        w("code.rs", "{++rs++}");
        d
    }

    #[test]
    fn finds_nested_and_skips() {
        let d = tree();
        let s = scan(d.path());
        let labels: Vec<_> = s.rows.iter().map(|r| r.label.clone()).collect();
        assert_eq!(s.rows.len(), 3, "{labels:?}");
        assert!(labels[0].starts_with("a.md:2  {++}  added"), "{labels:?}");
        assert!(labels.iter().any(|l| l.starts_with("sub/deep/b.markdown:1  {>>}  Проверить"))); // i18n-allow-cyrillic
        assert!(labels.iter().any(|l| l.starts_with("n.txt:1  {--}  gone")));
        assert!(labels.iter().all(|l| !l.contains('\\')));
        assert!(!s.cut);
    }

    #[test]
    fn ids_are_absolute_path_and_line() {
        let d = tree();
        let s = scan(d.path());
        let id = &s.rows[0].id;
        let (p, l) = id.rsplit_once(':').unwrap();
        assert_eq!(l, "2");
        assert!(Path::new(p).is_absolute() && p.ends_with("a.md"), "{id}");
    }

    #[test]
    fn empty_tree() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("x.md"), "plain").unwrap();
        let s = scan(d.path());
        assert!(s.rows.is_empty() && !s.cut);
    }

    #[test]
    fn text_is_sanitized_and_cut_on_graphemes() {
        let d = tempfile::tempdir().unwrap();
        let long = "Я".repeat(100); // i18n-allow-cyrillic
        fs::write(d.path().join("a.md"), format!("{{++bid\u{202e}i\u{7}x++}}\n{{>>{long}<<}}\n")).unwrap();
        let s = scan(d.path());
        assert_eq!(s.rows.len(), 2);
        let first = &s.rows[0].label;
        assert!(!first.contains('\u{202e}') && !first.contains('\u{7}'), "{first:?}");
        let second = &s.rows[1].label;
        assert!(second.ends_with('…') && second.matches('Я').count() == TEXT_CELLS, "{second}"); // i18n-allow-cyrillic
    }

    #[test]
    fn multi_line_mark_uses_first_line() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.md"), "{~~old\nmore~>новое\nещё~~}").unwrap(); // i18n-allow-cyrillic
        let s = scan(d.path());
        assert!(s.rows[0].label.ends_with("old → новое"), "{}", s.rows[0].label); // i18n-allow-cyrillic
    }

    #[test]
    fn row_limit_cuts() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.md"), "{++x++}".repeat(MAX_ROWS + 5)).unwrap();
        let s = scan(d.path());
        assert_eq!(s.rows.len(), MAX_ROWS);
        assert!(s.cut);
    }

    #[test]
    fn big_file_skipped() {
        let d = tempfile::tempdir().unwrap();
        let mut t = "{++x++}".to_string();
        t.push_str(&" ".repeat(MAX_FILE_SIZE as usize));
        fs::write(d.path().join("big.md"), t).unwrap();
        assert!(scan(d.path()).rows.is_empty());
    }

    #[test]
    fn list_args_keep_odd_paths_whole() {
        let a = list_args(Path::new("C:/Мои файлы/проект x"), Path::new("t/res.txt")); // i18n-allow-cyrillic
        assert_eq!(a.len(), 5);
        assert_eq!(a[0], "annotations");
        assert_eq!(a[1], "--result-file");
        assert_eq!(a[2], "t/res.txt");
        assert_eq!(a[3], "--");
        assert_eq!(a[4], "C:/Мои файлы/проект x"); // i18n-allow-cyrillic
    }

    #[test]
    fn choice_with_cyrillic_path_and_spaces() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("мои заметки").join("файл один.md"); // i18n-allow-cyrillic
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, "{++x++}").unwrap();
        let (p, l) = parse_choice(&format!("{}:12", f.display()), d.path()).unwrap();
        assert_eq!((p, l), (fs::canonicalize(&f).unwrap(), 11));
        assert!(parse_choice(&format!("{}:1\n", f.display()), d.path()).is_some());
    }

    #[test]
    fn foreign_or_invalid_choice_is_ignored() {
        let d = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let inside = d.path().join("a.md");
        let outside = other.path().join("b.md");
        fs::write(&inside, "x").unwrap();
        fs::write(&outside, "x").unwrap();
        fs::write(d.path().join("c.rs"), "x").unwrap();
        let bad = [
            format!("{}:1", outside.display()),
            format!("{}:0", inside.display()),
            format!("{}:-3", inside.display()),
            format!("{}:x", inside.display()),
            format!("{}", inside.display()),
            format!("{}:1", d.path().join("missing.md").display()),
            format!("{}:1", d.path().display()),
            format!("{}:1", d.path().join("c.rs").display()),
            format!("{}\x1b]52;c;AAAA\x07:1", inside.display()),
            format!("{}:1\n{}:2", inside.display(), inside.display()),
            format!("{}/../../x.md:1", d.path().display()),
            String::new(),
        ];
        for b in &bad {
            assert!(parse_choice(b, d.path()).is_none(), "{b:?}");
        }
    }

    #[test]
    fn result_file_is_read_once_and_removed() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.md");
        fs::write(&a, "x").unwrap();
        let rf = new_result_file().unwrap();
        assert_eq!(fs::metadata(&rf).unwrap().len(), 0);
        assert!(take_choice(&rf, d.path()).is_none()); // cancelled: empty file
        assert!(!rf.exists());
        let rf = new_result_file().unwrap();
        fs::write(&rf, format!("{}:3", a.display())).unwrap();
        assert_eq!(take_choice(&rf, d.path()).unwrap().1, 2);
        assert!(!rf.exists());
        // An oversized answer is cut and rejected.
        let rf = new_result_file().unwrap();
        fs::write(&rf, format!("{}:3{}", a.display(), "9".repeat(20_000))).unwrap();
        assert!(take_choice(&rf, d.path()).is_none());
    }

    #[test]
    fn non_utf8_skipped() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.md"), b"{++x++}\xff").unwrap();
        assert!(scan(d.path()).rows.is_empty());
    }
}
