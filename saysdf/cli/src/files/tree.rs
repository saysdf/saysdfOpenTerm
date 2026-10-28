//! File browser tree model (file-browser D2): a flat list of visible nodes, lazy folder reading
//! on expansion, sorting (folders first, case-insensitive name), a [`MAX_ENTRIES`] limit per
//! folder, symlinks are shown but not expanded (no cycles), the filter is a case-insensitive name
//! substring over the nodes already read. No terminal involved — testable.

use crate::cmd::git_status::Mark;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-files");

pub const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    /// A link (symlink/junction); `dir` — the target is a folder. Not expanded.
    Link { dir: bool },
    /// The "…truncated" row after [`MAX_ENTRIES`].
    Truncated,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub kind: Kind,
    pub expanded: bool,
    /// The folder could not be read (no access, etc.).
    pub denied: bool,
}

pub struct Tree {
    pub root: PathBuf,
    pub nodes: Vec<Node>,
    pub show_hidden: bool,
    pub filter: String,
    pub git: HashMap<PathBuf, Mark>,
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Folder contents: (nodes, truncated?). A read error is `Err`.
pub fn read_children(dir: &Path, depth: usize, show_hidden: bool) -> std::io::Result<Vec<Node>> {
    read_limited(dir, depth, show_hidden, MAX_ENTRIES)
}

fn read_limited(dir: &Path, depth: usize, show_hidden: bool, limit: usize) -> std::io::Result<Vec<Node>> {
    let mut out = Vec::new();
    let mut truncated = false;
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if !show_hidden && is_hidden(&name) {
            continue;
        }
        if out.len() >= limit {
            truncated = true;
            break;
        }
        let path = e.path();
        // symlink_metadata: do not resolve the link when determining the kind
        let kind = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => Kind::Link { dir: path.is_dir() },
            Ok(m) if m.is_dir() => Kind::Dir,
            _ => Kind::File,
        };
        out.push(Node { path, name, depth, kind, expanded: false, denied: false });
    }
    out.sort_by(|a, b| {
        let da = matches!(a.kind, Kind::Dir | Kind::Link { dir: true });
        let db = matches!(b.kind, Kind::Dir | Kind::Link { dir: true });
        db.cmp(&da).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name))
    });
    if truncated {
        out.push(Node {
            path: dir.join("\u{2026}"),
            name: I18N.t("…truncated").into_owned(),
            depth,
            kind: Kind::Truncated,
            expanded: false,
            denied: false,
        });
    }
    Ok(out)
}

impl Tree {
    pub fn new(root: &Path, show_hidden: bool) -> std::io::Result<Self> {
        let nodes = read_children(root, 0, show_hidden)?;
        Ok(Self { root: root.to_path_buf(), nodes, show_hidden, filter: String::new(), git: HashMap::new() })
    }

    /// Expand folder `i` (symlinks and files are not expanded). `false` — nothing to expand.
    pub fn expand(&mut self, i: usize) -> bool {
        let Some(n) = self.nodes.get(i) else { return false };
        if n.kind != Kind::Dir || n.expanded {
            return false;
        }
        match read_children(&n.path, n.depth + 1, self.show_hidden) {
            Ok(children) => {
                self.nodes[i].expanded = true;
                self.nodes[i].denied = false;
                let tail = self.nodes.split_off(i + 1);
                self.nodes.extend(children);
                self.nodes.extend(tail);
            }
            Err(_) => self.nodes[i].denied = true,
        }
        true
    }

    /// Collapse folder `i`: remove its descendants.
    pub fn collapse(&mut self, i: usize) -> bool {
        let Some(n) = self.nodes.get(i) else { return false };
        if !n.expanded {
            return false;
        }
        let d = n.depth;
        let end = self.nodes[i + 1..].iter().position(|x| x.depth <= d).map_or(self.nodes.len(), |p| i + 1 + p);
        self.nodes.drain(i + 1..end);
        self.nodes[i].expanded = false;
        true
    }

    /// Index of the parent of node `i` in the list.
    pub fn parent(&self, i: usize) -> Option<usize> {
        let d = self.nodes.get(i)?.depth;
        (0..i).rev().find(|&j| self.nodes[j].depth + 1 == d && self.nodes[j].depth < d)
    }

    /// Re-read keeping the expanded folders (after an operation or on F5). Vanished nodes
    /// disappear; so do expanded folders that no longer exist.
    pub fn refresh(&mut self) -> std::io::Result<()> {
        let open: Vec<PathBuf> = self.nodes.iter().filter(|n| n.expanded).map(|n| n.path.clone()).collect();
        self.nodes = read_children(&self.root, 0, self.show_hidden)?;
        let mut i = 0;
        while i < self.nodes.len() {
            if open.contains(&self.nodes[i].path) {
                self.expand(i);
            }
            i += 1;
        }
        Ok(())
    }

    /// Change the root: expanded folders are reset.
    pub fn set_root(&mut self, root: &Path) -> std::io::Result<()> {
        let nodes = read_children(root, 0, self.show_hidden)?;
        self.root = root.to_path_buf();
        self.nodes = nodes;
        self.git.clear();
        Ok(())
    }

    pub fn toggle_hidden(&mut self) -> std::io::Result<()> {
        self.show_hidden = !self.show_hidden;
        self.refresh()
    }

    /// Indices of visible nodes: without a filter — all; with a filter — those matching by name and their ancestors.
    pub fn visible(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.nodes.len()).collect();
        }
        let q = self.filter.to_lowercase();
        let mut keep = vec![false; self.nodes.len()];
        // ancestor stack: (depth, index)
        let mut stack: Vec<usize> = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            while stack.last().is_some_and(|&j| self.nodes[j].depth >= n.depth) {
                stack.pop();
            }
            if n.kind != Kind::Truncated && n.name.to_lowercase().contains(&q) {
                keep[i] = true;
                for &j in &stack {
                    keep[j] = true;
                }
            }
            stack.push(i);
        }
        (0..self.nodes.len()).filter(|&i| keep[i]).collect()
    }

    /// Git mark of the node.
    pub fn mark(&self, i: usize) -> Option<Mark> {
        self.git.get(&self.nodes.get(i)?.path).copied()
    }

    /// Index of the node by path.
    pub fn find(&self, path: &Path) -> Option<usize> {
        self.nodes.iter().position(|n| n.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn names(t: &Tree) -> Vec<String> {
        t.visible().iter().map(|&i| format!("{}{}", "  ".repeat(t.nodes[i].depth), t.nodes[i].name)).collect()
    }

    fn sample() -> (tempfile::TempDir, Tree) {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("src/inner")).unwrap();
        fs::write(d.path().join("src/main.rs"), "").unwrap();
        fs::write(d.path().join("src/inner/x.rs"), "").unwrap();
        fs::write(d.path().join("B.txt"), "").unwrap();
        fs::write(d.path().join("a.txt"), "").unwrap();
        fs::write(d.path().join(".hidden"), "").unwrap();
        fs::create_dir(d.path().join("Docs")).unwrap();
        let t = Tree::new(d.path(), false).unwrap();
        (d, t)
    }

    #[test]
    fn sort_expand_collapse_hidden_filter() {
        let (_d, mut t) = sample();
        assert_eq!(names(&t), ["Docs", "src", "a.txt", "B.txt"]);
        assert!(t.expand(1));
        assert_eq!(names(&t), ["Docs", "src", "  inner", "  main.rs", "a.txt", "B.txt"]);
        assert!(t.expand(2));
        assert_eq!(t.parent(3), Some(2));
        assert_eq!(t.parent(4), Some(1));
        t.filter = "X.R".into();
        assert_eq!(names(&t), ["src", "  inner", "    x.rs"]);
        t.filter.clear();
        assert!(t.collapse(1));
        assert_eq!(names(&t), ["Docs", "src", "a.txt", "B.txt"]);
        t.expand(1);
        t.toggle_hidden().unwrap();
        assert!(names(&t).contains(&".hidden".to_string()));
        // refresh keeps the expanded ones
        assert!(t.nodes[t.find(&t.root.join("src")).unwrap()].expanded);
        assert!(!t.expand(t.find(&t.root.join("a.txt")).unwrap()));
    }

    #[test]
    fn truncated_mark() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..5 {
            fs::write(d.path().join(format!("f{i}")), "").unwrap();
        }
        let n = read_limited(d.path(), 0, false, 3).unwrap();
        assert_eq!(n.len(), 4);
        assert_eq!(n[3].kind, Kind::Truncated);
        assert_eq!(read_limited(d.path(), 0, false, 5).unwrap().len(), 5);
    }

    #[test]
    fn deleted_dir_disappears_on_refresh() {
        let (d, mut t) = sample();
        t.expand(1);
        fs::remove_dir_all(d.path().join("src")).unwrap();
        t.refresh().unwrap();
        assert_eq!(names(&t), ["Docs", "a.txt", "B.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loop_not_expanded() {
        let (d, mut t) = sample();
        std::os::unix::fs::symlink(d.path(), d.path().join("src/up")).unwrap();
        t.expand(1);
        let i = t.find(&d.path().join("src/up")).unwrap();
        assert_eq!(t.nodes[i].kind, Kind::Link { dir: true });
        assert!(!t.expand(i));
    }
}
