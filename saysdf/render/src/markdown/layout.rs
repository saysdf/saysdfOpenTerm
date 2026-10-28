//! Layout of the model by width (design D3) → [`crate::doc::Doc`] view lines + anchors, headings,
//! links, image slots. All texts pass through [`crate::sanitize`].

use super::image::{self, ImageSource};
use super::model::{plain_text, slug, Align, Block, BlockKind, Doc, Inline, Item};
use crate::doc::Doc as ViewDoc;
use crate::highlight::Highlighter;
use crate::sanitize::{char_width, str_width, visible_line, DEFAULT_TAB_WIDTH};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::{Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("render-markdown");

/// Hook for rendering ```mermaid blocks (change mermaid-preview):
/// `(source, width, ascii, zoom) -> lines`; `None` shows the source as code.
/// Zoom is a step from normal (-1 denser, 0, 1 roomier), shared by the document.
pub type MermaidRenderer = fn(&str, usize, bool, i8) -> Option<Vec<Line<'static>>>;

/// Layout options.
#[derive(Clone, Default)]
pub struct LayoutOptions {
    pub width: usize,
    /// ASCII set of markers and frames instead of ☐ • │ ┌.
    pub ascii: bool,
    /// Reserve space for local images (otherwise a caption).
    pub images: bool,
    /// Document folder: relative image and link paths are resolved against it.
    pub base_dir: Option<PathBuf>,
    pub mermaid: Option<MermaidRenderer>,
    /// Zoom of mermaid blocks (step from normal).
    pub mermaid_zoom: i8,
}

/// Heading in the layout.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadingRef {
    pub line: usize,
    pub level: u8,
    pub slug: String,
}

/// Link in the layout (view line where the text starts).
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRef {
    pub line: usize,
    pub text: String,
    pub url: String,
}

/// Image slot: `rows` empty lines starting at `line`, column `col`, width `cols`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImagePlace {
    pub line: usize,
    pub col: usize,
    pub cols: usize,
    pub rows: usize,
    pub path: PathBuf,
}

/// Layout result.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub doc: ViewDoc,
    pub headings: Vec<HeadingRef>,
    pub links: Vec<LinkRef>,
    pub images: Vec<ImagePlace>,
}

impl Layout {
    /// Heading by anchor (`#slug`).
    pub fn heading_by_slug(&self, s: &str) -> Option<&HeadingRef> {
        self.headings.iter().find(|h| h.slug == s)
    }
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

/// Styled text fragment; `link` is an index into `links`.
struct Seg {
    text: String,
    style: Style,
    link: Option<usize>,
}

struct Ctx<'a> {
    o: &'a LayoutOptions,
    lines: Vec<Line<'static>>,
    /// (source line from 1, view line) for each block.
    marks: Vec<(usize, usize)>,
    headings: Vec<HeadingRef>,
    /// Heading slugs without a suffix, used for the `-1`, `-2` of duplicates.
    bases: Vec<String>,
    links: Vec<LinkRef>,
    images: Vec<ImagePlace>,
}

type Prefix = Vec<Span<'static>>;

fn prefix_width(p: &Prefix) -> usize {
    p.iter().map(|s| str_width(&s.content)).sum()
}

fn spaces(n: usize) -> Span<'static> {
    Span::raw(" ".repeat(n))
}

impl Ctx<'_> {
    fn sym<'s>(&self, uni: &'s str, ascii: &'s str) -> &'s str {
        if self.o.ascii {
            ascii
        } else {
            uni
        }
    }

    fn push(&mut self, prefix: &Prefix, mut spans: Vec<Span<'static>>) {
        let mut v = prefix.clone();
        v.append(&mut spans);
        self.lines.push(Line::from(v));
    }

    fn flatten(&mut self, inl: &[Inline], style: Style, link: Option<usize>, out: &mut Vec<Seg>) {
        let t = |s: &str| visible_line(s, DEFAULT_TAB_WIDTH);
        for i in inl {
            match i {
                Inline::Text(s) => out.push(Seg { text: t(s), style, link }),
                Inline::Code(s) => out.push(Seg {
                    text: t(s),
                    style: style.fg(Color::Yellow),
                    link,
                }),
                Inline::Emph(v) => self.flatten(v, style.add_modifier(Modifier::ITALIC), link, out),
                Inline::Strong(v) => self.flatten(v, style.add_modifier(Modifier::BOLD), link, out),
                Inline::Strike(v) => self.flatten(v, style.add_modifier(Modifier::CROSSED_OUT), link, out),
                Inline::Link { text, url } => {
                    let text_s = plain_text(text);
                    let idx = self.links.len();
                    self.links.push(LinkRef { line: usize::MAX, text: t(&text_s), url: url.clone() });
                    let ls = style.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
                    self.flatten(text, ls, Some(idx), out);
                    if text_s != *url {
                        out.push(Seg { text: format!(" ‹{}›", t(url)), style: dim(), link: Some(idx) });
                    }
                }
                Inline::SoftBreak => out.push(Seg { text: " ".into(), style, link }),
                Inline::HardBreak => out.push(Seg { text: "\n".into(), style, link }),
                Inline::Image { alt, url } => out.push(Seg {
                    text: format!("{} {} ({})", self.sym("🖼", "[img]"), t(alt), t(url)),
                    style: dim(),
                    link,
                }),
            }
        }
    }

    /// Word wrap: the first line gets `pf`, continuations get `pc` (of equal width).
    #[allow(unused_assignments)] // the flush! macro resets the state
    fn wrap(&mut self, segs: Vec<Seg>, pf: &Prefix, pc: &Prefix) {
        let avail = self.o.width.saturating_sub(prefix_width(pc)).max(1);
        let mut cur: Vec<Span<'static>> = Vec::new();
        let mut w = 0usize;
        let mut pending: Option<Style> = None;
        let mut first = true;
        let mut linked: Vec<bool> = vec![false; self.links.len()];
        macro_rules! flush {
            () => {{
                let p = if first { pf } else { pc };
                self.push(p, std::mem::take(&mut cur));
                first = false;
                w = 0;
                pending = None;
            }};
        }
        for seg in segs {
            if seg.text == "\n" {
                flush!();
                continue;
            }
            // Tokens: words and spaces
            let mut word = String::new();
            let mut tokens: Vec<(bool, String)> = Vec::new();
            for c in seg.text.chars() {
                if c == ' ' {
                    if !word.is_empty() {
                        tokens.push((false, std::mem::take(&mut word)));
                    }
                    tokens.push((true, String::new()));
                } else {
                    word.push(c);
                }
            }
            if !word.is_empty() {
                tokens.push((false, word));
            }
            for (is_space, mut word) in tokens {
                if is_space {
                    if w > 0 {
                        pending = Some(seg.style);
                    }
                    continue;
                }
                let mut ww = str_width(&word);
                let sp = usize::from(pending.is_some());
                if w > 0 && w + sp + ww > avail {
                    flush!();
                } else if let Some(st) = pending.take() {
                    cur.push(Span::styled(" ", st));
                    w += 1;
                }
                // Word longer than the width: split it
                while w + ww > avail {
                    let mut cut = String::new();
                    let mut cw = 0;
                    let mut rest = word.chars().peekable();
                    while let Some(&c) = rest.peek() {
                        let x = char_width(c);
                        if w + cw + x > avail && (w + cw) > 0 {
                            break;
                        }
                        cut.push(c);
                        cw += x;
                        rest.next();
                    }
                    let tail: String = rest.collect();
                    if let Some(l) = seg.link {
                        mark_link(&mut self.links, &mut linked, l, self.lines.len());
                    }
                    cur.push(Span::styled(cut, seg.style));
                    flush!();
                    word = tail;
                    ww = str_width(&word);
                }
                if !word.is_empty() {
                    if let Some(l) = seg.link {
                        mark_link(&mut self.links, &mut linked, l, self.lines.len());
                    }
                    cur.push(Span::styled(word, seg.style));
                    w += ww;
                }
            }
        }
        if !cur.is_empty() || first {
            flush!();
        }
    }

    fn blocks(&mut self, blocks: &[Block], pf: &Prefix, pc: &Prefix, loose: bool, depth: usize) {
        for (k, b) in blocks.iter().enumerate() {
            if k > 0 && loose {
                self.push(pc, Vec::new());
            }
            let p = if k == 0 { pf } else { pc };
            self.block(b, p, pc, depth);
        }
    }

    fn block(&mut self, b: &Block, pf: &Prefix, pc: &Prefix, depth: usize) {
        self.marks.push((b.src_line, self.lines.len()));
        let inner = self.o.width.saturating_sub(prefix_width(pc)).max(1);
        match &b.kind {
            BlockKind::Heading { level, inlines } => {
                let s = slug(&plain_text(inlines));
                let n = self.bases.iter().filter(|b| **b == s).count();
                self.bases.push(s.clone());
                let s = if n == 0 { s } else { format!("{s}-{n}") };
                self.headings.push(HeadingRef { line: self.lines.len(), level: *level, slug: s });
                let mut segs = Vec::new();
                if *level >= 3 {
                    segs.push(Seg { text: format!("{} ", "#".repeat(*level as usize)), style: dim(), link: None });
                }
                self.flatten(inlines, Style::new().add_modifier(Modifier::BOLD), None, &mut segs);
                let start = self.lines.len();
                self.wrap(segs, pf, pc);
                if *level <= 2 {
                    let w = self.lines[start..].iter().map(|l| l.width()).max().unwrap_or(0);
                    let w = w.saturating_sub(prefix_width(pc)).clamp(1, inner);
                    let ch = match (*level, self.o.ascii) {
                        (1, false) => "═",
                        (_, false) => "─",
                        (1, true) => "=",
                        _ => "-",
                    };
                    self.push(pc, vec![Span::styled(ch.repeat(w), dim())]);
                }
            }
            BlockKind::Paragraph(inl) => {
                let mut segs = Vec::new();
                self.flatten(inl, Style::new(), None, &mut segs);
                self.wrap(segs, pf, pc);
            }
            BlockKind::List { ordered, start, items } => self.list(*ordered, *start, items, pf, pc, depth),
            BlockKind::Quote(blocks) => {
                let bar = Span::styled(self.sym("│ ", "| ").to_owned(), dim());
                let mut qf = pf.clone();
                qf.push(bar.clone());
                let mut qc = pc.clone();
                qc.push(bar);
                self.blocks(blocks, &qf, &qc, true, depth);
            }
            BlockKind::Code { lang, text } => self.code(lang, text, pf, pc),
            BlockKind::Table { align, header, rows } => self.table(align, header, rows, pf, pc),
            BlockKind::Rule => {
                let ch = self.sym("─", "-");
                self.push(pf, vec![Span::styled(ch.repeat(inner), dim())]);
            }
            BlockKind::Html(text) => {
                for (k, l) in text.lines().enumerate() {
                    let p = if k == 0 { pf } else { pc };
                    self.push(p, vec![Span::styled(visible_line(l, DEFAULT_TAB_WIDTH), dim())]);
                }
            }
            BlockKind::Image { alt, url } => self.image(alt, url, pf, pc, inner),
            BlockKind::Footnote { label, blocks } => {
                let m = format!("[^{}]: ", visible_line(label, DEFAULT_TAB_WIDTH));
                let mut ff = pf.clone();
                ff.push(Span::styled(m.clone(), dim()));
                let mut fc = pc.clone();
                fc.push(spaces(str_width(&m)));
                self.blocks(blocks, &ff, &fc, false, depth);
            }
        }
    }

    fn list(&mut self, ordered: bool, start: u64, items: &[Item], pf: &Prefix, pc: &Prefix, depth: usize) {
        let bullets: [&str; 3] = if self.o.ascii { ["*", "-", "+"] } else { ["•", "◦", "▪"] };
        let num_w = (start + items.len() as u64).to_string().len();
        for (k, it) in items.iter().enumerate() {
            let mut m = if ordered {
                format!("{:>w$}. ", start + k as u64, w = num_w)
            } else {
                format!("{} ", bullets[depth % 3])
            };
            match it.task {
                Some(true) => m.push_str(self.sym("☑ ", "[x] ")),
                Some(false) => m.push_str(self.sym("☐ ", "[ ] ")),
                None => {}
            }
            let p0 = if k == 0 { pf } else { pc };
            let mut lf = p0.clone();
            lf.push(Span::styled(m.clone(), Style::new().fg(Color::Blue)));
            let mut lc = pc.clone();
            lc.push(spaces(str_width(&m)));
            if it.blocks.is_empty() {
                self.push(&lf, Vec::new());
            }
            self.blocks(&it.blocks, &lf, &lc, false, depth + 1);
        }
    }

    fn code(&mut self, lang: &str, text: &str, pf: &Prefix, pc: &Prefix) {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        if lang.eq_ignore_ascii_case("mermaid") {
            if let Some(render) = self.o.mermaid {
                let inner = self.o.width.saturating_sub(prefix_width(pc));
                if let Some(out) = render(text, inner, self.o.ascii, self.o.mermaid_zoom) {
                    for (k, l) in out.into_iter().enumerate() {
                        self.push(if k == 0 { pf } else { pc }, l.spans);
                    }
                    return;
                }
            }
        }
        let mut hl = if lang.is_empty() {
            Highlighter::plain(DEFAULT_TAB_WIDTH)
        } else {
            Highlighter::for_token(lang, DEFAULT_TAB_WIDTH)
        };
        let (top, bar, bot) = if self.o.ascii { ("+- ", "| ", "+-") } else { ("╭─ ", "│ ", "╰─") };
        let title = visible_line(lang, DEFAULT_TAB_WIDTH);
        self.push(pf, vec![Span::styled(format!("{top}{title}"), dim())]);
        for l in hl.highlight_all(&lines) {
            let mut v = vec![Span::styled(bar.to_owned(), dim())];
            v.extend(l.spans);
            self.push(pc, v);
        }
        self.push(pc, vec![Span::styled(bot.to_owned(), dim())]);
    }

    fn table(&mut self, align: &[Align], header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>], pf: &Prefix, pc: &Prefix) {
        let ncol = rows.iter().map(Vec::len).chain([header.len(), align.len()]).max().unwrap_or(0);
        if ncol == 0 {
            return;
        }
        let cells = |ctx: &mut Self, row: &[Vec<Inline>], style: Style| -> Vec<Vec<Seg>> {
            (0..ncol)
                .map(|c| {
                    let mut segs = Vec::new();
                    if let Some(inl) = row.get(c) {
                        ctx.flatten(inl, style, None, &mut segs);
                    }
                    for s in &mut segs {
                        if s.text == "\n" {
                            s.text = " ".into();
                        }
                    }
                    segs
                })
                .collect()
        };
        let head = cells(self, header, Style::new().add_modifier(Modifier::BOLD));
        let body: Vec<Vec<Vec<Seg>>> = rows.iter().map(|r| cells(self, r, Style::new())).collect();
        let cw = |c: &Vec<Seg>| c.iter().map(|s| str_width(&s.text)).sum::<usize>();
        let mut widths = vec![1usize; ncol];
        for r in std::iter::once(&head).chain(body.iter()) {
            for (c, cell) in r.iter().enumerate() {
                widths[c] = widths[c].max(cw(cell));
            }
        }
        let g: [&str; 11] = if self.o.ascii {
            ["+", "+", "+", "+", "+", "+", "+", "+", "+", "-", "|"]
        } else {
            ["┌", "┬", "┐", "├", "┼", "┤", "└", "┴", "┘", "─", "│"]
        };
        let rule = |l: &str, m: &str, r: &str| {
            let mid: Vec<String> = widths.iter().map(|w| g[9].repeat(w + 2)).collect();
            Span::styled(format!("{l}{}{r}", mid.join(m)), dim())
        };
        let row_line = |r: Vec<Vec<Seg>>| {
            let mut v = vec![Span::styled(g[10].to_owned(), dim())];
            for (c, cell) in r.into_iter().enumerate() {
                let pad = widths[c] - cw(&cell);
                let (l, rt) = match align.get(c).copied().unwrap_or(Align::None) {
                    Align::Right => (pad, 0),
                    Align::Center => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                v.push(spaces(l + 1));
                v.extend(cell.into_iter().map(|s| Span::styled(s.text, s.style)));
                v.push(spaces(rt + 1));
                v.push(Span::styled(g[10].to_owned(), dim()));
            }
            v
        };
        self.push(pf, vec![rule(g[0], g[1], g[2])]);
        self.push(pc, row_line(head));
        self.push(pc, vec![rule(g[3], g[4], g[5])]);
        for r in body {
            self.push(pc, row_line(r));
        }
        self.push(pc, vec![rule(g[6], g[7], g[8])]);
    }

    fn image(&mut self, alt: &str, url: &str, pf: &Prefix, pc: &Prefix, inner: usize) {
        let src = image::resolve(url, self.o.base_dir.as_deref());
        let t = |s: &str| visible_line(s, DEFAULT_TAB_WIDTH);
        let icon = self.sym("🖼", "[img]");
        let caption = format!("{icon} {} ({})", t(alt), t(url));
        let note = match &src {
            ImageSource::Remote => Some(I18N.t("remote image, not loaded")),
            ImageSource::Local(p) if self.o.images => match image::dimensions(p) {
                Some((w, h)) => {
                    let (cols, rows) = image::cells(w, h, inner);
                    self.push(pf, vec![Span::styled(caption, dim())]);
                    let line = self.lines.len();
                    for _ in 0..rows {
                        self.push(pc, Vec::new());
                    }
                    self.images.push(ImagePlace { line, col: prefix_width(pc), cols, rows, path: p.clone() });
                    return;
                }
                None => Some(I18N.t("not found or format not recognized")),
            },
            _ => None,
        };
        let mut text = caption;
        if let Some(n) = note {
            text.push_str(&I18N.f(" — {note}", &[("note", &n)]));
        }
        self.push(pf, vec![Span::styled(text, dim())]);
    }
}

fn mark_link(links: &mut [LinkRef], linked: &mut Vec<bool>, idx: usize, line: usize) {
    if linked.len() <= idx {
        linked.resize(idx + 1, false);
    }
    if !linked[idx] {
        linked[idx] = true;
        if let Some(l) = links.get_mut(idx) {
            l.line = line;
        }
    }
}

/// Document layout by width.
pub fn layout(doc: &Doc, o: &LayoutOptions) -> Layout {
    let mut ctx = Ctx {
        o,
        lines: Vec::new(),
        marks: Vec::new(),
        headings: Vec::new(),
        bases: Vec::new(),
        links: Vec::new(),
        images: Vec::new(),
    };
    ctx.blocks(&doc.blocks, &Vec::new(), &Vec::new(), true, 0);
    // Anchors: source line i (0-based) → view line of the last block with src_line ≤ i + 1
    let mut anchors = Vec::with_capacity(doc.src_lines);
    let mut marks = ctx.marks.clone();
    marks.sort_by_key(|m| m.0);
    let mut k = 0;
    let mut cur = 0;
    for i in 0..doc.src_lines.max(1) {
        while k < marks.len() && marks[k].0 <= i + 1 {
            cur = cur.max(marks[k].1);
            k += 1;
        }
        anchors.push(cur);
    }
    let mut links = ctx.links;
    for l in &mut links {
        if l.line == usize::MAX {
            l.line = 0;
        }
    }
    Layout {
        doc: ViewDoc::with_anchors(ctx.lines, anchors),
        headings: ctx.headings,
        links,
        images: ctx.images,
    }
}

/// Link target path relative to the document: `None` for an external link (scheme) or an anchor.
pub fn local_target(url: &str, base_dir: Option<&Path>) -> Option<(PathBuf, Option<String>)> {
    if url.starts_with('#') || image::has_scheme(url) {
        return None;
    }
    let (path, frag) = match url.split_once('#') {
        Some((p, f)) => (p, Some(f.to_owned())),
        None => (url, None),
    };
    let p = Path::new(path);
    let full = match base_dir {
        Some(b) if p.is_relative() => b.join(p),
        _ => p.to_path_buf(),
    };
    Some((full, frag))
}
