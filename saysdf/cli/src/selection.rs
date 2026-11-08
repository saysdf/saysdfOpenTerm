//! Keyboard selection model of the text viewers (text-annotations D5).
//!
//! A selection is an anchor and a caret, both [`Pos`] (line, grapheme column) over the lines of
//! the annotation session text ([`split_text`]), so a position means the same thing for the
//! drawing, the mouse and the file write. Motions move the caret; the anchor stays.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use saysdf_render::critic::Pos;
use saysdf_render::sanitize::{char_width, grapheme_to_col};
use unicode_segmentation::UnicodeSegmentation;

/// Lines of `text` as the viewer shows them: no BOM, no `\r\n`/`\n` breaks, and no extra empty
/// line after the final break. The line and column numbers agree with `critic::position`.
pub fn split_text(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).collect()
}

/// A caret motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordNext,
    WordPrev,
    Up,
    Down,
    LineStart,
    LineEnd,
    FileStart,
    FileEnd,
    PageUp(usize),
    PageDown(usize),
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Punct,
}

fn glen(line: &str) -> usize {
    line.graphemes(true).count()
}

fn class_of(g: &str) -> Class {
    match g.chars().next() {
        Some(c) if c.is_whitespace() => Class::Space,
        Some(c) if c.is_alphanumeric() || c == '_' => Class::Word,
        Some(_) => Class::Punct,
        None => Class::Space,
    }
}

/// Class of the grapheme under `p`; a line end counts as a space.
fn class_at(lines: &[String], p: Pos) -> Class {
    lines[p.line].graphemes(true).nth(p.col).map_or(Class::Space, class_of)
}

fn step_right(lines: &[String], p: Pos) -> Option<Pos> {
    if p.col < glen(&lines[p.line]) {
        Some(Pos { line: p.line, col: p.col + 1 })
    } else if p.line + 1 < lines.len() {
        Some(Pos { line: p.line + 1, col: 0 })
    } else {
        None
    }
}

fn step_left(lines: &[String], p: Pos) -> Option<Pos> {
    if p.col > 0 {
        Some(Pos { line: p.line, col: p.col - 1 })
    } else if p.line > 0 {
        Some(Pos { line: p.line - 1, col: glen(&lines[p.line - 1]) })
    } else {
        None
    }
}

/// Anchor and caret of a selection; empty when they are equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    anchor: Pos,
    caret: Pos,
    /// Column kept by vertical motions.
    want: usize,
}

impl Selection {
    /// Empty selection at `pos`.
    pub fn new(pos: Pos) -> Self {
        Self { anchor: pos, caret: pos, want: pos.col }
    }

    pub fn caret(&self) -> Pos {
        self.caret
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.caret
    }

    /// `(start, end)` with `start <= end`.
    pub fn range(&self) -> (Pos, Pos) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// Moves the caret to `p`, keeping the anchor (motions by visual row of the wrapped view).
    pub fn set_caret(&mut self, p: Pos) {
        self.caret = p;
        self.want = p.col;
    }

    /// Drops the selected text but keeps the caret.
    pub fn collapse(&mut self) {
        self.anchor = self.caret;
    }

    /// Moves the caret; the lines must not be empty.
    pub fn apply(&mut self, m: Motion, lines: &[String]) {
        let last = lines.len().saturating_sub(1);
        let vertical = matches!(m, Motion::Up | Motion::Down | Motion::PageUp(_) | Motion::PageDown(_));
        match m {
            Motion::Left => self.caret = step_left(lines, self.caret).unwrap_or(self.caret),
            Motion::Right => self.caret = step_right(lines, self.caret).unwrap_or(self.caret),
            Motion::WordNext => self.caret = word_next(lines, self.caret),
            Motion::WordPrev => self.caret = word_prev(lines, self.caret),
            Motion::Up => self.to_line(self.caret.line.saturating_sub(1), lines),
            Motion::Down => self.to_line(self.caret.line + 1, lines),
            Motion::PageUp(n) => self.to_line(self.caret.line.saturating_sub(n.max(1)), lines),
            Motion::PageDown(n) => self.to_line(self.caret.line + n.max(1), lines),
            Motion::LineStart => self.caret.col = 0,
            Motion::LineEnd => self.caret.col = glen(&lines[self.caret.line]),
            Motion::FileStart => self.caret = Pos { line: 0, col: 0 },
            Motion::FileEnd => self.caret = Pos { line: last, col: glen(&lines[last]) },
        }
        if !vertical {
            self.want = self.caret.col;
        }
    }

    /// Vertical move to `target` (clamped), keeping the wanted column.
    fn to_line(&mut self, target: usize, lines: &[String]) {
        let line = target.min(lines.len().saturating_sub(1));
        self.caret = Pos { line, col: self.want.min(glen(&lines[line])) };
    }

    /// Display cells `[start, end)` of line `i` to draw reversed (cells as drawn by `visible_line`).
    /// A line break inside the selection adds one cell after the line end; an empty selection
    /// is the cell of the grapheme under the caret (one cell past the end of the line).
    pub fn cells(&self, i: usize, line: &str, tab_width: usize) -> Option<(usize, usize)> {
        let (s, e) = self.range();
        if i < s.line || i > e.line {
            return None;
        }
        let n = glen(line);
        let a = if i == s.line { s.col.min(n) } else { 0 };
        let (b, brk) = if i == e.line { (e.col.min(n), false) } else { (n, true) };
        let start = grapheme_to_col(line, tab_width, a);
        let mut end = grapheme_to_col(line, tab_width, b);
        if self.is_empty() {
            end = grapheme_to_col(line, tab_width, a + 1).max(start + 1);
        } else if brk {
            end += 1;
        }
        (end > start).then_some((start, end))
    }
}

fn word_next(lines: &[String], from: Pos) -> Pos {
    let mut p = from;
    let c = class_at(lines, p);
    if c != Class::Space {
        while class_at(lines, p) == c {
            match step_right(lines, p) {
                Some(q) if q.line == p.line => p = q,
                Some(q) => {
                    p = q;
                    break;
                }
                None => return Pos { line: p.line, col: glen(&lines[p.line]) },
            }
        }
    }
    while class_at(lines, p) == Class::Space {
        match step_right(lines, p) {
            Some(q) => p = q,
            None => break,
        }
    }
    p
}

fn word_prev(lines: &[String], from: Pos) -> Pos {
    let Some(mut p) = step_left(lines, from) else { return from };
    while class_at(lines, p) == Class::Space {
        match step_left(lines, p) {
            Some(q) => p = q,
            None => return p,
        }
    }
    let c = class_at(lines, p);
    while let Some(q) = step_left(lines, p) {
        if q.line != p.line || class_at(lines, q) != c {
            break;
        }
        p = q;
    }
    p
}

/// Applies `style` to the display cells `[start, end)` of a rendered line; cells past the end of
/// the line are padded with styled spaces.
pub fn style_cells(line: Line<'static>, start: usize, end: usize, style: Style) -> Line<'static> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0;
    for span in line.spans {
        let mut run = String::new();
        let mut run_hit = false;
        for c in span.content.chars() {
            let hit = col >= start && col < end;
            col += char_width(c);
            if !run.is_empty() && hit != run_hit {
                out.push(Span::styled(std::mem::take(&mut run), if run_hit { span.style.patch(style) } else { span.style }));
            }
            run_hit = hit;
            run.push(c);
        }
        if !run.is_empty() {
            out.push(Span::styled(run, if run_hit { span.style.patch(style) } else { span.style }));
        }
    }
    if end > col {
        out.push(Span::styled(" ".repeat(end - col.max(start)), style));
    }
    Line::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(t: &str) -> Vec<String> {
        split_text(t)
    }

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn split_matches_critic_lines() {
        assert_eq!(split_text("\u{feff}a\r\nb\n"), ["a", "b"]);
        assert_eq!(split_text("a\n\nb"), ["a", "", "b"]);
        assert!(split_text("").is_empty());
        assert!(split_text("\n").is_empty());
    }

    #[test]
    fn horizontal_motions_count_graphemes() {
        let l = lines("при\u{301}вет 日本\r\nx"); // i18n-allow-cyrillic
        let mut s = Selection::new(p(0, 0));
        for _ in 0..3 {
            s.apply(Motion::Right, &l);
        }
        assert_eq!(s.caret(), p(0, 3)); // i18n-allow-cyrillic
        s.apply(Motion::LineEnd, &l);
        assert_eq!(s.caret(), p(0, 9)); // 6 letters, space, 2 wide chars
        s.apply(Motion::Right, &l); // wraps to the next line, CRLF is not a column
        assert_eq!(s.caret(), p(1, 0));
        s.apply(Motion::Left, &l);
        assert_eq!(s.caret(), p(0, 9));
        s.apply(Motion::LineStart, &l);
        s.apply(Motion::Left, &l);
        assert_eq!(s.caret(), p(0, 0));
    }

    #[test]
    fn words_cross_lines_and_scripts() {
        let l = lines("foo, bar_1\r\n  привет мир"); // i18n-allow-cyrillic
        let mut s = Selection::new(p(0, 0));
        s.apply(Motion::WordNext, &l);
        assert_eq!(s.caret(), p(0, 3)); // punctuation is its own word
        s.apply(Motion::WordNext, &l);
        assert_eq!(s.caret(), p(0, 5));
        s.apply(Motion::WordNext, &l);
        assert_eq!(s.caret(), p(1, 2));
        s.apply(Motion::WordNext, &l);
        assert_eq!(s.caret(), p(1, 9));
        s.apply(Motion::WordPrev, &l);
        assert_eq!(s.caret(), p(1, 2));
        s.apply(Motion::WordPrev, &l);
        assert_eq!(s.caret(), p(0, 5));
        s.apply(Motion::WordPrev, &l);
        s.apply(Motion::WordPrev, &l);
        assert_eq!(s.caret(), p(0, 0));
        s.apply(Motion::WordPrev, &l);
        assert_eq!(s.caret(), p(0, 0));
    }

    #[test]
    fn vertical_motions_keep_the_wanted_column() {
        let l = lines("日本語テキスト\nab\nxxxxxxxxxx\n"); // i18n-allow-cyrillic
        let mut s = Selection::new(p(0, 5));
        s.apply(Motion::Down, &l);
        assert_eq!(s.caret(), p(1, 2));
        s.apply(Motion::Down, &l);
        assert_eq!(s.caret(), p(2, 5));
        s.apply(Motion::Down, &l);
        assert_eq!(s.caret(), p(2, 5)); // last line
        s.apply(Motion::PageUp(10), &l);
        assert_eq!(s.caret(), p(0, 5));
        s.apply(Motion::PageDown(2), &l);
        assert_eq!(s.caret(), p(2, 5));
        s.apply(Motion::FileStart, &l);
        assert_eq!(s.caret(), p(0, 0));
        s.apply(Motion::FileEnd, &l);
        assert_eq!(s.caret(), p(2, 10));
    }

    #[test]
    fn range_is_normalised_and_spans_lines() {
        let l = lines("abc\ndef\nghi");
        let mut s = Selection::new(p(1, 1));
        s.apply(Motion::Up, &l);
        s.apply(Motion::Left, &l);
        assert_eq!(s.range(), (p(0, 0), p(1, 1)));
        assert!(!s.is_empty());
        // reverse drawing: line 0 whole + break cell, line 1 up to the anchor, line 2 none
        assert_eq!(s.cells(0, "abc", 4), Some((0, 4)));
        assert_eq!(s.cells(1, "def", 4), Some((0, 1)));
        assert_eq!(s.cells(2, "ghi", 4), None);
        s.collapse();
        assert!(s.is_empty());
        assert_eq!(s.cells(0, "abc", 4), Some((0, 1)));
    }

    #[test]
    fn cells_follow_wide_chars_and_line_end() {
        let s = Selection { anchor: p(0, 1), caret: p(0, 3), want: 3 };
        assert_eq!(s.cells(0, "a日本b", 4), Some((1, 5)));
        let e = Selection::new(p(0, 4));
        assert_eq!(e.cells(0, "a日本b", 4), Some((6, 7))); // past the end: one blank cell
    }

    #[test]
    fn style_cells_splits_and_pads() {
        let rev = Style::new().add_modifier(ratatui::style::Modifier::REVERSED);
        let line = style_cells(Line::raw("a日b"), 1, 3, rev);
        let spans: Vec<(String, bool)> =
            line.spans.iter().map(|s| (s.content.to_string(), s.style == rev)).collect();
        assert_eq!(spans, [("a".to_owned(), false), ("日".to_owned(), true), ("b".to_owned(), false)]);
        let padded = style_cells(Line::raw("ab"), 2, 3, rev);
        assert_eq!(padded.spans.last().map(|s| s.content.to_string()), Some(" ".to_owned()));
    }
}
