use super::image::{self, ImageSource};
use super::*;
use std::path::Path;

fn text(i: &str) -> Inline {
    Inline::Text(i.into())
}

fn opts(width: usize) -> LayoutOptions {
    LayoutOptions { width, ..Default::default() }
}

fn lines(l: &Layout) -> Vec<String> {
    (0..l.doc.len()).map(|i| l.doc.line_text(i)).collect()
}

#[test]
fn heading_and_paragraph_with_strong() {
    let d = parse("# Title\n\nsome **bold** text\n");
    assert_eq!(d.blocks.len(), 2);
    assert_eq!(d.blocks[0].kind, BlockKind::Heading { level: 1, inlines: vec![text("Title")] });
    assert_eq!(
        d.blocks[1].kind,
        BlockKind::Paragraph(vec![text("some "), Inline::Strong(vec![text("bold")]), text(" text")])
    );
    assert_eq!(d.blocks[1].src_line, 3);
}

#[test]
fn src_line_of_block_on_third_line() {
    let d = parse("a\n\n## h\n");
    assert_eq!(d.blocks[1].src_line, 3);
}

#[test]
fn nested_list() {
    let d = parse("- a\n  - b\n- c\n");
    let BlockKind::List { ordered: false, items, .. } = &d.blocks[0].kind else { panic!("{:?}", d.blocks) };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].blocks[0].kind, BlockKind::Paragraph(vec![text("a")]));
    let BlockKind::List { items: sub, .. } = &items[0].blocks[1].kind else { panic!("{:?}", items[0]) };
    assert_eq!(sub[0].blocks[0].kind, BlockKind::Paragraph(vec![text("b")]));
    assert_eq!(items[0].blocks[1].src_line, 2);
}

#[test]
fn ordered_list_and_tasks() {
    let d = parse("3. x\n4. y\n\n- [x] done\n- [ ] todo\n");
    assert!(matches!(d.blocks[0].kind, BlockKind::List { ordered: true, start: 3, .. }));
    let BlockKind::List { items, .. } = &d.blocks[1].kind else { panic!() };
    assert_eq!(items[0].task, Some(true));
    assert_eq!(items[1].task, Some(false));
    assert_eq!(items[0].blocks[0].kind, BlockKind::Paragraph(vec![text("done")]));
}

#[test]
fn table_with_alignment() {
    let d = parse("| a | b |\n|:-:|--:|\n| 1 | 2 |\n");
    let BlockKind::Table { align, header, rows } = &d.blocks[0].kind else { panic!("{:?}", d.blocks) };
    assert_eq!(align, &vec![Align::Center, Align::Right]);
    assert_eq!(header, &vec![vec![text("a")], vec![text("b")]]);
    assert_eq!(rows, &vec![vec![vec![text("1")], vec![text("2")]]]);
}

#[test]
fn code_block_with_lang_and_html() {
    let d = parse("```rust\nfn main() {}\n```\n\n<script>alert(1)</script>\n");
    assert_eq!(d.blocks[0].kind, BlockKind::Code { lang: "rust".into(), text: "fn main() {}".into() });
    assert_eq!(d.blocks[1].kind, BlockKind::Html("<script>alert(1)</script>".into()));
    assert_eq!(d.blocks[1].src_line, 5);
}

#[test]
fn links_images_footnotes() {
    let d = parse("see [docs](a.md) and `x`[^1]\n\n![logo](img/a.png)\n\n[^1]: note\n");
    let BlockKind::Paragraph(inl) = &d.blocks[0].kind else { panic!() };
    assert_eq!(inl[1], Inline::Link { text: vec![text("docs")], url: "a.md".into() });
    assert_eq!(inl[3], Inline::Code("x".into()));
    assert_eq!(inl[4], text("[^1]"));
    assert_eq!(d.blocks[1].kind, BlockKind::Image { alt: "logo".into(), url: "img/a.png".into() });
    assert!(matches!(&d.blocks.last().unwrap().kind, BlockKind::Footnote { label, .. } if label == "1"));
}

#[test]
fn layout_wraps_and_cuts_long_words() {
    let d = parse("aaaa bbbb cccc dddd eeee ffff\n\nxxxxxxxxxxxxxxxxxxxxxxxxx\n");
    let l = layout(&d, &opts(10));
    let t = lines(&l);
    assert!(t.iter().all(|s| s.chars().count() <= 10), "{t:?}");
    assert!(t.len() >= 3 + 1 + 3, "{t:?}");
    assert_eq!(t[0], "aaaa bbbb");
}

#[test]
fn layout_nested_list_continuation_indent() {
    let d = parse("- one two three four\n  - sub\n");
    let t = lines(&layout(&d, &opts(10)));
    assert_eq!(t[0], "• one two");
    assert!(t[1].starts_with("  "), "{t:?}");
    assert!(t.iter().any(|s| s.starts_with("  ◦ sub")), "{t:?}");
}

#[test]
fn layout_table_widths_and_right_align() {
    let d = parse("| name | n |\n|---|--:|\n| a | 100 |\n");
    let t = lines(&layout(&d, &opts(80)));
    assert_eq!(t[0], "┌──────┬─────┐");
    assert_eq!(t[1], "│ name │   n │");
    assert_eq!(t[3], "│ a    │ 100 │");
}

#[test]
fn layout_links_slugs_and_sanitize() {
    let d = parse("## Hello, World!\n\n[a\u{1b}b](https://x.org/)\n\n## Hello, World!\n");
    let l = layout(&d, &opts(80));
    assert_eq!(l.headings[0].slug, "hello-world");
    assert_eq!(l.headings[1].slug, "hello-world-1");
    assert_eq!(l.links.len(), 1);
    assert_eq!(l.links[0].url, "https://x.org/");
    let t = lines(&l);
    let line = &t[l.links[0].line];
    assert!(!line.contains('\u{1b}') && line.contains("‹https://x.org/›"), "{line:?}");
    assert_eq!(model::slug("Привет, мир"), "привет-мир"); // i18n-allow-cyrillic
}

#[test]
fn layout_code_not_wrapped_and_mermaid_hook() {
    let src = "```mermaid\nflowchart LR\n  A --> B --> C --> D --> E\n```\n";
    let d = parse(src);
    let l = layout(&d, &opts(10));
    assert!(lines(&l).iter().any(|s| s.contains("A --> B --> C --> D --> E")));
    fn hook(_: &str, _: usize, _: bool, _: i8) -> Option<Vec<ratatui::text::Line<'static>>> {
        Some(vec!["DIAGRAM".into()])
    }
    let o = LayoutOptions { width: 10, mermaid: Some(hook), ..Default::default() };
    assert_eq!(lines(&layout(&d, &o)), vec!["DIAGRAM"]);
}

#[test]
fn source_line_anchors() {
    let d = parse("# A\n\npara\n\n- x\n");
    let l = layout(&d, &opts(40));
    // source line 3 (0-based 2) → the paragraph after "A", the underline and the empty line
    assert_eq!(l.doc.line_text(l.doc.view_line(2)), "para");
    assert_eq!(l.doc.line_text(l.doc.view_line(4)), "• x");
}

#[test]
fn remote_images_are_not_loaded() {
    for url in ["https://example.com/a.png", "http://x/a.png", "//cdn/a.png", "data:image/png;base64,AA"] {
        assert_eq!(image::resolve(url, None), ImageSource::Remote, "{url}");
    }
    let d = parse("![logo](https://example.com/a.png)\n");
    let o = LayoutOptions { width: 80, images: true, ..Default::default() };
    let l = layout(&d, &o);
    assert!(l.images.is_empty());
    let t = lines(&l);
    assert!(t[0].contains("https://example.com/a.png") && t[0].contains("remote image, not loaded"), "{t:?}");
}

#[test]
fn local_image_reserves_rows() {
    let dir = tempfile::tempdir().unwrap();
    // PNG header 80×32
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(80u32.to_be_bytes());
    png.extend(32u32.to_be_bytes());
    std::fs::write(dir.path().join("a.png"), &png).unwrap();
    assert_eq!(image::resolve("C:/x.png", None), ImageSource::Local("C:/x.png".into()));
    let d = parse("![a](a.png)\n");
    let o = LayoutOptions { width: 80, images: true, base_dir: Some(dir.path().into()), ..Default::default() };
    let l = layout(&d, &o);
    assert_eq!(l.images.len(), 1);
    assert_eq!((l.images[0].cols, l.images[0].rows), (10, 2));
    assert_eq!(l.doc.len(), 1 + 2);
    let no = layout(&d, &LayoutOptions { images: false, ..o });
    assert!(no.images.is_empty());
    assert!(layout::local_target("https://x", None).is_none());
    assert_eq!(
        layout::local_target("b.md#x", Some(Path::new("/d"))),
        Some((Path::new("/d").join("b.md"), Some("x".into())))
    );
}

#[test]
fn image_notes_follow_language() {
    // The process language is global and shared by parallel tests: look in the catalogs directly
    let (ru, _) = saysdf_i18n::load("ru", None);
    assert_eq!(ru.text("render-markdown", "remote image, not loaded", &[]), "удалённая, не загружается"); // i18n-allow-cyrillic
    let (en, _) = saysdf_i18n::load("en", None);
    assert_eq!(en.text("render-markdown", "remote image, not loaded", &[]), "remote image, not loaded");
}
