//! Taking an annotation off ([`remove`]) and changing its typed text ([`amend`]) (design D2).
//!
//! Both work on a [`Mark`] from [`parse`] of the same text; a mark that is no longer there is
//! refused ([`Refusal::Stale`]). Both end with the same round-trip guard as [`super::annotate`]:
//! the other marks must parse back unchanged (shifted).

use std::ops::Range;

use super::annotate::{check_text, line_ending, round_trip, with_line_ending};
use super::{parse, Kind, Mark, Refusal};

/// The marks of `text` without `mark`, or [`Refusal::Stale`] if `mark` is not one of them.
fn others(text: &str, mark: &Mark) -> Result<Vec<Mark>, Refusal> {
    let mut marks = parse(text);
    let i = marks.iter().position(|m| m == mark).ok_or(Refusal::Stale)?;
    marks.remove(i);
    Ok(marks)
}

/// `text` with the `at` bytes replaced by `with`.
fn splice(text: &str, at: Range<usize>, with: &str) -> String {
    let mut out = String::with_capacity(text.len() - at.len() + with.len());
    out.push_str(&text[..at.start]);
    out.push_str(with);
    out.push_str(&text[at.end..]);
    out
}

/// Removes `mark` from `text`, keeping the text it covered: an insertion and a standalone comment
/// disappear, a deletion keeps the deleted text, a substitution keeps the old text, a highlight
/// keeps the highlighted text (its comment goes too). Removing a mark that [`super::annotate`]
/// added gives back the original bytes.
pub fn remove(text: &str, mark: &Mark) -> Result<String, Refusal> {
    let old = others(text, mark)?;
    let kept = match mark.kind {
        Kind::Insert | Kind::Comment => "",
        Kind::Delete | Kind::Substitute | Kind::Highlight => &text[mark.text.clone()],
    };
    let out = splice(text, mark.range.clone(), kept);
    let delta = kept.len() as isize - mark.range.len() as isize;
    round_trip(&out, old, mark.range.end, delta, None)?;
    Ok(out)
}

/// Replaces the typed part of `mark` with `new_text`: the inserted text, the new text of a
/// substitution, the comment text, or the comment of a highlight (a highlight without one gets
/// it attached). A deletion has nothing typed ([`Refusal::NothingToAmend`]). `new_text` follows
/// the rules of [`super::annotate`]: not empty, no markup token, newlines written with the line
/// ending of the line where the mark starts.
pub fn amend(text: &str, mark: &Mark, new_text: &str) -> Result<String, Refusal> {
    if mark.kind == Kind::Delete {
        return Err(Refusal::NothingToAmend);
    }
    let old = others(text, mark)?;
    check_text(new_text)?;
    let body = with_line_ending(new_text, line_ending(text, mark.range.start));
    let (at, with, typed_start) = match (mark.kind, &mark.new_text, &mark.comment) {
        (Kind::Substitute, Some(r), _) | (Kind::Highlight, _, Some(r)) => {
            (r.clone(), body.clone(), r.start)
        }
        (Kind::Highlight, _, None) => {
            let end = mark.range.end;
            (end..end, format!("{{>>{body}<<}}"), end + 3)
        }
        _ => (mark.text.clone(), body.clone(), mark.text.start),
    };
    let out = splice(text, at.clone(), &with);
    let delta = with.len() as isize - at.len() as isize;
    let moved = |x: usize| x.wrapping_add_signed(delta);
    let typed = typed_start..typed_start + body.len();
    let mut new = mark.clone();
    new.range.end = moved(mark.range.end);
    match mark.kind {
        Kind::Substitute => new.new_text = Some(typed),
        Kind::Highlight => new.comment = Some(typed),
        _ => new.text = typed,
    }
    round_trip(&out, old, mark.range.end, delta, Some(new))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::{annotate, Action, Edit, Place, TOKENS};
    use super::*;

    fn only(text: &str) -> Mark {
        let m = parse(text);
        assert_eq!(m.len(), 1, "{text:?}");
        m.into_iter().next().unwrap()
    }

    fn rm(text: &str) -> String {
        remove(text, &only(text)).unwrap()
    }

    fn am(text: &str, new_text: &str) -> Result<String, Refusal> {
        amend(text, &only(text), new_text)
    }

    #[test]
    fn remove_each_hand_written_kind() {
        assert_eq!(rm("a {++new ++}b"), "a b");
        assert_eq!(rm("a {--old--} b"), "a old b");
        assert_eq!(rm("a {~~old~>new~~} b"), "a old b");
        assert_eq!(rm("a {==hl==} b"), "a hl b");
        assert_eq!(rm("a {==hl==}{>>why<<} b"), "a hl b");
        assert_eq!(rm("a {>>note<<} b"), "a  b");
        assert_eq!(rm("x {~~кот\r\nпёс~>a~~}\r\n"), "x кот\r\nпёс\r\n"); // i18n-allow-cyrillic
        assert_eq!(rm("\u{feff}{--w--}\r\n"), "\u{feff}w\r\n");
    }

    #[test]
    fn remove_keeps_other_marks() {
        let t = "{++a++} {--b--} {>>c<<}";
        let m = parse(t);
        assert_eq!(remove(t, &m[1]).unwrap(), "{++a++} b {>>c<<}");
        assert_eq!(remove(t, &m[0]).unwrap(), " {--b--} {>>c<<}");
    }

    #[test]
    fn stale_mark_is_refused() {
        let t = "a {--old--} b";
        let m = only(t);
        assert_eq!(remove("a old b", &m), Err(Refusal::Stale));
        assert_eq!(remove("xa {--old--} b", &m), Err(Refusal::Stale));
        assert_eq!(amend("a old b", &only("{>>x<<}"), "y"), Err(Refusal::Stale));
    }

    #[test]
    fn remove_that_would_join_neighbours_is_refused() {
        // Without the insertion the comment would attach to the highlight.
        let t = "{==h==}{++x++}{>>c<<}";
        assert_eq!(remove(t, &parse(t)[1]), Err(Refusal::RoundTrip));
    }

    #[test]
    fn amend_each_kind() {
        assert_eq!(am("a {++new++} b", "newer").unwrap(), "a {++newer++} b");
        assert_eq!(am("a {~~old~>new~~} b", "other").unwrap(), "a {~~old~>other~~} b");
        assert_eq!(am("a {==hl==}{>>why<<} b", "because").unwrap(), "a {==hl==}{>>because<<} b");
        assert_eq!(am("a {==hl==} b", "note").unwrap(), "a {==hl==}{>>note<<} b");
        assert_eq!(am("{>>проверить<<}", "проверить даты").unwrap(), "{>>проверить даты<<}"); // i18n-allow-cyrillic
        assert_eq!(am("a {--old--} b", "x"), Err(Refusal::NothingToAmend));
        assert_eq!(am("a {++x++} b", ""), Err(Refusal::EmptyText));
    }

    #[test]
    fn amend_multiline_crlf() {
        let t = "one\r\n{~~two\r\nthree~>a~~}\r\n";
        assert_eq!(am(t, "b\nc").unwrap(), "one\r\n{~~two\r\nthree~>b\r\nc~~}\r\n");
        let t = "x\r\n{>>a\r\nb<<}";
        assert_eq!(am(t, "c\r\nd\ne").unwrap(), "x\r\n{>>c\r\nd\r\ne<<}");
        assert_eq!(am("x\n{++a++}\n", "b\nc").unwrap(), "x\n{++b\nc++}\n");
        // Amending keeps later marks, shifted.
        let t = "{++a++} {--b--}";
        let out = amend(t, &parse(t)[0], "long").unwrap();
        assert_eq!(out, "{++long++} {--b--}");
        assert_eq!(parse(&out).len(), 2);
    }

    #[test]
    fn amend_token_is_refused() {
        for token in TOKENS {
            let typed = format!("a {token} b");
            let want = Err(Refusal::Token { token, place: Place::Text });
            for t in ["{++x++}", "{~~o~>n~~}", "{>>c<<}", "{==h==}{>>c<<}", "{==h==}"] {
                assert_eq!(am(t, &typed), want, "{t} {token}");
            }
        }
        assert!(Refusal::Stale.to_string().contains("no longer"));
    }

    #[test]
    fn amend_with_marker_chars_at_edges() {
        assert_eq!(am("{++a++}", "b+").unwrap(), "{++b+++}");
        assert_eq!(am("{~~a~>b~~}", "~c").unwrap(), "{~~a~>~c~~}");
        assert_eq!(am("{>>a<<}", ">c<").unwrap(), "{>>>c<<<}");
    }

    /// Property: for every kind at every boundary, annotate then remove gives the original bytes.
    #[test]
    fn annotate_then_remove_is_byte_exact() {
        let texts = [
            "\u{feff}Ab {++i++} кот\r\n{~~o~>n~~} {==h==}{>>c<<}\nend {>>p<<}", // i18n-allow-cyrillic
            "\u{feff}line one\r\nстрока два\r\n\r\nthree\r\n", // i18n-allow-cyrillic
            "plain\nmulti\nline",
        ];
        let actions = [
            Action::Substitute("x\ny".into()),
            Action::Delete,
            Action::Insert("x\ny".into()),
            Action::Comment("x\ny".into()),
        ];
        let mut ok = 0;
        for t in texts {
            let bounds: Vec<usize> = (0..=t.len()).filter(|&i| t.is_char_boundary(i)).collect();
            for &s in &bounds {
                for &e in bounds.iter().filter(|&&e| e >= s) {
                    for a in &actions {
                        let edit = Edit { start: s, end: e, action: a.clone() };
                        let Ok(out) = annotate(t, &edit) else { continue };
                        // Markup goes in at `s` (at `e` for an insertion); old marks there shift after it.
                        let at = if matches!(a, Action::Insert(_)) { e } else { s };
                        let new = parse(&out).into_iter().find(|m| m.range.start == at).unwrap();
                        assert_eq!(remove(&out, &new).as_deref(), Ok(t), "{s}..{e} {a:?}: {out:?}");
                        // Amending then removing is byte-exact too.
                        if !matches!(a, Action::Delete) {
                            let am = amend(&out, &new, "z\nw").unwrap();
                            let m2 = parse(&am).into_iter().find(|m| m.range.start == new.range.start).unwrap();
                            assert_eq!(remove(&am, &m2).as_deref(), Ok(t), "{am:?}");
                        }
                        ok += 1;
                    }
                }
            }
        }
        assert!(ok > 300, "{ok}");
    }
}
