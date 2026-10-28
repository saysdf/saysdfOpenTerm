//! Pager document model (design D2): view lines + anchors "source line -> view line",
//! substring search (smart-case: case-insensitive if the query has no uppercase letters).

use ratatui::text::Line;

/// Document: ready (escaped and highlighted) view lines.
#[derive(Debug, Default, Clone)]
pub struct Doc {
    pub lines: Vec<Line<'static>>,
    /// `anchors[i]` is the view line for source line `i`; non-decreasing.
    anchors: Vec<usize>,
}

/// Search match: view line and byte range in its text (concatenation of spans).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// Search direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

impl Doc {
    /// View line = source line (code viewing).
    pub fn from_lines(lines: Vec<Line<'static>>) -> Self {
        let anchors = (0..lines.len()).collect();
        Self { lines, anchors }
    }

    /// Arbitrary mapping (Markdown: one source line may span several view lines).
    pub fn with_anchors(lines: Vec<Line<'static>>, mut anchors: Vec<usize>) -> Self {
        // Non-decreasing order is needed for binary search; values are clamped
        let max = lines.len().saturating_sub(1);
        let mut prev = 0;
        for a in &mut anchors {
            *a = (*a).clamp(prev, max);
            prev = *a;
        }
        Self { lines, anchors }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// View line for a source line (clamped past the end).
    pub fn view_line(&self, source_line: usize) -> usize {
        match self.anchors.get(source_line) {
            Some(&v) => v,
            None => self.anchors.last().copied().unwrap_or(0),
        }
    }

    /// Source line a view line belongs to (last anchor <= view).
    pub fn source_line(&self, view_line: usize) -> usize {
        self.anchors
            .partition_point(|&v| v <= view_line)
            .saturating_sub(1)
    }

    /// Text of a view line without styles.
    pub fn line_text(&self, i: usize) -> String {
        self.lines
            .get(i)
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default()
    }

    /// Next/previous match strictly after/before position `from` (`(line, byte)`),
    /// wrapping around the end/start. `from = None` searches from the start (forward) or the end (backward).
    pub fn search(
        &self,
        query: &str,
        from: Option<(usize, usize)>,
        dir: Direction,
    ) -> Option<Match> {
        if query.is_empty() || self.lines.is_empty() {
            return None;
        }
        let n = self.lines.len();
        let case_sensitive = query.chars().any(char::is_uppercase);
        let needle = if case_sensitive {
            query.to_owned()
        } else {
            query.to_lowercase()
        };
        let in_line = |i: usize| find_all(&self.line_text(i), &needle, case_sensitive);
        match dir {
            Direction::Forward => {
                let (line0, after) = match from {
                    Some((l, b)) => (l.min(n - 1), Some(b)),
                    None => (0, None),
                };
                if let Some(m) = in_line(line0)
                    .into_iter()
                    .find(|&(s, _)| after.is_none_or(|b| s > b))
                {
                    return Some(Match { line: line0, start: m.0, end: m.1 });
                }
                // Remaining lines in a circle, then the start of the original line
                for k in 1..=n {
                    let i = (line0 + k) % n;
                    let found = in_line(i);
                    let pick = if i == line0 {
                        found.into_iter().find(|&(s, _)| after.is_none_or(|b| s <= b))
                    } else {
                        found.into_iter().next()
                    };
                    if let Some((s, e)) = pick {
                        return Some(Match { line: i, start: s, end: e });
                    }
                }
                None
            }
            Direction::Backward => {
                let (line0, before) = match from {
                    Some((l, b)) => (l.min(n - 1), Some(b)),
                    None => (n - 1, None),
                };
                if let Some(m) = in_line(line0)
                    .into_iter()
                    .rev()
                    .find(|&(s, _)| before.is_none_or(|b| s < b))
                {
                    return Some(Match { line: line0, start: m.0, end: m.1 });
                }
                for k in 1..=n {
                    let i = (line0 + n - k) % n;
                    let found = in_line(i);
                    let pick = if i == line0 {
                        found.into_iter().rev().find(|&(s, _)| before.is_none_or(|b| s >= b))
                    } else {
                        found.into_iter().next_back()
                    };
                    if let Some((s, e)) = pick {
                        return Some(Match { line: i, start: s, end: e });
                    }
                }
                None
            }
        }
    }

    /// All matches in a view line, for highlighting the found text.
    pub fn matches_in_line(&self, query: &str, line: usize) -> Vec<(usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let case_sensitive = query.chars().any(char::is_uppercase);
        let needle = if case_sensitive {
            query.to_owned()
        } else {
            query.to_lowercase()
        };
        find_all(&self.line_text(line), &needle, case_sensitive)
    }
}

/// Non-overlapping occurrences of `needle` in `hay` (byte ranges in `hay`).
/// Case-insensitive: matches on the lowercased text and maps positions back to the original string
/// (a character's byte length may change when its case changes).
fn find_all(hay: &str, needle: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    if case_sensitive {
        return hay
            .match_indices(needle)
            .map(|(s, m)| (s, s + m.len()))
            .collect();
    }
    let mut lower = String::with_capacity(hay.len());
    // map[j] is the byte of the original string where the character that produced byte j of `lower` starts
    let mut map = Vec::with_capacity(hay.len() + 1);
    for (i, c) in hay.char_indices() {
        for lc in c.to_lowercase() {
            let before = lower.len();
            lower.push(lc);
            map.extend(std::iter::repeat_n(i, lower.len() - before));
        }
    }
    map.push(hay.len());
    let orig_end = |j: usize| {
        // End of the match is the boundary of the next original character
        let start = map[j - 1];
        hay[start..]
            .chars()
            .next()
            .map_or(hay.len(), |c| start + c.len_utf8())
    };
    lower
        .match_indices(needle)
        .map(|(s, m)| (map[s], orig_end(s + m.len())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(lines: &[&str]) -> Doc {
        Doc::from_lines(lines.iter().map(|l| Line::raw(l.to_string())).collect())
    }

    #[test]
    fn search_forward_wraps() {
        let d = doc(&["fn a", "x", "let fn", "fn b"]);
        let m = d.search("fn", None, Direction::Forward).unwrap();
        assert_eq!((m.line, m.start), (0, 0));
        let m = d.search("fn", Some((0, 0)), Direction::Forward).unwrap();
        assert_eq!((m.line, m.start), (2, 4));
        let m = d.search("fn", Some((3, 0)), Direction::Forward).unwrap();
        assert_eq!((m.line, m.start), (0, 0), "wraps around the end");
        assert!(d.search("zzz", None, Direction::Forward).is_none());
    }

    #[test]
    fn search_backward_wraps() {
        let d = doc(&["fn a", "x", "let fn", "fn b"]);
        let m = d.search("fn", Some((2, 4)), Direction::Backward).unwrap();
        assert_eq!((m.line, m.start), (0, 0));
        let m = d.search("fn", Some((0, 0)), Direction::Backward).unwrap();
        assert_eq!((m.line, m.start), (3, 0), "wraps around the start");
        let d = doc(&["only fn here"]);
        let m = d.search("fn", Some((0, 5)), Direction::Backward).unwrap();
        assert_eq!((m.line, m.start), (0, 5), "the only match is found again");
    }

    #[test]
    fn smart_case() {
        let d = doc(&["Foo foo", "ПРИВЕТ привет"]); // i18n-allow-cyrillic
        assert_eq!(d.matches_in_line("foo", 0), vec![(0, 3), (4, 7)]);
        assert_eq!(d.matches_in_line("Foo", 0), vec![(0, 3)]);
        assert_eq!(d.matches_in_line("привет", 1).len(), 2); // i18n-allow-cyrillic
        // İ -> "i̇" (3 bytes lowercased instead of 2): positions stay in the original string
        let d = doc(&["İx x"]);
        assert_eq!(d.matches_in_line("x", 0), vec![(2, 3), (4, 5)]);
    }

    #[test]
    fn anchors_map_both_ways() {
        let lines = (0..5).map(|i| Line::raw(i.to_string())).collect();
        let d = Doc::with_anchors(lines, vec![0, 2, 4]);
        assert_eq!(d.view_line(1), 2);
        assert_eq!(d.view_line(10), 4);
        assert_eq!(d.source_line(3), 1);
        assert_eq!(d.source_line(4), 2);
    }
}
