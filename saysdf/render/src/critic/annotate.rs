//! Building a new CriticMarkup annotation into a text (design D2).
//!
//! [`annotate`] inserts markup at two byte offsets (one for point marks); the bytes between them
//! and everywhere else are never rewritten. Every refusal is a [`Refusal`] with a localized
//! message; nothing is produced on refusal. The final guard is a round trip: parsing the result
//! must give the old marks (shifted) plus exactly the intended new mark.

use std::fmt;
use std::ops::Range;

use super::{parse, Kind, Mark};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("render-critic");

/// CriticMarkup tokens that may appear neither in the wrapped selection nor in the typed text.
pub const TOKENS: [&str; 11] = ["{++", "++}", "{--", "--}", "{~~", "~>", "~~}", "{==", "==}", "{>>", "<<}"];

/// What to write. Typed text uses `\n` for newlines; it is written with the file's line ending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `{~~selection~>new~~}`; needs a non-empty selection.
    Substitute(String),
    /// `{--selection--}`; needs a non-empty selection.
    Delete,
    /// `{++text++}` at the caret, or after the selection end when the selection is not empty.
    Insert(String),
    /// `{==selection==}{>>comment<<}`, or `{>>comment<<}` at the caret for an empty selection.
    Comment(String),
}

/// One annotation request. `start..end` is the selection in bytes of the text (`start == end`
/// is an empty selection at the caret); callers get it from [`super::byte_offset`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub action: Action,
}

/// Where a conflicting token was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Selection,
    Text,
}

/// Why an annotation was not built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The range is reversed, past the end or not on a character boundary.
    BadRange,
    /// Substitute or delete with an empty selection.
    EmptySelection,
    /// The typed text is empty.
    EmptyText,
    /// The selection or the typed text contains a CriticMarkup token.
    Token { token: &'static str, place: Place },
    /// The annotation would be inside an existing mark, overlap it or contain it.
    Overlap,
    /// A selection end is at or inside the UTF-8 byte order mark.
    Bom,
    /// A selection end is between `\r` and `\n`.
    SplitsLineBreak,
    /// The result would not parse as the old marks plus exactly the new one.
    RoundTrip,
    /// The mark to remove or amend is no longer at that place in the text.
    Stale,
    /// The mark has no typed text to amend (a deletion).
    NothingToAmend,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Refusal::BadRange => I18N.t("the selection is outside the text").into_owned(),
            Refusal::EmptySelection => I18N.t("select some text first").into_owned(),
            Refusal::EmptyText => I18N.t("the annotation text is empty").into_owned(),
            Refusal::Token { token, place: Place::Selection } => {
                I18N.f("the selection contains the markup token {token}", &[("token", token)])
            }
            Refusal::Token { token, place: Place::Text } => {
                I18N.f("the text contains the markup token {token}", &[("token", token)])
            }
            Refusal::Overlap => I18N.t("annotations cannot overlap or nest").into_owned(),
            Refusal::Bom => I18N.t("the selection touches the byte order mark").into_owned(),
            Refusal::SplitsLineBreak => I18N.t("the selection splits a line break").into_owned(),
            Refusal::RoundTrip => {
                I18N.t("the annotation would conflict with the markup around it").into_owned()
            }
            Refusal::Stale => {
                I18N.t("the annotation is no longer at this place in the text").into_owned()
            }
            Refusal::NothingToAmend => I18N.t("a deletion has no text to amend").into_owned(),
        };
        f.write_str(&s)
    }
}

impl std::error::Error for Refusal {}

/// The earliest conflicting token in `s`.
fn find_token(s: &str) -> Option<&'static str> {
    TOKENS
        .iter()
        .filter_map(|t| s.find(t).map(|i| (i, *t)))
        .min_by_key(|(i, t)| (*i, std::cmp::Reverse(t.len())))
        .map(|(_, t)| t)
}

/// Line ending of the line containing byte `at`: its own break, or the previous one on a last
/// line without a break; LF when the text has no line break at all.
pub(super) fn line_ending(text: &str, at: usize) -> &'static str {
    let crlf_at = |nl: usize| nl > 0 && text.as_bytes()[nl - 1] == b'\r';
    if let Some(i) = text[at..].find('\n') {
        return if crlf_at(at + i) { "\r\n" } else { "\n" };
    }
    match text[..at].rfind('\n') {
        Some(nl) if crlf_at(nl) => "\r\n",
        _ => "\n",
    }
}

/// Typed text with every newline (`\n`, or `\r\n` as typed) written as `eol`.
pub(super) fn with_line_ending(typed: &str, eol: &str) -> String {
    typed.replace("\r\n", "\n").replace('\n', eol)
}

pub(super) fn check_text(typed: &str) -> Result<(), Refusal> {
    if typed.is_empty() {
        return Err(Refusal::EmptyText);
    }
    match find_token(typed) {
        Some(token) => Err(Refusal::Token { token, place: Place::Text }),
        None => Ok(()),
    }
}

fn check_endpoint(text: &str, p: usize) -> Result<(), Refusal> {
    if text.starts_with('\u{feff}') && p < 3 {
        return Err(Refusal::Bom);
    }
    if text[..p].ends_with('\r') && text[p..].starts_with('\n') {
        return Err(Refusal::SplitsLineBreak);
    }
    Ok(())
}

/// The new mark expected after the insertion, in offsets of the new text.
struct Plan {
    /// Insert `open` at `at.start` and `close` at `at.end` (old-text offsets).
    at: Range<usize>,
    open: String,
    close: String,
    mark: Mark,
}

fn plan(text: &str, edit: &Edit) -> Result<Plan, Refusal> {
    let (s, e) = (edit.start, edit.end);
    let eol = line_ending(text, s);
    let sel = e - s;
    let wrap = |open: &str, close: String, kind, new_text: Option<Range<usize>>, comment| Plan {
        at: s..e,
        mark: Mark {
            kind,
            range: s..e + open.len() + close.len(),
            text: s + open.len()..e + open.len(),
            new_text,
            comment,
        },
        open: open.to_string(),
        close,
    };
    let point = |at: usize, open: &str, body: String, close: &str, kind| {
        let start = at + open.len();
        let all = format!("{open}{body}{close}");
        Plan {
            at: at..at,
            mark: Mark {
                kind,
                range: at..at + all.len(),
                text: start..start + body.len(),
                new_text: None,
                comment: None,
            },
            open: all,
            close: String::new(),
        }
    };
    Ok(match &edit.action {
        Action::Substitute(new) => {
            if sel == 0 {
                return Err(Refusal::EmptySelection);
            }
            check_text(new)?;
            let new = with_line_ending(new, eol);
            let n = e + 5; // `{~~` + old + `~>`
            let close = format!("~>{new}~~}}");
            wrap("{~~", close, Kind::Substitute, Some(n..n + new.len()), None)
        }
        Action::Delete => {
            if sel == 0 {
                return Err(Refusal::EmptySelection);
            }
            wrap("{--", "--}".to_string(), Kind::Delete, None, None)
        }
        Action::Insert(typed) => {
            check_text(typed)?;
            point(e, "{++", with_line_ending(typed, eol), "++}", Kind::Insert)
        }
        Action::Comment(typed) => {
            check_text(typed)?;
            let body = with_line_ending(typed, eol);
            if sel == 0 {
                point(s, "{>>", body, "<<}", Kind::Comment)
            } else {
                let c = e + 9; // `{==` + sel + `==}{>>`
                let close = format!("==}}{{>>{body}<<}}");
                wrap("{==", close, Kind::Highlight, None, Some(c..c + body.len()))
            }
        }
    })
}

/// Round-trip guard shared by every writer: `parse(out)` must equal `old` with the marks that
/// start at or after `from` (old offsets) shifted by `delta`, and `new` (if any) placed right
/// before them. `old` must not contain the mark being removed or amended.
pub(super) fn round_trip(
    out: &str,
    old: Vec<Mark>,
    from: usize,
    delta: isize,
    new: Option<Mark>,
) -> Result<(), Refusal> {
    let shift = |r: &Range<usize>| {
        let m = |x: usize| x.wrapping_add_signed(delta);
        m(r.start)..m(r.end)
    };
    let mut expected: Vec<Mark> = Vec::with_capacity(old.len() + 1);
    let mut new = new;
    for m in old {
        if m.range.start >= from {
            expected.extend(new.take());
            expected.push(Mark {
                kind: m.kind,
                range: shift(&m.range),
                text: shift(&m.text),
                new_text: m.new_text.as_ref().map(shift),
                comment: m.comment.as_ref().map(shift),
            });
        } else {
            expected.push(m);
        }
    }
    expected.extend(new);
    if parse(out) != expected {
        return Err(Refusal::RoundTrip);
    }
    Ok(())
}

/// Builds `text` with the annotation `edit` added, or says why it cannot be added.
pub fn annotate(text: &str, edit: &Edit) -> Result<String, Refusal> {
    let (s, e) = (edit.start, edit.end);
    if s > e || e > text.len() || !text.is_char_boundary(s) || !text.is_char_boundary(e) {
        return Err(Refusal::BadRange);
    }
    let p = plan(text, edit)?;
    let wraps = p.at.start != p.at.end;
    let old = parse(text);
    // Nesting / overlap: the written range must lie fully before or after every mark
    // (touching is allowed).
    let conflicts = |m: &Mark| {
        if wraps {
            m.range.start < p.at.end && p.at.start < m.range.end
        } else {
            m.range.start < p.at.start && p.at.start < m.range.end
        }
    };
    if old.iter().any(conflicts) {
        return Err(Refusal::Overlap);
    }
    if wraps {
        check_endpoint(text, s)?;
        check_endpoint(text, e)?;
        if let Some(token) = find_token(&text[s..e]) {
            return Err(Refusal::Token { token, place: Place::Selection });
        }
    } else {
        check_endpoint(text, p.at.start)?;
    }

    let mut out = String::with_capacity(text.len() + p.open.len() + p.close.len());
    out.push_str(&text[..p.at.start]);
    out.push_str(&p.open);
    out.push_str(&text[p.at.start..p.at.end]);
    out.push_str(&p.close);
    out.push_str(&text[p.at.end..]);

    let added = p.open.len() + p.close.len();
    round_trip(&out, old, p.at.end, added as isize, Some(p.mark))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, start: usize, end: usize, action: Action) -> Result<String, Refusal> {
        annotate(text, &Edit { start, end, action })
    }

    /// Byte range of `needle` in `text`.
    fn at(text: &str, needle: &str) -> (usize, usize) {
        let i = text.find(needle).unwrap();
        (i, i + needle.len())
    }

    fn sub(s: &str) -> Action {
        Action::Substitute(s.into())
    }
    fn ins(s: &str) -> Action {
        Action::Insert(s.into())
    }
    fn com(s: &str) -> Action {
        Action::Comment(s.into())
    }

    #[test]
    fn every_kind_on_cyrillic() {
        let t = "Это кот и лишнее слово."; // i18n-allow-cyrillic
        let (s, e) = at(t, "кот"); // i18n-allow-cyrillic
        assert_eq!(run(t, s, e, sub("пёс")).unwrap(), "Это {~~кот~>пёс~~} и лишнее слово."); // i18n-allow-cyrillic
        let (s, e) = at(t, "лишнее"); // i18n-allow-cyrillic
        assert_eq!(run(t, s, e, Action::Delete).unwrap(), "Это кот и {--лишнее--} слово."); // i18n-allow-cyrillic
        assert_eq!(run(t, s, s, ins("новое ")).unwrap(), "Это кот и {++новое ++}лишнее слово."); // i18n-allow-cyrillic
        // Insert with a selection goes after its end.
        assert_eq!(run(t, s, e, ins("!")).unwrap(), "Это кот и лишнее{++!++} слово."); // i18n-allow-cyrillic
        assert_eq!(
            run(t, s, e, com("зачем")).unwrap(), // i18n-allow-cyrillic
            "Это кот и {==лишнее==}{>>зачем<<} слово." // i18n-allow-cyrillic
        );
        assert_eq!(
            run(t, s, s, com("уточнить дату")).unwrap(), // i18n-allow-cyrillic
            "Это кот и {>>уточнить дату<<}лишнее слово." // i18n-allow-cyrillic
        );
    }

    #[test]
    fn crlf_multiline_selection_and_text() {
        let t = "one\r\ntwo\r\nthree\r\n";
        let (s, _) = at(t, "two");
        let (_, e) = at(t, "three");
        let out = run(t, s, e, sub("a\nb")).unwrap();
        assert_eq!(out, "one\r\n{~~two\r\nthree~>a\r\nb~~}\r\n");
        let out = run(t, 0, 0, com("first\nsecond")).unwrap();
        assert_eq!(out, "{>>first\r\nsecond<<}one\r\ntwo\r\nthree\r\n");
        // Typed CRLF is not doubled.
        assert_eq!(run(t, 0, 0, ins("x\r\ny")).unwrap(), "{++x\r\ny++}one\r\ntwo\r\nthree\r\n");
    }

    #[test]
    fn mixed_file_uses_the_start_line_ending() {
        let t = "lf\ncrlf\r\nlast";
        assert_eq!(run(t, 0, 0, ins("a\nb")).unwrap(), "{++a\nb++}lf\ncrlf\r\nlast");
        let (s, _) = at(t, "crlf");
        assert!(run(t, s, s, ins("a\nb")).unwrap().contains("{++a\r\nb++}"));
        // Last line without a break: the previous break decides.
        let (s, _) = at(t, "last");
        assert!(run(t, s, s, ins("a\nb")).unwrap().contains("{++a\r\nb++}"));
        assert_eq!(run("x", 0, 0, ins("a\nb")).unwrap(), "{++a\nb++}x");
    }

    #[test]
    fn bom_file() {
        let t = "\u{feff}word\r\n";
        assert_eq!(run(t, 3, 7, Action::Delete).unwrap(), "\u{feff}{--word--}\r\n");
        assert_eq!(run(t, 3, 3, ins("a")).unwrap(), "\u{feff}{++a++}word\r\n");
        assert_eq!(run(t, 0, 7, Action::Delete), Err(Refusal::Bom));
        assert_eq!(run(t, 0, 0, ins("a")), Err(Refusal::Bom));
        assert_eq!(run(t, 1, 7, Action::Delete), Err(Refusal::BadRange));
    }

    #[test]
    fn line_break_is_not_split() {
        let t = "a\r\nb";
        assert_eq!(run(t, 0, 2, Action::Delete), Err(Refusal::SplitsLineBreak));
        assert_eq!(run(t, 2, 2, ins("x")), Err(Refusal::SplitsLineBreak));
        assert_eq!(run(t, 0, 3, Action::Delete).unwrap(), "{--a\r\n--}b");
    }

    #[test]
    fn empty_selection_and_text() {
        let t = "abc";
        assert_eq!(run(t, 1, 1, Action::Delete), Err(Refusal::EmptySelection));
        assert_eq!(run(t, 1, 1, sub("x")), Err(Refusal::EmptySelection));
        assert_eq!(run(t, 0, 1, sub("")), Err(Refusal::EmptyText));
        assert_eq!(run(t, 1, 1, ins("")), Err(Refusal::EmptyText));
        assert_eq!(run(t, 0, 1, com("")), Err(Refusal::EmptyText));
        assert_eq!(run(t, 2, 1, Action::Delete), Err(Refusal::BadRange));
        assert_eq!(run(t, 0, 9, Action::Delete), Err(Refusal::BadRange));
    }

    #[test]
    fn every_token_is_refused() {
        for token in TOKENS {
            let typed = format!("a {token} b");
            let want = Err(Refusal::Token { token, place: Place::Text });
            assert_eq!(run("xyz", 0, 1, sub(&typed)), want, "{token}");
            assert_eq!(run("xyz", 1, 1, ins(&typed)), want, "{token}");
            assert_eq!(run("xyz", 1, 1, com(&typed)), want, "{token}");
            // In the selection (text without marks: unbalanced tokens only parse as plain text).
            let t = format!("x {token} y");
            let want = Err(Refusal::Token { token, place: Place::Selection });
            assert_eq!(run(&t, 0, t.len(), Action::Delete), want, "{token}");
            assert_eq!(run(&t, 0, t.len(), com("c")), want, "{token}");
        }
        // The spec scenario: `a ~> b` as replacement names the token.
        let r = run("x", 0, 1, sub("a ~> b")).unwrap_err();
        assert!(r.to_string().contains("~>"), "{r}");
    }

    #[test]
    fn nested_and_overlapping_are_refused() {
        let t = "aa {--del--} bb";
        let (ms, me) = at(t, "{--del--}");
        // Inside.
        assert_eq!(run(t, ms + 3, ms + 6, Action::Delete), Err(Refusal::Overlap));
        assert_eq!(run(t, ms + 4, ms + 4, ins("x")), Err(Refusal::Overlap));
        assert_eq!(run(t, ms + 4, ms + 4, com("x")), Err(Refusal::Overlap));
        // Overlapping start / end.
        assert_eq!(run(t, ms + 4, t.len(), Action::Delete), Err(Refusal::Overlap));
        assert_eq!(run(t, 0, ms + 4, sub("x")), Err(Refusal::Overlap));
        // Containing.
        assert_eq!(run(t, 0, t.len(), com("x")), Err(Refusal::Overlap));
        // Insert after a selection that ends inside a mark.
        assert_eq!(run(t, 0, ms + 4, ins("x")), Err(Refusal::Overlap));
        // Between a highlight and its comment.
        let t = "{==a==}{>>c<<}";
        assert_eq!(run(t, 7, 7, ins("x")), Err(Refusal::Overlap));
        assert!(run("aa {--del--} bb", me, me, ins("x")).is_ok());
    }

    #[test]
    fn adjacent_to_existing_mark_is_allowed() {
        let t = "aa{--del--}bb";
        let (ms, me) = at(t, "{--del--}");
        assert_eq!(run(t, 0, ms, Action::Delete).unwrap(), "{--aa--}{--del--}bb");
        assert_eq!(run(t, me, t.len(), sub("x")).unwrap(), "aa{--del--}{~~bb~>x~~}");
        assert_eq!(run(t, me, me, ins("x")).unwrap(), "aa{--del--}{++x++}bb");
        assert_eq!(run(t, ms, ms, com("x")).unwrap(), "aa{>>x<<}{--del--}bb");
    }

    #[test]
    fn round_trip_guard() {
        // A point comment right after a bare highlight would attach to it.
        assert_eq!(run("{==a==}b", 7, 7, com("x")), Err(Refusal::RoundTrip));
        // An unclosed opener earlier would swallow the new closer.
        assert_eq!(run("{++ open x", 9, 9, ins("y")), Err(Refusal::RoundTrip));
    }

    #[test]
    fn trailing_and_leading_marker_chars() {
        let cases: [(&str, Action); 10] = [
            ("a-", Action::Delete),
            ("-a", Action::Delete),
            ("a~", sub("b~")),
            ("~a", sub("~b")),
            ("a=", com("c<")),
            ("=a", com(">c")),
            ("a+", ins("b+")),
            ("a", ins("+b")),
            ("a>", sub(">b")),
            ("a<", com("<")),
        ];
        for (t, action) in cases {
            let out = run(t, 0, t.len(), action.clone()).unwrap_or_else(|r| panic!("{t:?} {action:?}: {r}"));
            assert_eq!(parse(&out).len(), 1, "{out}");
        }
    }

    /// Property: for every kind at every boundary of a text with marks, a success parses back as
    /// the old marks plus one, and the bytes outside the markup are unchanged.
    #[test]
    fn round_trip_property() {
        let t = "\u{feff}Ab {++i++} кот\r\n{~~o~>n~~} {==h==}{>>c<<}\nend {>>p<<}"; // i18n-allow-cyrillic
        let n_old = parse(t).len();
        let bounds: Vec<usize> = (0..=t.len()).filter(|&i| t.is_char_boundary(i)).collect();
        let actions = [sub("x\ny"), Action::Delete, ins("x\ny"), com("x\ny")];
        let mut ok = 0;
        for &s in &bounds {
            for &e in bounds.iter().filter(|&&e| e >= s) {
                for a in &actions {
                    let Ok(out) = run(t, s, e, a.clone()) else { continue };
                    ok += 1;
                    let marks = parse(&out);
                    assert_eq!(marks.len(), n_old + 1, "{s}..{e} {a:?}: {out:?}");
                    // Markup is inserted only at or after `s` and at or before `e`; the selection
                    // itself is kept verbatim.
                    assert!(out.starts_with(&t[..s]) && out.ends_with(&t[e..]), "{out:?}");
                    assert!(out[s..].contains(&t[s..e]), "{out:?}");
                }
            }
        }
        assert!(ok > 100, "{ok}");
    }

    #[test]
    fn messages_are_localized() {
        let (ru, _) = saysdf_i18n::load("ru", None);
        assert_eq!(
            ru.text("render-critic", "annotations cannot overlap or nest", &[]),
            "аннотации не могут пересекаться или вкладываться" // i18n-allow-cyrillic
        );
        assert_eq!(Refusal::Overlap.to_string(), "annotations cannot overlap or nest");
    }
}
