//! Mermaid as character graphics (change mermaid-preview): flowchart and sequence diagrams are drawn
//! with box-drawing (or ASCII) right in the text; anything else is shown as source with a note ([`Fallback`]).
//!
//! - [`detect`] — diagram kind from the first significant line;
//! - [`flowchart`], [`sequence`] — hand-written parsers (design D3);
//! - [`graph`] — flowchart layout (simplified Sugiyama, D4) and drawing;
//! - [`canvas`] — character grid (D5);
//! - [`render`] — entry point for `saysdf mermaid` and `saysdf md` (via [`markdown_renderer`]).

pub mod canvas;
pub mod flowchart;
pub mod graph;
pub mod sequence;

use crate::sanitize::{char_width, str_width, visible_line, DEFAULT_TAB_WIDTH};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("render-mermaid");

/// Node limit: above it the source is shown (design: Risks).
pub const MAX_NODES: usize = 300;
/// Source length limit (bytes), protection against pathological input.
pub const MAX_SOURCE: usize = 1 << 20;

/// Parse error with a line number (1-based).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

/// Diagram kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Flowchart,
    Sequence,
    Unsupported(String),
    Empty,
}

/// Significant lines: no blank lines, `%%` comments or `---` front matter; `(0-based number, text)`.
pub fn body_lines(src: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut front = false;
    let mut first = true;
    src.lines().enumerate().filter(move |(_, l)| {
        let t = l.trim();
        if first && t == "---" {
            front = true;
            first = false;
            return false;
        }
        if front {
            if t == "---" {
                front = false;
            }
            return false;
        }
        if t.is_empty() || t.starts_with("%%") {
            return false;
        }
        first = false;
        true
    })
}

/// Diagram kind from the first significant line.
pub fn detect(src: &str) -> Kind {
    let Some((_, l)) = body_lines(src).next() else { return Kind::Empty };
    let kw = l.split(|c: char| c.is_whitespace() || c == ';').next().unwrap_or("");
    match kw {
        "flowchart" | "graph" => Kind::Flowchart,
        "sequenceDiagram" => Kind::Sequence,
        other => Kind::Unsupported(other.to_owned()),
    }
}

/// Density level (design D7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum Zoom {
    Compact,
    #[default]
    Normal,
    Spacious,
}

impl Zoom {
    pub fn zoom_in(self) -> Self {
        match self {
            Zoom::Compact => Zoom::Normal,
            _ => Zoom::Spacious,
        }
    }
    pub fn zoom_out(self) -> Self {
        match self {
            Zoom::Spacious => Zoom::Normal,
            _ => Zoom::Compact,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Zoom::Compact => "compact",
            Zoom::Normal => "normal",
            Zoom::Spacious => "spacious",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "compact" => Some(Zoom::Compact),
            "normal" => Some(Zoom::Normal),
            "spacious" => Some(Zoom::Spacious),
            _ => None,
        }
    }
    /// Offset from Normal: -1, 0, 1 (for [`crate::markdown::LayoutOptions::mermaid_zoom`]).
    pub fn from_step(step: i8) -> Self {
        match step {
            i8::MIN..=-1 => Zoom::Compact,
            0 => Zoom::Normal,
            _ => Zoom::Spacious,
        }
    }
    pub fn step(self) -> i8 {
        self as i8 - 1
    }
}

/// Layout parameters derived from [`Zoom`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutParams {
    pub node_gap: usize,
    pub layer_gap: usize,
    pub pad_x: usize,
    pub max_label: Option<usize>,
    /// Wrap node labels by width (Spacious).
    pub wrap: Option<usize>,
    pub seq_min_gap: usize,
}

impl From<Zoom> for LayoutParams {
    fn from(z: Zoom) -> Self {
        match z {
            Zoom::Compact => Self { node_gap: 2, layer_gap: 2, pad_x: 0, max_label: Some(12), wrap: None, seq_min_gap: 4 },
            Zoom::Normal => Self { node_gap: 4, layer_gap: 3, pad_x: 1, max_label: Some(24), wrap: None, seq_min_gap: 8 },
            Zoom::Spacious => {
                Self { node_gap: 6, layer_gap: 5, pad_x: 2, max_label: None, wrap: Some(30), seq_min_gap: 12 }
            }
        }
    }
}

/// Truncate a label to `max` cells with `…`; control characters are shown as visible glyphs.
pub fn fit_label(s: &str, max: Option<usize>) -> String {
    let s = visible_line(s.trim(), DEFAULT_TAB_WIDTH);
    let Some(max) = max else { return s };
    if str_width(&s) <= max {
        return s;
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// A node and its centre in diagram lines (zoom anchor).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub id: String,
    pub x: usize,
    pub y: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Opts {
    pub zoom: Zoom,
    pub ascii: bool,
}

/// A rendered diagram.
#[derive(Clone, Debug)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub anchors: Vec<Anchor>,
    pub width: usize,
}

/// Why the diagram is shown as source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fallback {
    Empty,
    Unsupported(String),
    Parse(ParseError),
    TooLarge(usize),
}

impl std::fmt::Display for Fallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fallback::Empty => f.write_str(&I18N.t("empty diagram")),
            Fallback::Unsupported(k) => f.write_str(&I18N.f("{kind} type is not supported yet", &[("kind", &visible_line(k, DEFAULT_TAB_WIDTH))])),
            Fallback::Parse(e) => f.write_str(&I18N.f("line {line}: {msg}", &[("line", &e.line), ("msg", &visible_line(&e.msg, DEFAULT_TAB_WIDTH))])),
            Fallback::TooLarge(n) => f.write_str(&I18N.f("diagram too large ({n} nodes, limit {max})", &[("n", n), ("max", &MAX_NODES)])),
        }
    }
}

/// Render Mermaid source.
pub fn render(src: &str, o: &Opts) -> Result<Rendered, Fallback> {
    if src.len() > MAX_SOURCE {
        return Err(Fallback::TooLarge(src.len()));
    }
    let p = LayoutParams::from(o.zoom);
    let (cv, anchors, notes) = match detect(src) {
        Kind::Empty => return Err(Fallback::Empty),
        Kind::Unsupported(k) => return Err(Fallback::Unsupported(k)),
        Kind::Flowchart => {
            let fc = flowchart::parse(src).map_err(Fallback::Parse)?;
            if fc.nodes.is_empty() {
                return Err(Fallback::Empty);
            }
            if fc.nodes.len() > MAX_NODES {
                return Err(Fallback::TooLarge(fc.nodes.len()));
            }
            let lay = graph::layout(&fc, &p);
            let d = graph::draw(&fc, &lay, &p, o.ascii);
            (d.canvas, d.anchors, d.footnotes)
        }
        Kind::Sequence => {
            let sq = sequence::parse(src).map_err(Fallback::Parse)?;
            if sq.participants.is_empty() {
                return Err(Fallback::Empty);
            }
            if sq.participants.len() > MAX_NODES || sq.events.len() > 10 * MAX_NODES {
                return Err(Fallback::TooLarge(sq.participants.len().max(sq.events.len())));
            }
            let (cv, a) = sequence::draw(&sq, &p, o.ascii);
            (cv, a, Vec::new())
        }
    };
    let mut lines = cv.to_lines();
    for (i, n) in notes.iter().enumerate() {
        lines.push(Line::from(Span::styled(format!("({}) {n}", i + 1), Style::new().fg(Color::DarkGray))));
    }
    Ok(Rendered { width: cv.width(), lines, anchors })
}

/// Source with a note (spec: unsupported kinds and errors are shown as source).
pub fn fallback_lines(src: &str, why: &Fallback, ascii: bool) -> Vec<Line<'static>> {
    let dim = Style::new().fg(Color::DarkGray);
    let (top, bar, bot) = if ascii { ("+- ", "| ", "+-") } else { ("╭─ ", "│ ", "╰─") };
    let mut out = vec![Line::from(Span::styled(format!("{top}{}", I18N.f("mermaid: {why} — source", &[("why", why)])), Style::new().fg(Color::Yellow)))];
    for l in src.lines() {
        out.push(Line::from(vec![Span::styled(bar.to_owned(), dim), Span::raw(visible_line(l, DEFAULT_TAB_WIDTH))]));
    }
    out.push(Line::from(Span::styled(bot.to_owned(), dim)));
    out
}

/// Hook for [`crate::markdown::LayoutOptions::mermaid`]: the diagram, or the source with a note.
pub fn markdown_renderer(src: &str, _width: usize, ascii: bool, zoom: i8) -> Option<Vec<Line<'static>>> {
    Some(match render(src, &Opts { zoom: Zoom::from_step(zoom), ascii }) {
        Ok(r) => r.lines,
        Err(f) => fallback_lines(src, &f, ascii),
    })
}

#[cfg(test)]
mod tests;
