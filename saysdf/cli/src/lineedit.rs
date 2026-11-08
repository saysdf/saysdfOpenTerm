//! Shared text input editor (design D11 of `text-annotations`): one `String` with a caret,
//! edited by grapheme. The pager's search and go-to-line inputs use it in single-line mode;
//! the annotation input will use the same type in multi-line mode (step B-3b), so the API is
//! already shaped for several lines: the caret position is reported as `(row, col)` and the
//! display window is computed by the editor, not by its callers.
//!
//! Display widths follow `saysdf_render::sanitize` (`visible_line` / `grapheme_to_col`), so the
//! caret lands on the cell the character is drawn in (wide characters, tabs, placeholders).

use std::cell::Cell;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use saysdf_render::sanitize::{char_width, grapheme_to_col, visible_line};
use unicode_segmentation::UnicodeSegmentation;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-pager");

/// Key hint shown while the multi-line editor is open.
pub fn hint() -> std::borrow::Cow<'static, str> {
    I18N.t("Enter save · Ctrl+J newline · Esc cancel")
}

/// Result of one key for the owner of the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The key was handled (text or caret may have changed).
    Handled,
    /// Enter: the owner takes the text.
    Submit,
    /// Esc: the owner closes the editor without using the text.
    Cancel,
    /// The key means nothing to the editor.
    Ignored,
}

/// Visible part of a multi-line editor: rows already escaped and scrolled horizontally to a
/// common left edge, plus the caret cell within that window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows {
    pub rows: Vec<String>,
    /// Caret as `(row within rows, cell within the field)`.
    pub caret: (usize, usize),
}

/// Tab width used for Up/Down column matching (the viewer's tab width).
const TAB: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineEdit {
    /// The text; lines are separated by `\n` (the file's own line ending is applied on write).
    text: String,
    /// Byte offset into `text`, always on a grapheme boundary.
    caret: usize,
    /// Multi-line mode: newline insert, Up/Down, line-wise Home/End, growing height. In
    /// single-line mode `text` never contains a newline typed by the user.
    multiline: bool,
    /// Display column Up/Down aim for; set by the first vertical move, cleared by any other key.
    desired_col: Option<usize>,
    /// First visible row of the multi-line view (kept between frames so scrolling is stable).
    top: Cell<usize>,
}

impl LineEdit {
    /// Empty single-line editor.
    pub fn new() -> Self {
        Self::with_text("")
    }

    /// Single-line editor prefilled with `text`, caret at the end.
    pub fn with_text(text: &str) -> Self {
        Self { text: text.to_owned(), caret: text.len(), multiline: false, desired_col: None, top: Cell::new(0) }
    }

    /// Multi-line editor prefilled with `text` (CRLF/CR line breaks become `\n`), caret at the end.
    pub fn multiline(text: &str) -> Self {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        Self { caret: text.len(), text, multiline: true, desired_col: None, top: Cell::new(0) }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_text(self) -> String {
        self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Caret as a grapheme index within the text.
    pub fn caret(&self) -> usize {
        self.text[..self.caret].graphemes(true).count()
    }

    /// True for the keys that insert a newline in multi-line mode: Ctrl+J (LF in raw mode or
    /// the kitty encoding) and Alt+Enter (legacy ESC CR, which the fork's `multiline` module
    /// sends for Shift+Enter/Ctrl+Enter, or Enter with the Alt modifier under the kitty flags).
    fn is_newline_key(k: &KeyEvent) -> bool {
        match k.code {
            KeyCode::Char('j' | 'J') => k.modifiers.contains(KeyModifiers::CONTROL),
            KeyCode::Char('\n') => true,
            // Ctrl+Enter: ConPTY may turn the LF byte (Ctrl+J) into a Ctrl+Enter key event
            KeyCode::Enter => k.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL),
            _ => false,
        }
    }

    /// Handles one key press. Typed characters are kept as they arrive (any layout, any
    /// modifiers the terminal folds into the character), the same as the old pager input.
    pub fn key(&mut self, k: &KeyEvent) -> Outcome {
        if self.multiline {
            if Self::is_newline_key(k) {
                self.desired_col = None;
                self.insert('\n');
                return Outcome::Handled;
            }
            match k.code {
                KeyCode::Up => {
                    self.vertical(false);
                    return Outcome::Handled;
                }
                KeyCode::Down => {
                    self.vertical(true);
                    return Outcome::Handled;
                }
                _ => {}
            }
        }
        self.desired_col = None;
        match k.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter => return Outcome::Submit,
            KeyCode::Char(c) => self.insert(c),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.home(),
            KeyCode::End => self.end(),
            _ => return Outcome::Ignored,
        }
        Outcome::Handled
    }

    /// Inserts a character at the caret. In single-line mode a newline is dropped; in
    /// multi-line mode CR is stored as `\n`.
    pub fn insert(&mut self, c: char) {
        let c = match c {
            '\n' | '\r' if !self.multiline => return,
            '\r' => '\n',
            c => c,
        };
        self.text.insert(self.caret, c);
        self.caret += c.len_utf8();
        // a typed joiner (ZWJ, regional indicator) can merge the graphemes around the caret
        self.snap();
    }

    fn prev_boundary(&self) -> usize {
        self.text[..self.caret].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.caret..].graphemes(true).next().map_or(self.caret, |g| self.caret + g.len())
    }

    /// Moves a caret that ended up inside a grapheme to the end of that grapheme.
    fn snap(&mut self) {
        let mut at = 0;
        for (i, g) in self.text.grapheme_indices(true) {
            if i >= self.caret {
                break;
            }
            at = i + g.len();
        }
        self.caret = at;
    }

    pub fn backspace(&mut self) {
        let p = self.prev_boundary();
        self.text.replace_range(p..self.caret, "");
        self.caret = p;
    }

    pub fn delete(&mut self) {
        let n = self.next_boundary();
        self.text.replace_range(self.caret..n, "");
    }

    pub fn left(&mut self) {
        self.caret = self.prev_boundary();
    }

    pub fn right(&mut self) {
        self.caret = self.next_boundary();
    }

    /// Byte range of the line holding the caret (without its `\n`).
    fn line_bounds(&self) -> (usize, usize) {
        let start = self.text[..self.caret].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[self.caret..].find('\n').map_or(self.text.len(), |i| self.caret + i);
        (start, end)
    }

    /// Start of the caret's line (of the whole text in single-line mode).
    pub fn home(&mut self) {
        self.caret = self.line_bounds().0;
    }

    /// End of the caret's line (of the whole text in single-line mode).
    pub fn end(&mut self) {
        self.caret = self.line_bounds().1;
    }

    /// Up or Down: the caret lands on the grapheme drawn at the wanted display column (or the
    /// line end if the line is shorter), aiming for the column the vertical run started at.
    /// On the first/last line the caret goes to its start/end, as in most editors.
    fn vertical(&mut self, down: bool) {
        let (start, end) = self.line_bounds();
        let (_, col) = self.caret_pos(TAB);
        let want = *self.desired_col.get_or_insert(col);
        let (ts, te) = if down {
            if end == self.text.len() {
                self.caret = end;
                return;
            }
            let s = end + 1;
            (s, self.text[s..].find('\n').map_or(self.text.len(), |i| s + i))
        } else {
            if start == 0 {
                self.caret = 0;
                return;
            }
            let e = start - 1;
            (self.text[..e].rfind('\n').map_or(0, |i| i + 1), e)
        };
        let line = &self.text[ts..te];
        // last grapheme start whose column does not pass the wanted column
        let mut at = te;
        for (g, (i, _)) in line.grapheme_indices(true).enumerate() {
            if grapheme_to_col(line, TAB, g + 1) > want {
                at = ts + i;
                break;
            }
        }
        self.caret = at;
    }

    /// Caret as `(row, display column within its line)`; the row is 0 in single-line mode.
    pub fn caret_pos(&self, tab_width: usize) -> (usize, usize) {
        let (start, end) = self.line_bounds();
        let row = self.text[..start].matches('\n').count();
        let g = self.text[start..self.caret].graphemes(true).count();
        (row, grapheme_to_col(&self.text[start..end], tab_width, g))
    }

    /// Cells to skip on the left so that a caret at `col` stays inside a field `width` wide,
    /// aligned to a grapheme start of `line` (`g` = caret grapheme within the line).
    fn h_skip(line: &str, tab_width: usize, g: usize, col: usize, width: usize) -> usize {
        if col < width {
            return 0;
        }
        (0..=g).map(|i| grapheme_to_col(line, tab_width, i)).find(|&start| col - start < width).unwrap_or(col)
    }

    /// Drops the first `skip` cells of an escaped line; a wide character cut by the edge
    /// becomes spaces so the columns of all rows stay aligned.
    fn skip_cells(full: &str, skip: usize) -> String {
        let mut cells = 0;
        let mut out = String::new();
        for c in full.chars() {
            if cells >= skip {
                out.push(c);
                continue;
            }
            cells += char_width(c);
            if cells > skip {
                out.extend(std::iter::repeat_n(' ', cells - skip));
            }
        }
        out
    }

    /// The escaped text as drawn in a field `width` cells wide, scrolled horizontally so the
    /// caret stays visible, plus the caret's cell within that field (single-line view).
    pub fn view(&self, tab_width: usize, width: usize) -> (String, usize) {
        let full = visible_line(&self.text, tab_width);
        let (_, col) = self.caret_pos(tab_width);
        let skip = Self::h_skip(&self.text, tab_width, self.caret(), col, width.max(1));
        (Self::skip_cells(&full, skip), col - skip)
    }

    /// Multi-line view: at most `max_height` rows (the caller passes about a third of the pane
    /// height), scrolled vertically so the caret row stays visible and horizontally (the same
    /// for every row) so the caret cell stays inside `width`. The number of returned rows is
    /// the editor's height: it grows with the text up to `max_height`. Rows are not cut to
    /// `width`; the widget clips them.
    pub fn rows(&self, tab_width: usize, width: usize, max_height: usize) -> Rows {
        let lines: Vec<&str> = self.text.split('\n').collect();
        let height = lines.len().min(max_height.max(1));
        let (row, col) = self.caret_pos(tab_width);
        let mut top = self.top.get().min(lines.len() - height);
        if row < top {
            top = row;
        } else if row >= top + height {
            top = row + 1 - height;
        }
        self.top.set(top);
        let (start, _) = self.line_bounds();
        let g = self.text[start..self.caret].graphemes(true).count();
        let skip = Self::h_skip(lines[row], tab_width, g, col, width.max(1));
        let rows = lines[top..top + height].iter().map(|l| Self::skip_cells(&visible_line(l, tab_width), skip)).collect();
        Rows { rows, caret: (row - top, col - skip) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn press(e: &mut LineEdit, code: KeyCode) -> Outcome {
        e.key(&KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(e: &mut LineEdit, s: &str) {
        for c in s.chars() {
            assert_eq!(press(e, KeyCode::Char(c)), Outcome::Handled);
        }
    }

    #[test]
    fn insert_and_delete_in_the_middle() {
        let mut e = LineEdit::new();
        typed(&mut e, "held");
        press(&mut e, KeyCode::Left);
        press(&mut e, KeyCode::Left);
        typed(&mut e, "l");
        assert_eq!((e.text(), e.caret()), ("helld", 3));
        press(&mut e, KeyCode::Backspace);
        press(&mut e, KeyCode::Delete);
        assert_eq!((e.text(), e.caret()), ("hed", 2));
        typed(&mut e, "llo worl");
        assert_eq!(e.text(), "hello world");
    }

    #[test]
    fn cyrillic_is_kept_and_edited() {
        let mut e = LineEdit::new();
        typed(&mut e, "кот"); // i18n-allow-cyrillic
        press(&mut e, KeyCode::Left);
        press(&mut e, KeyCode::Backspace);
        typed(&mut e, "р"); // i18n-allow-cyrillic
        assert_eq!(e.text(), "крт"); // i18n-allow-cyrillic
        assert_eq!(e.caret_pos(4), (0, 2));
    }

    #[test]
    fn combining_and_emoji_are_one_grapheme() {
        // e + COMBINING ACUTE, a ZWJ family emoji, a flag
        let mut e = LineEdit::with_text("e\u{301}\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{1F1F7}\u{1F1FA}x");
        assert_eq!(e.caret(), 4);
        press(&mut e, KeyCode::Left);
        press(&mut e, KeyCode::Backspace);
        assert_eq!(e.text(), "e\u{301}\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}x");
        press(&mut e, KeyCode::Home);
        press(&mut e, KeyCode::Right);
        assert_eq!(e.caret(), 1);
        press(&mut e, KeyCode::Delete);
        assert_eq!(e.text(), "e\u{301}x");
        // a combining mark typed after a base letter joins it
        press(&mut e, KeyCode::End);
        typed(&mut e, "\u{308}");
        assert_eq!(e.caret(), 2);
        press(&mut e, KeyCode::Backspace);
        assert_eq!(e.text(), "e\u{301}");
    }

    #[test]
    fn home_end_and_prefill() {
        let mut e = LineEdit::with_text("abc");
        assert_eq!(e.caret(), 3);
        press(&mut e, KeyCode::Home);
        assert_eq!(e.caret(), 0);
        typed(&mut e, ">");
        press(&mut e, KeyCode::End);
        typed(&mut e, "<");
        assert_eq!(e.text(), ">abc<");
    }

    #[test]
    fn backspace_at_start_and_delete_at_end_are_noops() {
        let mut e = LineEdit::with_text("ab");
        press(&mut e, KeyCode::Delete);
        press(&mut e, KeyCode::Home);
        press(&mut e, KeyCode::Backspace);
        press(&mut e, KeyCode::Left);
        assert_eq!((e.text(), e.caret()), ("ab", 0));
        let mut empty = LineEdit::new();
        press(&mut empty, KeyCode::Backspace);
        press(&mut empty, KeyCode::Right);
        assert_eq!((empty.text(), empty.caret()), ("", 0));
    }

    #[test]
    fn enter_esc_and_unknown_keys() {
        let mut e = LineEdit::new();
        assert_eq!(press(&mut e, KeyCode::Enter), Outcome::Submit);
        assert_eq!(press(&mut e, KeyCode::Esc), Outcome::Cancel);
        assert_eq!(press(&mut e, KeyCode::F(5)), Outcome::Ignored);
        e.insert('\n');
        assert!(e.is_empty());
    }

    #[test]
    fn view_uses_display_widths_and_scrolls_to_caret() {
        let e = LineEdit::with_text("a\u{4E2D}\tb");
        // a=1 cell, CJK=2 cells, tab to column 4, b
        assert_eq!(e.caret_pos(4), (0, 5));
        assert_eq!(e.view(4, 80), ("a\u{4E2D} b".to_owned(), 5));
        let long = LineEdit::with_text("0123456789");
        let (s, col) = long.view(4, 4);
        assert_eq!((s.as_str(), col), ("789", 3));
    }

    fn chord(e: &mut LineEdit, code: KeyCode, m: KeyModifiers) -> Outcome {
        e.key(&KeyEvent::new(code, m))
    }

    #[test]
    fn multiline_newline_by_ctrl_j_and_alt_enter() {
        use ratatui::crossterm::event::{KeyEventKind, KeyEventState};
        let mut e = LineEdit::multiline("");
        typed(&mut e, "a");
        // Ctrl+J: raw-mode LF and the kitty CSI 106;5u both arrive as Char('j') + CONTROL
        assert_eq!(chord(&mut e, KeyCode::Char('j'), KeyModifiers::CONTROL), Outcome::Handled);
        typed(&mut e, "b");
        // legacy ESC CR (the fork's `multiline` module for Shift+Enter/Ctrl+Enter): crossterm
        // reads ESC + byte as that key with ALT, i.e. Enter + ALT
        assert_eq!(chord(&mut e, KeyCode::Enter, KeyModifiers::ALT), Outcome::Handled);
        typed(&mut e, "c");
        // kitty CSI 13;3u: Enter + ALT with the kitty event kind/state filled in
        let kitty = KeyEvent::new_with_kind_and_state(KeyCode::Enter, KeyModifiers::ALT, KeyEventKind::Press, KeyEventState::NONE);
        assert_eq!(e.key(&kitty), Outcome::Handled);
        // Ctrl+Shift+J under the kitty flags
        assert_eq!(chord(&mut e, KeyCode::Char('J'), KeyModifiers::CONTROL | KeyModifiers::SHIFT), Outcome::Handled);
        assert_eq!(e.text(), "a\nb\nc\n\n");
        assert_eq!(e.caret_pos(4), (4, 0));
        // Windows console input: an LF byte may come back from ConPTY as VK_RETURN + Ctrl,
        // which crossterm reports as Enter + CONTROL
        assert_eq!(chord(&mut e, KeyCode::Enter, KeyModifiers::CONTROL), Outcome::Handled);
        assert_eq!(e.text(), "a\nb\nc\n\n\n");
        e.backspace();
        assert_eq!(press(&mut e, KeyCode::Enter), Outcome::Submit);
        assert_eq!(press(&mut e, KeyCode::Esc), Outcome::Cancel);
        assert_eq!(e.text(), "a\nb\nc\n\n");
    }

    #[test]
    fn single_line_keeps_ctrl_j_and_alt_enter_behaviour() {
        let mut e = LineEdit::with_text("a");
        // as before B-3b: the character is inserted as it arrives, Alt+Enter submits
        assert_eq!(chord(&mut e, KeyCode::Char('j'), KeyModifiers::CONTROL), Outcome::Handled);
        assert_eq!(e.text(), "aj");
        assert_eq!(chord(&mut e, KeyCode::Enter, KeyModifiers::ALT), Outcome::Submit);
        assert_eq!(press(&mut e, KeyCode::Up), Outcome::Ignored);
        assert_eq!(press(&mut e, KeyCode::Down), Outcome::Ignored);
        assert_eq!(e.text(), "aj");
    }

    #[test]
    fn multiline_prefill_normalizes_line_breaks() {
        let e = LineEdit::multiline("one\r\ntwo\rthree");
        assert_eq!(e.text(), "one\ntwo\nthree");
        assert_eq!(e.caret_pos(4), (2, 5));
        let mut e = LineEdit::multiline("x");
        typed(&mut e, "\r");
        assert_eq!(e.text(), "x\n");
    }

    #[test]
    fn up_down_keep_desired_column_over_cyrillic_and_wide() {
        let mut e = LineEdit::multiline("абвгд\nx\n\u{4E2D}\u{6587}\u{5B57}"); // i18n-allow-cyrillic
        assert_eq!(e.caret_pos(4), (2, 6));
        press(&mut e, KeyCode::Up);
        assert_eq!(e.caret_pos(4), (1, 1));
        press(&mut e, KeyCode::Up);
        assert_eq!(e.caret_pos(4), (0, 5));
        press(&mut e, KeyCode::Down);
        press(&mut e, KeyCode::Down);
        // the desired column 6 survives the short line in between
        assert_eq!(e.caret_pos(4), (2, 6));
        // a new horizontal move resets it
        press(&mut e, KeyCode::Home);
        press(&mut e, KeyCode::Right);
        assert_eq!(e.caret_pos(4), (2, 2));
        press(&mut e, KeyCode::Up);
        press(&mut e, KeyCode::Up);
        assert_eq!(e.caret_pos(4), (0, 2));
        typed(&mut e, "Ж"); // i18n-allow-cyrillic
        assert_eq!(e.text(), "абЖвгд\nx\n\u{4E2D}\u{6587}\u{5B57}"); // i18n-allow-cyrillic
        // Up on the first line goes to its start, Down on the last line to its end
        press(&mut e, KeyCode::Up);
        assert_eq!(e.caret_pos(4), (0, 0));
        press(&mut e, KeyCode::End);
        assert_eq!(e.caret_pos(4), (0, 6));
        press(&mut e, KeyCode::Down);
        press(&mut e, KeyCode::Down);
        press(&mut e, KeyCode::Down);
        assert_eq!(e.caret_pos(4), (2, 6));
    }

    #[test]
    fn up_down_into_the_middle_of_a_wide_char_lands_before_it() {
        let mut e = LineEdit::multiline("abc\n\u{4E2D}\u{6587}");
        press(&mut e, KeyCode::Up);
        press(&mut e, KeyCode::Home);
        press(&mut e, KeyCode::Right);
        assert_eq!(e.caret_pos(4), (0, 1));
        press(&mut e, KeyCode::Down);
        assert_eq!(e.caret_pos(4), (1, 0));
        press(&mut e, KeyCode::Up);
        assert_eq!(e.caret_pos(4), (0, 1));
    }

    #[test]
    fn rows_grow_up_to_max_then_scroll_to_caret() {
        let mut e = LineEdit::multiline("");
        let r = e.rows(4, 20, 3);
        assert_eq!((r.rows.len(), r.caret), (1, (0, 0)));
        for (i, s) in ["l1", "l2", "l3", "l4"].iter().enumerate() {
            typed(&mut e, s);
            if i < 3 {
                chord(&mut e, KeyCode::Char('j'), KeyModifiers::CONTROL);
            }
            let r = e.rows(4, 20, 3);
            assert_eq!(r.rows.len(), (i + 2).min(3));
        }
        let r = e.rows(4, 20, 3);
        assert_eq!(r.rows, ["l2", "l3", "l4"]);
        assert_eq!(r.caret, (2, 2));
        // moving up within the window does not scroll; past its top it does
        press(&mut e, KeyCode::Up);
        assert_eq!(e.rows(4, 20, 3).rows, ["l2", "l3", "l4"]);
        press(&mut e, KeyCode::Up);
        press(&mut e, KeyCode::Up);
        let r = e.rows(4, 20, 3);
        assert_eq!((r.rows, r.caret), (vec!["l1".to_owned(), "l2".to_owned(), "l3".to_owned()], (0, 2)));
        press(&mut e, KeyCode::Down);
        assert_eq!(e.rows(4, 20, 3).caret, (1, 2));
    }

    #[test]
    fn rows_scroll_horizontally_and_keep_columns_aligned() {
        let e = LineEdit::multiline("\u{4E2D}\u{4E2D}\u{4E2D}\u{4E2D}\n0123456789");
        // caret at column 10, field 6 wide: 5 cells are skipped on every row
        let r = e.rows(4, 6, 5);
        assert_eq!(r.rows, [" \u{4E2D}", "56789"]);
        assert_eq!(r.caret, (1, 5));
        // tabs and control characters are escaped by the sanitize rules
        let e = LineEdit::multiline("a\tb\n\u{1b}x");
        let r = e.rows(4, 20, 5);
        assert_eq!(r.rows[0], visible_line("a\tb", 4));
        assert_eq!(r.rows[1], visible_line("\u{1b}x", 4));
    }

    #[test]
    fn hint_is_translated() {
        let _lang = saysdf_i18n::testing::use_lang("en");
        assert_eq!(hint(), "Enter save · Ctrl+J newline · Esc cancel");
    }
}
