//! Sequence diagram: the model, parser (design D3) and layout (D6).

use super::canvas::{Canvas, Ink, Stroke, E, N, S, W};
use super::{fit_label, Anchor, LayoutParams, ParseError};
use crate::sanitize::str_width;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("render-mermaid");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participant {
    pub id: String,
    pub label: String,
    pub actor: bool,
}

/// Message arrowhead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgHead {
    /// `->>` — arrow.
    Arrow,
    /// `->` — no arrowhead.
    Open,
    /// `-x` — cross.
    Cross,
    /// `-)` — asynchronous.
    Async,
    /// `<<->>` — both directions.
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePos {
    Left(usize),
    Right(usize),
    Over(usize, usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Msg { from: usize, to: usize, text: String, dotted: bool, head: MsgHead },
    Note { pos: NotePos, text: String },
    BlockStart { kind: String, label: String },
    BlockElse { kind: String, label: String },
    BlockEnd,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sequence {
    pub participants: Vec<Participant>,
    pub events: Vec<Event>,
    pub autonumber: bool,
}

impl Sequence {
    pub fn messages(&self) -> usize {
        self.events.iter().filter(|e| matches!(e, Event::Msg { .. })).count()
    }

    fn part(&mut self, id: &str) -> usize {
        if let Some(i) = self.participants.iter().position(|p| p.id == id) {
            return i;
        }
        self.participants.push(Participant { id: id.to_owned(), label: id.to_owned(), actor: false });
        self.participants.len() - 1
    }
}

/// Arrows, longest to shortest (the order matters for matching).
const ARROWS: &[(&str, bool, MsgHead)] = &[
    ("<<-->>", true, MsgHead::Both),
    ("<<->>", false, MsgHead::Both),
    ("-->>", true, MsgHead::Arrow),
    ("->>", false, MsgHead::Arrow),
    ("--x", true, MsgHead::Cross),
    ("-x", false, MsgHead::Cross),
    ("--)", true, MsgHead::Async),
    ("-)", false, MsgHead::Async),
    ("-->", true, MsgHead::Open),
    ("->", false, MsgHead::Open),
];

const BLOCKS: &[&str] = &["loop", "alt", "opt", "par", "critical", "break", "rect"];
const ELSES: &[&str] = &["else", "and", "option"];

pub fn parse(src: &str) -> Result<Sequence, ParseError> {
    let mut sq = Sequence::default();
    let mut header = false;
    let mut stack: Vec<usize> = Vec::new();
    for (no, raw) in super::body_lines(src) {
        let line = no + 1;
        let err = |msg: &str| ParseError { line, msg: msg.to_owned() };
        let t = raw.trim().trim_end_matches(';').trim();
        if !header {
            if t != "sequenceDiagram" {
                return Err(err(&I18N.t("expected a `sequenceDiagram` header")));
            }
            header = true;
            continue;
        }
        let (kw, rest) = match t.split_once(char::is_whitespace) {
            Some((k, r)) => (k, r.trim()),
            None => (t, ""),
        };
        match kw {
            "participant" | "actor" => {
                if rest.is_empty() {
                    return Err(err(&I18N.t("participant without a name")));
                }
                let (id, label) = match rest.split_once(" as ") {
                    Some((i, l)) => (i.trim(), l.trim()),
                    None => (rest, rest),
                };
                let i = sq.part(id);
                sq.participants[i].label = label.to_owned();
                sq.participants[i].actor = kw == "actor";
            }
            "autonumber" => sq.autonumber = true,
            "activate" | "deactivate" | "title" | "accTitle" | "accDescr" | "link" | "links" | "box" | "create"
            | "destroy" => {}
            "end" => {
                if stack.pop().is_none() {
                    return Err(err(&I18N.t("`end` without an open block")));
                }
                sq.events.push(Event::BlockEnd);
            }
            k if BLOCKS.contains(&k) => {
                stack.push(line);
                sq.events.push(Event::BlockStart { kind: k.to_owned(), label: rest.to_owned() });
            }
            k if ELSES.contains(&k) => {
                if stack.is_empty() {
                    return Err(err(&I18N.t("`else` outside a block")));
                }
                sq.events.push(Event::BlockElse { kind: k.to_owned(), label: rest.to_owned() });
            }
            "Note" | "note" => {
                let (head, text) = rest.split_once(':').ok_or_else(|| err(&I18N.t("Note without `:`")))?;
                let head = head.trim();
                let (pos, who) = if let Some(w) = head.strip_prefix("left of") {
                    ("l", w)
                } else if let Some(w) = head.strip_prefix("right of") {
                    ("r", w)
                } else if let Some(w) = head.strip_prefix("over") {
                    ("o", w)
                } else {
                    return Err(err(&I18N.t("Note: expected left of / right of / over")));
                };
                let ids: Vec<&str> = who.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
                if ids.is_empty() || ids.len() > 2 {
                    return Err(err(&I18N.t("Note: one or two participants")));
                }
                let a = sq.part(ids[0]);
                let b = ids.get(1).map(|i| sq.part(i)).unwrap_or(a);
                let pos = match pos {
                    "l" => NotePos::Left(a),
                    "r" => NotePos::Right(a),
                    _ => NotePos::Over(a.min(b), a.max(b)),
                };
                sq.events.push(Event::Note { pos, text: text.trim().to_owned() });
            }
            _ => {
                let (lhs, text) = match t.split_once(':') {
                    Some((l, r)) => (l, r.trim()),
                    None => (t, ""),
                };
                let mut found = None;
                for &(a, dotted, head) in ARROWS {
                    if let Some(i) = lhs.find(a) {
                        found = Some((i, a.len(), dotted, head));
                        break;
                    }
                }
                let Some((i, len, dotted, head)) = found else {
                    return Err(err(&I18N.t("unrecognized line")));
                };
                let from = lhs[..i].trim();
                let to = lhs[i + len..].trim().trim_start_matches(['+', '-']).trim();
                if from.is_empty() || to.is_empty() || from.contains(char::is_whitespace) && !from.contains('"') {
                    return Err(err(&I18N.t("message: expected `A->>B: text`")));
                }
                let from = sq.part(from);
                let to = sq.part(to);
                sq.events.push(Event::Msg { from, to, text: text.to_owned(), dotted, head });
            }
        }
    }
    if !header {
        return Err(ParseError { line: 1, msg: I18N.t("empty diagram").into_owned() });
    }
    if let Some(&l) = stack.last() {
        return Err(ParseError { line: l, msg: I18N.t("unclosed block (no `end`)").into_owned() });
    }
    Ok(sq)
}

/// Layout result (for tests and drawing).
pub struct SeqLayout {
    /// Lifeline centres.
    pub cols: Vec<usize>,
    pub head_w: Vec<usize>,
    pub height: usize,
}

/// Message label taking autonumber and zoom into account.
fn msg_text(sq: &Sequence, n: usize, text: &str, p: &LayoutParams) -> String {
    let t = fit_label(text, p.max_label);
    if sq.autonumber {
        format!("{n}. {t}")
    } else {
        t
    }
}

fn nesting(sq: &Sequence) -> usize {
    let (mut d, mut m) = (0usize, 0usize);
    for e in &sq.events {
        match e {
            Event::BlockStart { .. } => {
                d += 1;
                m = m.max(d);
            }
            Event::BlockEnd => d = d.saturating_sub(1),
            _ => {}
        }
    }
    m
}

/// Columns (D6): the distance between neighbours is max(zoom minimum, width of the labels between them).
pub fn layout(sq: &Sequence, p: &LayoutParams) -> SeqLayout {
    let np = sq.participants.len();
    let head_w: Vec<usize> = sq
        .participants
        .iter()
        .map(|pt| str_width(&fit_label(&pt.label, p.max_label)) + 2 + 2 * p.pad_x)
        .collect();
    // minimum distance between neighbour centres
    let mut gap: Vec<usize> = (0..np.saturating_sub(1))
        .map(|i| head_w[i].div_ceil(2) + head_w[i + 1].div_ceil(2) + p.seq_min_gap)
        .collect();
    let mut tail_extra = 0usize;
    let mut num = 0;
    for e in &sq.events {
        let (a, b, need) = match e {
            Event::Msg { from, to, text, .. } => {
                num += 1;
                let w = str_width(&msg_text(sq, num, text, p));
                if from == to {
                    (*from, *from + 1, w + 6)
                } else {
                    ((*from).min(*to), (*from).max(*to), w + 4)
                }
            }
            Event::Note { pos: NotePos::Right(a), text } => (*a, a + 1, str_width(&fit_label(text, p.max_label)) + 6),
            Event::Note { pos: NotePos::Left(a), text } if *a > 0 => {
                (a - 1, *a, str_width(&fit_label(text, p.max_label)) + 6)
            }
            _ => continue,
        };
        if b >= np {
            tail_extra = tail_extra.max(need);
            continue;
        }
        let have: usize = gap[a..b].iter().sum();
        if have < need {
            gap[b - 1] += need - have;
        }
    }
    let margin = 2 + nesting(sq);
    let mut left = head_w.first().map_or(0, |w| w / 2) + margin;
    for e in &sq.events {
        if let Event::Note { pos: NotePos::Left(0), text } = e {
            left = left.max(str_width(&fit_label(text, p.max_label)) + 6);
        }
    }
    let mut cols = Vec::with_capacity(np);
    let mut x = left;
    if np > 0 {
        cols.push(x);
    }
    for g in &gap {
        x += g;
        cols.push(x);
    }
    let _ = tail_extra;
    let mut h = 3;
    for e in &sq.events {
        h += match e {
            Event::Msg { from, to, .. } if from == to => 3,
            Event::Msg { .. } | Event::Note { .. } => 2,
            _ => 1,
        };
        if let Event::Note { .. } = e {
            h += 1;
        }
    }
    SeqLayout { cols, head_w, height: h + 3 }
}

/// Draw a sequence diagram.
pub fn draw(sq: &Sequence, p: &LayoutParams, ascii: bool) -> (Canvas, Vec<Anchor>) {
    let lay = layout(sq, p);
    let mut cv = Canvas::new(ascii);
    let bottom = lay.height - 3;
    let heads = |cv: &mut Canvas, y: usize, anchors: &mut Vec<Anchor>| {
        for (i, pt) in sq.participants.iter().enumerate() {
            let w = lay.head_w[i];
            let x = lay.cols[i] - w / 2;
            cv.rect(x, y, w, 3, Stroke::Thin, pt.actor, Ink::Border);
            let l = fit_label(&pt.label, p.max_label);
            cv.text(x + (w - str_width(&l)) / 2, y + 1, &l, Ink::Label);
            anchors.push(Anchor { id: pt.id.clone(), x: lay.cols[i], y: y + 1 });
        }
    };
    let mut anchors = Vec::new();
    heads(&mut cv, 0, &mut anchors);
    heads(&mut cv, bottom, &mut Vec::new());
    for &c in &lay.cols {
        cv.vline(c, 2, bottom, Stroke::Thin, Ink::Border);
    }
    let a = |u: char, s: char| if ascii { s } else { u };
    let mut y = 3;
    let mut num = 0;
    // block stack: (start y, min column, max column, kind, label, else lines)
    struct Blk {
        y0: usize,
        lo: usize,
        hi: usize,
        label: String,
        elses: Vec<(usize, String)>,
        /// Depth of nested blocks inside (the frame is that much wider).
        inner: usize,
    }
    let mut stack: Vec<Blk> = Vec::new();
    let touch = |stack: &mut Vec<Blk>, l: usize, h: usize| {
        for b in stack.iter_mut() {
            b.lo = b.lo.min(l);
            b.hi = b.hi.max(h);
        }
    };
    let last = lay.cols.len().saturating_sub(1);
    for e in &sq.events {
        match e {
            Event::Msg { from, to, text, dotted, head } => {
                num += 1;
                let t = msg_text(sq, num, text, p);
                let st = if *dotted { Stroke::Dotted } else { Stroke::Thin };
                let (cf, ct) = (lay.cols[*from], lay.cols[*to]);
                touch(&mut stack, *from.min(to), *from.max(to));
                if from == to {
                    cv.text(cf + 2, y, &t, Ink::EdgeLabel);
                    cv.hline(cf, cf + 3, y + 1, st, Ink::Border);
                    cv.vline(cf + 3, y + 1, y + 2, st, Ink::Border);
                    cv.hline(cf + 1, cf + 3, y + 2, st, Ink::Border);
                    cv.add(cf, y + 1, E, st, Ink::Border);
                    cv.put(cf + 1, y + 2, a('◀', '<'), Ink::Border);
                    y += 3;
                    continue;
                }
                let (l, r) = (cf.min(ct), cf.max(ct));
                let tw = str_width(&t);
                cv.text(l + (r - l).saturating_sub(tw) / 2 + 1, y, &t, Ink::EdgeLabel);
                cv.hline(l + 1, r - 1, y + 1, st, Ink::Border);
                cv.add(l, y + 1, E, st, Ink::Border);
                cv.add(r, y + 1, W, st, Ink::Border);
                let right = ct > cf;
                let mark = match head {
                    MsgHead::Arrow | MsgHead::Both => Some(if right { a('▶', '>') } else { a('◀', '<') }),
                    MsgHead::Open => None,
                    MsgHead::Cross => Some('x'),
                    MsgHead::Async => Some(if right { ')' } else { '(' }),
                };
                if let Some(m) = mark {
                    cv.put(if right { r - 1 } else { l + 1 }, y + 1, m, Ink::Border);
                }
                if *head == MsgHead::Both {
                    cv.put(if right { l + 1 } else { r - 1 }, y + 1, if right { a('◀', '<') } else { a('▶', '>') }, Ink::Border);
                }
                y += 2;
            }
            Event::Note { pos, text } => {
                let t = fit_label(text, p.max_label);
                let w = str_width(&t) + 4;
                let x = match *pos {
                    NotePos::Over(a0, b0) => {
                        touch(&mut stack, a0, b0);
                        let c = (lay.cols[a0] + lay.cols[b0]) / 2;
                        let span = lay.cols[b0] - lay.cols[a0];
                        let w2 = w.max(span + 4);
                        let x = c.saturating_sub(w2 / 2);
                        cv.clear(x, y, w2, 3);
                        cv.rect(x, y, w2, 3, Stroke::Thin, false, Ink::Frame);
                        cv.text(x + (w2 - str_width(&t)) / 2, y + 1, &t, Ink::Label);
                        y += 3;
                        continue;
                    }
                    NotePos::Right(a0) => {
                        touch(&mut stack, a0, (a0 + 1).min(last));
                        lay.cols[a0] + 2
                    }
                    NotePos::Left(a0) => {
                        touch(&mut stack, a0.saturating_sub(1), a0);
                        lay.cols[a0].saturating_sub(w + 1)
                    }
                };
                cv.clear(x, y, w, 3);
                cv.rect(x, y, w, 3, Stroke::Thin, false, Ink::Frame);
                cv.text(x + 2, y + 1, &t, Ink::Label);
                y += 3;
            }
            Event::BlockStart { kind, label } => {
                let label = fit_label(format!("{kind} {label}").trim(), p.max_label);
                stack.push(Blk { y0: y, lo: usize::MAX, hi: 0, label, elses: Vec::new(), inner: 0 });
                y += 1;
            }
            Event::BlockElse { kind, label } => {
                if let Some(b) = stack.last_mut() {
                    b.elses.push((y, fit_label(&format!("[{kind} {label}]").replace(" ]", "]"), p.max_label)));
                }
                y += 1;
            }
            Event::BlockEnd => {
                if let Some(b) = stack.pop() {
                    let (lo, hi) = if b.lo == usize::MAX { (0, 0) } else { (b.lo, b.hi) };
                    let pad = 2 + b.inner;
                    let x0 = lay.cols[lo].saturating_sub(pad);
                    let x1 = lay.cols[hi] + pad + 2;
                    cv.rect(x0, b.y0, x1 - x0 + 1, y - b.y0 + 1, Stroke::Thin, false, Ink::Frame);
                    cv.text(x0 + 1, b.y0, &format!(" {} ", b.label), Ink::Frame);
                    for (ey, el) in &b.elses {
                        cv.hline(x0, x1, *ey, Stroke::Dotted, Ink::Frame);
                        cv.add(x0, *ey, N | S | E, Stroke::Thin, Ink::Frame);
                        cv.add(x1, *ey, N | S | W, Stroke::Thin, Ink::Frame);
                        cv.text(x0 + 2, *ey, el, Ink::Frame);
                    }
                    for b2 in stack.iter_mut() {
                        b2.lo = b2.lo.min(lo);
                        b2.hi = b2.hi.max(hi);
                    }
                    if let Some(parent) = stack.last_mut() {
                        parent.inner = parent.inner.max(b.inner + 1);
                    }
                }
                y += 1;
            }
        }
    }
    (cv, anchors)
}
