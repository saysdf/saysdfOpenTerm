//! Visual language of CriticMarkup marks, shared by the text viewers (`saysdf view`, source view of
//! `saysdf md`) and the rendered Markdown view. Each kind also carries a modifier (strikethrough,
//! underline, bold, dim), so marks stay distinguishable on a mono display.

use ratatui::style::{Color, Modifier, Style};

/// Markup characters (`{++`, `~>`, `<<}` ...) of a mark shown in its styled form.
pub fn markup() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// Markup characters of a mark that cannot be styled in place (rendered Markdown: the mark spans
/// several blocks), shown raw but coloured so it still reads as an annotation.
pub fn raw_markup() -> Style {
    Style::new().fg(Color::Yellow).add_modifier(Modifier::DIM)
}

pub fn deleted() -> Style {
    Style::new().fg(Color::Red).add_modifier(Modifier::CROSSED_OUT)
}

pub fn inserted() -> Style {
    Style::new().fg(Color::Green).add_modifier(Modifier::UNDERLINED)
}

pub fn highlighted() -> Style {
    Style::new().bg(Color::Rgb(110, 95, 30))
}

pub fn comment_badge() -> Style {
    Style::new().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
}
