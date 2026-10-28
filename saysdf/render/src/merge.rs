//! Merge (design diff-merge-tool D6): parsing conflict markers (2-way and diff3), three-way
//! merge of base/ours/theirs on top of `similar`, block resolutions and result assembly.

use similar::{Algorithm, DiffOp, TextDiff};
use std::fmt;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("render-diff");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    Stable,
    OursOnly,
    TheirsOnly,
    /// Both sides made the same change.
    Both,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
    Base,
    Edited(Vec<String>),
}

/// Block: lines without line terminators. For `Stable` all three sides are equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub kind: ChunkKind,
    /// `None` means the base is unknown (2-way markers).
    pub base: Option<Vec<String>>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    pub resolution: Option<Resolution>,
}

impl Chunk {
    fn stable(lines: Vec<String>) -> Self {
        Self {
            kind: ChunkKind::Stable,
            base: Some(lines.clone()),
            ours: lines.clone(),
            theirs: lines,
            resolution: None,
        }
    }

    /// Unresolved conflict.
    pub fn is_unresolved(&self) -> bool {
        self.kind == ChunkKind::Conflict && self.resolution.is_none()
    }

    /// Result lines; `None` means an unresolved conflict.
    pub fn result(&self) -> Option<Vec<String>> {
        let cat = |a: &[String], b: &[String]| a.iter().chain(b).cloned().collect::<Vec<_>>();
        Some(match &self.resolution {
            Some(Resolution::Ours) => self.ours.clone(),
            Some(Resolution::Theirs) => self.theirs.clone(),
            Some(Resolution::OursThenTheirs) => cat(&self.ours, &self.theirs),
            Some(Resolution::TheirsThenOurs) => cat(&self.theirs, &self.ours),
            Some(Resolution::Base) => self.base.clone().unwrap_or_default(),
            Some(Resolution::Edited(v)) => v.clone(),
            None => match self.kind {
                ChunkKind::Stable | ChunkKind::OursOnly | ChunkKind::Both => self.ours.clone(),
                ChunkKind::TheirsOnly => self.theirs.clone(),
                ChunkKind::Conflict => return None,
            },
        })
    }
}

/// Marker parse error (line number starts at 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&I18N.f("line {line}: {msg}", &[("line", &self.line), ("msg", &self.msg)]))
    }
}

/// Line terminator of the text and whether there is a trailing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Eol {
    pub crlf: bool,
    pub trailing: bool,
}

/// Lines without terminators + terminator style (by majority).
pub fn split(text: &str) -> (Vec<String>, Eol) {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count();
    let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).collect();
    let trailing = text.ends_with('\n');
    if trailing || text.is_empty() {
        lines.pop();
    }
    (lines, Eol { crlf: crlf * 2 > lf, trailing })
}

/// Assembles text from lines.
pub fn join(lines: &[String], eol: Eol) -> String {
    let nl = if eol.crlf { "\r\n" } else { "\n" };
    let mut s = lines.join(nl);
    if eol.trailing && !lines.is_empty() {
        s.push_str(nl);
    }
    s
}

fn marker(line: &str, c: char) -> bool {
    let n = line.chars().take_while(|&x| x == c).count();
    n >= 7 && line[n..].chars().next().is_none_or(|x| x == ' ')
}

/// Parses conflict markers: `<<<<<<<`, optional `|||||||` (diff3), `=======`, `>>>>>>>`.
pub fn parse_conflicts(text: &str) -> Result<(Vec<Chunk>, Eol), ParseError> {
    #[derive(PartialEq)]
    enum St {
        Normal,
        Ours,
        Base,
        Theirs,
    }
    let (lines, eol) = split(text);
    let mut chunks = Vec::new();
    let mut stable = Vec::new();
    let (mut ours, mut base, mut theirs): (Vec<String>, Option<Vec<String>>, Vec<String>) = (vec![], None, vec![]);
    let mut st = St::Normal;
    let mut open_at = 0;
    for (i, l) in lines.into_iter().enumerate() {
        match st {
            St::Normal if marker(&l, '<') => {
                if !stable.is_empty() {
                    chunks.push(Chunk::stable(std::mem::take(&mut stable)));
                }
                st = St::Ours;
                open_at = i + 1;
            }
            St::Normal => stable.push(l),
            St::Ours | St::Base | St::Theirs if marker(&l, '<') => {
                return Err(ParseError {
                    line: i + 1,
                    msg: I18N.f("nested conflict marker (opened at line {open})", &[("open", &open_at)]),
                })
            }
            St::Ours if marker(&l, '|') => {
                st = St::Base;
                base = Some(Vec::new());
            }
            St::Ours | St::Base if marker(&l, '=') => st = St::Theirs,
            St::Ours => ours.push(l),
            St::Base => base.get_or_insert_with(Vec::new).push(l),
            St::Theirs if marker(&l, '>') => {
                chunks.push(Chunk {
                    kind: ChunkKind::Conflict,
                    base: base.take(),
                    ours: std::mem::take(&mut ours),
                    theirs: std::mem::take(&mut theirs),
                    resolution: None,
                });
                st = St::Normal;
            }
            St::Theirs => theirs.push(l),
        }
    }
    if st != St::Normal {
        return Err(ParseError {
            line: open_at,
            msg: I18N.t("conflict marker is not closed").into_owned(),
        });
    }
    if !stable.is_empty() {
        chunks.push(Chunk::stable(stable));
    }
    Ok((chunks, eol))
}

/// Changed span of a side in base coordinates: `[bs, be)` to `[ss, se)`.
#[derive(Debug, Clone, Copy)]
struct Region {
    bs: usize,
    be: usize,
    ss: usize,
    se: usize,
}

fn regions(base: &[&str], side: &[&str]) -> Vec<Region> {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .timeout(crate::diff::TIMEOUT)
        .diff_slices(base, side);
    let mut out: Vec<Region> = Vec::new();
    for op in diff.ops() {
        if matches!(op, DiffOp::Equal { .. }) {
            continue;
        }
        let (o, n) = (op.old_range(), op.new_range());
        match out.last_mut() {
            Some(r) if r.be == o.start && r.se == n.start => {
                r.be = o.end;
                r.se = n.end;
            }
            _ => out.push(Region {
                bs: o.start,
                be: o.end,
                ss: n.start,
                se: n.end,
            }),
        }
    }
    out
}

/// Three-way merge: non-overlapping changes are applied, identical ones become `Both`,
/// overlapping (and adjacent) ones become `Conflict`.
pub fn merge3(base: &str, ours: &str, theirs: &str) -> (Vec<Chunk>, Eol) {
    let (b, _) = split(base);
    let (o, eol) = split(ours);
    let (t, _) = split(theirs);
    let br: Vec<&str> = b.iter().map(String::as_str).collect();
    let or: Vec<&str> = o.iter().map(String::as_str).collect();
    let tr: Vec<&str> = t.iter().map(String::as_str).collect();
    let ra = regions(&br, &or);
    let rb = regions(&br, &tr);
    let off = |rs: &[Region], k: usize| -> isize { rs[..k].iter().map(|r| (r.se - r.ss) as isize - (r.be - r.bs) as isize).sum() };
    let at = |p: usize, d: isize| (p as isize + d) as usize;
    let mut chunks = Vec::new();
    let (mut ia, mut ib, mut pos) = (0usize, 0usize, 0usize);
    let push_stable = |chunks: &mut Vec<Chunk>, from: usize, to: usize| {
        if to > from {
            chunks.push(Chunk::stable(b[from..to].to_vec()));
        }
    };
    while ia < ra.len() || ib < rb.len() {
        let start = match (ra.get(ia), rb.get(ib)) {
            (Some(x), Some(y)) => x.bs.min(y.bs),
            (Some(x), None) => x.bs,
            (None, Some(y)) => y.bs,
            (None, None) => unreachable!(),
        };
        push_stable(&mut chunks, pos, start);
        let (a0, b0) = (ia, ib);
        let mut hi = start;
        loop {
            let mut grew = false;
            if let Some(r) = ra.get(ia) {
                if r.bs <= hi {
                    hi = hi.max(r.be);
                    ia += 1;
                    grew = true;
                }
            }
            if let Some(r) = rb.get(ib) {
                if r.bs <= hi {
                    hi = hi.max(r.be);
                    ib += 1;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        let side = |rs: &[Region], k0: usize, k1: usize, lines: &[String]| -> Vec<String> {
            let lo = at(start, off(rs, k0));
            let h = at(hi, off(rs, k1));
            lines[lo..h].to_vec()
        };
        let base_part = b[start..hi].to_vec();
        let ours_part = side(&ra, a0, ia, &o);
        let theirs_part = side(&rb, b0, ib, &t);
        let kind = match (ia > a0, ib > b0) {
            (true, false) => ChunkKind::OursOnly,
            (false, true) => ChunkKind::TheirsOnly,
            _ if ours_part == theirs_part => ChunkKind::Both,
            _ => ChunkKind::Conflict,
        };
        chunks.push(Chunk {
            kind,
            base: Some(base_part),
            ours: ours_part,
            theirs: theirs_part,
            resolution: None,
        });
        pos = hi;
    }
    push_stable(&mut chunks, pos, b.len());
    (chunks, eol)
}

/// Assembles the result: unresolved conflicts become markers. Returns the text and the number of unresolved ones.
pub fn build(chunks: &[Chunk], eol: Eol) -> (String, usize) {
    let mut lines = Vec::new();
    let mut unresolved = 0;
    for c in chunks {
        match c.result() {
            Some(v) => lines.extend(v),
            None => {
                unresolved += 1;
                lines.push("<<<<<<< ours".to_owned());
                lines.extend(c.ours.iter().cloned());
                if let Some(b) = &c.base {
                    lines.push("||||||| base".to_owned());
                    lines.extend(b.iter().cloned());
                }
                lines.push("=======".to_owned());
                lines.extend(c.theirs.iter().cloned());
                lines.push(">>>>>>> theirs".to_owned());
            }
        }
    }
    (join(&lines, eol), unresolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflicts(c: &[Chunk]) -> usize {
        c.iter().filter(|c| c.kind == ChunkKind::Conflict).count()
    }

    #[test]
    fn parse_two_way_and_diff3() {
        let t = "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> br\nb\n<<<<<<< HEAD\nx\n||||||| base\nbase\n=======\ny\n>>>>>>> br\n";
        let (c, eol) = parse_conflicts(t).unwrap();
        assert_eq!(conflicts(&c), 2);
        assert_eq!(c[1].ours, vec!["ours"]);
        assert_eq!(c[1].base, None);
        assert_eq!(c[3].base, Some(vec!["base".to_owned()]));
        assert!(eol.trailing && !eol.crlf);
        let (out, n) = build(&c, eol);
        assert_eq!(n, 2);
        assert!(out.contains("||||||| base\nbase\n"));
    }

    #[test]
    fn parse_unclosed_and_crlf() {
        let e = parse_conflicts("a\n<<<<<<< HEAD\nx\n=======\ny\n").unwrap_err();
        assert_eq!(e.line, 2);
        let (mut c, eol) = parse_conflicts("a\r\n<<<<<<< HEAD\r\nx\r\n=======\r\ny\r\n>>>>>>> b\r\n").unwrap();
        assert!(eol.crlf);
        assert_eq!(c[1].ours, vec!["x"]);
        c[1].resolution = Some(Resolution::Theirs);
        assert_eq!(build(&c, eol), ("a\r\ny\r\n".to_owned(), 0));
    }

    #[test]
    fn three_way_merge_small() {
        let base = "1\n2\n3\n4\n5\n";
        // non-overlapping changes merge on their own
        let (c, eol) = merge3(base, "ONE\n2\n3\n4\n5\n", "1\n2\n3\n4\nFIVE\n");
        assert_eq!(conflicts(&c), 0);
        assert_eq!(build(&c, eol).0, "ONE\n2\n3\n4\nFIVE\n");
        // identical ones: Both
        let (c, _) = merge3(base, "1\n2\nX\n4\n5\n", "1\n2\nX\n4\n5\n");
        assert!(c.iter().any(|c| c.kind == ChunkKind::Both));
        // overlapping ones: Conflict; theirs and ours-then-theirs
        let (mut c, eol) = merge3(base, "1\n2\nO\n4\n5\n", "1\n2\nT\n4\n5\n");
        assert_eq!(conflicts(&c), 1);
        let k = c.iter().position(|c| c.kind == ChunkKind::Conflict).unwrap();
        c[k].resolution = Some(Resolution::Theirs);
        assert_eq!(build(&c, eol).0, "1\n2\nT\n4\n5\n");
        c[k].resolution = Some(Resolution::OursThenTheirs);
        assert_eq!(build(&c, eol).0, "1\n2\nO\nT\n4\n5\n");
        // empty base
        let (c, _) = merge3("", "a\n", "b\n");
        assert_eq!(conflicts(&c), 1);
    }
}
