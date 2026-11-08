//! Styling of CriticMarkup marks in the text viewers (text-annotations 3.4).
//!
//! The marks are parsed once per load by the shared parser (`saysdf_render::critic::parse`) and
//! turned into per-line display-cell ranges; drawing only applies them over the syntax colours
//! with [`selection::style_cells`] (the keyboard selection is applied after, so it stays on top).
//! Colours are always on in the TUI (see `pager::init_colors`), and each kind also carries a
//! modifier (strikethrough, underline, bold, dim), so marks stay distinguishable on a mono display.

use crate::selection;
use ratatui::style::Style;
use ratatui::text::Line;
// The styles are shared with the rendered Markdown view.
use saysdf_render::critic::style::{comment_badge, deleted, highlighted, inserted, markup};
use saysdf_render::critic::{self, Kind, Mark};
use saysdf_render::sanitize::grapheme_to_col;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Styled byte ranges of one mark, in order and disjoint; the gaps are markup characters.
fn segments(m: &Mark) -> Vec<(Range<usize>, Style)> {
    let mut v = Vec::new();
    match m.kind {
        Kind::Insert => v.push((m.text.clone(), inserted())),
        Kind::Delete => v.push((m.text.clone(), deleted())),
        Kind::Substitute => {
            v.push((m.text.clone(), deleted()));
            if let Some(n) = &m.new_text {
                v.push((n.clone(), inserted()));
            }
        }
        Kind::Highlight => {
            v.push((m.text.clone(), highlighted()));
            if let Some(c) = &m.comment {
                v.push((c.clone(), comment_badge()));
            }
        }
        Kind::Comment => v.push((m.text.clone(), comment_badge())),
    }
    v
}

/// Per-line display-cell ranges of all marks of a text.
#[derive(Default)]
pub struct MarkStyles {
    /// The parsed marks in text order (navigation and the counter use them, not a second parse).
    pub list: Vec<critic::Mark>,
    by_line: Vec<Vec<(usize, usize, Style)>>,
    /// How many times the text was parsed (once per load; tests check that drawing does not parse).
    #[cfg(test)]
    pub parses: usize,
}

impl MarkStyles {
    /// Parses `text` (the annotation session text) and maps the marks onto `lines`
    /// (`selection::split_text(text)`) with tabs expanded to `tab_width`.
    pub fn build(text: &str, lines: &[String], tab_width: usize) -> Self {
        let marks = critic::parse(text);
        let mut out = Self { list: marks.clone(), by_line: vec![Vec::new(); lines.len()], #[cfg(test)] parses: 1 };
        if marks.is_empty() || lines.is_empty() {
            return out;
        }
        // Byte offset of each line start (the BOM is not part of any line).
        let base = if text.starts_with('\u{feff}') { '\u{feff}'.len_utf8() } else { 0 };
        let mut starts = vec![base];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        let place = |byte: usize| -> (usize, usize) {
            let li = starts.partition_point(|&s| s <= byte).saturating_sub(1).min(lines.len() - 1);
            let line = &lines[li];
            let mut off = byte.saturating_sub(starts[li]).min(line.len());
            while !line.is_char_boundary(off) {
                off -= 1;
            }
            let g = line[..off].graphemes(true).count();
            (li, grapheme_to_col(line, tab_width, g))
        };
        let line_end = |li: usize| grapheme_to_col(&lines[li], tab_width, lines[li].graphemes(true).count());
        for m in &marks {
            let mut cur = m.range.start;
            let mut parts = segments(m);
            parts.push((m.range.end..m.range.end, Style::new()));
            for (r, style) in parts {
                if r.start > cur {
                    out.add(&place, &line_end, cur..r.start, markup());
                }
                if r.end > r.start {
                    out.add(&place, &line_end, r.clone(), style);
                }
                cur = cur.max(r.end);
            }
        }
        out
    }

    fn add(
        &mut self,
        place: &dyn Fn(usize) -> (usize, usize),
        line_end: &dyn Fn(usize) -> usize,
        r: Range<usize>,
        style: Style,
    ) {
        let (l0, c0) = place(r.start);
        let (l1, c1) = place(r.end);
        for li in l0..=l1.max(l0) {
            let a = if li == l0 { c0 } else { 0 };
            let b = if li == l1 { c1 } else { line_end(li) };
            if b > a {
                self.by_line[li].push((a, b, style));
            }
        }
    }

    /// True if the text has no marks.
    pub fn is_empty(&self) -> bool {
        self.by_line.iter().all(Vec::is_empty)
    }

    /// Applies the styles of line `i` over the rendered `line`.
    pub fn apply(&self, i: usize, line: Line<'static>) -> Line<'static> {
        match self.by_line.get(i) {
            Some(ranges) if !ranges.is_empty() => {
                ranges.iter().fold(line, |l, &(a, b, s)| selection::style_cells(l, a, b, s))
            }
            _ => line,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn styles(text: &str) -> (Vec<String>, MarkStyles) {
        let lines = selection::split_text(text);
        let s = MarkStyles::build(text, &lines, 4);
        (lines, s)
    }

    /// Text of the cells of line `i` that carry exactly `style`.
    fn styled(text: &str, i: usize, style: Style) -> String {
        let (lines, s) = styles(text);
        let line = s.apply(i, Line::raw(lines[i].clone()));
        line.spans.iter().filter(|sp| sp.style == style).map(|sp| sp.content.to_string()).collect()
    }

    #[test]
    fn each_kind_gets_its_style() {
        let t = "a {--del--} {++ins++} {~~old~>new~~} {==hl==}{>>note<<} {>>c<<}";
        assert_eq!(styled(t, 0, deleted()), "delold");
        assert_eq!(styled(t, 0, inserted()), "insnew");
        assert_eq!(styled(t, 0, highlighted()), "hl");
        assert_eq!(styled(t, 0, comment_badge()), "notec");
        assert_eq!(styled(t, 0, markup()), "{----}{++++}{~~~>~~}{====}{>><<}{>><<}");
    }

    #[test]
    fn cyrillic_and_wide_chars_map_to_cells() {
        let t = "日本 {++привет++} 語"; // i18n-allow-cyrillic
        let (_, s) = styles(t);
        // "日本 " is 5 cells, "{++" 3 more: the green text starts at cell 8.
        assert_eq!(s.by_line[0][1], (8, 14, inserted()));
        assert_eq!(styled(t, 0, inserted()), "привет"); // i18n-allow-cyrillic
    }

    #[test]
    fn multi_line_mark_covers_each_line() {
        let t = "x {--one\r\ntwo\r\nthree--} y";
        assert_eq!(styled(t, 0, deleted()), "one");
        assert_eq!(styled(t, 1, deleted()), "two");
        assert_eq!(styled(t, 2, deleted()), "three");
        assert_eq!(styled(t, 2, markup()), "--}");
    }

    #[test]
    fn bom_tabs_and_plain_text() {
        let t = "\u{feff}\t{++x++}";
        let (_, s) = styles(t);
        assert_eq!(s.by_line[0][1], (4 + 3, 4 + 4, inserted()));
        let (_, s) = styles("no marks {+ here");
        assert!(s.is_empty());
    }

    #[test]
    fn marks_stay_distinguishable_without_colours() {
        assert!(deleted().add_modifier.contains(Modifier::CROSSED_OUT));
        assert!(inserted().add_modifier.contains(Modifier::UNDERLINED));
        assert!(comment_badge().add_modifier.contains(Modifier::BOLD));
        assert!(markup().add_modifier.contains(Modifier::DIM));
    }
}
