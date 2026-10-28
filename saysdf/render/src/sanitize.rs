//! Escaping of control characters (design D6): everything that ends up in a `Span` passes through here.
//! File output is untrusted data: ESC, OSC 52 and the like must never reach the terminal.

use unicode_width::UnicodeWidthChar;

/// Default tab width.
pub const DEFAULT_TAB_WIDTH: usize = 4;

/// Replacement for C1 (`U+0080–U+009F`).
pub const C1_REPLACEMENT: char = '\u{FFFD}';

// Bidi, zero-width and other invisible format characters (one set with the GUI bars and the
// catalog validation, rule 2): they can disguise a file name or text.
use saysdf_i18n::text::is_invisible_format;

/// Character width in cells after escaping (tabs are counted by the caller).
pub fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Appends the visible representation of `s` to `out`, starting at column `*col`, and advances `*col`.
/// Needed for highlighting: fragments of one line are escaped in turn, tabs are measured from the line start.
pub fn push_visible(out: &mut String, s: &str, col: &mut usize, tab_width: usize) {
    let tab_width = tab_width.max(1);
    for c in s.chars() {
        match c {
            '\t' => {
                let n = tab_width - *col % tab_width;
                out.extend(std::iter::repeat_n(' ', n));
                *col += n;
            }
            '\u{0}'..='\u{1F}' => {
                // U+2400 + code: ␀ … ␟ (including a lone \r → ␍)
                let pic = char::from_u32(0x2400 + c as u32).unwrap_or(C1_REPLACEMENT);
                out.push(pic);
                *col += char_width(pic);
            }
            '\u{7F}' => {
                out.push('\u{2421}'); // ␡
                *col += 1;
            }
            '\u{80}'..='\u{9F}' => {
                out.push(C1_REPLACEMENT);
                *col += 1;
            }
            c if is_invisible_format(c) => {
                let marker = format!("\u{27E6}U+{:04X}\u{27E7}", c as u32);
                *col += marker.chars().map(char_width).sum::<usize>();
                out.push_str(&marker);
            }
            c => {
                out.push(c);
                *col += char_width(c);
            }
        }
    }
}

/// Visible line: control characters become symbols, tabs become spaces up to a column that is a multiple of `tab_width`.
pub fn visible_line(s: &str, tab_width: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut col = 0;
    push_visible(&mut out, s, &mut col, tab_width);
    out
}

/// Width of an already escaped line in cells.
pub fn str_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_become_visible() {
        let osc52 = "a\x1b]52;c;ZXZpbA==\x07b";
        let v = visible_line(osc52, 4);
        assert_eq!(v, "a\u{241B}]52;c;ZXZpbA==\u{2407}b");
        assert!(!v.chars().any(|c| c.is_control()));
        assert_eq!(visible_line("\0\x7f\u{9b}x\ry", 4), "\u{2400}\u{2421}\u{FFFD}x\u{240D}y");
        assert_eq!(visible_line("a\u{202E}b", 4), "a\u{27E6}U+202E\u{27E7}b");
    }

    #[test]
    fn invisible_format_chars_become_visible() {
        assert_eq!(visible_line("a\u{200B}b", 4), "a\u{27E6}U+200B\u{27E7}b");
        assert_eq!(
            visible_line("\u{FEFF}x\u{061C}", 4),
            "\u{27E6}U+FEFF\u{27E7}x\u{27E6}U+061C\u{27E7}"
        );
        assert_eq!(
            visible_line("a\u{AD}\u{2028}", 4),
            "a\u{27E6}U+00AD\u{27E7}\u{27E6}U+2028\u{27E7}"
        );
    }

    #[test]
    fn tabs_expand_to_multiple() {
        assert_eq!(visible_line("\tx", 4), "    x");
        assert_eq!(visible_line("ab\tx", 4), "ab  x");
        assert_eq!(visible_line("abcd\tx", 4), "abcd    x");
        assert_eq!(visible_line("a\tx", 0), "a x");
    }

    #[test]
    fn wide_chars_count_two_cells() {
        // "中" takes 2 cells: the tab after it pads up to 4
        assert_eq!(visible_line("中\tx", 4), "中  x");
        assert_eq!(str_width("中文a"), 5);
    }
}
