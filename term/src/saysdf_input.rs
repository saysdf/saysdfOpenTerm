use crate::{CellAttributes, Line, SemanticType, TerminalState};

const MAX_ROWS: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptInput {
    pub text: String,
    pub cursor_at_end: bool,
}

pub fn prompt_input(term: &TerminalState) -> Option<PromptInput> {
    if term.is_alt_screen_active() || term.pen().semantic_type() == SemanticType::Output {
        return None;
    }
    let screen = term.screen();
    let cursor = term.cursor_pos();
    let cursor_phys = screen.phys_row(cursor.y);
    let total = screen.scrollback_rows();
    if cursor_phys >= total {
        return None;
    }
    let first = cursor_phys.saturating_sub(MAX_ROWS - 1);
    let last = total.min(cursor_phys + MAX_ROWS);
    let mut result = None;
    screen.with_phys_lines(first..last, |lines| {
        result = read_input(lines, cursor_phys - first, cursor.x);
    });
    result
}

fn is_blank(text: &str) -> bool {
    text.chars().all(|c| c == ' ')
}

fn same_look(a: &CellAttributes, b: &CellAttributes) -> bool {
    a.foreground() == b.foreground() && a.intensity() == b.intensity() && a.italic() == b.italic()
}

fn find_start(lines: &[&Line], cursor_row: usize, cursor_x: usize) -> Option<(usize, usize)> {
    let mut row = cursor_row;
    let mut limit = cursor_x;
    loop {
        let mut boundary = None;
        for cell in lines[row].visible_cells() {
            if cell.cell_index() >= limit {
                break;
            }
            let kind = cell.attrs().semantic_type();
            if kind != SemanticType::Input {
                boundary = Some((cell.cell_index() + cell.width(), kind));
            }
        }
        match boundary {
            Some((col, SemanticType::Prompt)) => return Some((row, col)),
            Some(_) => return None,
            None if row == 0 || !lines[row - 1].last_cell_was_wrapped() => return None,
            None => {
                row -= 1;
                limit = usize::MAX;
            }
        }
    }
}

fn read_input(lines: &[&Line], cursor_row: usize, cursor_x: usize) -> Option<PromptInput> {
    let (start_row, start_col) = find_start(lines, cursor_row, cursor_x)?;

    let mut text = String::new();
    let mut typed_look = CellAttributes::default();
    for (row, line) in lines
        .iter()
        .enumerate()
        .take(cursor_row + 1)
        .skip(start_row)
    {
        for cell in line.visible_cells() {
            let idx = cell.cell_index();
            if row == start_row && idx < start_col {
                continue;
            }
            if row == cursor_row && idx >= cursor_x {
                break;
            }
            text.push_str(cell.str());
            typed_look = cell.attrs().clone();
        }
    }

    let mut cursor_at_end = true;
    for (row, line) in lines.iter().enumerate().skip(cursor_row) {
        for cell in line.visible_cells() {
            if row == cursor_row && cell.cell_index() < cursor_x {
                continue;
            }
            let attrs = cell.attrs();
            let ok = match attrs.semantic_type() {
                _ if is_blank(cell.str()) => true,
                SemanticType::Prompt => true,
                SemanticType::Input => !same_look(attrs, &typed_look),
                SemanticType::Output => false,
            };
            if !ok {
                cursor_at_end = false;
                break;
            }
        }
        if !cursor_at_end || !line.last_cell_was_wrapped() {
            break;
        }
    }

    Some(PromptInput {
        text,
        cursor_at_end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorPalette;
    use crate::{Terminal, TerminalConfiguration, TerminalSize};
    use std::sync::Arc;

    #[derive(Debug)]
    struct Config;
    impl TerminalConfiguration for Config {
        fn color_palette(&self) -> ColorPalette {
            ColorPalette::default()
        }
    }

    fn term(cols: usize, bytes: &str) -> Terminal {
        let mut term = Terminal::new(
            TerminalSize {
                rows: 10,
                cols,
                pixel_width: cols * 8,
                pixel_height: 160,
                dpi: 0,
            },
            Arc::new(Config),
            "test",
            "0",
            Box::new(Vec::new()),
        );
        term.advance_bytes("\x1b[?7h");
        term.advance_bytes(bytes);
        term
    }

    const A: &str = "\x1b]133;A\x07";
    const B: &str = "\x1b]133;B\x07";
    const C: &str = "\x1b]133;C\x07";
    const P: &str = "\x1b]133;P\x07";

    fn read(cols: usize, bytes: &str) -> Option<PromptInput> {
        prompt_input(&term(cols, bytes))
    }

    fn input(text: &str, cursor_at_end: bool) -> Option<PromptInput> {
        Some(PromptInput {
            text: text.to_string(),
            cursor_at_end,
        })
    }

    #[test]
    fn typed_at_end() {
        assert_eq!(
            read(80, &format!("{A}user$ {B}git ch")),
            input("git ch", true)
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(read(80, &format!("{A}$ {B}")), input("", true));
    }

    #[test]
    fn cursor_moved_left() {
        assert_eq!(
            read(80, &format!("{A}$ {B}git ch\x1b[2D")),
            input("git ", false)
        );
    }

    #[test]
    fn trailing_spaces_after_cursor() {
        assert_eq!(
            read(80, &format!("{A}$ {B}ls   \x1b[3D")),
            input("ls", true)
        );
    }

    #[test]
    fn grey_tail_fish() {
        assert_eq!(
            read(80, &format!("{A}> {B}gi\x1b[90mt log\x1b[0m\x1b[5D")),
            input("gi", true)
        );
    }

    #[test]
    fn grey_tail_psreadline() {
        assert_eq!(
            read(80, &format!("{A}PS> {B}gi\x1b[97;2;3mt log\x1b[0m\x1b[5D")),
            input("gi", true)
        );
    }

    #[test]
    fn tail_same_look_is_not_end() {
        assert_eq!(
            read(80, &format!("{A}> {B}git log\x1b[5D")),
            input("gi", false)
        );
    }

    #[test]
    fn zsh_right_prompt() {
        assert_eq!(
            read(80, &format!("{A}% {B}ls\x1b7{P}\x1b[70G[main]\x1b8{B}")),
            input("ls", true)
        );
    }

    #[test]
    fn running_command_is_not_prompt() {
        assert_eq!(read(80, &format!("{A}$ {B}cat\r\n{C}hello")), None);
        assert_eq!(read(80, &format!("{A}$ {B}cat{C}")), None);
    }

    #[test]
    fn alt_screen_is_not_prompt() {
        assert_eq!(read(80, &format!("{A}$ {B}vim\x1b[?1049h")), None);
    }

    #[test]
    fn no_marks_is_not_prompt() {
        assert_eq!(read(80, "$ git ch"), None);
    }

    #[test]
    fn wrapped_input() {
        assert_eq!(
            read(10, &format!("{A}$ {B}echo abcdefghij")),
            input("echo abcdefghij", true)
        );
    }

    #[test]
    fn wrapped_input_cursor_on_first_row() {
        assert_eq!(
            read(10, &format!("{A}$ {B}echo abcdefghij\x1bM\x1b[8G")),
            input("echo ", false)
        );
    }

    #[test]
    fn multi_line_input_is_not_prompt() {
        assert_eq!(read(80, &format!("{A}$ {B}echo 'a\r\nb")), None);
    }

    #[test]
    fn prompt_farther_than_limit_is_ignored() {
        let mut bytes = format!("{A}$ {B}");
        bytes.push_str(&"x".repeat(10 * MAX_ROWS));
        assert_eq!(read(10, &bytes), None);
    }
}
