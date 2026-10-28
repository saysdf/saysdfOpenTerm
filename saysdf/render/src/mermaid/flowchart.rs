//! Flowchart: the model and a hand-written line-by-line parser (design D3).

use super::ParseError;
use std::collections::HashMap;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("render-mermaid");

/// Layout direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dir {
    #[default]
    TD,
    BT,
    LR,
    RL,
}

/// Node shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Rect,
    Round,
    Stadium,
    Subroutine,
    Cylinder,
    Circle,
    Asym,
    Diamond,
    Hexagon,
    Parallelogram,
}

/// Edge line style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineKind {
    #[default]
    Normal,
    Thick,
    Dotted,
}

/// Arrowhead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Head {
    #[default]
    None,
    Arrow,
    Cross,
    Circle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: Shape,
    pub subgraph: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub line: LineKind,
    /// Arrowhead at `to`.
    pub head: Head,
    /// Arrowhead at `from` (`<-->`).
    pub tail: Head,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subgraph {
    pub id: String,
    pub title: String,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flowchart {
    pub dir: Dir,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub subgraphs: Vec<Subgraph>,
}

/// Maximum subgraph nesting.
pub const MAX_DEPTH: usize = 16;

fn parse_dir(s: &str) -> Option<Dir> {
    match s.to_ascii_uppercase().as_str() {
        "TD" | "TB" => Some(Dir::TD),
        "BT" => Some(Dir::BT),
        "LR" => Some(Dir::LR),
        "RL" => Some(Dir::RL),
        _ => None,
    }
}

/// Split a line into statements at `;` outside quotes and brackets.
fn split_statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut quote, mut start) = (0i32, false, 0);
    for (i, c) in line.char_indices() {
        match c {
            '"' => quote = !quote,
            '[' | '(' | '{' if !quote => depth += 1,
            ']' | ')' | '}' if !quote => depth -= 1,
            ';' if !quote && depth <= 0 => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out
}

/// A parsed edge operator: line, arrowhead at the target, at the source, label.
type EdgeSpec = (LineKind, Head, Head, Option<String>);

struct Builder {
    fc: Flowchart,
    ids: HashMap<String, usize>,
    stack: Vec<usize>,
}

impl Builder {
    fn node(&mut self, id: &str, shape: Option<(Shape, String)>) -> usize {
        let sg = self.stack.last().copied();
        let i = match self.ids.get(id) {
            Some(&i) => i,
            None => {
                self.fc.nodes.push(Node { id: id.to_owned(), label: id.to_owned(), shape: Shape::Rect, subgraph: sg });
                self.ids.insert(id.to_owned(), self.fc.nodes.len() - 1);
                self.fc.nodes.len() - 1
            }
        };
        if self.fc.nodes[i].subgraph.is_none() {
            self.fc.nodes[i].subgraph = sg;
        }
        if let Some((s, l)) = shape {
            self.fc.nodes[i].shape = s;
            self.fc.nodes[i].label = l;
        }
        i
    }
}

/// Parser of a single statement (chains of nodes and edges).
struct Lexer<'a> {
    s: &'a str,
    p: usize,
}

const SHAPES: &[(&str, &str, Shape)] = &[
    ("(((", ")))", Shape::Circle),
    ("((", "))", Shape::Circle),
    ("([", "])", Shape::Stadium),
    ("[[", "]]", Shape::Subroutine),
    ("[(", ")]", Shape::Cylinder),
    ("[/", "]", Shape::Parallelogram),
    ("[\\", "]", Shape::Parallelogram),
    ("(", ")", Shape::Round),
    ("[", "]", Shape::Rect),
    ("{{", "}}", Shape::Hexagon),
    ("{", "}", Shape::Diamond),
    (">", "]", Shape::Asym),
];

fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.p..]
    }

    fn ws(&mut self) {
        let r = self.rest();
        self.p += r.len() - r.trim_start().len();
    }

    fn eof(&mut self) -> bool {
        self.ws();
        self.p >= self.s.len()
    }

    fn id(&mut self) -> Option<&'a str> {
        self.ws();
        let r = self.rest();
        let mut end = 0;
        let cs: Vec<(usize, char)> = r.char_indices().collect();
        for (k, &(i, c)) in cs.iter().enumerate() {
            let next = cs.get(k + 1).map(|x| x.1);
            if is_id_char(c) || (c == '-' && end > 0 && next.is_some_and(is_id_char)) {
                end = i + c.len_utf8();
            } else {
                break;
            }
        }
        if end == 0 {
            return None;
        }
        self.p += end;
        Some(&r[..end])
    }

    /// Shape and label after the id: `[..]`, `(..)`, `{..}`, etc.
    fn shape(&mut self) -> Result<Option<(Shape, String)>, String> {
        let r = self.rest();
        for &(open, close, shape) in SHAPES {
            if let Some(body) = r.strip_prefix(open) {
                let (label, used) = if let Some(q) = body.trim_start().strip_prefix('"') {
                    let lead = body.len() - body.trim_start().len();
                    let end = q.find('"').ok_or_else(|| I18N.t("unclosed quote"))?;
                    let after = &q[end + 1..];
                    let after_t = after.trim_start();
                    if !after_t.starts_with(close) {
                        return Err(I18N.f("expected `{close}`", &[("close", &close)]));
                    }
                    let used = lead + 1 + end + 1 + (after.len() - after_t.len()) + close.len();
                    (q[..end].to_owned(), used)
                } else {
                    let end = body.find(close).ok_or_else(|| I18N.f("expected `{close}`", &[("close", &close)]))?;
                    let mut l = body[..end].trim().to_owned();
                    if shape == Shape::Parallelogram {
                        l = l.trim_end_matches(['/', '\\']).trim().to_owned();
                    }
                    (l, end + close.len())
                };
                self.p += open.len() + used;
                return Ok(Some((shape, label)));
            }
        }
        Ok(None)
    }

    /// `:::class` is skipped.
    fn class_suffix(&mut self) {
        if self.rest().starts_with(":::") {
            self.p += 3;
            let _ = self.id();
        }
    }

    /// Edge: operator, label `-- text -->` or `-->|text|`.
    fn edge(&mut self) -> Result<Option<EdgeSpec>, String> {
        self.ws();
        let start = self.p;
        let r = self.rest();
        let mut tail = Head::None;
        let mut i = 0;
        if r.starts_with('<') {
            tail = Head::Arrow;
            i = 1;
        }
        let run = |s: &str| s.chars().take_while(|c| matches!(c, '-' | '=' | '.')).count();
        let n = run(&r[i..]);
        if n < 2 {
            self.p = start;
            return Ok(None);
        }
        let op = &r[i..i + n];
        let mut j = i + n;
        let head_at = |s: &str| -> Option<Head> {
            let mut it = s.chars();
            match it.next() {
                Some('>') => Some(Head::Arrow),
                Some(c @ ('x' | 'o')) => {
                    let nx = it.next();
                    if nx.is_none_or(|c2| !is_id_char(c2)) {
                        Some(if c == 'x' { Head::Cross } else { Head::Circle })
                    } else {
                        None
                    }
                }
                _ => None,
            }
        };
        let mut head = head_at(&r[j..]);
        if head.is_some() {
            j += 1;
        }
        let kind = |op: &str| {
            if op.contains('=') {
                LineKind::Thick
            } else if op.contains('.') {
                LineKind::Dotted
            } else {
                LineKind::Normal
            }
        };
        let mut line = kind(op);
        let mut label = None;
        // `-- text -->`: a short operator without an arrowhead, then text up to the closing operator
        if head.is_none() && n == 2 && r[j..].starts_with(char::is_whitespace) {
            let body = &r[j..];
            let mut found = None;
            let bytes: Vec<(usize, char)> = body.char_indices().collect();
            for &(k, c) in &bytes {
                if matches!(c, '-' | '=' | '.') && (k == 0 || !matches!(body[..k].chars().last(), Some('-' | '=' | '.'))) {
                    let m = run(&body[k..]);
                    let h = head_at(&body[k + m..]);
                    if m >= 2 && (h.is_some() || m >= 3) {
                        found = Some((k, m, h));
                        break;
                    }
                }
            }
            if let Some((k, m, h)) = found {
                label = Some(body[..k].trim().to_owned());
                line = kind(&body[k..k + m]);
                head = h;
                j += k + m + usize::from(h.is_some());
            }
        }
        self.p = start + j;
        // `-->|text|`
        self.ws();
        if let Some(b) = self.rest().strip_prefix('|') {
            let end = b.find('|').ok_or_else(|| I18N.t("unclosed `|label|`"))?;
            label = Some(b[..end].trim().trim_matches('"').to_owned());
            self.p += end + 2;
        }
        Ok(Some((line, head.unwrap_or(Head::None), tail, label.filter(|l| !l.is_empty()))))
    }
}

fn statement(b: &mut Builder, st: &str) -> Result<(), String> {
    let mut lx = Lexer { s: st, p: 0 };
    let mut prev: Option<(Vec<usize>, EdgeSpec)> = None;
    loop {
        // node group `A & B`
        let mut group = Vec::new();
        loop {
            let id = lx.id().ok_or_else(|| I18N.f("expected a node id: `{text}`", &[("text", &lx.rest().trim())]))?;
            let shape = lx.shape()?;
            lx.class_suffix();
            group.push(b.node(id, shape));
            lx.ws();
            if lx.rest().starts_with('&') {
                lx.p += 1;
                continue;
            }
            break;
        }
        if let Some((from, (line, head, tail, label))) = prev.take() {
            for &f in &from {
                for &t in &group {
                    b.fc.edges.push(Edge { from: f, to: t, label: label.clone(), line, head, tail });
                }
            }
        }
        if lx.eof() {
            return Ok(());
        }
        match lx.edge()? {
            Some(e) => {
                if lx.eof() {
                    return Err(I18N.t("edge without a target node").into_owned());
                }
                prev = Some((group, e));
            }
            None => return Err(I18N.f("unrecognized text: `{text}`", &[("text", &lx.rest().trim())])),
        }
    }
}

/// Skipped statements (styles, classes, clicks).
fn skipped(first: &str) -> bool {
    matches!(first, "style" | "classDef" | "class" | "click" | "linkStyle" | "direction" | "accTitle" | "accDescr")
        || first.starts_with("accTitle:")
        || first.starts_with("accDescr:")
}

/// Parse a flowchart; `lines` are the source lines, the header is the first significant line.
pub fn parse(src: &str) -> Result<Flowchart, ParseError> {
    let mut b = Builder { fc: Flowchart::default(), ids: HashMap::new(), stack: Vec::new() };
    let mut header = false;
    let mut sg_line = Vec::new();
    for (no, raw) in super::body_lines(src) {
        let line = no + 1;
        let err = |msg: String| ParseError { line, msg };
        let mut text = raw.trim();
        if !header {
            let mut parts = text.splitn(2, char::is_whitespace);
            let kw = parts.next().unwrap_or("");
            if kw != "graph" && kw != "flowchart" {
                return Err(err(I18N.t("expected a `flowchart`/`graph` header").into_owned()));
            }
            let rest = parts.next().unwrap_or("").trim();
            let (d, tail) = match rest.split_once(|c: char| c.is_whitespace() || c == ';') {
                Some((d, t)) => (d, t),
                None => (rest, ""),
            };
            match parse_dir(d) {
                Some(dir) => {
                    b.fc.dir = dir;
                    text = tail.trim();
                }
                None => text = rest.trim_start_matches(';'),
            }
            header = true;
        }
        for st in split_statements(text) {
            let st = st.trim();
            if st.is_empty() {
                continue;
            }
            let first = st.split_whitespace().next().unwrap_or("");
            if skipped(first) {
                continue;
            }
            if first == "subgraph" {
                if b.stack.len() >= MAX_DEPTH {
                    return Err(err(I18N.f("subgraph nesting is deeper than {max}", &[("max", &MAX_DEPTH)])));
                }
                let rest = st["subgraph".len()..].trim();
                let (id, title) = subgraph_head(rest);
                let id = if id.is_empty() { format!("subgraph{}", b.fc.subgraphs.len()) } else { id };
                b.fc.subgraphs.push(Subgraph { id, title, parent: b.stack.last().copied() });
                b.stack.push(b.fc.subgraphs.len() - 1);
                sg_line.push(line);
                continue;
            }
            if st == "end" {
                if b.stack.pop().is_none() {
                    return Err(err(I18N.t("`end` without `subgraph`").into_owned()));
                }
                sg_line.pop();
                continue;
            }
            statement(&mut b, st).map_err(err)?;
        }
    }
    if !header {
        return Err(ParseError { line: 1, msg: I18N.t("empty diagram").into_owned() });
    }
    if let Some(&l) = sg_line.last() {
        return Err(ParseError { line: l, msg: I18N.t("unclosed `subgraph`").into_owned() });
    }
    redirect_subgraph_edges(&mut b.fc);
    Ok(b.fc)
}

/// `id [title]`, `id["title"]`, `"title"`, `title words`.
fn subgraph_head(rest: &str) -> (String, String) {
    let unq = |s: &str| s.trim().trim_matches('"').to_owned();
    if rest.starts_with('"') {
        let t = unq(rest);
        return (t.clone(), t);
    }
    if let Some(open) = rest.find('[') {
        let id = rest[..open].trim().to_owned();
        let title = unq(rest[open + 1..].trim_end().trim_end_matches(']'));
        return (id, title);
    }
    let id = rest.split_whitespace().next().unwrap_or("").to_owned();
    (id, rest.to_owned())
}

/// Edges to a subgraph-as-node go to its first node (design: Risks); such "nodes" are removed.
fn redirect_subgraph_edges(fc: &mut Flowchart) {
    let first_of = |fc: &Flowchart, sg: usize| -> Option<usize> {
        fc.nodes.iter().position(|n| {
            let mut s = n.subgraph;
            while let Some(i) = s {
                if i == sg {
                    return true;
                }
                s = fc.subgraphs[i].parent;
            }
            false
        })
    };
    let mut remap: Vec<Option<usize>> = (0..fc.nodes.len()).map(Some).collect();
    let mut drop = vec![false; fc.nodes.len()];
    for (i, n) in fc.nodes.iter().enumerate() {
        if let Some(sg) = fc.subgraphs.iter().position(|s| s.id == n.id) {
            if let Some(t) = first_of(fc, sg).filter(|&t| t != i && !fc.subgraphs.iter().any(|s| s.id == fc.nodes[t].id)) {
                remap[i] = Some(t);
                drop[i] = true;
            }
        }
    }
    if !drop.iter().any(|&d| d) {
        return;
    }
    let mut new_index = vec![0; fc.nodes.len()];
    let mut k = 0;
    for i in 0..fc.nodes.len() {
        new_index[i] = k;
        if !drop[i] {
            k += 1;
        }
    }
    for e in &mut fc.edges {
        e.from = new_index[remap[e.from].unwrap_or(e.from)];
        e.to = new_index[remap[e.to].unwrap_or(e.to)];
    }
    let mut i = 0;
    fc.nodes.retain(|_| {
        let keep = !drop[i];
        i += 1;
        keep
    });
}
