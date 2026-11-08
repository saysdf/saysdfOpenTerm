// saysdfTerminal: fork file. `wezterm.saysdf_dir_entries(dir, { suffix, limit })`.
//
// Upstream `read_dir`/`glob` return names only, and Lua cannot read file times. This lists regular
// files directly in `dir` with their modification time, newest first. The directory is read on
// a blocking-pool thread (`smol::unblock`), so the GUI thread is not held.
//
// Returns a list of `{ path, name, modified, created, size }`; `modified`/`created` are unix
// seconds (float), `created` is nil where the platform does not provide it. A missing directory
// gives an empty list; any other error gives `nil, message`.
use config::lua::get_or_create_module;
use config::lua::mlua::{self, Lua};
use std::path::Path;
use std::time::SystemTime;

const DEFAULT_LIMIT: usize = 256;
const MAX_LIMIT: usize = 4096;

pub fn register(lua: &Lua) -> anyhow::Result<()> {
    let wezterm_mod = get_or_create_module(lua, "wezterm")?;
    wezterm_mod.set(
        "saysdf_dir_entries",
        lua.create_async_function(saysdf_dir_entries)?,
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirEntry {
    pub path: String,
    pub name: String,
    pub modified: f64,
    pub created: Option<f64>,
    pub size: u64,
}

fn unix_secs(t: SystemTime) -> f64 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// Regular files directly in `dir` (symlinks are not followed and are skipped), names ending
/// in `suffix`, newest first, at most `limit`. A missing directory is an empty list.
pub fn list_dir(dir: &Path, suffix: &str, limit: usize) -> std::io::Result<Vec<DirEntry>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut out = vec![];
    for entry in rd {
        let Ok(entry) = entry else { continue };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.ends_with(suffix) {
            continue;
        }
        // `DirEntry::metadata` does not follow symlinks.
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(path) = entry.path().to_str().map(str::to_string) else {
            continue;
        };
        out.push(DirEntry {
            path,
            name,
            modified: meta.modified().map(unix_secs).unwrap_or(0.0),
            created: meta.created().ok().map(unix_secs),
            size: meta.len(),
        });
    }
    out.sort_by(|a, b| {
        b.modified
            .partial_cmp(&a.modified)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    out.truncate(limit);
    Ok(out)
}

async fn saysdf_dir_entries<'lua>(
    lua: &'lua Lua,
    (dir, opts): (String, Option<mlua::Table<'lua>>),
) -> mlua::Result<(mlua::Value<'lua>, Option<String>)> {
    let (suffix, limit) = match &opts {
        Some(t) => (
            t.get::<_, Option<String>>("suffix")?.unwrap_or_default(),
            t.get::<_, Option<usize>>("limit")?
                .unwrap_or(DEFAULT_LIMIT)
                .min(MAX_LIMIT),
        ),
        None => (String::new(), DEFAULT_LIMIT),
    };
    let result = smol::unblock(move || list_dir(Path::new(&dir), &suffix, limit)).await;
    match result {
        Err(e) => Ok((mlua::Value::Nil, Some(e.to_string()))),
        Ok(entries) => {
            let list = lua.create_table()?;
            for (i, e) in entries.into_iter().enumerate() {
                let t = lua.create_table()?;
                t.set("path", e.path)?;
                t.set("name", e.name)?;
                t.set("modified", e.modified)?;
                t.set("created", e.created)?;
                t.set("size", e.size)?;
                list.set(i + 1, t)?;
            }
            Ok((mlua::Value::Table(list), None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, FileTimes};
    use std::time::Duration;

    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("saysdf-stat-{}-{}", tag, std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn touch(dir: &Path, name: &str, age_secs: u64) {
        let f = File::create(dir.join(name)).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        f.set_times(FileTimes::new().set_modified(t)).unwrap();
    }

    #[test]
    fn filters_suffix_sorts_newest_first_and_skips_dirs() {
        let tmp = Tmp::new("sort");
        touch(&tmp.0, "old.jsonl", 300);
        touch(&tmp.0, "new.jsonl", 10);
        touch(&tmp.0, "mid.jsonl", 100);
        touch(&tmp.0, "other.txt", 1);
        fs::create_dir(tmp.0.join("sub.jsonl")).unwrap();
        let got = list_dir(&tmp.0, ".jsonl", 100).unwrap();
        let names: Vec<_> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["new.jsonl", "mid.jsonl", "old.jsonl"]);
        assert!(got[0].modified > got[1].modified);
    }

    #[test]
    fn limit_caps_and_keeps_newest() {
        let tmp = Tmp::new("limit");
        for i in 0..5u64 {
            touch(&tmp.0, &format!("f{i}.jsonl"), 100 * (i + 1));
        }
        let got = list_dir(&tmp.0, ".jsonl", 2).unwrap();
        let names: Vec<_> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["f0.jsonl", "f1.jsonl"]);
    }

    #[test]
    fn missing_dir_is_empty() {
        let tmp = Tmp::new("missing");
        assert!(list_dir(&tmp.0.join("nope"), "", 10).unwrap().is_empty());
    }
}
