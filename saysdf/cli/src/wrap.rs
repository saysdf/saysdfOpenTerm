//! Soft wrap of long lines for the text viewers (text-annotations D12): a visual-row map
//! built once per load, reload, resize or toggle (rule 5), never per frame.
//!
//! A visual row is a grapheme range of one source line plus the display cells it covers, as
//! drawn by `visible_line` (tabs, wide characters, placeholders: `sanitize::grapheme_cells`).
//! Rows break after whitespace; a word wider than the pane is broken at a grapheme boundary and
//! a wide character is never split. Positions stay [`Pos`] (line, grapheme) like the selection.

use saysdf_render::critic::Pos;
use saysdf_render::sanitize::{col_to_grapheme, grapheme_cells, grapheme_to_col};
use unicode_segmentation::UnicodeSegmentation;

/// One visual row: graphemes `[g0, g1)` and display cells `[c0, c1)` of source line `line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub g0: usize,
    pub g1: usize,
    pub c0: usize,
    pub c1: usize,
}

/// Visual rows of a text and the first row of every source line.
#[derive(Debug, Default, Clone)]
pub struct WrapMap {
    rows: Vec<Row>,
    first: Vec<usize>,
    tab_width: usize,
    width: usize,
}

impl WrapMap {
    /// Wraps `lines` for a text area `width` cells wide. Text uses at most `width - 1` cells so
    /// the whitespace at a break and the caret cell past a line end still fit in the pane;
    /// further whitespace hangs past the edge.
    pub fn build(lines: &[String], tab_width: usize, width: usize) -> Self {
        let wc = width.saturating_sub(1).max(1);
        let mut rows = Vec::with_capacity(lines.len());
        let mut first = Vec::with_capacity(lines.len());
        for (line, text) in lines.iter().enumerate() {
            first.push(rows.len());
            let (mut g0, mut c0) = (0, 0);
            // Break opportunity: grapheme index and cell right after the last whitespace.
            let mut brk: Option<(usize, usize)> = None;
            let (mut n, mut end) = (0, 0);
            for (i, (g, (s, e))) in text.graphemes(true).zip(grapheme_cells(text, tab_width)).enumerate() {
                let space = g.chars().next().is_some_and(char::is_whitespace);
                if !space && e - c0 > wc && i > g0 {
                    let (bg, bc) = match brk {
                        Some(b) if b.0 > g0 => b,
                        _ => (i, s),
                    };
                    rows.push(Row { line, g0, g1: bg, c0, c1: bc });
                    (g0, c0, brk) = (bg, bc, None);
                }
                if space {
                    brk = Some((i + 1, e));
                }
                (n, end) = (i + 1, e);
            }
            rows.push(Row { line, g0, g1: n, c0, c1: end });
        }
        Self { rows, first, tab_width, width }
    }

    /// Number of visual rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Row `i` (clamped to the last row); `None` for an empty text.
    pub fn row(&self, i: usize) -> Option<Row> {
        self.rows.get(i.min(self.rows.len().saturating_sub(1))).copied()
    }

    /// First visual row of source line `line` (clamped).
    pub fn first_row(&self, line: usize) -> usize {
        self.first.get(line).or(self.first.last()).copied().unwrap_or(0)
    }

    /// Whether row `i` starts its source line (the gutter shows the number only there).
    pub fn is_first(&self, i: usize) -> bool {
        self.rows.get(i).is_some_and(|r| r.g0 == 0)
    }

    /// First rows of the source lines, for `Doc::with_anchors`.
    pub fn anchors(&self) -> Vec<usize> {
        self.first.clone()
    }

    /// Whether row `i` is the last row of its source line.
    pub fn is_last(&self, i: usize) -> bool {
        self.rows.get(i + 1).is_none_or(|r| r.g0 == 0)
    }

    /// Visual row and display cell within it of `pos`. A position at a break belongs to the
    /// next row; the line end belongs to the last row.
    pub fn pos_to_visual(&self, lines: &[String], pos: Pos) -> (usize, usize) {
        let Some(text) = lines.get(pos.line) else { return (0, 0) };
        let from = self.first_row(pos.line);
        let to = self.first.get(pos.line + 1).copied().unwrap_or(self.rows.len());
        let row = (from..to).rev().find(|&r| self.rows[r].g0 <= pos.col).unwrap_or(from);
        let col = grapheme_to_col(text, self.tab_width, pos.col);
        // Whitespace hanging at a break (several spaces, a tab) can lie past the pane edge: the
        // caret stays on the last column instead of leaving the screen.
        let col = col.saturating_sub(self.rows.get(row).map_or(0, |r| r.c0));
        (row, col.min(self.width.saturating_sub(1)))
    }

    /// Position drawn at display cell `col` of visual row `row` (both clamped): past the end of
    /// a row that continues on the next one, the last grapheme of the row.
    pub fn visual_to_pos(&self, lines: &[String], row: usize, col: usize) -> Pos {
        let Some(r) = self.row(row) else { return Pos { line: 0, col: 0 } };
        let row = row.min(self.rows.len() - 1);
        let text = lines.get(r.line).map_or("", String::as_str);
        let max = if self.is_last(row) { r.g1 } else { r.g1.saturating_sub(1).max(r.g0) };
        let g = col_to_grapheme(text, self.tab_width, r.c0 + col);
        Pos { line: r.line, col: g.clamp(r.g0, max) }
    }

    /// Moves `pos` by `delta` visual rows keeping display column `want` (as editors do).
    pub fn move_rows(&self, lines: &[String], pos: Pos, want: usize, delta: isize) -> Pos {
        if self.rows.is_empty() {
            return pos;
        }
        let (row, _) = self.pos_to_visual(lines, pos);
        let target = row.saturating_add_signed(delta).min(self.rows.len() - 1);
        self.visual_to_pos(lines, target, want)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use saysdf_render::sanitize::{str_width, visible_line};

    fn ls(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Visible text of every row (slice of the drawn line by the row's cells).
    fn texts(m: &WrapMap, lines: &[String]) -> Vec<String> {
        (0..m.len())
            .map(|i| {
                let r = m.row(i).unwrap();
                let v = visible_line(&lines[r.line], 4);
                let mut col = 0;
                v.chars()
                    .filter(|c| {
                        let a = col;
                        col += saysdf_render::sanitize::char_width(*c);
                        a >= r.c0 && a < r.c1
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn cyrillic_paragraph_breaks_between_words() {
        let l = ls(&["мама мыла раму долго и тщательно"]); // i18n-allow-cyrillic
        let m = WrapMap::build(&l, 4, 12);
        let t = texts(&m, &l);
        assert_eq!(t, ["мама мыла ", "раму долго ", "и тщательно"]); // i18n-allow-cyrillic
        assert!(m.is_first(0) && !m.is_first(1) && !m.is_first(2));
        for s in &t {
            assert!(str_width(s) <= 12, "{s}");
        }
        assert_eq!(m.first_row(0), 0);
    }

    #[test]
    fn wide_chars_never_split_and_rows_fit() {
        let l = ls(&["日本語のテキストです"]);
        let m = WrapMap::build(&l, 4, 7);
        for i in 0..m.len() {
            let r = m.row(i).unwrap();
            assert!(r.c1 - r.c0 <= 6, "{r:?}");
            assert_eq!((r.c1 - r.c0) % 2, 0, "{r:?}");
        }
        assert_eq!(texts(&m, &l).concat(), "日本語のテキストです");
        // Width 2: one wide char per row, even though it does not fit `width - 1`.
        let m = WrapMap::build(&l, 4, 2);
        assert_eq!(m.len(), 10);
    }

    #[test]
    fn long_word_is_broken_by_grapheme() {
        let l = ls(&["ab abcdefghijkl x", ""]);
        let m = WrapMap::build(&l, 4, 6);
        assert_eq!(texts(&m, &l), ["ab ", "abcde", "fghij", "kl x", ""]);
        assert!(m.is_first(4)); // the empty line has its own row
    }

    #[test]
    fn tabs_are_measured_from_the_line_start() {
        let l = ls(&["a\tb\tcc dd"]);
        // drawn as "a   b   cc dd": tab stops at 4 and 8
        let m = WrapMap::build(&l, 4, 9);
        let r0 = m.row(0).unwrap();
        assert_eq!((r0.g0, r0.g1, r0.c0, r0.c1), (0, 4, 0, 8));
        assert_eq!(texts(&m, &l), ["a   b   ", "cc dd"]);
        assert_eq!(m.pos_to_visual(&l, Pos { line: 0, col: 4 }), (1, 0));
        assert_eq!(m.pos_to_visual(&l, Pos { line: 0, col: 2 }), (0, 4));
    }

    #[test]
    fn positions_map_both_ways() {
        let l = ls(&["one two three", "x"]);
        let m = WrapMap::build(&l, 4, 9); // "one two ", "three"
        assert_eq!(m.len(), 3);
        assert_eq!(m.pos_to_visual(&l, Pos { line: 0, col: 9 }), (1, 1));
        assert_eq!(m.pos_to_visual(&l, Pos { line: 0, col: 13 }), (1, 5)); // line end: last row
        assert_eq!(m.visual_to_pos(&l, 1, 2), Pos { line: 0, col: 10 });
        assert_eq!(m.visual_to_pos(&l, 0, 50), Pos { line: 0, col: 7 }); // continues: last grapheme
        assert_eq!(m.visual_to_pos(&l, 1, 50), Pos { line: 0, col: 13 });
        assert_eq!(m.visual_to_pos(&l, 9, 0), Pos { line: 1, col: 0 });
        let p = m.move_rows(&l, Pos { line: 0, col: 2 }, 2, 1);
        assert_eq!(p, Pos { line: 0, col: 10 });
        assert_eq!(m.move_rows(&l, p, 2, 1), Pos { line: 1, col: 1 });
        assert_eq!(m.anchors(), [0, 2]);
    }

    #[test]
    fn whitespace_run_at_a_break_keeps_the_caret_on_screen() {
        // "ab" + 10 spaces + "cd" and a tab at a break, pane 6 cells wide
        for src in ["ab          cd", "ab			cd"] {
            let l = ls(&[src]);
            let m = WrapMap::build(&l, 4, 6);
            let n = src.chars().count();
            for col in 0..=n {
                let (row, cell) = m.pos_to_visual(&l, Pos { line: 0, col });
                assert!(row < m.len() && cell < 6, "{src:?} col={col} -> ({row}, {cell})");
            }
        }
    }
}
