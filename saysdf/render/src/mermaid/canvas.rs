//! Canvas (design D5): a grid of cells; lines are an N/E/S/W direction mask + style, the final character
//! is chosen from the mask, so junctions and crossings (`┼`, `├`, `┬`) come out on their own. Text goes through
//! [`crate::sanitize`] width-aware (a wide character takes 2 cells).

use crate::sanitize::{char_width, visible_line, DEFAULT_TAB_WIDTH};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub const N: u8 = 1;
pub const E: u8 = 2;
pub const S: u8 = 4;
pub const W: u8 = 8;

/// Line style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Stroke {
    #[default]
    Thin,
    Thick,
    Dotted,
}

/// Cell role → colour (design D8, ANSI palette only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Ink {
    #[default]
    Border,
    Label,
    EdgeLabel,
    Frame,
}

impl Ink {
    fn style(self) -> Style {
        match self {
            Ink::Border => Style::new(),
            Ink::Label => Style::new().add_modifier(Modifier::BOLD),
            Ink::EdgeLabel => Style::new().fg(Color::DarkGray),
            Ink::Frame => Style::new().fg(Color::Cyan),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Cell {
    mask: u8,
    stroke: Stroke,
    /// Explicit character (text, arrow, special corner); takes priority over the mask.
    ch: Option<char>,
    /// Continuation of a wide character on the left.
    cont: bool,
    /// Rounded corners (`╭╮╰╯`) if the mask is a corner.
    round: bool,
    ink: Ink,
}

/// Character grid that grows as drawing proceeds.
#[derive(Clone, Debug)]
pub struct Canvas {
    pub ascii: bool,
    w: usize,
    h: usize,
    cells: Vec<Vec<Cell>>,
}

/// Character for a direction mask.
pub fn glyph(mask: u8, stroke: Stroke, round: bool, ascii: bool) -> char {
    let horiz = mask & (N | S) == 0;
    let vert = mask & (E | W) == 0;
    if ascii {
        return match (horiz, vert, stroke) {
            (_, _, _) if mask == 0 => ' ',
            (true, _, Stroke::Thick) => '=',
            (true, _, Stroke::Dotted) => '.',
            (true, _, _) => '-',
            (_, true, Stroke::Dotted) => ':',
            (_, true, _) => '|',
            _ => '+',
        };
    }
    if round {
        match mask {
            m if m == E | S => return '╭',
            m if m == S | W => return '╮',
            m if m == N | E => return '╰',
            m if m == N | W => return '╯',
            _ => {}
        }
    }
    if stroke == Stroke::Dotted {
        if horiz && mask != 0 {
            return '┄';
        }
        if vert && mask != 0 {
            return '┆';
        }
    }
    let thin = [
        ' ', '│', '─', '└', '│', '│', '┌', '├', '─', '┘', '─', '┴', '┐', '┤', '┬', '┼',
    ];
    let thick = [
        ' ', '┃', '━', '┗', '┃', '┃', '┏', '┣', '━', '┛', '━', '┻', '┓', '┫', '┳', '╋',
    ];
    let t = if stroke == Stroke::Thick { &thick } else { &thin };
    t[(mask & 15) as usize]
}

impl Canvas {
    pub fn new(ascii: bool) -> Self {
        Self { ascii, w: 0, h: 0, cells: Vec::new() }
    }

    pub fn width(&self) -> usize {
        self.w
    }

    pub fn height(&self) -> usize {
        self.h
    }

    fn cell(&mut self, x: usize, y: usize) -> &mut Cell {
        if y >= self.h {
            self.h = y + 1;
            self.cells.resize_with(self.h, Vec::new);
        }
        if x >= self.w {
            self.w = x + 1;
        }
        let row = &mut self.cells[y];
        if row.len() <= x {
            row.resize_with(x + 1, Cell::default);
        }
        &mut row[x]
    }

    /// Add directions to a cell (line junction). Cells with text are left alone.
    pub fn add(&mut self, x: usize, y: usize, mask: u8, stroke: Stroke, ink: Ink) {
        let c = self.cell(x, y);
        if c.ch.is_some() || c.cont {
            return;
        }
        if c.mask == 0 || stroke != Stroke::Thin {
            c.stroke = stroke;
        }
        if c.mask == 0 {
            c.ink = ink;
        }
        c.mask |= mask;
    }

    /// Mark a corner as rounded.
    pub fn set_round(&mut self, x: usize, y: usize) {
        self.cell(x, y).round = true;
    }

    /// Explicit character (arrow, special corner).
    pub fn put(&mut self, x: usize, y: usize, ch: char, ink: Ink) {
        let c = self.cell(x, y);
        *c = Cell { ch: Some(ch), ink, ..Cell::default() };
    }

    /// Clear a rectangle (room for a node).
    pub fn clear(&mut self, x: usize, y: usize, w: usize, h: usize) {
        for yy in y..y + h {
            for xx in x..x + w {
                *self.cell(xx, yy) = Cell::default();
            }
        }
    }

    /// Horizontal line `[x0, x1]` (inclusive), ends open towards the inside.
    pub fn hline(&mut self, x0: usize, x1: usize, y: usize, stroke: Stroke, ink: Ink) {
        let (a, b) = (x0.min(x1), x0.max(x1));
        for x in a..=b {
            let mut m = 0;
            if x > a {
                m |= W;
            }
            if x < b {
                m |= E;
            }
            if a == b {
                m = E | W;
            }
            self.add(x, y, m, stroke, ink);
        }
    }

    /// Vertical line `[y0, y1]`.
    pub fn vline(&mut self, x: usize, y0: usize, y1: usize, stroke: Stroke, ink: Ink) {
        let (a, b) = (y0.min(y1), y0.max(y1));
        for y in a..=b {
            let mut m = 0;
            if y > a {
                m |= N;
            }
            if y < b {
                m |= S;
            }
            if a == b {
                m = N | S;
            }
            self.add(x, y, m, stroke, ink);
        }
    }

    /// Rectangular frame.
    #[allow(clippy::too_many_arguments)]
    pub fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, stroke: Stroke, round: bool, ink: Ink) {
        if w < 2 || h < 2 {
            return;
        }
        let (x1, y1) = (x + w - 1, y + h - 1);
        self.add(x, y, E | S, stroke, ink);
        self.add(x1, y, S | W, stroke, ink);
        self.add(x, y1, N | E, stroke, ink);
        self.add(x1, y1, N | W, stroke, ink);
        if round {
            for (cx, cy) in [(x, y), (x1, y), (x, y1), (x1, y1)] {
                self.set_round(cx, cy);
            }
        }
        for xx in x + 1..x1 {
            self.add(xx, y, E | W, stroke, ink);
            self.add(xx, y1, E | W, stroke, ink);
        }
        for yy in y + 1..y1 {
            self.add(x, yy, N | S, stroke, ink);
            self.add(x1, yy, N | S, stroke, ink);
        }
    }

    /// Text (escaped); returns the width in cells.
    pub fn text(&mut self, x: usize, y: usize, s: &str, ink: Ink) -> usize {
        let vis = visible_line(s, DEFAULT_TAB_WIDTH);
        let mut col = x;
        for ch in vis.chars() {
            let w = char_width(ch);
            if w == 0 {
                continue;
            }
            self.put(col, y, ch, ink);
            if w == 2 {
                let c = self.cell(col + 1, y);
                *c = Cell { cont: true, ink, ..Cell::default() };
            }
            col += w;
        }
        col - x
    }

    /// Cell character (for tests and checks).
    pub fn char_at(&self, x: usize, y: usize) -> char {
        let Some(c) = self.cells.get(y).and_then(|r| r.get(x)) else { return ' ' };
        if c.cont {
            return '\0';
        }
        c.ch.unwrap_or_else(|| glyph(c.mask, c.stroke, c.round, self.ascii))
    }

    /// Whether the cell holds anything but a space.
    pub fn is_used(&self, x: usize, y: usize) -> bool {
        self.cells
            .get(y)
            .and_then(|r| r.get(x))
            .is_some_and(|c| c.mask != 0 || c.ch.is_some() || c.cont)
    }

    /// Lines without styles (no trailing spaces).
    pub fn to_strings(&self) -> Vec<String> {
        (0..self.h)
            .map(|y| {
                let s: String = (0..self.w).map(|x| self.char_at(x, y)).filter(|&c| c != '\0').collect();
                s.trim_end().to_owned()
            })
            .collect()
    }

    /// Styled lines for the pager.
    pub fn to_lines(&self) -> Vec<Line<'static>> {
        let mut out = Vec::with_capacity(self.h);
        for y in 0..self.h {
            let row = &self.cells[y];
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut cur = String::new();
            let mut cur_ink = Ink::Border;
            for (x, c) in row.iter().enumerate() {
                if c.cont {
                    continue;
                }
                let ch = self.char_at(x, y);
                let ink = if ch == ' ' { Ink::Border } else { c.ink };
                if ink != cur_ink && !cur.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut cur), cur_ink.style()));
                }
                cur_ink = ink;
                cur.push(ch);
            }
            let t = cur.trim_end().to_owned();
            if !t.is_empty() {
                spans.push(Span::styled(t, cur_ink.style()));
            }
            out.push(Line::from(spans));
        }
        out
    }
}
