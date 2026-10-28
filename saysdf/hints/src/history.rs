use crate::Shell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_SESSION_ENTRIES: usize = 1_000;
const MAX_CACHED_FILES: usize = 16;

fn is_usable(cmd: &str) -> bool {
    !cmd.trim().is_empty() && !cmd.chars().any(char::is_control)
}

fn newest_unique(entries: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in entries.into_iter().rev() {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        if is_usable(&e) && seen.insert(e.clone()) {
            out.push(e);
        }
    }
    out
}

fn parse_bash(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| {
            !(l.len() > 1 && l.starts_with('#') && l[1..].bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_owned)
        .collect()
}

fn zsh_command(line: &str) -> &str {
    line.strip_prefix(": ")
        .and_then(|rest| rest.split_once(';'))
        .filter(|(meta, _)| {
            meta.split_once(':').is_some_and(|(ts, dur)| {
                !ts.is_empty()
                    && ts.bytes().all(|b| b.is_ascii_digit())
                    && dur.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .map_or(line, |(_, cmd)| cmd)
}

fn parse_with_continuations(text: &str, cont: char, start: fn(&str) -> &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        let part = match pending.as_mut() {
            Some(p) => {
                p.push('\n');
                p.push_str(line);
                pending.take().unwrap()
            }
            None => start(line).to_owned(),
        };
        if part.ends_with(cont) {
            pending = Some(part[..part.len() - cont.len_utf8()].to_owned());
        } else {
            out.push(part);
        }
    }
    out.extend(pending);
    out
}

fn parse_zsh(text: &str) -> Vec<String> {
    parse_with_continuations(text, '\\', zsh_command)
}

fn parse_pwsh(text: &str) -> Vec<String> {
    parse_with_continuations(text, '`', |l| l)
}

fn fish_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn parse_fish(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("- cmd: "))
        .map(fish_unescape)
        .collect()
}

fn zsh_unmetafy(bytes: Vec<u8>) -> Vec<u8> {
    if !bytes.contains(&0x83) {
        return bytes;
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut it = bytes.into_iter();
    while let Some(b) = it.next() {
        if b == 0x83 {
            if let Some(n) = it.next() {
                out.push(n ^ 0x20);
            }
        } else {
            out.push(b);
        }
    }
    out
}

pub fn parse(shell: Shell, text: &str) -> Vec<String> {
    newest_unique(match shell {
        Shell::Zsh => parse_zsh(text),
        Shell::Fish => parse_fish(text),
        Shell::Pwsh => parse_pwsh(text),
        Shell::Bash | Shell::Plain => parse_bash(text),
    })
}

fn read_tail(path: &Path, len: u64) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let start = len.saturating_sub(MAX_READ_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.take(MAX_READ_BYTES).read_to_end(&mut buf)?;
    if start > 0 {
        let first_line_end = buf
            .iter()
            .position(|&b| b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        buf.drain(..first_line_end);
    }
    Ok(buf)
}

struct Cached {
    shell: Shell,
    mtime: Option<SystemTime>,
    len: u64,
    entries: Arc<Vec<String>>,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

pub fn load(shell: Shell, path: &Path) -> std::io::Result<Arc<Vec<String>>> {
    let meta = std::fs::metadata(path)?;
    let (mtime, len) = (meta.modified().ok(), meta.len());
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache.get(path) {
        if c.shell == shell && c.mtime == mtime && c.len == len {
            return Ok(Arc::clone(&c.entries));
        }
    }
    let mut bytes = read_tail(path, len)?;
    if shell == Shell::Zsh {
        bytes = zsh_unmetafy(bytes);
    }
    let entries = Arc::new(parse(shell, &String::from_utf8_lossy(&bytes)));
    if cache.len() >= MAX_CACHED_FILES && !cache.contains_key(path) {
        cache.clear();
    }
    cache.insert(
        path.to_path_buf(),
        Cached {
            shell,
            mtime,
            len,
            entries: Arc::clone(&entries),
        },
    );
    Ok(entries)
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn default_path(shell: Shell) -> Option<PathBuf> {
    let home = env_dir("HOME").or_else(|| env_dir("USERPROFILE"));
    match shell {
        Shell::Bash | Shell::Plain => home.map(|h| h.join(".bash_history")),
        Shell::Zsh => home.map(|h| h.join(".zsh_history")),
        Shell::Fish => env_dir("XDG_DATA_HOME")
            .or_else(|| home.map(|h| h.join(".local").join("share")))
            .map(|d| d.join("fish").join("fish_history")),
        Shell::Pwsh => {
            #[cfg(windows)]
            let base = env_dir("APPDATA").map(|d| d.join("Microsoft").join("Windows"));
            #[cfg(not(windows))]
            let base = env_dir("XDG_DATA_HOME")
                .or_else(|| home.map(|h| h.join(".local").join("share")))
                .map(|d| d.join("powershell"));
            base.map(|d| d.join("PSReadLine").join("ConsoleHost_history.txt"))
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct SessionHistory {
    entries: VecDeque<String>,
}

impl SessionHistory {
    pub fn push(&mut self, cmd: &str) {
        if cmd.starts_with(' ') || !is_usable(cmd) {
            return;
        }
        self.entries.retain(|e| e != cmd);
        self.entries.push_front(cmd.to_owned());
        self.entries.truncate(MAX_SESSION_ENTRIES);
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_skips_timestamps_and_dedupes_newest_first() {
        let text = "#1700000000\nls\n#1700000001\ngit status\nls\n# comment cmd\n\n";
        assert_eq!(
            parse(Shell::Bash, text),
            ["# comment cmd", "ls", "git status"]
        );
    }

    #[test]
    fn zsh_extended_and_multiline_dropped() {
        let text = ": 1700000000:0;git status\n: 1700000001:3;for i in 1 2; do\\\necho $i\\\ndone\nplain cmd\n: 1700000002:0;a: b;c\n";
        assert_eq!(
            parse(Shell::Zsh, text),
            ["a: b;c", "plain cmd", "git status"]
        );
    }

    #[test]
    fn zsh_metafied_bytes() {
        let mut bytes = b": 1:0;echo ".to_vec();
        for b in "й".bytes() { // i18n-allow-cyrillic
            if b >= 0x83 {
                bytes.extend([0x83, b ^ 0x20]);
            } else {
                bytes.push(b);
            }
        }
        let text = String::from_utf8_lossy(&zsh_unmetafy(bytes)).into_owned();
        assert_eq!(parse(Shell::Zsh, &text), ["echo й"]); // i18n-allow-cyrillic
    }

    #[test]
    fn fish_yaml_and_unescape() {
        let text = "- cmd: git status\n  when: 1700000000\n- cmd: echo a\\\\b\n  when: 1\n  paths:\n    - a\\\\b\n- cmd: echo 1\\necho 2\n  when: 2\n";
        assert_eq!(parse(Shell::Fish, text), ["echo a\\b", "git status"]);
    }

    #[test]
    fn pwsh_backtick_continuation_dropped() {
        let text = "Get-ChildItem\r\nWrite-Host `\r\n  'x'\r\ngit status\r\nGet-ChildItem\r\n";
        assert_eq!(parse(Shell::Pwsh, text), ["Get-ChildItem", "git status"]);
    }

    #[test]
    fn caps_entries() {
        let text: String = (0..MAX_ENTRIES + 50)
            .map(|i| format!("cmd {i}\n"))
            .collect();
        let got = parse(Shell::Bash, &text);
        assert_eq!(got.len(), MAX_ENTRIES);
        assert_eq!(got[0], format!("cmd {}", MAX_ENTRIES + 49));
    }

    #[test]
    fn control_chars_dropped() {
        assert_eq!(parse(Shell::Bash, "ok\nbad\x1b[31m\n"), ["ok"]);
    }

    #[test]
    fn load_reads_tail_and_caches_by_mtime_and_len() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("h");
        let line = "x".repeat(99);
        let big: String = (0..(MAX_READ_BYTES / 100 + 10))
            .map(|_| format!("{line}\n"))
            .collect();
        std::fs::write(&p, format!("old-first\n{big}newest\n")).unwrap();
        let a = load(Shell::Bash, &p).unwrap();
        assert_eq!(a.as_slice(), ["newest", line.as_str()]);
        let b = load(Shell::Bash, &p).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        std::fs::write(&p, "one\ntwo\n").unwrap();
        assert_eq!(load(Shell::Bash, &p).unwrap().as_slice(), ["two", "one"]);
        assert!(load(Shell::Bash, &d.path().join("missing")).is_err());
    }

    #[test]
    fn session_history() {
        let mut s = SessionHistory::default();
        s.push("ls");
        s.push(" secret");
        s.push("a\nb");
        s.push("git status");
        s.push("ls");
        assert_eq!(s.iter().collect::<Vec<_>>(), ["ls", "git status"]);
    }
}
