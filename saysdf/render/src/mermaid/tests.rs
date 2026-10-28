use super::canvas::{Canvas, Ink, Stroke};
use super::flowchart::{self, Dir, Head, LineKind, Shape};
use super::graph::{self, Rect};
use super::sequence::{self, Event};
use super::*;

fn params(z: Zoom) -> LayoutParams {
    LayoutParams::from(z)
}

fn ascii_strings(src: &str, zoom: Zoom) -> Vec<String> {
    let fc = flowchart::parse(src).unwrap();
    let p = params(zoom);
    let lay = graph::layout(&fc, &p);
    graph::draw(&fc, &lay, &p, true).canvas.to_strings()
}

// --- 1. canvas

#[test]
fn canvas_junctions() {
    let mut c = Canvas::new(false);
    c.hline(0, 4, 2, Stroke::Thin, Ink::Border);
    c.vline(2, 0, 4, Stroke::Thin, Ink::Border);
    assert_eq!(c.char_at(2, 2), '┼');
    let mut c = Canvas::new(false);
    c.rect(0, 0, 3, 3, Stroke::Thin, false, Ink::Border);
    assert_eq!(c.char_at(0, 0), '┌');
    assert_eq!(c.char_at(2, 2), '┘');
    let mut c = Canvas::new(true);
    c.hline(0, 4, 2, Stroke::Thin, Ink::Border);
    c.vline(2, 0, 4, Stroke::Thin, Ink::Border);
    assert_eq!(c.char_at(2, 2), '+');
    assert_eq!(c.char_at(0, 2), '-');
    assert_eq!(c.char_at(2, 0), '|');
}

#[test]
fn canvas_wide_text_and_escape() {
    let mut c = Canvas::new(false);
    let w = c.text(0, 0, "日本\x1b", Ink::Label);
    assert!(w >= 5, "wide characters take 2 cells, ESC is shown as a glyph: {w}");
    assert!(!c.to_strings()[0].contains('\x1b'));
}

#[test]
fn rect_node_is_three_lines_width_seven() {
    let out = ascii_strings("flowchart TD\n  A[Start]", Zoom::Compact);
    assert_eq!(out, vec!["+-----+", "|Start|", "+-----+"]);
}

// --- 2. flowchart: detect and parser

#[test]
fn detect_kinds() {
    assert_eq!(detect("graph TD\nA-->B"), Kind::Flowchart);
    assert_eq!(detect("flowchart LR"), Kind::Flowchart);
    assert_eq!(detect("%% comment\n\nflowchart LR"), Kind::Flowchart);
    assert_eq!(detect("---\ntitle: x\n---\nsequenceDiagram"), Kind::Sequence);
    assert_eq!(detect("gantt\n  title x"), Kind::Unsupported("gantt".into()));
    assert_eq!(detect("  \n"), Kind::Empty);
}

#[test]
fn parse_shapes_and_edges() {
    let fc = flowchart::parse("graph TD\n  A[Start] --> B{Ok?}").unwrap();
    assert_eq!(fc.nodes.len(), 2);
    assert_eq!(fc.nodes[1].shape, Shape::Diamond);
    assert_eq!(fc.nodes[1].label, "Ok?");
    assert_eq!(fc.edges[0].head, Head::Arrow);

    let fc = flowchart::parse("flowchart LR\n A & B --> C").unwrap();
    assert_eq!(fc.dir, Dir::LR);
    assert_eq!(fc.edges.len(), 2);

    let fc = flowchart::parse("graph TD\nA --> B --> C").unwrap();
    assert_eq!(fc.edges.len(), 2);
    assert_eq!((fc.edges[1].from, fc.edges[1].to), (1, 2));

    let fc = flowchart::parse(
        "graph LR\n a((circle)) -- yes --> b([stadium]); b -.-> c[(db)]\n c ==>|big| d{{hex}}\n d --- e>asym]\n e --x f[\"q [x]\"]\n f <--> g",
    )
    .unwrap();
    let shapes: Vec<Shape> = fc.nodes.iter().map(|n| n.shape).collect();
    assert_eq!(
        &shapes[..6],
        &[Shape::Circle, Shape::Stadium, Shape::Cylinder, Shape::Hexagon, Shape::Asym, Shape::Rect]
    );
    assert_eq!(fc.nodes[5].label, "q [x]");
    assert_eq!(fc.edges[0].label.as_deref(), Some("yes"));
    assert_eq!(fc.edges[1].line, LineKind::Dotted);
    assert_eq!(fc.edges[2].line, LineKind::Thick);
    assert_eq!(fc.edges[2].label.as_deref(), Some("big"));
    assert_eq!(fc.edges[3].head, Head::None);
    assert_eq!(fc.edges[4].head, Head::Cross);
    assert_eq!((fc.edges[5].tail, fc.edges[5].head), (Head::Arrow, Head::Arrow));
}

#[test]
fn parse_subgraphs_skips_and_errors() {
    let src = "flowchart TD\n  subgraph one [First]\n    A --> B\n    subgraph two\n      C\n    end\n  end\n  style A fill:#f00\n  classDef x fill:#0f0\n  click A call\n  B --> C";
    let fc = flowchart::parse(src).unwrap();
    assert_eq!(fc.subgraphs.len(), 2);
    assert_eq!(fc.subgraphs[0].title, "First");
    assert_eq!(fc.nodes[0].subgraph, Some(0));
    assert_eq!(fc.nodes[2].subgraph, Some(1));
    assert_eq!(fc.subgraphs[1].parent, Some(0));

    let e = flowchart::parse("graph TD\n  A --> B\n  this is ### junk\n").unwrap_err();
    assert_eq!(e.line, 3);
    let e = flowchart::parse("graph TD\n  subgraph s\n  A").unwrap_err();
    assert_eq!(e.line, 2);
    let e = flowchart::parse("graph TD\n  A -->").unwrap_err();
    assert_eq!(e.line, 2);
}

#[test]
fn edge_to_subgraph_goes_to_its_first_node() {
    let fc = flowchart::parse("graph TD\n X --> s1\n subgraph s1\n  A --> B\n end").unwrap();
    assert!(fc.nodes.iter().all(|n| n.id != "s1"));
    let a = fc.nodes.iter().position(|n| n.id == "A").unwrap();
    assert_eq!(fc.edges[0].to, a);
}

// --- 2. layout

#[test]
fn layers_chain_cycle_and_dummies() {
    let fc = flowchart::parse("graph TD\nA-->B-->C").unwrap();
    let lay = graph::layout(&fc, &params(Zoom::Normal));
    assert_eq!(lay.layer, vec![0, 1, 2]);

    let fc = flowchart::parse("graph TD\nA-->B\nB-->A").unwrap();
    let lay = graph::layout(&fc, &params(Zoom::Normal));
    assert_eq!(lay.reversed.iter().filter(|&&r| r).count(), 1);
    assert_eq!(lay.layer, vec![0, 1]);

    let fc = flowchart::parse("graph TD\nA-->B-->C\nA-->C").unwrap();
    let lay = graph::layout(&fc, &params(Zoom::Normal));
    assert_eq!(lay.dummies, 1);
}

fn assert_no_overlap(boxes: &[Rect]) {
    for i in 0..boxes.len() {
        for j in i + 1..boxes.len() {
            assert!(!boxes[i].overlaps(&boxes[j]), "nodes {i} and {j} overlap: {:?} {:?}", boxes[i], boxes[j]);
        }
    }
}

#[test]
fn diamond_graph_no_overlap_and_d_below_a() {
    for z in [Zoom::Compact, Zoom::Normal, Zoom::Spacious] {
        let fc = flowchart::parse("graph TD\nA-->B\nA-->C\nB-->D\nC-->D").unwrap();
        let lay = graph::layout(&fc, &params(z));
        assert_no_overlap(&lay.boxes);
        let (a, d) = (lay.boxes[0], lay.boxes[3]);
        assert!(d.y > a.y);
        assert!((d.cx() - a.cx()).abs() <= 2, "D under A: {a:?} {d:?}");
        assert!(lay.boxes[1].y > a.y && lay.boxes[1].y == lay.boxes[2].y);
    }
}

#[test]
fn lr_x_grows_by_layer() {
    let fc = flowchart::parse("flowchart LR\nA-->B-->C").unwrap();
    let lay = graph::layout(&fc, &params(Zoom::Normal));
    let b = &lay.boxes;
    assert!(b[0].x + b[0].w <= b[1].x && b[1].x + b[1].w <= b[2].x);
    assert_eq!(b[0].y, b[1].y);
    let fc = flowchart::parse("flowchart BT\nA-->B").unwrap();
    let lay = graph::layout(&fc, &params(Zoom::Normal));
    assert!(lay.boxes[1].y < lay.boxes[0].y);
}

#[test]
fn barycenter_reduces_crossing() {
    // A→D, B→C: after the barycentre C need not be left of D, but the order must follow the parents
    let fc = flowchart::parse("graph TD\nA-->D\nB-->C").unwrap();
    let lay = graph::layout(&fc, &params(Zoom::Normal));
    let (d, c) = (lay.boxes[1], lay.boxes[3]);
    assert!(d.x < c.x, "D under A (left), C under B: {d:?} {c:?}");
    assert_no_overlap(&lay.boxes);
}

#[test]
fn bigger_graph_no_overlap() {
    let src = "graph TD\n a-->b & c & d\n b-->e\n c-->e & f\n d-->f & g\n e-->h\n f-->h\n g-->h\n a-->h\n h-->a";
    for dir in ["TD", "LR"] {
        let s = src.replace("TD", dir);
        let fc = flowchart::parse(&s).unwrap();
        let lay = graph::layout(&fc, &params(Zoom::Compact));
        assert_no_overlap(&lay.boxes);
    }
}

// --- routing and drawing

#[test]
fn edge_with_label_has_line_and_label_outside_boxes() {
    let src = "graph TD\nA[Start] -->|yes| B[End]";
    let fc = flowchart::parse(src).unwrap();
    let p = params(Zoom::Normal);
    let lay = graph::layout(&fc, &p);
    let d = graph::draw(&fc, &lay, &p, false);
    let out = d.canvas.to_strings();
    let text = out.join("\n");
    assert!(text.contains("yes"), "{text}");
    assert!(text.contains('▼'), "{text}");
    let (a, b) = (lay.boxes[0], lay.boxes[1]);
    let (ly, row) = out.iter().enumerate().find(|(_, l)| l.contains("yes")).unwrap();
    let lx = row.chars().take_while(|_| true).collect::<String>().find("yes").unwrap() as i64;
    let lbl = Rect { x: lx, y: ly as i64, w: 3, h: 1 };
    assert!(!lbl.overlaps(&a) && !lbl.overlaps(&b), "{text}");
    // there is a vertical line between the nodes
    let x = a.cx() as usize;
    assert!((a.y + a.h) < b.y);
    assert_eq!(out[(a.y + a.h) as usize].chars().nth(x), Some('│'), "{text}");
}

#[test]
fn ascii_golden_small_td() {
    let out = ascii_strings("graph TD\nA[Start] --> B[End]", Zoom::Compact);
    assert_eq!(out, vec!["+-----+", "|Start|", "+--+--+", "   |", "   v", " +---+", " |End|", " +---+"]);
}

#[test]
fn ascii_has_no_box_drawing() {
    let out = ascii_strings("graph LR\nA-->B & C\nB-.->D\nC==>D\nD-->A", Zoom::Normal);
    for l in &out {
        assert!(l.is_ascii(), "non-ASCII in ASCII mode: {l}");
    }
}

#[test]
fn compact_narrower_than_normal() {
    let src = "graph TD\nA[Alpha]-->B[Beta]\nA-->C[Gamma]\nB-->D[Delta]\nC-->D";
    let w = |z| render(src, &Opts { zoom: z, ascii: false }).unwrap().width;
    assert!(w(Zoom::Compact) < w(Zoom::Normal));
    assert!(w(Zoom::Normal) < w(Zoom::Spacious));
}

#[test]
fn long_labels_truncated_in_compact() {
    let r = render("graph TD\nA[a very long label that goes on and on]", &Opts { zoom: Zoom::Compact, ascii: false }).unwrap();
    assert!(r.lines.iter().any(|l| l.to_string().contains('…')));
}

#[test]
fn anchors_for_every_node() {
    let r = render("graph TD\nA-->B\nA-->C", &Opts::default()).unwrap();
    let ids: Vec<&str> = r.anchors.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, vec!["A", "B", "C"]);
}

#[test]
fn too_many_nodes_fallback() {
    let mut src = String::from("graph TD\n");
    for i in 0..=MAX_NODES {
        src.push_str(&format!("n{i}\n"));
    }
    assert_eq!(render(&src, &Opts::default()).unwrap_err(), Fallback::TooLarge(MAX_NODES + 1));
    assert!(matches!(render("pie\n \"a\": 1", &Opts::default()), Err(Fallback::Unsupported(_))));
    let lines = fallback_lines("pie", &Fallback::Unsupported("pie".into()), false);
    assert!(lines[0].to_string().contains("pie"));
}

// --- 3. sequence

#[test]
fn sequence_parse() {
    let sq = sequence::parse("sequenceDiagram\n  Alice->>Bob: Hi").unwrap();
    assert_eq!(sq.participants.len(), 2);
    assert_eq!(sq.messages(), 1);
    let e = sequence::parse("sequenceDiagram\n  A->>B: x\n  loop every\n  B-->>A: y").unwrap_err();
    assert_eq!(e.line, 3);
    let sq = sequence::parse(
        "sequenceDiagram\n autonumber\n participant A as Alice\n actor B\n A-)B: async\n B--xA: lost\n Note over A,B: hello\n alt ok\n A->>+B: go\n else no\n A->B: stop\n end\n activate A",
    )
    .unwrap();
    assert!(sq.autonumber);
    assert_eq!(sq.participants[0].label, "Alice");
    assert!(sq.participants[1].actor);
    assert_eq!(sq.messages(), 4);
    assert!(sq.events.iter().any(|e| matches!(e, Event::BlockElse { .. })));
}

#[test]
fn sequence_layout_gaps_and_rows() {
    let src = "sequenceDiagram\n A->>B: a rather long message text\n B-->>A: ok";
    let sq = sequence::parse(src).unwrap();
    let p = params(Zoom::Normal);
    let lay = sequence::layout(&sq, &p);
    assert!(lay.cols[1] - lay.cols[0] >= "a rather long message text".len());
    assert_eq!(lay.height, 6 + 2 * 2);
    let r = render(src, &Opts::default()).unwrap();
    assert_eq!(r.lines.len(), 6 + 2 * 2);
    let text: String = r.lines.iter().map(|l| l.to_string() + "\n").collect();
    assert!(text.contains('▶') && text.contains('◀'), "{text}");
}

#[test]
fn sequence_ascii_and_blocks_render() {
    let src = "sequenceDiagram\n participant A\n participant B\n loop retry\n A->>A: self\n Note right of B: n\n end";
    let r = render(src, &Opts { zoom: Zoom::Compact, ascii: true }).unwrap();
    let text: String = r.lines.iter().map(|l| l.to_string() + "\n").collect();
    assert!(text.is_ascii(), "{text}");
    assert!(text.contains("loop retry"), "{text}");
}

// --- 4.4 pathological inputs

#[test]
fn pathological_inputs_do_not_panic() {
    let mut chain = String::from("graph TD\n");
    for i in 0..1000 {
        chain.push_str(&format!("n{i}-->n{}\n", i + 1));
    }
    let mut deep = String::from("graph TD\n");
    for i in 0..20 {
        deep.push_str(&format!("subgraph s{i}\n"));
    }
    deep.push_str("A\n");
    for _ in 0..20 {
        deep.push_str("end\n");
    }
    let mut nested = String::from("graph TD\n");
    for i in 0..10 {
        nested.push_str(&format!("subgraph s{i}\nN{i}-->M{i}\n"));
    }
    for _ in 0..10 {
        nested.push_str("end\n");
    }
    let inputs = [
        "".to_owned(),
        "graph".to_owned(),
        "graph TD".to_owned(),
        "flowchart LR;".to_owned(),
        "sequenceDiagram".to_owned(),
        "graph TD\nA[\x1b[31mred]-->B".to_owned(),
        "graph TD\nA-->A".to_owned(),
        "graph TD\nA[\"unclosed".to_owned(),
        "graph TD\nA-- text".to_owned(),
        "graph TD\n&&&".to_owned(),
        "sequenceDiagram\nA->>A: \x07bell".to_owned(),
        "sequenceDiagram\nend".to_owned(),
        chain,
        deep,
        nested,
    ];
    for src in &inputs {
        for z in [Zoom::Compact, Zoom::Normal, Zoom::Spacious] {
            for ascii in [false, true] {
                let lines = markdown_renderer(src, 80, ascii, z.step()).unwrap();
                for l in &lines {
                    assert!(!l.to_string().contains('\x1b'));
                }
            }
        }
    }
}

#[test]
fn messages_follow_language() {
    // The process language is global and shared by parallel tests: look in the catalogs directly
    let (ru, _) = saysdf_i18n::load("ru", None);
    assert_eq!(ru.text("render-mermaid", "empty diagram", &[]), "пустая диаграмма"); // i18n-allow-cyrillic
    assert_eq!(
        ru.text("render-mermaid", "{kind} type is not supported yet", &[("kind", "gantt")]),
        "вид `gantt` не поддерживается" // i18n-allow-cyrillic
    );
    let (en, _) = saysdf_i18n::load("en", None);
    assert_eq!(en.text("render-mermaid", "empty diagram", &[]), "empty diagram");
    assert_eq!(
        en.text("render-mermaid", "{kind} type is not supported yet", &[("kind", "gantt")]),
        "gantt type is not supported yet"
    );
}
