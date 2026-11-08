//! Positions of the annotation model: `(line, grapheme column)` <-> byte offset.
//!
//! Lines are separated by `\n`; a `\r` right before it belongs to the line break (CRLF), so a
//! line's end is before `\r\n`. A lone `\r` is an ordinary character. A leading UTF-8 BOM
//! (U+FEFF) is not part of line 0: column 0 starts after it. A text ending with a newline has a
//! final empty line. Columns count extended grapheme clusters of the line.
//!
//! Out of range is an error (`None`), never a clamp: the caller must not write at a guessed place.

use unicode_segmentation::UnicodeSegmentation;

/// A caret position: zero-based line and grapheme column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

/// Byte range of each line without its break, and the byte offset where line content starts.
fn lines(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let bom = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let mut start = bom;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let rest = &text[start..];
        let (end, next) = match rest.find('\n') {
            Some(i) => (start + i, Some(start + i + 1)),
            None => (text.len(), None),
        };
        let content_end = if next.is_some() && text[start..end].ends_with('\r') {
            end - 1
        } else {
            end
        };
        let item = (start, content_end);
        match next {
            Some(n) => start = n,
            None => done = true,
        }
        Some(item)
    })
}

/// Byte offset of `pos`; `col` equal to the grapheme count is the line end (before `\r\n`).
/// `None` if the line or column does not exist.
pub fn byte_offset(text: &str, pos: Pos) -> Option<usize> {
    let (start, end) = lines(text).nth(pos.line)?;
    let line = &text[start..end];
    if pos.col == 0 {
        return Some(start);
    }
    let mut it = line.grapheme_indices(true).skip(pos.col - 1);
    match (it.next(), it.next()) {
        (Some(_), Some((i, _))) => Some(start + i),
        (Some(_), None) => Some(end),
        _ => None,
    }
}

/// Position of a byte offset. `None` if it is out of range, inside the BOM, inside a grapheme,
/// or between `\r` and `\n`. The offset at the end of a line (before `\r\n`) is that line's end;
/// the offset after the break is column 0 of the next line.
pub fn position(text: &str, byte: usize) -> Option<Pos> {
    if byte > text.len() || !text.is_char_boundary(byte) {
        return None;
    }
    for (line, (start, end)) in lines(text).enumerate() {
        if byte < start {
            return None; // inside the BOM or the line break
        }
        if byte <= end {
            let rel = byte - start;
            if rel == 0 {
                return Some(Pos { line, col: 0 });
            }
            for (col, (i, g)) in text[start..end].grapheme_indices(true).enumerate() {
                if i == rel {
                    return Some(Pos { line, col });
                }
                if i + g.len() == rel {
                    return Some(Pos { line, col: col + 1 });
                }
                if i > rel {
                    break;
                }
            }
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn ascii_lf() {
        let t = "ab\ncd";
        assert_eq!(byte_offset(t, p(0, 0)), Some(0));
        assert_eq!(byte_offset(t, p(0, 2)), Some(2));
        assert_eq!(byte_offset(t, p(1, 1)), Some(4));
        assert_eq!(byte_offset(t, p(1, 2)), Some(5)); // last line without newline
        assert_eq!(position(t, 5), Some(p(1, 2)));
        assert_eq!(position(t, 3), Some(p(1, 0)));
    }

    #[test]
    fn out_of_range_is_none() {
        let t = "ab\ncd";
        assert_eq!(byte_offset(t, p(2, 0)), None);
        assert_eq!(byte_offset(t, p(0, 3)), None);
        assert_eq!(position(t, 6), None);
        assert_eq!(byte_offset("", p(0, 0)), Some(0));
        assert_eq!(byte_offset("", p(1, 0)), None);
    }

    #[test]
    fn trailing_newline_has_empty_last_line() {
        let t = "a\n";
        assert_eq!(byte_offset(t, p(1, 0)), Some(2));
        assert_eq!(position(t, 2), Some(p(1, 0)));
    }

    #[test]
    fn cyrillic() {
        let t = "при\nвет"; // i18n-allow-cyrillic
        assert_eq!(byte_offset(t, p(0, 1)), Some(2));
        assert_eq!(byte_offset(t, p(1, 2)), Some(11)); // 7 bytes of the first line + 2 chars * 2 bytes
        assert_eq!(position(t, 2), Some(p(0, 1)));
        assert_eq!(position(t, 3), None); // inside a char
    }

    #[test]
    fn combining_and_emoji_are_one_grapheme() {
        let t = "e\u{301}x\u{1F468}\u{200D}\u{1F469}y";
        assert_eq!(byte_offset(t, p(0, 1)), Some(3));
        assert_eq!(byte_offset(t, p(0, 2)), Some(4));
        let zwj_len = "\u{1F468}\u{200D}\u{1F469}".len();
        assert_eq!(byte_offset(t, p(0, 3)), Some(4 + zwj_len));
        assert_eq!(byte_offset(t, p(0, 4)), Some(t.len()));
        assert_eq!(position(t, 1), None); // between e and the accent
        assert_eq!(position(t, 4 + 4), None); // inside the ZWJ cluster
        assert_eq!(position(t, 4 + zwj_len), Some(p(0, 3)));
    }

    #[test]
    fn bom() {
        let t = "\u{feff}ab\ncd";
        assert_eq!(byte_offset(t, p(0, 0)), Some(3));
        assert_eq!(byte_offset(t, p(0, 1)), Some(4));
        assert_eq!(position(t, 3), Some(p(0, 0)));
        assert_eq!(position(t, 0), None);
        assert_eq!(position(t, 1), None);
        assert_eq!(byte_offset("\u{feff}", p(0, 0)), Some(3));
    }

    #[test]
    fn crlf() {
        let t = "ab\r\ncd\r\n";
        assert_eq!(byte_offset(t, p(0, 2)), Some(2)); // before \r\n
        assert_eq!(byte_offset(t, p(0, 3)), None);
        assert_eq!(byte_offset(t, p(1, 0)), Some(4));
        assert_eq!(byte_offset(t, p(2, 0)), Some(8));
        assert_eq!(position(t, 2), Some(p(0, 2)));
        assert_eq!(position(t, 3), None); // inside \r\n
        assert_eq!(position(t, 4), Some(p(1, 0)));
    }

    #[test]
    fn mixed_endings_and_lone_cr() {
        let t = "a\r\nb\nc\rd";
        assert_eq!(byte_offset(t, p(0, 1)), Some(1));
        assert_eq!(byte_offset(t, p(1, 1)), Some(4));
        assert_eq!(byte_offset(t, p(2, 0)), Some(5));
        assert_eq!(byte_offset(t, p(2, 3)), Some(8)); // lone \r is a character
        assert_eq!(position(t, 7), Some(p(2, 2)));
    }

    #[test]
    fn roundtrip_every_position() {
        let t = "\u{feff}при\u{301}вет\r\n\r\nx\u{1F468}\u{200D}\u{1F469}\nlast"; // i18n-allow-cyrillic
        for (line, (s, e)) in lines(t).enumerate() {
            let n = t[s..e].graphemes(true).count();
            for col in 0..=n {
                let b = byte_offset(t, p(line, col)).unwrap();
                assert_eq!(position(t, b), Some(p(line, col)));
            }
        }
    }
}
