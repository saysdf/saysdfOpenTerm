//! Syntax highlighting via syntect (design D3, D4): colors are the 16 terminal palette colors,
//! no syntect themes; the parser state is saved in checkpoints, only the window is highlighted.

use crate::sanitize::push_visible;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

/// Parser state checkpoint every this many lines.
pub const CHECKPOINT_EVERY: usize = 1000;
/// Longer lines (in characters) are not highlighted (fancy-regex is slow on long lines).
pub const MAX_LINE_CHARS: usize = 20_000;

/// Built-in syntax set, loaded lazily on the first highlight.
pub fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Table of scope prefixes → palette color; `None` means "no color" (stops the stack search).
/// Order matters: more specific prefixes come before general ones.
const SCOPE_COLORS: &[(&str, Option<Color>)] = &[
    ("comment", Some(Color::DarkGray)),
    ("string", Some(Color::Green)),
    ("constant.character.escape", Some(Color::Cyan)),
    ("constant.numeric", Some(Color::Yellow)),
    ("constant", Some(Color::Yellow)),
    ("keyword.operator", None),
    ("keyword", Some(Color::Magenta)),
    ("storage.type", Some(Color::Cyan)),
    ("storage", Some(Color::Magenta)),
    ("entity.name.function", Some(Color::Blue)),
    ("support.function", Some(Color::Blue)),
    ("variable.function", Some(Color::Blue)),
    ("entity.name.tag", Some(Color::Red)),
    ("entity.name", Some(Color::Cyan)),
    ("support.type", Some(Color::Cyan)),
    ("support.class", Some(Color::Cyan)),
    ("entity.other.attribute-name", Some(Color::Yellow)),
    ("markup.heading", Some(Color::Blue)),
    ("markup.inserted", Some(Color::Green)),
    ("markup.deleted", Some(Color::Red)),
    ("markup.raw", Some(Color::Green)),
    ("invalid", Some(Color::LightRed)),
];

fn scope_table() -> &'static [(Scope, Option<Color>)] {
    static TABLE: OnceLock<Vec<(Scope, Option<Color>)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        SCOPE_COLORS
            .iter()
            .filter_map(|(s, c)| Scope::new(s).ok().map(|s| (s, *c)))
            .collect()
    })
}

/// Color of one scope from the table: `Some(None)` is explicitly no color, `None` means the scope is not in the table.
fn scope_color(scope: Scope) -> Option<Option<Color>> {
    scope_table()
        .iter()
        .find(|(prefix, _)| prefix.is_prefix_of(scope))
        .map(|(_, c)| *c)
}

/// Highlighting of one file: the selected syntax plus state checkpoints.
pub struct Highlighter {
    syntax: Option<&'static SyntaxReference>,
    tab_width: usize,
    /// `checkpoints[k]` is the state before line `k * CHECKPOINT_EVERY`.
    checkpoints: Vec<(ParseState, ScopeStack)>,
    cache: HashMap<Scope, Option<Option<Color>>>,
}

impl Highlighter {
    /// Syntax by file name/extension, then by the first line (shebang, modeline);
    /// if none is found, there is no highlighting.
    pub fn for_path(path: &Path, first_line: &str, tab_width: usize) -> Self {
        let ss = syntax_set();
        let syntax = ss
            .find_syntax_for_file(path)
            .ok()
            .flatten()
            .or_else(|| ss.find_syntax_by_first_line(first_line))
            .filter(|s| s.name != ss.find_syntax_plain_text().name);
        Self::with_syntax(syntax, tab_width)
    }

    /// Highlighting by language token (`rs`, `python`, …), for Markdown code blocks and the like.
    pub fn for_token(token: &str, tab_width: usize) -> Self {
        Self::with_syntax(syntax_set().find_syntax_by_token(token), tab_width)
    }

    /// No highlighting (large files, unknown language).
    pub fn plain(tab_width: usize) -> Self {
        Self::with_syntax(None, tab_width)
    }

    fn with_syntax(syntax: Option<&'static SyntaxReference>, tab_width: usize) -> Self {
        Self {
            syntax,
            tab_width,
            checkpoints: Vec::new(),
            cache: HashMap::new(),
        }
    }

    /// Syntax name, if one is selected.
    pub fn syntax_name(&self) -> Option<&str> {
        self.syntax.map(|s| s.name.as_str())
    }

    /// Reset the checkpoints (the file was re-read).
    pub fn invalidate(&mut self) {
        self.checkpoints.clear();
    }

    /// Highlights lines `[from, to)` of `lines` (clamped); the lines are raw, escaping happens here.
    pub fn highlight_window(&mut self, lines: &[String], from: usize, to: usize) -> Vec<Line<'static>> {
        let to = to.min(lines.len());
        let from = from.min(to);
        let Some(syntax) = self.syntax else {
            return lines[from..to]
                .iter()
                .map(|l| plain_line(l, self.tab_width))
                .collect();
        };
        let ss = syntax_set();
        if self.checkpoints.is_empty() {
            self.checkpoints
                .push((ParseState::new(syntax), ScopeStack::new()));
        }
        // Advance to the needed checkpoint, building the missing ones
        let want = from / CHECKPOINT_EVERY;
        while self.checkpoints.len() <= want {
            let k = self.checkpoints.len() - 1;
            let (mut ps, mut st) = self.checkpoints[k].clone();
            let start = k * CHECKPOINT_EVERY;
            let end = (start + CHECKPOINT_EVERY).min(lines.len());
            for l in &lines[start..end] {
                advance(&mut ps, &mut st, l, ss);
            }
            self.checkpoints.push((ps, st));
        }
        let (mut ps, mut st) = self.checkpoints[want].clone();
        for l in &lines[want * CHECKPOINT_EVERY..from] {
            advance(&mut ps, &mut st, l, ss);
        }
        let mut out = Vec::with_capacity(to - from);
        for l in &lines[from..to] {
            out.push(self.render_line(&mut ps, &mut st, l, ss));
        }
        out
    }

    /// Highlighting of the whole text (small fragments).
    pub fn highlight_all(&mut self, lines: &[String]) -> Vec<Line<'static>> {
        self.highlight_window(lines, 0, lines.len())
    }

    fn render_line(
        &mut self,
        ps: &mut ParseState,
        st: &mut ScopeStack,
        line: &str,
        ss: &SyntaxSet,
    ) -> Line<'static> {
        if line.chars().count() > MAX_LINE_CHARS {
            return plain_line(line, self.tab_width);
        }
        let with_nl = format!("{line}\n");
        let Ok(ops) = ps.parse_line(&with_nl, ss) else {
            return plain_line(line, self.tab_width);
        };
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut col = 0;
        let mut pos = 0;
        let tab_width = self.tab_width;
        let emit = |spans: &mut Vec<Span<'static>>, text: &str, style: Style, col: &mut usize| {
            if text.is_empty() {
                return;
            }
            let mut s = String::new();
            push_visible(&mut s, text, col, tab_width);
            match spans.last_mut() {
                Some(last) if last.style == style => last.content.to_mut().push_str(&s),
                _ => spans.push(Span::styled(s, style)),
            }
        };
        let mut style = self.style_for(st);
        for (at, op) in ops {
            let at = at.min(line.len());
            if at > pos {
                emit(&mut spans, &line[pos..at], style, &mut col);
                pos = at;
            }
            if st.apply(&op).is_ok() {
                style = self.style_for(st);
            }
        }
        if pos < line.len() {
            emit(&mut spans, &line[pos..], style, &mut col);
        }
        Line::from(spans)
    }

    fn style_for(&mut self, st: &ScopeStack) -> Style {
        for &scope in st.as_slice().iter().rev() {
            let c = *self.cache.entry(scope).or_insert_with(|| scope_color(scope));
            if let Some(color) = c {
                return match color {
                    Some(color) => Style::new().fg(color),
                    None => Style::new(),
                };
            }
        }
        Style::new()
    }
}

/// Advance the parser state by one line without building spans.
fn advance(ps: &mut ParseState, st: &mut ScopeStack, line: &str, ss: &SyntaxSet) {
    if line.chars().count() > MAX_LINE_CHARS {
        return;
    }
    if let Ok(ops) = ps.parse_line(&format!("{line}\n"), ss) {
        for (_, op) in ops {
            let _ = st.apply(&op);
        }
    }
}

/// Line without highlighting (already escaped).
pub fn plain_line(line: &str, tab_width: usize) -> Line<'static> {
    Line::raw(crate::sanitize::visible_line(line, tab_width))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn rust_keyword_gets_color() {
        let mut h = Highlighter::for_path(Path::new("main.rs"), "fn main() {}", 4);
        assert_eq!(h.syntax_name(), Some("Rust"));
        let out = h.highlight_all(&lines("fn main() {}"));
        let fn_span = out[0]
            .spans
            .iter()
            .find(|s| s.content.contains("fn"))
            .unwrap();
        assert_eq!(fn_span.content, "fn");
        assert!(fn_span.style.fg.is_some(), "{:?}", out[0]);
    }

    #[test]
    fn unknown_extension_is_plain() {
        let mut h = Highlighter::for_path(Path::new("x.unknownext"), "hello\tworld", 4);
        assert!(h.syntax_name().is_none());
        let out = h.highlight_all(&lines("hello\tworld\x1b"));
        assert_eq!(out[0].spans.len(), 1);
        assert_eq!(out[0].spans[0].style, Style::new());
        assert_eq!(out[0].spans[0].content, "hello   world\u{241B}");
    }

    #[test]
    fn window_matches_full_highlight() {
        // Multi-line comment across a checkpoint boundary
        let mut text = String::new();
        for i in 0..CHECKPOINT_EVERY + 5 {
            text.push_str(&format!("let x{i} = {i};\n"));
        }
        text.push_str("/* start\n");
        for _ in 0..CHECKPOINT_EVERY {
            text.push_str("inside comment fn let\n");
        }
        text.push_str("end */ fn f() {}\n");
        let src = lines(&text);
        let full = Highlighter::for_path(Path::new("a.rs"), "", 4).highlight_all(&src);
        let from = 2 * CHECKPOINT_EVERY - 3;
        let to = src.len();
        let mut h = Highlighter::for_path(Path::new("a.rs"), "", 4);
        let window = h.highlight_window(&src, from, to);
        assert_eq!(window, full[from..to]);
        assert_eq!(window[0].spans[0].style.fg, Some(Color::DarkGray));
        // Repeated window served from the checkpoint cache
        assert_eq!(h.highlight_window(&src, 3, 5), full[3..5]);
    }
}
