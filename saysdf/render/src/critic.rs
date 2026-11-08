//! saysdfTerm: CriticMarkup model shared by the text-annotation features.
//!
//! [`parse`] finds `{++ins++}`, `{--del--}`, `{~~old~>new~~}`, `{==hl==}` (optionally followed
//! immediately by `{>>comment<<}`, which makes it one mark) and standalone `{>>comment<<}`.
//! All ranges are byte offsets into the parsed text and always lie on char boundaries.
//! Marks may span several lines. An unclosed opener (and a substitution without `~>`) is plain text.

use std::ops::Range;

mod annotate;
mod pos;
mod remove;
pub mod style;
pub use annotate::{annotate, Action, Edit, Place, Refusal, TOKENS};
pub use pos::{byte_offset, position, Pos};
pub use remove::{amend, remove};

/// Kind of a CriticMarkup mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Insert,
    Delete,
    Substitute,
    /// `{==text==}`, optionally with an attached comment.
    Highlight,
    /// A comment that does not follow a highlight.
    Comment,
}

/// One parsed mark. Ranges are byte offsets into the parsed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub kind: Kind,
    /// The whole mark, openers and closers included (for a highlight with a comment, both braces).
    pub range: Range<usize>,
    /// Main content: inserted / deleted / highlighted / comment text; the old text for a substitution.
    pub text: Range<usize>,
    /// Substitution only: the new text.
    pub new_text: Option<Range<usize>>,
    /// Highlight only: the text of the attached comment.
    pub comment: Option<Range<usize>>,
}

/// Finds `close` in `text` starting at `from`; returns the start of `close`.
fn find_from(text: &str, from: usize, close: &str) -> Option<usize> {
    text.get(from..)?.find(close).map(|i| from + i)
}

/// Parses all marks in `text`, in order of appearance.
pub fn parse(text: &str) -> Vec<Mark> {
    let bytes = text.as_bytes();
    let mut marks = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let (kind, close) = match bytes[i + 1] {
            b'+' => (Kind::Insert, "++}"),
            b'-' => (Kind::Delete, "--}"),
            b'~' => (Kind::Substitute, "~~}"),
            b'=' => (Kind::Highlight, "==}"),
            b'>' => (Kind::Comment, "<<}"),
            _ => {
                i += 1;
                continue;
            }
        };
        // The opener is three bytes: `{` plus the doubled marker char.
        if bytes.get(i + 2) != Some(&bytes[i + 1]) {
            i += 1;
            continue;
        }
        let inner = i + 3;
        let Some(end) = find_from(text, inner, close) else {
            i += 1;
            continue;
        };
        let mut mark = Mark {
            kind,
            range: i..end + 3,
            text: inner..end,
            new_text: None,
            comment: None,
        };
        match kind {
            Kind::Substitute => {
                let Some(arrow) = find_from(text, inner, "~>").filter(|a| a + 2 <= end) else {
                    i += 1;
                    continue;
                };
                mark.text = inner..arrow;
                mark.new_text = Some(arrow + 2..end);
            }
            Kind::Highlight => {
                if text[mark.range.end..].starts_with("{>>") {
                    let c_inner = mark.range.end + 3;
                    if let Some(c_end) = find_from(text, c_inner, "<<}") {
                        mark.comment = Some(c_inner..c_end);
                        mark.range.end = c_end + 3;
                    }
                }
            }
            _ => {}
        }
        i = mark.range.end;
        marks.push(mark);
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s<'a>(t: &'a str, r: &Range<usize>) -> &'a str {
        &t[r.clone()]
    }

    #[test]
    fn every_kind() {
        let t = "a {++ins++} b {--del--} c {~~old~>new~~} d {==hl==} e {>>note<<} f";
        let m = parse(t);
        let kinds: Vec<Kind> = m.iter().map(|x| x.kind).collect();
        assert_eq!(
            kinds,
            [Kind::Insert, Kind::Delete, Kind::Substitute, Kind::Highlight, Kind::Comment]
        );
        assert_eq!(s(t, &m[0].range), "{++ins++}");
        assert_eq!(s(t, &m[0].text), "ins");
        assert_eq!(s(t, &m[1].text), "del");
        assert_eq!(s(t, &m[2].range), "{~~old~>new~~}");
        assert_eq!(s(t, &m[2].text), "old");
        assert_eq!(s(t, m[2].new_text.as_ref().unwrap()), "new");
        assert_eq!(s(t, &m[3].text), "hl");
        assert!(m[3].comment.is_none());
        assert_eq!(s(t, &m[4].text), "note");
    }

    #[test]
    fn cyrillic() {
        let t = "Это {~~кот~>пёс~~} и {--лишнее--}!"; // i18n-allow-cyrillic
        let m = parse(t);
        assert_eq!(m.len(), 2);
        assert_eq!(s(t, &m[0].text), "кот"); // i18n-allow-cyrillic
        assert_eq!(s(t, m[0].new_text.as_ref().unwrap()), "пёс"); // i18n-allow-cyrillic
        assert_eq!(s(t, &m[1].text), "лишнее"); // i18n-allow-cyrillic
    }

    #[test]
    fn crlf_and_multiline() {
        let t = "x {++one\r\ntwo\r\n\r\nthree++} y {>>a\nb<<}";
        let m = parse(t);
        assert_eq!(m.len(), 2);
        assert_eq!(s(t, &m[0].text), "one\r\ntwo\r\n\r\nthree");
        assert_eq!(s(t, &m[1].text), "a\nb");
    }

    #[test]
    fn unclosed_openers_are_plain_text() {
        for t in ["{++ no end", "{-- x", "{~~a~>b", "{== x", "{>> x", "a { b {+ c {++"] {
            assert!(parse(t).is_empty(), "{t}");
        }
        // An unclosed opener does not hide a later valid mark.
        let t = "{++ open {--del--}";
        let m = parse(t);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].kind, Kind::Delete);
    }

    #[test]
    fn adjacent_marks() {
        let t = "{++a++}{--b--}{~~c~>d~~}";
        let m = parse(t);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].range, 0..7);
        assert_eq!(m[1].range, 7..14);
        assert_eq!(m[2].range, 14..t.len());
    }

    #[test]
    fn highlight_with_comment_is_one_mark() {
        let t = "{==word==}{>>why<<} tail";
        let m = parse(t);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].kind, Kind::Highlight);
        assert_eq!(s(t, &m[0].range), "{==word==}{>>why<<}");
        assert_eq!(s(t, &m[0].text), "word");
        assert_eq!(s(t, m[0].comment.as_ref().unwrap()), "why");
    }

    #[test]
    fn comment_separated_from_highlight_is_standalone() {
        let t = "{==word==} {>>why<<}";
        let m = parse(t);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].kind, Kind::Highlight);
        assert!(m[0].comment.is_none());
        assert_eq!(m[1].kind, Kind::Comment);
        assert_eq!(parse("{>>solo<<}")[0].kind, Kind::Comment);
    }

    #[test]
    fn highlight_with_unclosed_comment() {
        let m = parse("{==w==}{>>oops");
        assert_eq!(m.len(), 1);
        assert!(m[0].comment.is_none());
        assert_eq!(m[0].range, 0..7);
    }

    #[test]
    fn substitute_without_arrow_is_plain_text() {
        assert!(parse("{~~a~~}").is_empty());
        // `~>` after the closer belongs to later text, not to this mark.
        assert!(parse("{~~a~~} x ~> y").is_empty());
    }
}
