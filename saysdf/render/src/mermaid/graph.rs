//! Flowchart layout (simplified Sugiyama on an integer grid, design D4) and drawing on
//! [`Canvas`] (D5). The layout is computed in TD coordinates (layer = rows, order = columns); for
//! LR/RL/BT the coordinates are swapped/mirrored at the end.

use super::canvas::{Canvas, Ink, Stroke, E, N, S, W};
use super::flowchart::{Dir, Flowchart, Head, LineKind, Shape};
use super::{fit_label, Anchor, LayoutParams};
use crate::sanitize::str_width;

/// Rectangle in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
    pub fn cx(&self) -> i64 {
        self.x + self.w / 2
    }
    pub fn cy(&self) -> i64 {
        self.y + self.h / 2
    }
}

/// Layout result (final coordinates, without the shift for subgraph frames).
#[derive(Clone, Debug, Default)]
pub struct GraphLayout {
    /// Layer of each node.
    pub layer: Vec<usize>,
    /// Position of a node within its layer.
    pub order: Vec<usize>,
    /// Node rectangles.
    pub boxes: Vec<Rect>,
    /// Node labels (after truncation/wrapping).
    pub labels: Vec<Vec<String>>,
    /// Edge reversed during cycle removal.
    pub reversed: Vec<bool>,
    /// Number of dummy nodes.
    pub dummies: usize,
    /// Edge polylines: from the cell at the source to the cell at the target (in the edge's original direction).
    pub paths: Vec<Vec<(i64, i64)>>,
}

/// Step 1: reverse cycles with DFS in declaration order (iterative, no stack overflow).
pub fn acyclic(n: usize, edges: &[(usize, usize)]) -> Vec<bool> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, &(a, b)) in edges.iter().enumerate() {
        if a != b {
            out[a].push(i);
        }
    }
    // 0 = unvisited, 1 = on the stack, 2 = done
    let mut state = vec![0u8; n];
    let mut rev = vec![false; edges.len()];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        state[root] = 1;
        while let Some(top) = stack.last_mut() {
            let (v, k) = *top;
            if let Some(&ei) = out[v].get(k) {
                top.1 += 1;
                let t = edges[ei].1;
                match state[t] {
                    0 => {
                        state[t] = 1;
                        stack.push((t, 0));
                    }
                    1 => rev[ei] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    rev
}

/// Step 2: longest-path layers (Kahn) on the acyclic graph.
pub fn longest_path_layers(n: usize, dag: &[(usize, usize)]) -> Vec<usize> {
    let mut indeg = vec![0usize; n];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in dag {
        if a != b {
            out[a].push(b);
            indeg[b] += 1;
        }
    }
    let mut layer = vec![0usize; n];
    let mut q: std::collections::VecDeque<usize> = (0..n).filter(|&v| indeg[v] == 0).collect();
    while let Some(v) = q.pop_front() {
        for &t in &out[v] {
            layer[t] = layer[t].max(layer[v] + 1);
            indeg[t] -= 1;
            if indeg[t] == 0 {
                q.push_back(t);
            }
        }
    }
    layer
}

/// Node label → lines according to the zoom parameters.
fn node_label(label: &str, p: &LayoutParams) -> Vec<String> {
    let label = label.replace("<br>", "\n").replace("<br/>", "\n").replace("<br />", "\n");
    let mut lines = Vec::new();
    for part in label.split('\n') {
        match p.wrap {
            Some(wd) if str_width(part) > wd => lines.extend(wrap(part, wd)),
            _ => lines.push(fit_label(part, p.max_label)),
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in s.split_whitespace() {
        if !cur.is_empty() && str_width(&cur) + 1 + str_width(w) > width {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|l| fit_label(&l, Some(width))).collect()
}

/// Node size (final width/height).
fn node_size(shape: Shape, lines: &[String], p: &LayoutParams) -> (i64, i64) {
    let lw = lines.iter().map(|l| str_width(l)).max().unwrap_or(0) as i64;
    let extra = match shape {
        Shape::Diamond | Shape::Hexagon | Shape::Circle => 2,
        _ => 0,
    };
    ((lw + 2 + 2 * p.pad_x as i64 + extra).max(3), lines.len() as i64 + 2)
}

struct LNode {
    /// Size along the layer (TD width).
    along: i64,
    /// Thickness (TD height).
    thick: i64,
    layer: usize,
    x: i64,
    y: i64,
    real: bool,
    group: Option<usize>,
}

/// Full flowchart layout.
pub fn layout(fc: &Flowchart, p: &LayoutParams) -> GraphLayout {
    let n = fc.nodes.len();
    let horizontal = matches!(fc.dir, Dir::LR | Dir::RL);
    let labels: Vec<Vec<String>> = fc.nodes.iter().map(|nd| node_label(&nd.label, p)).collect();
    let sizes: Vec<(i64, i64)> = fc.nodes.iter().zip(&labels).map(|(nd, l)| node_size(nd.shape, l, p)).collect();

    let pairs: Vec<(usize, usize)> = fc.edges.iter().map(|e| (e.from, e.to)).collect();
    let reversed = acyclic(n, &pairs);
    let dag: Vec<(usize, usize)> =
        pairs.iter().zip(&reversed).map(|(&(a, b), &r)| if r { (b, a) } else { (a, b) }).collect();
    let layer = longest_path_layers(n, &dag);

    let mut ln: Vec<LNode> = (0..n)
        .map(|i| {
            let (w, h) = sizes[i];
            let (along, thick) = if horizontal { (h, w) } else { (w, h) };
            LNode { along, thick, layer: layer[i], x: 0, y: 0, real: true, group: fc.nodes[i].subgraph }
        })
        .collect();

    // Step 3: dummy nodes; a node chain for each edge (in the DAG direction)
    let mut chains: Vec<Vec<usize>> = Vec::with_capacity(dag.len());
    for &(a, b) in &dag {
        if a == b {
            chains.push(vec![a]);
            continue;
        }
        let mut ch = vec![a];
        for l in layer[a] + 1..layer[b] {
            ln.push(LNode { along: 1, thick: 0, layer: l, x: 0, y: 0, real: false, group: None });
            ch.push(ln.len() - 1);
        }
        ch.push(b);
        chains.push(ch);
    }
    let dummies = ln.len() - n;
    let nl = layer.iter().copied().max().map_or(0, |m| m + 1);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); nl];
    for (i, v) in ln.iter().enumerate() {
        layers[v.layer].push(i);
    }
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); ln.len()];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); ln.len()];
    for ch in &chains {
        for w in ch.windows(2) {
            succs[w[0]].push(w[1]);
            preds[w[1]].push(w[0]);
        }
    }

    // Step 4: barycentre, 4 passes (down, up, down, up); nodes of one subgraph stay together
    let mut pos = vec![0usize; ln.len()];
    let set_pos = |layers: &Vec<Vec<usize>>, pos: &mut Vec<usize>| {
        for l in layers {
            for (k, &v) in l.iter().enumerate() {
                pos[v] = k;
            }
        }
    };
    set_pos(&layers, &mut pos);
    for pass in 0..4 {
        let down = pass % 2 == 0;
        let range: Vec<usize> = if down { (1..nl).collect() } else { (0..nl.saturating_sub(1)).rev().collect() };
        for li in range {
            let nb = if down { &preds } else { &succs };
            let bary: Vec<(usize, f64)> = layers[li]
                .iter()
                .map(|&v| {
                    let ns = &nb[v];
                    let b = if ns.is_empty() {
                        pos[v] as f64
                    } else {
                        ns.iter().map(|&u| pos[u] as f64).sum::<f64>() / ns.len() as f64
                    };
                    (v, b)
                })
                .collect();
            // group key: mean barycentre of the subgraph members
            let mut gsum: std::collections::HashMap<usize, (f64, usize)> = Default::default();
            for &(v, b) in &bary {
                if let Some(g) = ln[v].group {
                    let e = gsum.entry(g).or_insert((0.0, 0));
                    e.0 += b;
                    e.1 += 1;
                }
            }
            let mut keyed: Vec<(f64, usize, f64, usize)> = bary
                .iter()
                .map(|&(v, b)| match ln[v].group {
                    Some(g) => {
                        let (s, c) = gsum[&g];
                        (s / c as f64, g + 1, b, v)
                    }
                    None => (b, 0, b, v),
                })
                .collect();
            keyed.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.1.cmp(&b.1))
                    .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
            });
            layers[li] = keyed.into_iter().map(|k| k.3).collect();
            for (k, &v) in layers[li].iter().enumerate() {
                pos[v] = k;
            }
        }
    }

    // Step 5: coordinates. Dense packing first, then alignment to the median of the neighbours.
    let gap = p.node_gap as i64;
    for l in &layers {
        let mut x = 0;
        for &v in l {
            ln[v].x = x;
            x += ln[v].along + gap;
        }
    }
    let center = |ln: &Vec<LNode>, v: usize| ln[v].x + ln[v].along / 2;
    for pass in 0..3 {
        let down = pass % 2 == 0;
        let range: Vec<usize> = if down { (1..nl).collect() } else { (0..nl.saturating_sub(1)).rev().collect() };
        for li in range {
            let mut prev_right: Option<i64> = None;
            for &v in &layers[li] {
                let nb = if down { &preds[v] } else { &succs[v] };
                let mut desired = ln[v].x;
                if !nb.is_empty() {
                    let mut cs: Vec<i64> = nb.iter().map(|&u| center(&ln, u)).collect();
                    cs.sort_unstable();
                    desired = cs[(cs.len() - 1) / 2] - ln[v].along / 2;
                }
                let x = match prev_right {
                    Some(r) => desired.max(r + gap),
                    None => desired,
                };
                ln[v].x = x;
                prev_right = Some(x + ln[v].along);
            }
        }
    }
    let minx = ln.iter().map(|v| v.x).min().unwrap_or(0);
    for v in ln.iter_mut() {
        v.x -= minx;
    }
    // layers by thickness
    let lgap = p.layer_gap.max(2) as i64;
    let mut ly = vec![0i64; nl];
    let mut lthick = vec![0i64; nl];
    let mut y = 0;
    for li in 0..nl {
        let t = layers[li].iter().map(|&v| ln[v].thick).max().unwrap_or(0).max(1);
        ly[li] = y;
        lthick[li] = t;
        y += t + lgap;
    }
    let total_layers = (y - lgap).max(0);
    for v in ln.iter_mut() {
        v.y = ly[v.layer] + (lthick[v.layer] - v.thick) / 2;
    }
    let total_along = ln.iter().map(|v| v.x + v.along).max().unwrap_or(0);

    // Step 6: routing (in TD coordinates). Ports are spread along the node side.
    let mut out_ports: Vec<Vec<usize>> = vec![Vec::new(); ln.len()]; // edge indices
    let mut in_ports: Vec<Vec<usize>> = vec![Vec::new(); ln.len()];
    for (ei, ch) in chains.iter().enumerate() {
        if ch.len() >= 2 {
            out_ports[ch[0]].push(ei);
            in_ports[ch[ch.len() - 1]].push(ei);
        }
    }
    // Port: opposite the neighbour (straight line) if free; otherwise spread along the node side.
    let port_x = |ln: &Vec<LNode>, v: usize, list: &Vec<usize>, ei: usize, key: &dyn Fn(usize) -> i64| -> i64 {
        let nd = &ln[v];
        if !nd.real || nd.along < 3 {
            return nd.x + nd.along / 2;
        }
        let (lo, hi) = (nd.x + 1, nd.x + nd.along - 2);
        let mut want: Vec<(i64, usize)> = list.iter().map(|&e| (key(e).clamp(lo, hi), e)).collect();
        want.sort();
        for k in 1..want.len() {
            want[k].0 = want[k].0.max(want[k - 1].0 + 2);
        }
        // ran past the right edge: shift back, keeping the order
        let mut limit = hi;
        for k in (0..want.len()).rev() {
            want[k].0 = want[k].0.min(limit).max(lo);
            limit = want[k].0 - 2;
        }
        want.iter().find(|w| w.1 == ei).map_or(nd.x + nd.along / 2, |w| w.0)
    };
    // row number within the gap for horizontal segments
    let mut gap_use = vec![0i64; nl];
    let mut td_paths: Vec<Vec<(i64, i64)>> = Vec::with_capacity(chains.len());
    for (ei, ch) in chains.iter().enumerate() {
        if ch.len() < 2 {
            td_paths.push(Vec::new());
            continue;
        }
        let first = ch[0];
        let last = ch[ch.len() - 1];
        let next = ch[1];
        let prev = ch[ch.len() - 2];
        let px0 = port_x(&ln, first, &out_ports[first], ei, &|e| center(&ln, chains[e][1]));
        let qxl = port_x(&ln, last, &in_ports[last], ei, &|e| {
            let c = &chains[e];
            center(&ln, c[c.len() - 2])
        });
        let _ = (next, prev);
        let mut pts: Vec<(i64, i64)> = Vec::new();
        for (h, w) in ch.windows(2).enumerate() {
            let (u, v) = (w[0], w[1]);
            let px = if h == 0 { px0 } else { center(&ln, u) };
            let qx = if h == ch.len() - 2 { qxl } else { center(&ln, v) };
            let ub = if ln[u].real { ln[u].y + ln[u].thick - 1 } else { ly[ln[u].layer] + lthick[ln[u].layer] - 1 };
            let vt = if ln[v].real { ln[v].y } else { ly[ln[v].layer] };
            let start = (px, ub + 1);
            let end = (qx, vt - 1);
            pts.push(start);
            if px != qx {
                let li = ln[u].layer;
                // horizontal row strictly between the ends (both arrows sit on vertical segments)
                let (r0, r1) = (start.1 + 1, end.1 - 1);
                let ym = if r0 <= r1 { r0 + gap_use[li] % (r1 - r0 + 1) } else { start.1 };
                gap_use[li] += 1;
                pts.push((px, ym));
                pts.push((qx, ym));
            }
            pts.push(end);
        }
        td_paths.push(pts);
    }

    // Convert to final coordinates
    let tr_pt = |(x, y): (i64, i64)| -> (i64, i64) {
        match fc.dir {
            Dir::TD => (x, y),
            Dir::BT => (x, total_layers - 1 - y),
            Dir::LR => (y, x),
            Dir::RL => (total_layers - 1 - y, x),
        }
    };
    let tr_rect = |x: i64, y: i64, w: i64, h: i64| -> Rect {
        match fc.dir {
            Dir::TD => Rect { x, y, w, h },
            Dir::BT => Rect { x, y: total_layers - y - h, w, h },
            Dir::LR => Rect { x: y, y: x, w: h, h: w },
            Dir::RL => Rect { x: total_layers - y - h, y: x, w: h, h: w },
        }
    };
    let _ = total_along;
    let boxes: Vec<Rect> = (0..n).map(|i| tr_rect(ln[i].x, ln[i].y, ln[i].along, ln[i].thick)).collect();
    let mut paths: Vec<Vec<(i64, i64)>> = Vec::with_capacity(td_paths.len());
    for (ei, pts) in td_paths.into_iter().enumerate() {
        let mut v: Vec<(i64, i64)> = pts.into_iter().map(tr_pt).collect();
        if v.is_empty() {
            // self-loop: to the right of the node
            let b = boxes[fc.edges[ei].from];
            v = vec![(b.x + b.w, b.y + 1), (b.x + b.w + 1, b.y + 1), (b.x + b.w + 1, b.y + b.h), (b.x + b.w - 2, b.y + b.h)];
        } else if reversed[ei] {
            v.reverse();
        }
        paths.push(v);
    }
    let order = (0..n).map(|i| layers[layer[i]].iter().position(|&v| v == i).unwrap_or(0)).collect();
    GraphLayout { layer, order, boxes, labels, reversed, dummies, paths }
}

fn stroke_of(k: LineKind) -> Stroke {
    match k {
        LineKind::Normal => Stroke::Thin,
        LineKind::Thick => Stroke::Thick,
        LineKind::Dotted => Stroke::Dotted,
    }
}

/// Direction from point `p` (outside) to frame `b`: side mask and arrow character.
fn attach(p: (i64, i64), b: &Rect, ascii: bool) -> Option<(u8, (i64, i64), char)> {
    let (x, y) = p;
    let inside_x = x >= b.x && x < b.x + b.w;
    let inside_y = y >= b.y && y < b.y + b.h;
    let a = |u: char, s: char| if ascii { s } else { u };
    if inside_x && y == b.y - 1 {
        Some((S, (x, y + 1), a('▼', 'v')))
    } else if inside_x && y == b.y + b.h {
        Some((N, (x, y - 1), a('▲', '^')))
    } else if inside_y && x == b.x - 1 {
        Some((E, (x + 1, y), a('▶', '>')))
    } else if inside_y && x == b.x + b.w {
        Some((W, (x - 1, y), a('◀', '<')))
    } else {
        None
    }
}

fn opposite(m: u8) -> u8 {
    match m {
        N => S,
        S => N,
        E => W,
        _ => E,
    }
}

/// A rendered diagram.
pub struct Drawn {
    pub canvas: Canvas,
    pub anchors: Vec<Anchor>,
    pub footnotes: Vec<String>,
}

/// Draw a layout: subgraph frames, nodes, edges, edge labels.
pub fn draw(fc: &Flowchart, lay: &GraphLayout, p: &LayoutParams, ascii: bool) -> Drawn {
    // subgraph frames: inside out (children are declared after their parents)
    let ns = fc.subgraphs.len();
    let mut frames: Vec<Option<Rect>> = vec![None; ns];
    for s in (0..ns).rev() {
        let mut r: Option<(i64, i64, i64, i64)> = None;
        let mut grow = |x0: i64, y0: i64, x1: i64, y1: i64| {
            r = Some(match r {
                None => (x0, y0, x1, y1),
                Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
            });
        };
        for (i, nd) in fc.nodes.iter().enumerate() {
            if nd.subgraph == Some(s) {
                let b = lay.boxes[i];
                grow(b.x - 2, b.y - 2, b.x + b.w + 1, b.y + b.h + 1);
            }
        }
        for (c, fr) in frames.iter().enumerate().skip(s + 1) {
            if fc.subgraphs[c].parent == Some(s) {
                if let Some(f) = *fr {
                    grow(f.x - 1, f.y - 2, f.x + f.w, f.y + f.h);
                }
            }
        }
        if let Some((x0, y0, x1, y1)) = r {
            let tw = str_width(&fc.subgraphs[s].title) as i64 + 4;
            frames[s] = Some(Rect { x: x0, y: y0, w: (x1 - x0 + 1).max(tw), h: y1 - y0 + 1 });
        }
    }
    let mut minx = 0;
    let mut miny = 0;
    for f in frames.iter().flatten() {
        minx = minx.min(f.x);
        miny = miny.min(f.y);
    }
    for path in &lay.paths {
        for &(x, y) in path {
            minx = minx.min(x);
            miny = miny.min(y);
        }
    }
    let (ox, oy) = (-minx, -miny);
    let at = |x: i64, y: i64| ((x + ox).max(0) as usize, (y + oy).max(0) as usize);
    let boxes: Vec<Rect> = lay.boxes.iter().map(|b| Rect { x: b.x + ox, y: b.y + oy, ..*b }).collect();

    let mut cv = Canvas::new(ascii);
    for (s, f) in frames.iter().enumerate() {
        if let Some(f) = f {
            let (x, y) = at(f.x, f.y);
            cv.rect(x, y, f.w as usize, f.h as usize, Stroke::Thin, false, Ink::Frame);
            let t = fit_label(&fc.subgraphs[s].title, p.max_label);
            if !t.is_empty() {
                cv.text(x + 2, y, &t, Ink::Frame);
            }
        }
    }
    let mut anchors = Vec::new();
    for (i, nd) in fc.nodes.iter().enumerate() {
        let b = boxes[i];
        let (x, y, w, h) = (b.x as usize, b.y as usize, b.w as usize, b.h as usize);
        cv.clear(x, y, w, h);
        let round = matches!(nd.shape, Shape::Round | Shape::Stadium | Shape::Cylinder | Shape::Circle);
        cv.rect(x, y, w, h, Stroke::Thin, round, Ink::Border);
        match nd.shape {
            Shape::Diamond | Shape::Hexagon => {
                cv.put(x, y, '/', Ink::Border);
                cv.put(x + w - 1, y, '\\', Ink::Border);
                cv.put(x, y + h - 1, '\\', Ink::Border);
                cv.put(x + w - 1, y + h - 1, '/', Ink::Border);
            }
            Shape::Asym => cv.put(x, y + h / 2, '>', Ink::Border),
            _ => {}
        }
        for (k, l) in lay.labels[i].iter().enumerate() {
            let lw = str_width(l);
            cv.text(x + (w.saturating_sub(lw)) / 2, y + 1 + k, l, Ink::Label);
        }
        anchors.push(Anchor { id: nd.id.clone(), x: b.cx().max(0) as usize, y: b.cy().max(0) as usize });
    }
    // edges
    for (ei, e) in fc.edges.iter().enumerate() {
        let path = &lay.paths[ei];
        if path.is_empty() {
            continue;
        }
        let st = stroke_of(e.line);
        for w in path.windows(2) {
            let (a, b) = (at(w[0].0, w[0].1), at(w[1].0, w[1].1));
            if a == b {
                continue;
            }
            if a.1 == b.1 {
                cv.hline(a.0, b.0, a.1, st, Ink::Border);
            } else if a.0 == b.0 {
                cv.vline(a.0, a.1, b.1, st, Ink::Border);
            }
        }
        if path.len() == 1 {
            let (x, y) = at(path[0].0, path[0].1);
            cv.vline(x, y, y, st, Ink::Border);
        }
        let ends = [(path[0], e.from, e.tail), (path[path.len() - 1], e.to, e.head)];
        for (pt, node, head) in ends {
            let b = boxes[node];
            let pt = (pt.0 + ox, pt.1 + oy);
            let Some((m, border, arrow)) = attach(pt, &b, ascii) else { continue };
            let (px, py) = (pt.0.max(0) as usize, pt.1.max(0) as usize);
            match head {
                Head::None => {
                    cv.add(px, py, m, st, Ink::Border);
                    cv.add(border.0 as usize, border.1 as usize, opposite(m), Stroke::Thin, Ink::Border);
                }
                Head::Arrow => cv.put(px, py, arrow, Ink::Border),
                Head::Cross => cv.put(px, py, 'x', Ink::Border),
                Head::Circle => cv.put(px, py, 'o', Ink::Border),
            }
        }
    }
    // edge labels
    let mut footnotes = Vec::new();
    for (ei, e) in fc.edges.iter().enumerate() {
        let Some(label) = e.label.as_deref() else { continue };
        let path: Vec<(usize, usize)> = lay.paths[ei].iter().map(|&(x, y)| at(x, y)).collect();
        if path.is_empty() {
            continue;
        }
        let text = fit_label(label, p.max_label);
        if !place_label(&mut cv, &path, &text) {
            footnotes.push(text.clone());
            let mark = format!("({})", footnotes.len());
            if !place_label(&mut cv, &path, &mark) {
                // no room even for the number: footnote only
            }
        }
    }
    Drawn { canvas: cv, anchors, footnotes }
}

/// Find room for a label by a polyline: on a horizontal segment (over the line) or to the right/left
/// of a vertical one; the cells must be free (nodes, other lines and labels).
fn place_label(cv: &mut Canvas, path: &[(usize, usize)], text: &str) -> bool {
    let lw = str_width(text);
    if lw == 0 {
        return true;
    }
    let free = |cv: &Canvas, x: usize, y: usize, own: Option<(usize, usize)>| {
        (x.saturating_sub(1)..x + lw + 1).all(|xx| {
            let mine = own.is_some_and(|(a, b)| xx >= a && xx <= b);
            mine || !cv.is_used(xx, y)
        })
    };
    type Cand = (usize, usize, Option<(usize, usize)>);
    let mut cands: Vec<Cand> = Vec::new();
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a.1 == b.1 {
            let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
            if x1 - x0 >= lw + 2 {
                cands.push((x0 + (x1 - x0 + 1 - lw) / 2, a.1, Some((x0, x1))));
            }
        }
    }
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a.0 == b.0 {
            let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
            let mid = (y0 + y1) / 2;
            let mut rows: Vec<usize> = (y0..=y1).collect();
            rows.sort_by_key(|&r| r.abs_diff(mid));
            for r in rows {
                cands.push((a.0 + 2, r, None));
                if a.0 >= lw + 2 {
                    cands.push((a.0 - lw - 1, r, None));
                }
            }
        }
    }
    for (x, y, own) in cands {
        if free(cv, x, y, own) {
            cv.text(x, y, text, Ink::EdgeLabel);
            return true;
        }
    }
    false
}
