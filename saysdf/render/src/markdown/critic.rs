//! CriticMarkup in the rendered view (change text-annotations, markdown-preview delta).
//!
//! The source is not rewritten: marks are found by the shared parser ([`crate::critic::parse`]) on
//! the whole source, and every text event of pulldown-cmark is split by the source byte offsets of
//! its characters. So block line numbers (the source-line anchors) are unaffected.
//!
//! - A mark that lies inside one leaf block and outside code/HTML is styled in place: its content
//!   gets the kind's role ([`CriticPart`]), its markup characters are hidden.
//! - A mark that crosses an element boundary (two paragraphs, two list items, half of a link or a
//!   code span) keeps its markers visible, coloured as [`CriticPart::RawMarkup`]; its content stays
//!   plain.
//! - Code spans, code blocks and HTML stay literal: a mark inside them is not touched.

use super::model::{CriticPart, Inline};
use crate::critic::{self, Kind};
use pulldown_cmark::{Event, Parser, Tag};
use std::ops::Range;

/// What a source byte inside a mark becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Markup character of a styled mark: not shown.
    Hidden,
    Part(CriticPart),
}

/// Classified source byte ranges, sorted and disjoint. Bytes outside them are plain text.
#[derive(Default)]
pub(super) struct Marks {
    ranges: Vec<(Range<usize>, Class)>,
}

fn contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

impl Marks {
    /// Classifies the marks of `src`; `opts` must be the options of the main parse.
    pub(super) fn new(src: &str, opts: pulldown_cmark::Options) -> Self {
        let marks = critic::parse(src);
        if marks.is_empty() {
            return Self::default();
        }
        // Element ranges: (range, is a block, is literal: code/HTML).
        let mut elems: Vec<(Range<usize>, bool, bool)> = Vec::new();
        for (ev, r) in Parser::new_ext(src, opts).into_offset_iter() {
            match ev {
                Event::Start(tag) => {
                    let literal = matches!(tag, Tag::CodeBlock(_) | Tag::HtmlBlock);
                    let block = matches!(
                        tag,
                        Tag::Paragraph
                            | Tag::Heading { .. }
                            | Tag::BlockQuote(_)
                            | Tag::CodeBlock(_)
                            | Tag::HtmlBlock
                            | Tag::List(_)
                            | Tag::Item
                            | Tag::FootnoteDefinition(_)
                            | Tag::Table(_)
                            | Tag::TableHead
                            | Tag::TableRow
                            | Tag::TableCell
                    );
                    elems.push((r, block, literal));
                }
                Event::Code(_) | Event::Html(_) | Event::InlineHtml(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {
                    elems.push((r, false, true))
                }
                _ => {}
            }
        }
        let mut ranges = Vec::new();
        for m in &marks {
            let mr = &m.range;
            // Literal text around the mark: leave it alone entirely.
            if elems.iter().any(|(r, _, lit)| *lit && overlaps(r, mr) && !contains(mr, r)) {
                continue;
            }
            // Styled in place iff every element either contains the mark, is disjoint from it,
            // or is an inline element inside it.
            let in_place = elems.iter().all(|(r, block, _)| {
                contains(r, mr) || !overlaps(r, mr) || (!*block && contains(mr, r))
            });
            let mut parts: Vec<(Range<usize>, CriticPart)> = match m.kind {
                Kind::Insert => vec![(m.text.clone(), CriticPart::Inserted)],
                Kind::Delete => vec![(m.text.clone(), CriticPart::Deleted)],
                Kind::Substitute => {
                    let mut v = vec![(m.text.clone(), CriticPart::Deleted)];
                    v.extend(m.new_text.clone().map(|n| (n, CriticPart::Inserted)));
                    v
                }
                Kind::Highlight => {
                    let mut v = vec![(m.text.clone(), CriticPart::Highlighted)];
                    v.extend(m.comment.clone().map(|c| (c, CriticPart::Comment)));
                    v
                }
                Kind::Comment => vec![(m.text.clone(), CriticPart::Comment)],
            };
            parts.push((mr.end..mr.end, CriticPart::RawMarkup));
            let markup = if in_place { Class::Hidden } else { Class::Part(CriticPart::RawMarkup) };
            let mut cur = mr.start;
            for (r, part) in parts {
                if r.start > cur {
                    ranges.push((cur..r.start, markup));
                }
                if in_place && r.end > r.start {
                    ranges.push((r.clone(), Class::Part(part)));
                }
                cur = cur.max(r.end);
            }
        }
        Self { ranges }
    }

    fn class_at(&self, off: usize) -> Option<Class> {
        let i = self.ranges.partition_point(|(r, _)| r.end <= off);
        self.ranges.get(i).filter(|(r, _)| r.start <= off).map(|(_, c)| *c)
    }

    /// True if the source byte `off` is a hidden markup character (a strikethrough tag starting
    /// there is the `~~` of a substitution, not real strikethrough).
    pub(super) fn hidden_at(&self, off: usize) -> bool {
        self.class_at(off) == Some(Class::Hidden)
    }

    /// Splits the text event `text` with the source range `src_range` into inlines.
    pub(super) fn split(&self, src: &str, text: &str, src_range: Range<usize>) -> Vec<Inline> {
        let plain = |s: &str| Inline::Text(s.to_owned());
        if self.ranges.is_empty() || !self.ranges.iter().any(|(r, _)| overlaps(r, &src_range)) {
            return vec![plain(text)];
        }
        let make = |c: Option<Class>, s: &str| match c {
            None => Some(plain(s)),
            Some(Class::Hidden) => None,
            Some(Class::Part(p)) => Some(Inline::Critic(p, s.to_owned())),
        };
        // Text that differs from its source (escapes, entities) is classified as a whole.
        if src.get(src_range.clone()) != Some(text) {
            return make(self.class_at(src_range.start), text).into_iter().collect();
        }
        let mut out = Vec::new();
        let mut start = 0;
        let mut cls = self.class_at(src_range.start);
        for (i, _) in text.char_indices().skip(1) {
            let c = self.class_at(src_range.start + i);
            if c != cls {
                out.extend(make(cls, &text[start..i]));
                start = i;
                cls = c;
            }
        }
        out.extend(make(cls, &text[start..]));
        out
    }
}
