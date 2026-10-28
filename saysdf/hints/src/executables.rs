use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

pub const MAX_PER_DIR: usize = 5_000;
pub const MAX_TOTAL: usize = 20_000;
pub const RECHECK_INTERVAL: Duration = Duration::from_secs(60);
const MAX_CACHED_PATHS: usize = 8;

#[cfg(unix)]
fn executable_name(path: &Path, name: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.permissions().mode() & 0o111 != 0).then(|| name.to_owned())
}

#[cfg(windows)]
fn executable_name(path: &Path, name: &str) -> Option<String> {
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
    let ext = path.extension()?.to_str()?;
    let known = pathext
        .split(';')
        .filter_map(|e| e.strip_prefix('.'))
        .any(|e| e.eq_ignore_ascii_case(ext));
    (known && path.is_file()).then(|| name[..name.len() - ext.len() - 1].to_owned())
}

#[cfg(not(any(unix, windows)))]
fn executable_name(_path: &Path, _name: &str) -> Option<String> {
    None
}

fn scan(dirs: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in rd.flatten().take(MAX_PER_DIR) {
            let name = e.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.chars().any(char::is_control) {
                continue;
            }
            if let Some(n) = executable_name(&e.path(), name) {
                out.push(n);
            }
            if out.len() >= MAX_TOTAL {
                break;
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn mtimes(dirs: &[PathBuf]) -> Vec<Option<SystemTime>> {
    dirs.iter()
        .map(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok())
        .collect()
}

struct Cached {
    mtimes: Vec<Option<SystemTime>>,
    checked: Instant,
    names: Arc<Vec<String>>,
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

pub fn executables(path_var: &str) -> Arc<Vec<String>> {
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache.get(path_var) {
        if c.checked.elapsed() < RECHECK_INTERVAL {
            return Arc::clone(&c.names);
        }
    }
    let dirs: Vec<PathBuf> = std::env::split_paths(OsStr::new(path_var))
        .filter(|d| d.is_absolute())
        .collect();
    let now = mtimes(&dirs);
    if let Some(c) = cache.get_mut(path_var) {
        if c.mtimes == now {
            c.checked = Instant::now();
            return Arc::clone(&c.names);
        }
    }
    let names = Arc::new(scan(&dirs));
    if cache.len() >= MAX_CACHED_PATHS && !cache.contains_key(path_var) {
        cache.clear();
    }
    cache.insert(
        path_var.to_owned(),
        Cached {
            mtimes: now,
            checked: Instant::now(),
            names: Arc::clone(&names),
        },
    );
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(dir: &Path, name: &str, exec: bool) {
        let p = dir.join(name);
        std::fs::write(&p, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if exec { 0o755 } else { 0o644 };
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = exec;
    }

    #[test]
    fn scans_path_dirs() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::create_dir(a.path().join("subdir")).unwrap();
        if cfg!(windows) {
            make(a.path(), "tool.exe", true);
            make(b.path(), "Script.CMD", true);
            make(b.path(), "readme.txt", true);
            make(b.path(), "tool.bat", true);
        } else {
            make(a.path(), "tool", true);
            make(b.path(), "Script", true);
            make(b.path(), "readme.txt", false);
            make(b.path(), "tool", true);
        }
        let var = std::env::join_paths([a.path(), b.path(), Path::new("relative")]).unwrap();
        let var = var.to_str().unwrap();
        assert_eq!(executables(var).as_slice(), ["Script", "tool"]);
        assert!(Arc::ptr_eq(&executables(var), &executables(var)));
        assert!(executables("").is_empty());
    }
}
