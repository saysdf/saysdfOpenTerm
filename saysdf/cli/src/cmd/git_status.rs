//! `saysdf git-status --json [<dir>]` — repository status as one line of JSON (git-integration D3).
use crate::git::{self, Git};
use clap::Args;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Args)]
pub struct GitStatusArgs {
    /// Output one line of JSON (the only format; the flag is for clarity)
    #[arg(long)]
    json: bool,
    /// Do not count untracked files (`-uno`, for large repositories)
    #[arg(long)]
    no_untracked: bool,
    /// Instead of the status, list submodules with unpushed commits (before push)
    #[arg(long)]
    submodules_push_check: bool,
    /// Directory (default: the current one)
    dir: Option<PathBuf>,
}

pub fn run(args: GitStatusArgs) -> i32 {
    let _ = args.json;
    let start = Instant::now();
    let dir = args.dir.unwrap_or_else(|| PathBuf::from("."));
    match collect(&dir, args.no_untracked, args.submodules_push_check) {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                if !obj.is_empty() {
                    obj.insert("ms".into(), json!(start.elapsed().as_millis() as u64));
                }
            }
            println!("{v}");
            0
        }
        Err(e @ git::Error::NotFound) => {
            eprintln!("saysdf git-status: {e}");
            2
        }
        Err(e) => {
            eprintln!("saysdf git-status: {e}");
            1
        }
    }
}

fn collect(dir: &std::path::Path, no_untracked: bool, push_check: bool) -> Result<Value, git::Error> {
    let Some(root) = git::toplevel(dir)? else {
        return Ok(json!({}));
    };
    if push_check {
        let list: Vec<Value> = git::unpushed_submodules(std::path::Path::new(&root))?
            .into_iter()
            .map(|(path, sha)| json!({ "path": path, "sha": sha }))
            .collect();
        return Ok(json!({ "root": root, "unpushed": list }));
    }
    let Some(st) = status(dir, no_untracked)? else {
        return Ok(json!({}));
    };
    Ok(json!({
        "root": root,
        "branch": st.branch,
        "oid": st.oid,
        "detached": st.detached,
        "upstream": st.upstream,
        "ahead": st.ahead,
        "behind": st.behind,
        "staged": st.staged,
        "modified": st.modified,
        "untracked": st.untracked,
        "conflicts": st.conflicts,
        "submodules_dirty": st.submodules_dirty,
        "dirty": st.is_dirty(),
    }))
}

/// `git status --porcelain=v2` of a directory; `Ok(None)` — git returned an error (message on stderr).
fn status(dir: &Path, no_untracked: bool) -> Result<Option<git::Status>, git::Error> {
    let mut a = vec!["status", "--porcelain=v2", "--branch", "-z", "--ignore-submodules=dirty"];
    if no_untracked {
        a.push("-uno");
    }
    let out = Git::safe().dir(dir).run(&a)?;
    if !out.success() {
        eprintln!("saysdf git-status: {}", out.stderr_str());
        return Ok(None);
    }
    Ok(Some(git::parse_status(&out.stdout)))
}

/// git mark of a file/folder (file-browser D3). The order is the priority for ancestor folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    Untracked,
    Added,
    Modified,
    Deleted,
    Conflict,
}

impl Mark {
    fn from_kind(k: char) -> Self {
        match k {
            'A' => Mark::Added,
            'D' => Mark::Deleted,
            'U' => Mark::Conflict,
            '?' => Mark::Untracked,
            _ => Mark::Modified,
        }
    }
}

/// git marks by absolute path.
pub type Marks = HashMap<PathBuf, Mark>;

/// Marks of the paths of the repository containing `dir`: the file and every ancestor folder up
/// to the root (a folder gets the highest mark of its descendants). `Ok(None)` — not a
/// repository. Same code as `saysdf git-status`.
pub fn status_map(dir: &Path) -> Result<Option<(PathBuf, Marks)>, git::Error> {
    let Some(root) = git::toplevel(dir)? else {
        return Ok(None);
    };
    let root = PathBuf::from(root);
    let Some(st) = status(dir, false)? else {
        return Ok(None);
    };
    Ok(Some((root.clone(), marks_from(&root, &st))))
}

fn marks_from(root: &Path, st: &git::Status) -> HashMap<PathBuf, Mark> {
    let mut map: HashMap<PathBuf, Mark> = HashMap::new();
    for (p, k) in st.paths.iter().zip(&st.kinds) {
        let mark = Mark::from_kind(*k);
        let mut full = root.to_path_buf();
        for part in p.split('/').filter(|s| !s.is_empty()) {
            full.push(part);
        }
        let mut cur: &Path = &full;
        loop {
            let e = map.entry(cur.to_path_buf()).or_insert(mark);
            if *e < mark {
                *e = mark;
            }
            if cur == root {
                break;
            }
            match cur.parent() {
                Some(par) if par.starts_with(root) => cur = par,
                _ => break,
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_bubble_to_ancestors() {
        let st = git::Status {
            paths: vec!["src/a.rs".into(), "src/x/b.rs".into(), "new/".into()],
            kinds: vec!['M', 'U', '?'],
            ..Default::default()
        };
        let root = Path::new("/r");
        let m = marks_from(root, &st);
        assert_eq!(m[&root.join("src").join("a.rs")], Mark::Modified);
        assert_eq!(m[&root.join("src")], Mark::Conflict);
        assert_eq!(m[&root.join("new")], Mark::Untracked);
        assert_eq!(m[root], Mark::Conflict);
    }
}
