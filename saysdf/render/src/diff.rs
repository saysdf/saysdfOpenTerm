//! Diff model (design diff-merge-tool D4/D5): hunks of two texts via `similar`, intra-line
//! ranges, parsing of unified diff from `git diff`, building a hunk patch for `git apply`, rendering
//! side-by-side / inline into pager lines (everything goes through `sanitize`).

use crate::highlight::Highlighter;
use crate::sanitize::{str_width, visible_line};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{Algorithm, ChangeTag, TextDiff};
use std::time::Duration;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("render-diff");

/// Hunk context (lines).
pub const CONTEXT: usize = 3;
/// Longer lines get no intra-line highlighting.
pub const INLINE_MAX_CHARS: usize = 10_000;
/// Diff algorithm timeout.
pub const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: Kind,
    pub old_no: Option<usize>,
    pub new_no: Option<usize>,
    /// Text without the line terminator.
    pub text: String,
    /// The line is followed by `\ No newline at end of file`.
    pub no_newline: bool,
    /// Intra-line changes: byte ranges of `text`.
    pub inline: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    /// Header tail after `@@` (function name).
    pub section: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    /// Path of the old side; `None` is `/dev/null` (new file).
    pub old: Option<String>,
    /// Path of the new side; `None` is `/dev/null` (deleted file).
    pub new: Option<String>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
    pub new_file_mode: Option<String>,
    pub deleted_file_mode: Option<String>,
    pub rename: bool,
}

impl FileDiff {
    /// Path to display (new side, otherwise old).
    pub fn path(&self) -> &str {
        self.new.as_deref().or(self.old.as_deref()).unwrap_or("")
    }

    /// Status letter: A/D/R/M/B.
    pub fn status(&self) -> char {
        if self.binary {
            'B'
        } else if self.old.is_none() || self.new_file_mode.is_some() {
            'A'
        } else if self.new.is_none() || self.deleted_file_mode.is_some() {
            'D'
        } else if self.rename {
            'R'
        } else {
            'M'
        }
    }
}

fn strip_eol(s: &str) -> (&str, bool) {
    match s.strip_suffix('\n') {
        Some(t) => (t.strip_suffix('\r').unwrap_or(t), false),
        None => (s, true),
    }
}

/// Diff of two contents; binary ones (per `source::is_binary`) have no hunks.
pub fn diff_bytes(old: &[u8], new: &[u8], old_path: Option<&str>, new_path: Option<&str>) -> FileDiff {
    if crate::source::is_binary(old) || crate::source::is_binary(new) {
        return FileDiff {
            old: old_path.map(str::to_owned),
            new: new_path.map(str::to_owned),
            binary: old != new,
            ..Default::default()
        };
    }
    diff_texts(&String::from_utf8_lossy(old), &String::from_utf8_lossy(new), old_path, new_path)
}

/// Hunks of two texts: Patience, context [`CONTEXT`], timeout [`TIMEOUT`].
pub fn diff_texts(old: &str, new: &str, old_path: Option<&str>, new_path: Option<&str>) -> FileDiff {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .timeout(TIMEOUT)
        .diff_lines(old, new);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(CONTEXT) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let o = first.old_range().start..last.old_range().end;
        let n = first.new_range().start..last.new_range().end;
        let mut h = Hunk {
            old_start: if o.is_empty() { o.start } else { o.start + 1 },
            old_len: o.len(),
            new_start: if n.is_empty() { n.start } else { n.start + 1 },
            new_len: n.len(),
            ..Default::default()
        };
        for op in &group {
            for ch in diff.iter_changes(op) {
                let (text, no_newline) = strip_eol(ch.value());
                h.lines.push(DiffLine {
                    kind: match ch.tag() {
                        ChangeTag::Equal => Kind::Context,
                        ChangeTag::Insert => Kind::Add,
                        ChangeTag::Delete => Kind::Del,
                    },
                    old_no: ch.old_index().map(|i| i + 1),
                    new_no: ch.new_index().map(|i| i + 1),
                    text: text.to_owned(),
                    no_newline,
                    inline: Vec::new(),
                });
            }
        }
        add_inline(&mut h);
        hunks.push(h);
    }
    FileDiff {
        old: old_path.map(str::to_owned),
        new: new_path.map(str::to_owned),
        hunks,
        ..Default::default()
    }
}

/// Tokens for the intra-line diff: words (letters/digits/`_`), whitespace runs, other characters one by one.
fn tokens(s: &str) -> Vec<&str> {
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<u8> = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        if let Some(p) = prev {
            if p != k || k == 2 {
                out.push(&s[start..i]);
                start = i;
            }
        }
        prev = Some(k);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Byte range `[start, end)`.
pub type Range = (usize, usize);

/// Intra-line ranges (by word) for a pair of lines; `None` if the lines are too long.
pub fn inline_ranges(old: &str, new: &str) -> Option<(Vec<Range>, Vec<Range>)> {
    if old.chars().count() > INLINE_MAX_CHARS || new.chars().count() > INLINE_MAX_CHARS {
        return None;
    }
    let (ta, tb) = (tokens(old), tokens(new));
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .timeout(TIMEOUT)
        .diff_slices(&ta, &tb);
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let (mut oa, mut ob) = (0usize, 0usize);
    let push = |v: &mut Vec<(usize, usize)>, s: usize, e: usize| match v.last_mut() {
        Some(last) if last.1 == s => last.1 = e,
        _ => v.push((s, e)),
    };
    for ch in diff.iter_all_changes() {
        let len = ch.value().len();
        match ch.tag() {
            ChangeTag::Equal => {
                oa += len;
                ob += len;
            }
            ChangeTag::Delete => {
                push(&mut a, oa, oa + len);
                oa += len;
            }
            ChangeTag::Insert => {
                push(&mut b, ob, ob + len);
                ob += len;
            }
        }
    }
    Some((a, b))
}

/// Pairs of consecutive `-`/`+` blocks get intra-line ranges.
pub fn add_inline(h: &mut Hunk) {
    let mut i = 0;
    while i < h.lines.len() {
        if h.lines[i].kind != Kind::Del {
            i += 1;
            continue;
        }
        let ds = i;
        while i < h.lines.len() && h.lines[i].kind == Kind::Del {
            i += 1;
        }
        let as_ = i;
        while i < h.lines.len() && h.lines[i].kind == Kind::Add {
            i += 1;
        }
        let n = (as_ - ds).min(i - as_);
        for k in 0..n {
            let (d, a) = (ds + k, as_ + k);
            if let Some((ra, rb)) = inline_ranges(&h.lines[d].text, &h.lines[a].text) {
                // Fully replaced line: no inline ranges (line background only)
                if ra != [(0, h.lines[d].text.len())] || rb != [(0, h.lines[a].text.len())] {
                    h.lines[d].inline = ra;
                    h.lines[a].inline = rb;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Parsing unified diff (`git diff`)

/// Unquoting of git's C-style quotes (`"a\tb\303\244"`); unquoted input is returned as is.
pub fn unquote_path(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|t| t.strip_suffix('"')) else {
        return s.to_owned();
    };
    let mut out = Vec::new();
    let b = inner.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            i += 1;
            match b[i] {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'a' => out.push(7),
                b'b' => out.push(8),
                b'f' => out.push(12),
                b'v' => out.push(11),
                b'0'..=b'7' => {
                    let mut v = 0u32;
                    let mut k = 0;
                    while k < 3 && i < b.len() && (b'0'..=b'7').contains(&b[i]) {
                        v = v * 8 + (b[i] - b'0') as u32;
                        i += 1;
                        k += 1;
                    }
                    out.push(v as u8);
                    continue;
                }
                c => out.push(c),
            }
            i += 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Quotes a patch path if it contains `"`, `\` or control characters (git format).
pub fn quote_path(p: &str) -> String {
    if !p.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return p.to_owned();
    }
    let mut s = String::from("\"");
    for c in p.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            '\r' => s.push_str("\\r"),
            c if c.is_control() => {
                let mut buf = [0u8; 4];
                for byte in c.encode_utf8(&mut buf).bytes() {
                    s.push_str(&format!("\\{byte:03o}"));
                }
            }
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// Path from a `---`/`+++` line: `/dev/null` becomes `None`; quotes, the `a/`/`b/` prefix and a trailing TAB are stripped.
fn side_path(s: &str) -> Option<String> {
    let s = s.trim_end_matches('\t');
    if s == "/dev/null" {
        return None;
    }
    let p = unquote_path(s);
    Some(match p.get(..2) {
        Some("a/") | Some("b/") => p[2..].to_owned(),
        _ => p,
    })
}

/// Paths from `diff --git a/x b/y` (for files without `---`/`+++`: binary, mode/rename only).
fn header_paths(rest: &str) -> (Option<String>, Option<String>) {
    if let Some(tail) = rest.strip_prefix('"') {
        // "a/x" "b/y" or "a/x" b/y
        if let Some(end) = tail.find("\" ").map(|i| i + 1) {
            let (a, b) = (&rest[..=end], rest[end + 1..].trim_start());
            return (side_path(a), side_path(b));
        }
    }
    // Unquoted: split at " b/" (identical names: the middle of the line)
    let n = rest.len();
    if n % 2 == 1 {
        let mid = n / 2;
        if rest.is_char_boundary(mid) && rest[mid..].starts_with(" b/") && rest[..mid].strip_prefix("a/") == Some(&rest[mid + 3..]) {
            return (side_path(&rest[..mid]), side_path(&rest[mid + 1..]));
        }
    }
    match rest.find(" b/") {
        Some(i) => (side_path(&rest[..i]), side_path(&rest[i + 1..])),
        None => (None, None),
    }
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
    let s = &s[1..];
    match s.split_once(',') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

/// Header `@@ -a,b +c,d @@ section`.
pub fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, section) = rest.split_once(" @@").unwrap_or((rest, ""));
    let mut it = ranges.split_whitespace();
    let (os, ol) = parse_range(it.next()?)?;
    let (ns, nl) = parse_range(it.next()?)?;
    Some(Hunk {
        old_start: os,
        old_len: ol,
        new_start: ns,
        new_len: nl,
        section: section.trim_start().to_owned(),
        lines: Vec::new(),
    })
}

/// Parses `git diff` output (several files, rename, binary, `\ No newline`, quoted paths).
pub fn parse_unified(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut hunk: Option<Hunk> = None;
    let (mut old_left, mut new_left) = (0usize, 0usize);
    let (mut old_no, mut new_no) = (0usize, 0usize);

    fn flush(files: &mut [FileDiff], hunk: &mut Option<Hunk>) {
        if let (Some(f), Some(mut h)) = (files.last_mut(), hunk.take()) {
            add_inline(&mut h);
            f.hunks.push(h);
        }
    }

    for raw in text.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let in_hunk = hunk.is_some() && (old_left > 0 || new_left > 0);
        if in_hunk {
            let (kind, body) = match line.as_bytes().first() {
                Some(b' ') => (Kind::Context, &line[1..]),
                Some(b'-') => (Kind::Del, &line[1..]),
                Some(b'+') => (Kind::Add, &line[1..]),
                None => (Kind::Context, ""),
                _ => {
                    // Unexpected line: the hunk ended before its counters
                    old_left = 0;
                    new_left = 0;
                    (Kind::Context, "\u{0}")
                }
            };
            if body != "\u{0}" {
                let h = hunk.as_mut().expect("in hunk");
                let (o, n) = match kind {
                    Kind::Context => {
                        old_no += 1;
                        new_no += 1;
                        old_left = old_left.saturating_sub(1);
                        new_left = new_left.saturating_sub(1);
                        (Some(old_no), Some(new_no))
                    }
                    Kind::Del => {
                        old_no += 1;
                        old_left = old_left.saturating_sub(1);
                        (Some(old_no), None)
                    }
                    Kind::Add => {
                        new_no += 1;
                        new_left = new_left.saturating_sub(1);
                        (None, Some(new_no))
                    }
                };
                h.lines.push(DiffLine {
                    kind,
                    old_no: o,
                    new_no: n,
                    text: body.to_owned(),
                    no_newline: false,
                    inline: Vec::new(),
                });
                continue;
            }
        }
        if line.starts_with('\\') {
            if let Some(l) = hunk.as_mut().and_then(|h| h.lines.last_mut()) {
                l.no_newline = true;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush(&mut files, &mut hunk);
            let (a, b) = header_paths(rest);
            files.push(FileDiff {
                old: a,
                new: b,
                ..Default::default()
            });
            continue;
        }
        if let Some(h) = parse_hunk_header(line) {
            flush(&mut files, &mut hunk);
            if files.is_empty() {
                files.push(FileDiff::default());
            }
            old_left = h.old_len;
            new_left = h.new_len;
            old_no = h.old_start.saturating_sub(if h.old_len == 0 { 0 } else { 1 });
            new_no = h.new_start.saturating_sub(if h.new_len == 0 { 0 } else { 1 });
            hunk = Some(h);
            continue;
        }
        if hunk.is_some() {
            flush(&mut files, &mut hunk);
        }
        let Some(f) = files.last_mut() else {
            if let Some(p) = line.strip_prefix("--- ") {
                files.push(FileDiff {
                    old: side_path(p),
                    ..Default::default()
                });
            }
            continue;
        };
        if let Some(p) = line.strip_prefix("--- ") {
            f.old = side_path(p);
        } else if let Some(p) = line.strip_prefix("+++ ") {
            f.new = side_path(p);
        } else if let Some(m) = line.strip_prefix("new file mode ") {
            f.new_file_mode = Some(m.to_owned());
            f.old = None;
        } else if let Some(m) = line.strip_prefix("deleted file mode ") {
            f.deleted_file_mode = Some(m.to_owned());
            f.new = None;
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.rename = true;
            f.old = Some(unquote_path(p));
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.rename = true;
            f.new = Some(unquote_path(p));
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            f.binary = true;
        }
    }
    flush(&mut files, &mut hunk);
    files
}

// ---------------------------------------------------------------------------------------------
// Patch for `git apply`

/// Patch of one hunk (`selected` are the chosen hunk lines; `None` is the whole hunk). Unselected `+`
/// lines are dropped, unselected `-` lines become context, counters are recomputed. Reverse
/// application (unstage) is `git apply -R` of the same patch. `None` means the selection has no changes.
pub fn hunk_patch(file: &FileDiff, hunk: usize, selected: Option<&[bool]>) -> Option<String> {
    let h = file.hunks.get(hunk)?;
    let sel = |i: usize| selected.is_none_or(|s| s.get(i).copied().unwrap_or(false));
    let mut body = String::new();
    let (mut ol, mut nl, mut changes) = (0usize, 0usize, 0usize);
    let mut partial = false;
    for (i, l) in h.lines.iter().enumerate() {
        let prefix = match (l.kind, sel(i)) {
            (Kind::Context, _) => ' ',
            (Kind::Add, true) => '+',
            (Kind::Del, true) => '-',
            (Kind::Add, false) => {
                partial = true;
                continue;
            }
            (Kind::Del, false) => {
                partial = true;
                ' '
            }
        };
        match prefix {
            ' ' => {
                ol += 1;
                nl += 1;
            }
            '-' => {
                ol += 1;
                changes += 1;
            }
            _ => {
                nl += 1;
                changes += 1;
            }
        }
        body.push(prefix);
        body.push_str(&l.text);
        body.push('\n');
        if l.no_newline {
            body.push_str("\\ No newline at end of file\n");
        }
    }
    if changes == 0 {
        return None;
    }
    let old_start = h.old_start;
    let new_start = match (ol, nl) {
        (0, _) => old_start + 1,
        (_, 0) => old_start.saturating_sub(1),
        _ => old_start,
    };
    let a = file.old.as_deref().or(file.new.as_deref()).unwrap_or("");
    let b = file.new.as_deref().or(file.old.as_deref()).unwrap_or("");
    let mut p = format!("diff --git {} {}\n", quote_path(&format!("a/{a}")), quote_path(&format!("b/{b}")));
    if let Some(m) = &file.new_file_mode {
        p.push_str(&format!("new file mode {m}\n"));
    }
    let deleted = file.deleted_file_mode.is_some() && !partial && file.hunks.len() == 1;
    if deleted {
        p.push_str(&format!("deleted file mode {}\n", file.deleted_file_mode.as_deref().unwrap_or("100644")));
    }
    let old_side = if file.old.is_none() { "/dev/null".to_owned() } else { quote_path(&format!("a/{a}")) };
    let new_side = if file.new.is_none() && deleted { "/dev/null".to_owned() } else { quote_path(&format!("b/{b}")) };
    p.push_str(&format!("--- {old_side}\n+++ {new_side}\n"));
    p.push_str(&format!("@@ -{old_start},{ol} +{new_start},{nl} @@\n"));
    p.push_str(&body);
    Some(p)
}

// ---------------------------------------------------------------------------------------------
// Rendering

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    SideBySide,
    Inline,
}

/// View line to a place in the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Header,
    HunkHeader(usize),
    /// Hunk, line index (inline).
    Line(usize, usize),
    /// Side-by-side: hunk, left and right lines of the pair.
    Pair(usize, Option<usize>, Option<usize>),
    /// Collapsed context span of a hunk `(hunk, from, to)`.
    Fold(usize, usize, usize),
    Message,
}

/// Result of rendering a file.
#[derive(Debug, Default)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub rows: Vec<Row>,
    /// View lines where hunks begin.
    pub hunk_rows: Vec<usize>,
}

pub fn add_bg() -> Color {
    Color::Rgb(0x1f, 0x3a, 0x24)
}
pub fn del_bg() -> Color {
    Color::Rgb(0x4a, 0x1f, 0x22)
}
pub fn add_hi() -> Color {
    Color::Rgb(0x2e, 0x6b, 0x38)
}
pub fn del_hi() -> Color {
    Color::Rgb(0x8a, 0x2c, 0x33)
}

/// Byte ranges of the raw text mapped to byte ranges of the visible text (after `sanitize`).
fn visible_ranges(raw: &str, ranges: &[(usize, usize)], tab_width: usize) -> Vec<(usize, usize)> {
    let vis = |b: usize| raw.get(..b).map_or(0, |p| visible_line(p, tab_width).len());
    ranges.iter().map(|&(s, e)| (vis(s), vis(e))).collect()
}

/// Overlays a style on byte ranges of a line.
pub fn patch_ranges(line: Line<'static>, ranges: &[(usize, usize)], style: Style) -> Line<'static> {
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

fn style_line(l: &DiffLine, hl: &[Line<'static>], idx: usize, tab_width: usize) -> Line<'static> {
    let mut line = hl.get(idx).cloned().unwrap_or_else(|| Line::raw(visible_line(&l.text, tab_width)));
    let (bg, hi) = match l.kind {
        Kind::Context => return line,
        Kind::Add => (add_bg(), add_hi()),
        Kind::Del => (del_bg(), del_hi()),
    };
    for s in &mut line.spans {
        s.style = s.style.bg(bg);
    }
    if !l.inline.is_empty() {
        let r = visible_ranges(&l.text, &l.inline, tab_width);
        line = patch_ranges(line, &r, Style::new().bg(hi).add_modifier(Modifier::BOLD));
    }
    line
}

/// Highlighting of hunk sides: old = context+removed, new = context+added, per hunk line.
fn highlight_hunk(h: &Hunk, hl: Option<&mut Highlighter>, tab_width: usize) -> Vec<Line<'static>> {
    let Some(hl) = hl else {
        return h.lines.iter().map(|l| Line::raw(visible_line(&l.text, tab_width))).collect();
    };
    let mut out = vec![Line::default(); h.lines.len()];
    for side in [Kind::Del, Kind::Add] {
        let idx: Vec<usize> = (0..h.lines.len()).filter(|&i| h.lines[i].kind == Kind::Context || h.lines[i].kind == side).collect();
        let texts: Vec<String> = idx.iter().map(|&i| h.lines[i].text.clone()).collect();
        hl.invalidate();
        let lines = hl.highlight_all(&texts);
        for (k, &i) in idx.iter().enumerate() {
            if h.lines[i].kind == side || side == Kind::Del {
                out[i] = lines.get(k).cloned().unwrap_or_default();
            }
        }
    }
    out
}

fn num(n: Option<usize>, w: usize) -> String {
    n.map_or_else(|| " ".repeat(w), |n| format!("{n:>w$}"))
}

fn pad_to(mut line: Line<'static>, width: usize) -> Line<'static> {
    let w: usize = line.spans.iter().map(|s| str_width(&s.content)).sum();
    if w < width {
        line.spans.push(Span::raw(" ".repeat(width - w)));
    }
    line
}

/// Render options.
pub struct RenderOpts {
    pub layout: Layout,
    /// Collapse long unchanged context (> 2×[`CONTEXT`]).
    pub fold: bool,
    /// Width of one side (side-by-side), in cells.
    pub side_width: usize,
    /// Horizontal offset inside the sides (side-by-side).
    pub hscroll: usize,
    pub tab_width: usize,
}

/// Renders a file into pager lines.
/// The "no differences" message (also shown by `saysdf diff` when there are no files at all).
pub fn no_differences() -> std::borrow::Cow<'static, str> {
    I18N.t("no differences")
}

pub fn render(file: &FileDiff, hl: Option<&mut Highlighter>, o: &RenderOpts) -> Rendered {
    let mut r = Rendered::default();
    let head = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let title = match (&file.old, &file.new) {
        (Some(a), Some(b)) if a != b => format!("{} → {}", visible_line(a, o.tab_width), visible_line(b, o.tab_width)),
        _ => visible_line(file.path(), o.tab_width),
    };
    r.lines.push(Line::styled(format!("[{}] {title}", file.status()), head));
    r.rows.push(Row::Header);
    if file.binary {
        r.lines.push(Line::raw(I18N.t("binary files differ")));
        r.rows.push(Row::Message);
        return r;
    }
    if file.hunks.is_empty() {
        r.lines.push(Line::raw(no_differences()));
        r.rows.push(Row::Message);
        return r;
    }
    let mut hl = hl;
    let nw = file
        .hunks
        .iter()
        .map(|h| (h.old_start + h.old_len).max(h.new_start + h.new_len))
        .max()
        .unwrap_or(1)
        .to_string()
        .len();
    let dim = Style::new().fg(Color::DarkGray);
    for (hi, h) in file.hunks.iter().enumerate() {
        r.hunk_rows.push(r.lines.len());
        r.lines.push(Line::styled(
            format!("@@ -{},{} +{},{} @@ {}", h.old_start, h.old_len, h.new_start, h.new_len, visible_line(&h.section, o.tab_width)),
            Style::new().fg(Color::Magenta),
        ));
        r.rows.push(Row::HunkHeader(hi));
        let hls = highlight_hunk(h, hl.as_deref_mut(), o.tab_width);
        // Folding: long context spans inside a hunk
        let mut i = 0;
        while i < h.lines.len() {
            if o.fold && h.lines[i].kind == Kind::Context {
                let s = i;
                let mut e = i;
                while e < h.lines.len() && h.lines[e].kind == Kind::Context {
                    e += 1;
                }
                if e - s > 2 * CONTEXT {
                    for k in s..s + CONTEXT {
                        emit(&mut r, h, hi, k, None, &hls, o, nw, dim);
                    }
                    r.lines.push(Line::styled(format!("  ⋯ {} ⋯", I18N.n("{n} unchanged line", "{n} unchanged lines", (e - s - 2 * CONTEXT) as u64, &[])), dim));
                    r.rows.push(Row::Fold(hi, s + CONTEXT, e - CONTEXT));
                    for k in e - CONTEXT..e {
                        emit(&mut r, h, hi, k, None, &hls, o, nw, dim);
                    }
                    i = e;
                    continue;
                }
            }
            match o.layout {
                Layout::Inline => {
                    emit(&mut r, h, hi, i, None, &hls, o, nw, dim);
                    i += 1;
                }
                Layout::SideBySide => {
                    if h.lines[i].kind == Kind::Context {
                        emit(&mut r, h, hi, i, Some(i), &hls, o, nw, dim);
                        i += 1;
                        continue;
                    }
                    let ds = i;
                    while i < h.lines.len() && h.lines[i].kind == Kind::Del {
                        i += 1;
                    }
                    let as_ = i;
                    while i < h.lines.len() && h.lines[i].kind == Kind::Add {
                        i += 1;
                    }
                    let (nd, na) = (as_ - ds, i - as_);
                    for k in 0..nd.max(na) {
                        let l = (k < nd).then_some(ds + k);
                        let rr = (k < na).then_some(as_ + k);
                        emit_pair(&mut r, h, hi, l, rr, &hls, o, nw, dim);
                    }
                }
            }
        }
    }
    r
}

#[allow(clippy::too_many_arguments)]
fn emit(r: &mut Rendered, h: &Hunk, hi: usize, i: usize, pair: Option<usize>, hls: &[Line<'static>], o: &RenderOpts, nw: usize, dim: Style) {
    if o.layout == Layout::SideBySide {
        emit_pair(r, h, hi, Some(i), pair.or(Some(i)), hls, o, nw, dim);
        return;
    }
    let l = &h.lines[i];
    let sign = match l.kind {
        Kind::Context => ' ',
        Kind::Add => '+',
        Kind::Del => '-',
    };
    let mut spans = vec![Span::styled(format!("{} {} {sign}", num(l.old_no, nw), num(l.new_no, nw)), dim)];
    spans.extend(style_line(l, hls, i, o.tab_width).spans);
    r.lines.push(Line::from(spans));
    r.rows.push(Row::Line(hi, i));
}

#[allow(clippy::too_many_arguments)]
fn emit_pair(r: &mut Rendered, h: &Hunk, hi: usize, left: Option<usize>, right: Option<usize>, hls: &[Line<'static>], o: &RenderOpts, nw: usize, dim: Style) {
    let text_w = o.side_width.saturating_sub(nw + 2).max(1);
    let side = |idx: Option<usize>, old: bool| -> Vec<Span<'static>> {
        match idx {
            Some(i) => {
                let l = &h.lines[i];
                let no = if old { l.old_no } else { l.new_no };
                let body = style_line(l, hls, i, o.tab_width);
                let body = pad_to(crate::diff::slice(&body, o.hscroll, text_w), text_w);
                let mut v = vec![Span::styled(format!("{} ", num(no, nw)), dim)];
                v.extend(body.spans);
                v
            }
            None => vec![Span::styled(" ".repeat(text_w + nw + 1), dim)],
        }
    };
    let mut spans = side(left, true);
    spans.push(Span::styled("│", dim));
    spans.extend(side(right, false));
    r.lines.push(Line::from(spans));
    r.rows.push(Row::Pair(hi, left, right));
}

/// Slice of a line by cells `[skip, skip+width)`.
pub fn slice(line: &Line<'_>, skip: usize, width: usize) -> Line<'static> {
    let mut out = Vec::new();
    let mut col = 0;
    let end = skip + width;
    for span in &line.spans {
        let mut s = String::new();
        for c in span.content.chars() {
            let w = crate::sanitize::char_width(c);
            let (a, b) = (col, col + w);
            col = b;
            if b <= skip || (w == 0 && a < skip) {
                continue;
            }
            if a >= end {
                break;
            }
            if a < skip || b > end {
                s.extend(std::iter::repeat_n(' ', b.min(end) - a.max(skip)));
            } else {
                s.push(c);
            }
        }
        if !s.is_empty() {
            out.push(Span::styled(s, span.style));
        }
    }
    Line::from(out)
}

/// Patch/file text as unified diff (for `--print`).
pub fn to_unified(file: &FileDiff) -> String {
    let mut s = format!(
        "--- {}\n+++ {}\n",
        file.old.as_deref().map_or("/dev/null".into(), |p| format!("a/{p}")),
        file.new.as_deref().map_or("/dev/null".into(), |p| format!("b/{p}"))
    );
    if file.binary {
        s.push_str("Binary files differ\n");
    }
    for h in &file.hunks {
        s.push_str(&format!("@@ -{},{} +{},{} @@\n", h.old_start, h.old_len, h.new_start, h.new_len));
        for l in &h.lines {
            let c = match l.kind {
                Kind::Context => ' ',
                Kind::Add => '+',
                Kind::Del => '-',
            };
            s.push(c);
            s.push_str(&visible_line(&l.text, 4));
            s.push('\n');
            if l.no_newline {
                s.push_str("\\ No newline at end of file\n");
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn one_changed_line_one_hunk() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nd\nE\nf\ng\nh\n";
        let f = diff_texts(old, new, Some("x"), Some("x"));
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!((h.old_start, h.old_len, h.new_start, h.new_len), (2, 7, 2, 7));
    }

    #[test]
    fn one_word_one_range() {
        let (a, b) = inline_ranges("let foo = bar;", "let foo = baz;").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(&"let foo = baz;"[b[0].0..b[0].1], "baz");
        assert!(inline_ranges(&"x".repeat(INLINE_MAX_CHARS + 1), "y").is_none());
    }

    const SAMPLE: &str = "diff --git a/one.txt b/one.txt\nindex 1..2 100644\n--- a/one.txt\n+++ b/one.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n\\ No newline at end of file\ndiff --git a/old name.txt b/new name.txt\nsimilarity index 90%\nrename from old name.txt\nrename to new name.txt\ndiff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\ndiff --git \"a/t\\303\\244b.txt\" \"b/t\\303\\244b.txt\"\nnew file mode 100644\n--- /dev/null\n+++ \"b/t\\303\\244b.txt\"\n@@ -0,0 +1 @@\n+x\n";

    #[test]
    fn parse_git_diff_cases() {
        let f = parse_unified(SAMPLE);
        assert_eq!(f.len(), 4);
        assert_eq!(f[0].hunks[0].lines.len(), 3);
        assert!(f[0].hunks[0].lines[2].no_newline);
        assert_eq!(f[0].hunks[0].lines[2].kind, Kind::Add);
        assert!(f[1].rename);
        assert_eq!(f[1].old.as_deref(), Some("old name.txt"));
        assert_eq!(f[1].new.as_deref(), Some("new name.txt"));
        assert!(f[2].binary);
        assert_eq!(f[3].new.as_deref(), Some("täb.txt"));
        assert_eq!(f[3].old, None);
        assert_eq!(f[3].status(), 'A');
    }

    #[test]
    fn partial_selection_recounts() {
        let f = diff_texts("a\nb\nc\n", "a\nB\nc\nd\n", Some("p q.txt"), Some("p q.txt"));
        let h = &f.hunks[0];
        // Select only the addition of "d"
        let sel: Vec<bool> = h.lines.iter().map(|l| l.text == "d").collect();
        let p = hunk_patch(&f, 0, Some(&sel)).unwrap();
        assert!(p.contains("@@ -1,3 +1,4 @@"), "{p}");
        assert!(p.contains("\n b\n"), "unselected '-' becomes context: {p}");
        assert!(!p.contains("+B"), "{p}");
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
        Command::new("git").arg("-C").arg(dir).args(args).output().expect("git")
    }

    fn git_apply_check(patch: &str, dir: &std::path::Path) -> bool {
        use std::io::Write;
        let mut c = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["apply", "--cached", "--check", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(patch.as_bytes()).unwrap();
        let out = c.wait_with_output().unwrap();
        if !out.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        }
        out.status.success()
    }

    /// A single-hunk patch from a file with two hunks passes `git apply --cached --check` (path with a space).
    #[test]
    fn hunk_patch_applies_in_temp_repo() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        assert!(git(d, &["init", "-q"]).status.success());
        let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 2\n", "line two\n").replace("line 18\n", "line eighteen\n");
        std::fs::write(d.join("a file.txt"), &old).unwrap();
        assert!(git(d, &["add", "a file.txt"]).status.success());
        std::fs::write(d.join("a file.txt"), &new).unwrap();
        let out = git(d, &["diff", "-U3", "--no-ext-diff", "--no-textconv", "--no-color"]);
        let files = parse_unified(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].path(), "a file.txt");
        let p = hunk_patch(&files[0], 1, None).unwrap();
        assert!(git_apply_check(&p, d), "{p}");
        // Partial selection: only the "+" lines of the second hunk without "-" also applies
        let h = &files[0].hunks[1];
        let sel: Vec<bool> = h.lines.iter().map(|l| l.kind == Kind::Add).collect();
        let p = hunk_patch(&files[0], 1, Some(&sel)).unwrap();
        assert!(git_apply_check(&p, d), "{p}");
        // The same hunk built by similar matches git's numbers
        let own = diff_texts(&old, &new, Some("a file.txt"), Some("a file.txt"));
        assert_eq!(own.hunks[1].old_start, files[0].hunks[1].old_start);
    }

    #[test]
    fn render_sanitizes_and_folds() {
        let old: String = (1..=12).map(|i| format!("l{i}\n")).collect::<String>();
        let new = old.replace("l1\n", "l1\x1b]52;c;x\x07\n").replace("l12\n", "L12\n");
        let mut f = diff_texts(&old, &new, Some("x"), Some("x"));
        // One hunk with long context: glue with context of 12
        let d = TextDiff::from_lines(old.as_str(), new.as_str());
        assert!(d.ops().len() > 1);
        f.hunks = vec![{
            let mut h = Hunk {
                old_start: 1,
                old_len: 12,
                new_start: 1,
                new_len: 12,
                ..Default::default()
            };
            for ch in d.iter_all_changes() {
                let (t, nn) = strip_eol(ch.value());
                h.lines.push(DiffLine {
                    kind: match ch.tag() {
                        ChangeTag::Equal => Kind::Context,
                        ChangeTag::Insert => Kind::Add,
                        ChangeTag::Delete => Kind::Del,
                    },
                    old_no: ch.old_index().map(|i| i + 1),
                    new_no: ch.new_index().map(|i| i + 1),
                    text: t.into(),
                    no_newline: nn,
                    inline: vec![],
                });
            }
            h
        }];
        let o = RenderOpts {
            layout: Layout::Inline,
            fold: true,
            side_width: 40,
            hscroll: 0,
            tab_width: 4,
        };
        let r = render(&f, None, &o);
        let text: Vec<String> = r.lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert!(text.iter().all(|t| !t.contains('\x1b')), "{text:?}");
        assert_eq!(r.rows.iter().filter(|r| matches!(r, Row::Fold(..))).count(), 1, "{text:?}");
        let o2 = RenderOpts { layout: Layout::SideBySide, ..o };
        let r2 = render(&f, None, &o2);
        assert!(r2.lines.len() > 3);
    }

    #[test]
    fn messages_follow_language() {
        // The process language is global and shared by parallel tests: look in the catalogs directly
        let (ru, _) = saysdf_i18n::load("ru", None);
        assert_eq!(ru.text("render-diff", "no differences", &[]), "нет различий"); // i18n-allow-cyrillic
        assert_eq!(
            ru.plural("render-diff", "{n} unchanged line", "{n} unchanged lines", 5, &[]),
            "5 неизменённых строк" // i18n-allow-cyrillic
        );
        let (en, _) = saysdf_i18n::load("en", None);
        assert_eq!(en.text("render-diff", "no differences", &[]), "no differences");
        assert_eq!(
            en.plural("render-diff", "{n} unchanged line", "{n} unchanged lines", 1, &[]),
            "1 unchanged line"
        );
    }
}
