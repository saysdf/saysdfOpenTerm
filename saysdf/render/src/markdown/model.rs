//! Markdown document model (change markdown-preview, design D2): blocks with a source line number.
//! The model is width-independent; layout lives in [`super::layout`].

/// Table column alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

/// Inline element. Texts are raw (as in the source); escaping happens at layout time.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Emph(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Link { text: Vec<Inline>, url: String },
    SoftBreak,
    HardBreak,
    Image { alt: String, url: String },
}

/// List item: `task` is `Some(checked)` for task lists.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

/// Block with its source line number (1-based).
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub src_line: usize,
    pub kind: BlockKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    Heading { level: u8, inlines: Vec<Inline> },
    Paragraph(Vec<Inline>),
    List { ordered: bool, start: u64, items: Vec<Item> },
    Quote(Vec<Block>),
    Code { lang: String, text: String },
    Table { align: Vec<Align>, header: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    Rule,
    /// Raw HTML is shown as text, not interpreted.
    Html(String),
    /// Paragraph consisting of a single image.
    Image { alt: String, url: String },
    /// Footnote definition `[^label]: …` (collected at the end of the document).
    Footnote { label: String, blocks: Vec<Block> },
}

/// Parsed document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Doc {
    pub blocks: Vec<Block>,
    /// Number of source lines (for "source line → view line" anchors).
    pub src_lines: usize,
}

/// Flat text of inlines (image alt, heading slugs, cell widths).
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut s = String::new();
    fn walk(i: &[Inline], s: &mut String) {
        for x in i {
            match x {
                Inline::Text(t) | Inline::Code(t) => s.push_str(t),
                Inline::Emph(v) | Inline::Strong(v) | Inline::Strike(v) => walk(v, s),
                Inline::Link { text, .. } => walk(text, s),
                Inline::SoftBreak | Inline::HardBreak => s.push(' '),
                Inline::Image { alt, .. } => s.push_str(alt),
            }
        }
    }
    walk(inlines, &mut s);
    s
}

/// Heading anchor as on GitHub: lower case, letters/digits/`-`/`_`, spaces → `-`, everything else dropped.
pub fn slug(text: &str) -> String {
    text.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}
