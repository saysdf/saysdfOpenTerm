//! pulldown-cmark events → block model (design D2). The line number comes from the byte offset
//! given by `into_offset_iter`.

use super::model::{plain_text, Align, Block, BlockKind, Doc, Inline, Item};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Block container on the parse stack.
enum Cont {
    Root,
    Quote(usize),
    List { line: usize, ordered: bool, start: u64, items: Vec<Item> },
    Item(Option<bool>),
    Footnote(usize, String),
}

/// Inline container on the stack.
enum Inl {
    /// Leaf block: heading, paragraph (implicit inside a "tight" list item), cell.
    Leaf,
    Emph,
    Strong,
    Strike,
    Link(String),
    Image(String),
}

struct Table {
    line: usize,
    align: Vec<Align>,
    header: Vec<Vec<Inline>>,
    rows: Vec<Vec<Vec<Inline>>>,
    row: Vec<Vec<Inline>>,
}

struct P {
    conts: Vec<(Cont, Vec<Block>)>,
    inl: Vec<(Inl, Vec<Inline>)>,
    /// Started leaf block: (line, heading level or 0 for a paragraph, implicit?).
    leaf: Option<(usize, u8, bool)>,
    code: Option<(usize, String, String)>,
    html: Option<(usize, String)>,
    table: Option<Table>,
    footnotes: Vec<Block>,
}

impl P {
    fn push_block(&mut self, b: Block) {
        self.conts.last_mut().expect("root").1.push(b);
    }

    /// Text directly in a "tight" list item is an implicit paragraph.
    fn ensure_leaf(&mut self, line: usize) {
        if self.inl.is_empty() {
            self.leaf = Some((line, 0, true));
            self.inl.push((Inl::Leaf, Vec::new()));
        }
    }

    fn push_inline(&mut self, line: usize, i: Inline) {
        self.ensure_leaf(line);
        self.inl.last_mut().expect("inline").1.push(i);
    }

    fn close_implicit(&mut self) {
        if matches!(self.leaf, Some((_, _, true))) {
            self.end_leaf();
        }
    }

    fn end_leaf(&mut self) {
        let Some((line, level, _)) = self.leaf.take() else { return };
        let inlines = self.inl.pop().map(|x| x.1).unwrap_or_default();
        self.inl.clear();
        let kind = if level > 0 {
            BlockKind::Heading { level, inlines }
        } else if let [Inline::Image { alt, url }] = inlines.as_slice() {
            BlockKind::Image { alt: alt.clone(), url: url.clone() }
        } else {
            BlockKind::Paragraph(inlines)
        };
        self.push_block(Block { src_line: line, kind });
    }
}

fn line_of(starts: &[usize], off: usize) -> usize {
    starts.partition_point(|&s| s <= off).max(1)
}

/// Markdown parsing (CommonMark + GFM: tables, tasks, strikethrough, footnotes).
pub fn parse(src: &str) -> Doc {
    let mut starts = vec![0];
    starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let src_lines = if src.ends_with('\n') { starts.len() - 1 } else { starts.len() };
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let mut p = P {
        conts: vec![(Cont::Root, Vec::new())],
        inl: Vec::new(),
        leaf: None,
        code: None,
        html: None,
        table: None,
        footnotes: Vec::new(),
    };
    for (ev, range) in Parser::new_ext(src, opts).into_offset_iter() {
        let line = line_of(&starts, range.start);
        match ev {
            Event::Start(tag) => {
                let block_tag = matches!(
                    tag,
                    Tag::Paragraph
                        | Tag::Heading { .. }
                        | Tag::BlockQuote(_)
                        | Tag::CodeBlock(_)
                        | Tag::HtmlBlock
                        | Tag::List(_)
                        | Tag::Item
                        | Tag::FootnoteDefinition(_)
                        | Tag::Table(_)
                );
                if block_tag {
                    p.close_implicit();
                }
                match tag {
                    Tag::Paragraph => {
                        p.leaf = Some((line, 0, false));
                        p.inl.push((Inl::Leaf, Vec::new()));
                    }
                    Tag::Heading { level, .. } => {
                        p.leaf = Some((line, level as u8, false));
                        p.inl.push((Inl::Leaf, Vec::new()));
                    }
                    Tag::BlockQuote(_) => p.conts.push((Cont::Quote(line), Vec::new())),
                    Tag::CodeBlock(kind) => {
                        let lang = match kind {
                            CodeBlockKind::Fenced(info) => {
                                info.split_whitespace().next().unwrap_or("").to_owned()
                            }
                            CodeBlockKind::Indented => String::new(),
                        };
                        p.code = Some((line, lang, String::new()));
                    }
                    Tag::HtmlBlock => p.html = Some((line, String::new())),
                    Tag::List(start) => p.conts.push((
                        Cont::List { line, ordered: start.is_some(), start: start.unwrap_or(1), items: Vec::new() },
                        Vec::new(),
                    )),
                    Tag::Item => p.conts.push((Cont::Item(None), Vec::new())),
                    Tag::FootnoteDefinition(label) => {
                        p.conts.push((Cont::Footnote(line, label.to_string()), Vec::new()))
                    }
                    Tag::Table(al) => {
                        p.table = Some(Table {
                            line,
                            align: al
                                .iter()
                                .map(|a| match a {
                                    Alignment::None => Align::None,
                                    Alignment::Left => Align::Left,
                                    Alignment::Center => Align::Center,
                                    Alignment::Right => Align::Right,
                                })
                                .collect(),
                            header: Vec::new(),
                            rows: Vec::new(),
                            row: Vec::new(),
                        })
                    }
                    Tag::TableCell => p.inl.push((Inl::Leaf, Vec::new())),
                    Tag::Emphasis => p.inl.push((Inl::Emph, Vec::new())),
                    Tag::Strong => p.inl.push((Inl::Strong, Vec::new())),
                    Tag::Strikethrough => p.inl.push((Inl::Strike, Vec::new())),
                    Tag::Link { dest_url, .. } => {
                        p.ensure_leaf(line);
                        p.inl.push((Inl::Link(dest_url.to_string()), Vec::new()))
                    }
                    Tag::Image { dest_url, .. } => {
                        p.ensure_leaf(line);
                        p.inl.push((Inl::Image(dest_url.to_string()), Vec::new()))
                    }
                    _ => {}
                }
            }
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) => p.end_leaf(),
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image => {
                    let Some((kind, v)) = p.inl.pop() else { continue };
                    let i = match kind {
                        Inl::Emph => Inline::Emph(v),
                        Inl::Strong => Inline::Strong(v),
                        Inl::Strike => Inline::Strike(v),
                        Inl::Link(url) => Inline::Link { text: v, url },
                        Inl::Image(url) => Inline::Image { alt: plain_text(&v), url },
                        Inl::Leaf => {
                            p.inl.push((Inl::Leaf, v));
                            continue;
                        }
                    };
                    p.push_inline(line, i);
                }
                TagEnd::TableCell => {
                    let v = p.inl.pop().map(|x| x.1).unwrap_or_default();
                    if let Some(t) = &mut p.table {
                        t.row.push(v);
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = &mut p.table {
                        t.header = std::mem::take(&mut t.row);
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = &mut p.table {
                        let r = std::mem::take(&mut t.row);
                        t.rows.push(r);
                    }
                }
                TagEnd::Table => {
                    if let Some(t) = p.table.take() {
                        p.push_block(Block {
                            src_line: t.line,
                            kind: BlockKind::Table { align: t.align, header: t.header, rows: t.rows },
                        });
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some((l, lang, mut text)) = p.code.take() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        p.push_block(Block { src_line: l, kind: BlockKind::Code { lang, text } });
                    }
                }
                TagEnd::HtmlBlock => {
                    if let Some((l, mut text)) = p.html.take() {
                        while text.ends_with('\n') {
                            text.pop();
                        }
                        p.push_block(Block { src_line: l, kind: BlockKind::Html(text) });
                    }
                }
                TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item | TagEnd::FootnoteDefinition => {
                    p.close_implicit();
                    let Some((cont, blocks)) = p.conts.pop() else { continue };
                    match cont {
                        Cont::Quote(l) => p.push_block(Block { src_line: l, kind: BlockKind::Quote(blocks) }),
                        Cont::List { line, ordered, start, items } => p.push_block(Block {
                            src_line: line,
                            kind: BlockKind::List { ordered, start, items },
                        }),
                        Cont::Item(task) => {
                            if let Some((Cont::List { items, .. }, _)) = p.conts.last_mut() {
                                items.push(Item { task, blocks });
                            }
                        }
                        Cont::Footnote(l, label) => {
                            p.footnotes.push(Block { src_line: l, kind: BlockKind::Footnote { label, blocks } })
                        }
                        Cont::Root => p.conts.push((Cont::Root, blocks)),
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, _, code)) = &mut p.code {
                    code.push_str(&t);
                } else if let Some((_, h)) = &mut p.html {
                    h.push_str(&t);
                } else {
                    p.push_inline(line, Inline::Text(t.to_string()));
                }
            }
            Event::Code(t) => p.push_inline(line, Inline::Code(t.to_string())),
            Event::InlineMath(t) | Event::DisplayMath(t) => p.push_inline(line, Inline::Code(t.to_string())),
            Event::Html(t) => {
                if let Some((_, h)) = &mut p.html {
                    h.push_str(&t);
                } else {
                    p.close_implicit();
                    p.push_block(Block { src_line: line, kind: BlockKind::Html(t.trim_end().to_owned()) });
                }
            }
            Event::InlineHtml(t) => p.push_inline(line, Inline::Text(t.to_string())),
            Event::FootnoteReference(l) => p.push_inline(line, Inline::Text(format!("[^{l}]"))),
            Event::SoftBreak => p.push_inline(line, Inline::SoftBreak),
            Event::HardBreak => p.push_inline(line, Inline::HardBreak),
            Event::Rule => {
                p.close_implicit();
                p.push_block(Block { src_line: line, kind: BlockKind::Rule });
            }
            Event::TaskListMarker(done) => {
                if let Some((Cont::Item(task), _)) = p.conts.last_mut() {
                    *task = Some(done);
                }
            }
        }
    }
    p.close_implicit();
    while p.conts.len() > 1 {
        let (_, blocks) = p.conts.pop().expect("cont");
        p.conts[0].1.extend(blocks);
    }
    let mut blocks = p.conts.pop().map(|x| x.1).unwrap_or_default();
    blocks.append(&mut p.footnotes);
    Doc { blocks, src_lines }
}
