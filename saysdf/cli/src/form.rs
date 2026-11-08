//! Shared ratatui form: input fields, Tab/Shift+Tab (up/down), a per-field error, Enter to submit,
//! Esc to cancel; a `[y/N]` confirmation screen (default "no") and a message screen.
//! The terminal comes from [`pager::Guard`] (restored on exit and on panic).

use crate::lineedit::{LineEdit, Outcome};
use crate::pager::Guard;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Position;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use saysdf_render::sanitize::visible_line;
use std::io;
use unicode_segmentation::UnicodeSegmentation;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-pager");

pub struct Field {
    pub label: String,
    pub value: String,
    pub hint: String,
    pub error: Option<String>,
    /// Masked input (password): shown as `*`.
    pub mask: bool,
}

impl Field {
    pub fn new(label: &str, hint: &str, value: &str) -> Self {
        Self { label: label.into(), value: value.into(), hint: hint.into(), error: None, mask: false }
    }
}

enum Flow {
    Continue,
    Submit,
    Cancel,
}

/// Text shown for a field and the caret cell within it; a masked field shows one `*` per grapheme.
fn field_view(edit: &LineEdit, mask: bool, room: usize) -> (String, usize) {
    if mask {
        ("*".repeat(edit.text().graphemes(true).count()), edit.caret())
    } else {
        edit.view(4, room)
    }
}

/// One key press: field switching, cancel/submit, otherwise the current field's editor.
fn handle_key(fields: &mut [Field], edits: &mut [LineEdit], cur: &mut usize, k: &KeyEvent) -> Flow {
    let n = fields.len();
    match k.code {
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => Flow::Cancel,
        KeyCode::Tab | KeyCode::Down => {
            *cur = (*cur + 1) % n;
            Flow::Continue
        }
        KeyCode::BackTab | KeyCode::Up => {
            *cur = (*cur + n - 1) % n;
            Flow::Continue
        }
        KeyCode::Char(c) if c.is_control() => Flow::Continue,
        _ => match edits[*cur].key(k) {
            Outcome::Cancel => Flow::Cancel,
            Outcome::Submit => Flow::Submit,
            Outcome::Handled => {
                if fields[*cur].value != edits[*cur].text() {
                    fields[*cur].value = edits[*cur].text().to_owned();
                    fields[*cur].error = None;
                }
                Flow::Continue
            }
            Outcome::Ignored => Flow::Continue,
        },
    }
}

/// Form session: one alt-screen for all screens.
pub struct Session {
    guard: Guard,
}

impl Session {
    pub fn enter() -> io::Result<Self> {
        Ok(Self { guard: Guard::enter()? })
    }

    /// Edits the fields. `submit` sets `error` on the fields and returns `true` if everything
    /// is accepted. `Ok(false)` means cancelled (Esc).
    pub fn edit(
        &mut self,
        title: &str,
        fields: &mut [Field],
        mut submit: impl FnMut(&mut [Field]) -> bool,
    ) -> io::Result<bool> {
        let mut cur = 0usize;
        // one editor per field (caret, grapheme editing); `Field::value` mirrors its text
        let mut edits: Vec<LineEdit> = fields.iter().map(|f| LineEdit::with_text(&f.value)).collect();
        let lw = fields.iter().map(|f| f.label.chars().count()).max().unwrap_or(0) + 2;
        loop {
            self.guard.term.draw(|f| {
                let mut lines = vec![
                    Line::styled(format!(" {title}"), Style::new().add_modifier(Modifier::BOLD)),
                    Line::raw(""),
                ];
                let mut cursor = (0u16, 0u16);
                for (i, fld) in fields.iter().enumerate() {
                    let sel = i == cur;
                    let label = format!(" {:<lw$}", fld.label);
                    let room = (f.area().width as usize).saturating_sub(label.chars().count() + 1);
                    let (value, caret_cell) = field_view(&edits[i], fld.mask, room);
                    if sel {
                        cursor = ((label.chars().count() + caret_cell) as u16, lines.len() as u16);
                    }
                    let lstyle = if sel {
                        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new()
                    };
                    let mut spans = vec![Span::styled(label, lstyle), Span::raw(value)];
                    if fld.value.is_empty() && !fld.hint.is_empty() {
                        spans.push(Span::styled(
                            visible_line(&fld.hint, 4),
                            Style::new().add_modifier(Modifier::DIM),
                        ));
                    }
                    lines.push(Line::from(spans));
                    if let Some(e) = &fld.error {
                        lines.push(Line::styled(
                            format!(" {:lw$}{}", "", visible_line(e, 4)),
                            Style::new().fg(Color::Red),
                        ));
                    }
                }
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    I18N.t(" Tab/Shift+Tab field, Enter save, Esc cancel"),
                    Style::new().add_modifier(Modifier::DIM),
                ));
                f.render_widget(Paragraph::new(lines), f.area());
                f.set_cursor_position(Position::new(cursor.0, cursor.1));
            })?;
            let Event::Key(k) = event::read()? else { continue };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match handle_key(fields, &mut edits, &mut cur, &k) {
                Flow::Cancel => return Ok(false),
                Flow::Submit => {
                    if submit(fields) {
                        return Ok(true);
                    }
                    if let Some(i) = fields.iter().position(|f| f.error.is_some()) {
                        cur = i;
                    }
                }
                Flow::Continue => {}
            }
        }
    }

    /// A custom screen in the same session (`saysdf files`: the tree between forms and confirmations).
    pub fn draw(&mut self, render: impl FnOnce(&mut ratatui::Frame)) -> io::Result<()> {
        self.guard.term.draw(render)?;
        Ok(())
    }

    /// Confirmation question: `true` only on Y.
    pub fn confirm(&mut self, text: &str) -> io::Result<bool> {
        self.show(&format!("{text}\n\n{}", crate::keys::CONFIRM_HINT))?;
        loop {
            if let (Event::Key(k), phys) = crate::keys::read()? {
                if k.kind == KeyEventKind::Press {
                    return Ok(crate::keys::confirmed(&k, phys));
                }
            }
        }
    }

    /// A message until any key.
    pub fn message(&mut self, text: &str) -> io::Result<()> {
        self.show(&I18N.f("{text}\n\nAny key to close", &[("text", &text)]))?;
        loop {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    return Ok(());
                }
            }
        }
    }

    fn show(&mut self, text: &str) -> io::Result<()> {
        let lines: Vec<Line> = text.lines().map(|l| Line::raw(format!(" {}", visible_line(l, 4)))).collect();
        self.guard.term.draw(|f| {
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), f.area());
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn setup(vals: &[&str]) -> (Vec<Field>, Vec<LineEdit>) {
        let fields: Vec<Field> = vals.iter().map(|v| Field::new("l", "", v)).collect();
        let edits = fields.iter().map(|f| LineEdit::with_text(&f.value)).collect();
        (fields, edits)
    }

    fn press(f: &mut [Field], e: &mut [LineEdit], cur: &mut usize, code: KeyCode) {
        handle_key(f, e, cur, &key(code));
    }

    #[test]
    fn edit_in_the_middle_of_a_field() {
        let (mut f, mut e) = setup(&["held"]);
        let mut cur = 0;
        press(&mut f, &mut e, &mut cur, KeyCode::Left);
        press(&mut f, &mut e, &mut cur, KeyCode::Left);
        press(&mut f, &mut e, &mut cur, KeyCode::Char('l'));
        assert_eq!(f[0].value, "helld");
        press(&mut f, &mut e, &mut cur, KeyCode::Delete);
        press(&mut f, &mut e, &mut cur, KeyCode::Home);
        press(&mut f, &mut e, &mut cur, KeyCode::Char('>'));
        press(&mut f, &mut e, &mut cur, KeyCode::End);
        press(&mut f, &mut e, &mut cur, KeyCode::Backspace);
        assert_eq!(f[0].value, ">hel");
    }

    #[test]
    fn masked_field_caret_and_cyrillic() {
        let (mut f, mut e) = setup(&["пароль"]); // i18n-allow-cyrillic
        let mut cur = 0;
        assert_eq!(field_view(&e[0], true, 20), ("******".to_owned(), 6));
        press(&mut f, &mut e, &mut cur, KeyCode::Left);
        press(&mut f, &mut e, &mut cur, KeyCode::Left);
        assert_eq!(field_view(&e[0], true, 20), ("******".to_owned(), 4));
        press(&mut f, &mut e, &mut cur, KeyCode::Char('x'));
        assert_eq!(field_view(&e[0], true, 20), ("*******".to_owned(), 5));
        assert_eq!(f[0].value, "пароxль"); // i18n-allow-cyrillic
    }

    #[test]
    fn tab_keeps_each_fields_text_and_caret() {
        let (mut f, mut e) = setup(&["ab", "cd"]);
        let mut cur = 0;
        press(&mut f, &mut e, &mut cur, KeyCode::Left);
        press(&mut f, &mut e, &mut cur, KeyCode::Tab);
        assert_eq!(cur, 1);
        press(&mut f, &mut e, &mut cur, KeyCode::Char('!'));
        press(&mut f, &mut e, &mut cur, KeyCode::BackTab);
        assert_eq!(cur, 0);
        press(&mut f, &mut e, &mut cur, KeyCode::Char('-'));
        assert_eq!((f[0].value.as_str(), f[1].value.as_str()), ("a-b", "cd!"));
        assert_eq!(e[0].caret(), 2);
    }

    #[test]
    fn enter_esc_ctrl_c_and_error_reset() {
        let (mut f, mut e) = setup(&["a"]);
        let mut cur = 0;
        f[0].error = Some("bad".into());
        assert!(matches!(handle_key(&mut f, &mut e, &mut cur, &key(KeyCode::Left)), Flow::Continue));
        assert!(f[0].error.is_some());
        press(&mut f, &mut e, &mut cur, KeyCode::Char('b'));
        assert!(f[0].error.is_none());
        assert!(matches!(handle_key(&mut f, &mut e, &mut cur, &key(KeyCode::Enter)), Flow::Submit));
        assert!(matches!(handle_key(&mut f, &mut e, &mut cur, &key(KeyCode::Esc)), Flow::Cancel));
        let c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(handle_key(&mut f, &mut e, &mut cur, &c), Flow::Cancel));
    }
}
