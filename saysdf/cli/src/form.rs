//! Shared ratatui form: input fields, Tab/Shift+Tab (up/down), a per-field error, Enter to submit,
//! Esc to cancel; a `[y/N]` confirmation screen (default "no") and a message screen.
//! The terminal comes from [`pager::Guard`] (restored on exit and on panic).

use crate::pager::Guard;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::Position;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use saysdf_render::sanitize::visible_line;
use std::io;

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
                    let value = if fld.mask { "*".repeat(fld.value.chars().count()) } else { visible_line(&fld.value, 4) };
                    if sel {
                        cursor = ((label.chars().count() + value.chars().count()) as u16, lines.len() as u16);
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
            let n = fields.len();
            match k.code {
                KeyCode::Esc => return Ok(false),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                KeyCode::Tab | KeyCode::Down => cur = (cur + 1) % n,
                KeyCode::BackTab | KeyCode::Up => cur = (cur + n - 1) % n,
                KeyCode::Enter => {
                    if submit(fields) {
                        return Ok(true);
                    }
                    if let Some(i) = fields.iter().position(|f| f.error.is_some()) {
                        cur = i;
                    }
                }
                KeyCode::Backspace => {
                    fields[cur].value.pop();
                    fields[cur].error = None;
                }
                KeyCode::Char(c) if !c.is_control() => {
                    fields[cur].value.push(c);
                    fields[cur].error = None;
                }
                _ => {}
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
