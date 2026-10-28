//! saysdfTerm: working with the git CLI (change git-integration, design D3/D4/D6).
//!
//! Only `git` from PATH, no libgit2/gix. Shared API for the subcommands (`git-status`,
//! `git-run`, later `diff`/`merge` of diff-merge-tool):
//! - [`Git`] — call builder: directory, "safe" polling mode, timeout;
//! - [`Git::run`] — run with captured output (timeout with kill);
//! - [`Git::command`] — a ready `std::process::Command` (e.g. to run with a TTY);
//! - [`parse_status`] — parses `git status --porcelain=v2 --branch -z`.
use std::path::Path;

pub use saysdf_hints::git::{Error, Git};

/// Root of the work tree (`rev-parse --show-toplevel`); `Ok(None)` — not a repository.
pub fn toplevel(dir: &Path) -> Result<Option<String>, Error> {
    let out = Git::safe().dir(dir).run(["rev-parse", "--show-toplevel"])?;
    if !out.success() {
        return Ok(None);
    }
    Ok(Some(out.stdout_str().trim().to_string()))
}

/// Parsed `git status --porcelain=v2 --branch`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Status {
    /// Branch name; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// HEAD SHA; `None` before the first commit (`(initial)`).
    pub oid: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    /// `None` when there is no upstream (no `branch.ab` line).
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub conflicts: u32,
    /// Submodules with changes (a new commit or a dirty tree).
    pub submodules_dirty: u32,
    /// Paths of the changed entries (the new path for a rename).
    pub paths: Vec<String>,
    /// Kind of each entry of `paths` (same order): `M` modified/renamed, `A` added,
    /// `D` deleted, `U` conflict, `?` untracked (file-browser D3).
    pub kinds: Vec<char>,
}

impl Status {
    pub fn is_dirty(&self) -> bool {
        self.staged + self.modified + self.untracked + self.conflicts > 0
    }
}

/// Parses the output of `git status --porcelain=v2 --branch -z`.
pub fn parse_status(raw: &[u8]) -> Status {
    let mut st = Status::default();
    let mut recs = raw.split(|b| *b == 0).map(|r| String::from_utf8_lossy(r).into_owned());
    while let Some(rec) = recs.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (key, val) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" => st.oid = (val != "(initial)").then(|| val.to_string()),
                "branch.head" => {
                    if val == "(detached)" {
                        st.detached = true;
                    } else {
                        st.branch = Some(val.to_string());
                    }
                }
                "branch.upstream" => st.upstream = Some(val.to_string()),
                "branch.ab" => {
                    for part in val.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            st.ahead = n.parse().ok();
                        } else if let Some(n) = part.strip_prefix('-') {
                            st.behind = n.parse().ok();
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = rec.as_bytes()[0];
        match kind {
            b'1' | b'2' => {
                // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
                // 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\0<origPath>
                let fields = if kind == b'1' { 9 } else { 10 };
                let parts: Vec<&str> = rec.splitn(fields, ' ').collect();
                if kind == b'2' {
                    recs.next(); // original path of a rename/copy
                }
                if parts.len() < fields {
                    continue;
                }
                count_xy(&mut st, parts[1], parts[2]);
                st.paths.push(parts[fields - 1].to_string());
                st.kinds.push(kind_of_xy(parts[1]));
            }
            b'u' => {
                st.conflicts += 1;
                if let Some(p) = rec.splitn(11, ' ').nth(10) {
                    st.paths.push(p.to_string());
                    st.kinds.push('U');
                }
            }
            b'?' => {
                st.untracked += 1;
                st.paths.push(rec[2.min(rec.len())..].to_string());
                st.kinds.push('?');
            }
            _ => {}
        }
    }
    st
}

fn kind_of_xy(xy: &str) -> char {
    if xy.contains('D') {
        'D'
    } else if xy.starts_with('A') {
        'A'
    } else {
        'M'
    }
}

fn count_xy(st: &mut Status, xy: &str, sub: &str) {
    let b = xy.as_bytes();
    let (x, y) = (b.first().copied().unwrap_or(b'.'), b.get(1).copied().unwrap_or(b'.'));
    if x != b'.' {
        st.staged += 1;
    }
    if y != b'.' {
        st.modified += 1;
    }
    // <sub>: "N..." — not a submodule; "S<c><m><u>" — a submodule
    let s = sub.as_bytes();
    if s.first() == Some(&b'S') && s[1..].iter().any(|c| *c != b'.') {
        st.submodules_dirty += 1;
    }
}

/// An entry of `git submodule status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    /// ' ' — up to date, '+' — moved, '-' — not initialized, 'U' — conflict.
    pub state: char,
    pub sha: String,
    pub path: String,
}

/// Parses `git submodule status [--recursive]`.
pub fn parse_submodule_status(text: &str) -> Vec<Submodule> {
    text.lines()
        .filter(|l| l.len() > 1)
        .filter_map(|l| {
            let state = l.chars().next()?;
            let rest = &l[state.len_utf8()..];
            let (sha, tail) = rest.split_once(' ')?;
            // the path before " (description)", if there is one
            let path = match tail.rfind(" (") {
                Some(i) if tail.ends_with(')') => &tail[..i],
                _ => tail,
            };
            Some(Submodule {
                state,
                sha: sha.to_string(),
                path: path.to_string(),
            })
        })
        .collect()
}

/// Commit SHA of a submodule from `git ls-tree HEAD <path>` (`160000 commit <sha>\t<path>`).
pub fn parse_ls_tree_commit(text: &str) -> Option<String> {
    let line = text.lines().next()?;
    let (meta, _path) = line.split_once('\t')?;
    let mut it = meta.split(' ');
    let (_mode, kind, sha) = (it.next()?, it.next()?, it.next()?);
    (kind == "commit").then(|| sha.to_string())
}

/// Submodules whose commit in the superproject's HEAD is in no remote branch (local refs only,
/// no network). Returns `(path, sha)`.
pub fn unpushed_submodules(root: &Path) -> Result<Vec<(String, String)>, Error> {
    let out = Git::safe().dir(root).run(["submodule", "status"])?;
    let mut res = Vec::new();
    for sm in parse_submodule_status(&out.stdout_str()) {
        if sm.state == '-' {
            continue;
        }
        let tree = Git::safe()
            .dir(root)
            .run(["ls-tree", "HEAD", "--", sm.path.as_str()])?;
        let Some(sha) = parse_ls_tree_commit(&tree.stdout_str()) else {
            continue;
        };
        let contains = Git::safe()
            .dir(root.join(&sm.path))
            .run(["branch", "-r", "--contains", sha.as_str()])?;
        if !contains.success() || contains.stdout_str().trim().is_empty() {
            res.push((sm.path, sha));
        }
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use saysdf_hints::git::DEFAULT_TIMEOUT;

    fn z(recs: &[&str]) -> Vec<u8> {
        let mut v = Vec::new();
        for r in recs {
            v.extend_from_slice(r.as_bytes());
            v.push(0);
        }
        v
    }

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn clean_with_upstream() {
        let st = parse_status(&z(&[
            &format!("# branch.oid {OID}"),
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
        ]));
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.oid.as_deref(), Some(OID));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (Some(2), Some(3)));
        assert!(!st.detached && !st.is_dirty());
    }

    #[test]
    fn detached_and_no_upstream() {
        let st = parse_status(&z(&[&format!("# branch.oid {OID}"), "# branch.head (detached)"]));
        assert!(st.detached);
        assert_eq!(st.branch, None);
        assert_eq!((st.ahead, st.behind, st.upstream), (None, None, None));
        let init = parse_status(&z(&["# branch.oid (initial)", "# branch.head master"]));
        assert_eq!(init.oid, None);
        assert_eq!(init.branch.as_deref(), Some("master"));
    }

    #[test]
    fn changes_rename_conflict_untracked_submodule() {
        let h = "100644 100644 100644 aaaa bbbb";
        let st = parse_status(&z(&[
            "# branch.head dev",
            &format!("1 M. N... {h} src/a b.rs"),
            &format!("1 .M N... {h} док/файл.md"), // i18n-allow-cyrillic
            &format!("1 MM N... {h} both.txt"),
            &format!("2 R. N... {h} R100 new name.rs"),
            "old name.rs",
            "u UU N... 100644 100644 100644 100644 h1 h2 h3 conflict.txt",
            "? новый файл.txt", // i18n-allow-cyrillic
            "1 .M S.M. 160000 160000 160000 cccc cccc vendor/lib",
            "1 .M S... 160000 160000 160000 cccc cccc vendor/clean",
        ]));
        assert_eq!(st.staged, 3);
        assert_eq!(st.modified, 4);
        assert_eq!(st.conflicts, 1);
        assert_eq!(st.untracked, 1);
        assert_eq!(st.submodules_dirty, 1);
        assert_eq!(
            st.paths,
            [
                "src/a b.rs",
                "док/файл.md", // i18n-allow-cyrillic
                "both.txt",
                "new name.rs",
                "conflict.txt",
                "новый файл.txt", // i18n-allow-cyrillic
                "vendor/lib",
                "vendor/clean"
            ]
        );
    }

    #[test]
    fn submodule_status_and_ls_tree() {
        let sms = parse_submodule_status(&format!(
            " {OID} vendor/a (v1.0)\n+{OID} vendor/b с пробелом (heads/main)\n-{OID} vendor/c\nU{OID} vendor/d\n" // i18n-allow-cyrillic
        ));
        let states: Vec<char> = sms.iter().map(|s| s.state).collect();
        assert_eq!(states, [' ', '+', '-', 'U']);
        assert_eq!(sms[1].path, "vendor/b с пробелом"); // i18n-allow-cyrillic
        assert_eq!(sms[2].path, "vendor/c");
        assert_eq!(
            parse_ls_tree_commit(&format!("160000 commit {OID}\tvendor/a\n")).as_deref(),
            Some(OID)
        );
        assert_eq!(parse_ls_tree_commit("100644 blob abc\tfile"), None);
    }

    /// Integration: a temporary repository (skipped when there is no git).
    #[test]
    fn temp_repo_status() {
        let dir = std::env::temp_dir().join(format!("saysdf-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let g = Git::new().dir(&dir).timeout(Some(DEFAULT_TIMEOUT));
        match g.run(["init", "-q", "-b", "main"]) {
            Err(Error::NotFound) => return,
            r => assert!(r.unwrap().success()),
        }
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::fs::write(dir.join("b.txt"), "y").unwrap();
        g.run(["add", "a.txt"]).unwrap();
        let out = Git::safe()
            .dir(&dir)
            .run(["status", "--porcelain=v2", "--branch", "-z"])
            .unwrap();
        let st = parse_status(&out.stdout);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!((st.staged, st.untracked, st.upstream), (1, 1, None));
    }
}
