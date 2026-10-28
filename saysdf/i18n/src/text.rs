//! Characters that can disguise text shown in the UI. One definition for every place that
//! filters them (no copies of this set): catalog validation here, the GUI bars
//! (`wezterm-gui/src/termwindow/saysdf_bars.rs`), the TUI escaping (`saysdf_render::sanitize`)
//! and `saysdf.util.notify` ([`single_line`] through the native `saysdf._i18n`).

/// Invisible format and bidi characters (Unicode `Cf` that render as nothing, plus the line and
/// paragraph separators): with them text can look like other text — reversed by RLO, two
/// labels that look identical but differ by a zero-width character, a hidden suffix.
/// Control characters (`Cc`) are not included: check them with `char::is_control`.
pub fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'                      // soft hyphen
            | '\u{061C}'                // Arabic letter mark
            | '\u{180E}'                // Mongolian vowel separator
            | '\u{200B}'..='\u{200F}'   // zero-width space/non-joiner/joiner, LRM, RLM
            | '\u{2028}'..='\u{202E}'   // line/paragraph separator, LRE RLE PDF LRO RLO
            | '\u{2060}'..='\u{2064}'   // word joiner, invisible operators
            | '\u{2066}'..='\u{2069}'   // LRI RLI FSI PDI
            | '\u{206A}'..='\u{206F}'   // deprecated format characters
            | '\u{FEFF}'                // BOM / zero-width no-break space
            | '\u{FFF9}'..='\u{FFFB}' // interlinear annotation
    )
}

/// One line of plain UI text (status bar notice, toast): every run of control characters
/// (newlines, ESC, ...) and invisible format/bidi characters becomes one space.
pub fn single_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.chars() {
        if c.is_control() || is_invisible_format(c) {
            if !gap {
                out.push(' ');
                gap = true;
            }
        } else {
            out.push(c);
            gap = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_strips_controls_and_format_chars() {
        assert_eq!(single_line("plain text"), "plain text");
        assert_eq!(single_line("a\r\nb\tc\u{1b}]52;c;x\u{7}"), "a b c ]52;c;x ");
        assert_eq!(single_line("ok\u{202E}txt.exe"), "ok txt.exe");
        assert_eq!(single_line("x\u{200B}\u{FEFF}y\u{85}z"), "x y z");
        assert_eq!(single_line("Вид"), "Вид"); // i18n-allow-cyrillic
    }

    #[test]
    fn invisible_format_set() {
        for c in [
            '\u{AD}', '\u{61C}', '\u{200B}', '\u{200D}', '\u{200F}', '\u{2028}', '\u{2029}',
            '\u{202A}', '\u{202E}', '\u{2060}', '\u{2066}', '\u{2069}', '\u{FEFF}',
        ] {
            assert!(is_invisible_format(c), "U+{:04X}", c as u32);
        }
        for c in ['a', ' ', '\n', '\u{1b}', '\u{2027}', '\u{202F}', '\u{2065}', '\u{E9}'] {
            assert!(!is_invisible_format(c), "U+{:04X}", c as u32);
        }
    }
}
