//! Source checks of change `i18n` (design D8):
//! - every key looked up in Lua (`t`, `tn`, `N_` of `saysdf.i18n.domain`) and Rust (`I18N.t/f/n`)
//!   has an entry in the Russian catalog of its domain, with the same placeholders, and every
//!   catalog entry is used (no orphans);
//! - the fork's code is free of Cyrillic, except the files and the line marker allowed by
//!   `[scan] cyrillic_allow` / `cyrillic_allow_marker` of `saysdf/public/manifest.toml` (locale
//!   files, keyboard layout data, test data lines). The release leak scan
//!   (`saysdf/tools/release-public.ps1`) reads the same allow-list, so there is one definition.
use regex::Regex;
use saysdf_i18n::catalog::{self, Msg};
use saysdf_i18n::format::{placeholders, plural_categories};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Languages whose catalogs must be complete.
const COMPLETE_LANGS: &[&str] = &["ru"];

/// Release manifest with the Cyrillic allow-list shared with the release leak scan.
const MANIFEST: &str = "saysdf/public/manifest.toml";

/// Fork code checked for Cyrillic, relative to the repo root.
const SCAN_DIRS: &[&str] = &["saysdf"];
/// Fork files inside upstream crates are named `saysdf_*` (any depth outside `saysdf/`).
const FORK_PREFIX: &str = "saysdf_";
/// Top-level directories without fork code files.
const NOT_FORK_TOP: &[&str] = &["saysdf", "target", "deps", ".git"];

/// Not the fork's own code: vendored crates, work journals, parked CI files.
const EXCLUDED: &[&str] = &["saysdf/vendor", "saysdf/work", "saysdf/ci"];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            walk(&p, out);
        } else if matches!(p.extension().and_then(|e| e.to_str()), Some("rs" | "lua")) {
            out.push(p);
        }
    }
}

/// Code files of the fork: `.rs`/`.lua` under `saysdf/` and `saysdf_*.rs`/`.lua` in upstream
/// crates (`wezterm-gui/src/termwindow/saysdf_menu.rs`, `window/src/os/x11/saysdf_ime.rs`, ...).
fn code_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    for d in SCAN_DIRS {
        walk(&root.join(d), &mut files);
    }
    for e in std::fs::read_dir(root).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if e.path().is_dir() && !NOT_FORK_TOP.contains(&name.as_str()) {
            let mut all = Vec::new();
            walk(&e.path(), &mut all);
            files.extend(all.into_iter().filter(|p| {
                p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(FORK_PREFIX))
            }));
        }
    }
    let mut v: Vec<_> = files
        .into_iter()
        .map(|p| (rel(root, &p), p))
        .filter(|(r, _)| !EXCLUDED.iter().any(|x| r == x || r.starts_with(&format!("{x}/"))))
        .collect();
    v.sort();
    v
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(c @ ('\\' | '"' | '\'')) => out.push(c),
            Some(c) => {
                out.push('\\');
                out.push(c);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[derive(Default, Debug)]
struct Keys {
    /// key -> files using it
    text: BTreeMap<String, BTreeSet<String>>,
    /// singular key -> (plural English form, files)
    plural: BTreeMap<String, (String, BTreeSet<String>)>,
}

const STR: &str = r#""((?:[^"\\\n]|\\.)*)""#;

/// Strips `--` comments from Lua lines (not inside strings: good enough for our sources,
/// where `--` in strings is rare and only in `t()` keys if never).
fn lua_code(src: &str) -> String {
    src.lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with("--") {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn rust_code(src: &str) -> String {
    src.lines()
        .map(|l| if l.trim_start().starts_with("//") { "" } else { l })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Domain -> keys, from all code files.
fn source_keys(root: &Path) -> (BTreeMap<String, Keys>, Vec<String>) {
    let lua_domain = Regex::new(r#"i18n"\)\.domain\(\s*"([a-z0-9_-]+)"\s*\)"#).unwrap();
    let lua_t = Regex::new(&format!(r#"(?m)(?:^|[^\w.:])(?:t|N_)\(\s*{STR}"#)).unwrap();
    let lua_tn = Regex::new(&format!(r#"(?m)(?:^|[^\w.:])tn\(\s*{STR}\s*,\s*{STR}"#)).unwrap();
    let lua_single = Regex::new(r#"(?m)(?:^|[^\w.:])(?:t|tn|N_)\(\s*'"#).unwrap();
    let rs_domain = Regex::new(r#"Domain::new\(\s*"([a-z0-9_-]+)"\s*\)"#).unwrap();
    let rs_t = Regex::new(&format!(r#"\bI18N\s*\.\s*(?:t|f)\(\s*{STR}"#)).unwrap();
    let rs_n = Regex::new(&format!(r#"\bI18N\s*\.\s*n\(\s*{STR}\s*,\s*{STR}"#)).unwrap();

    let mut out: BTreeMap<String, Keys> = BTreeMap::new();
    let mut errors = Vec::new();
    // The i18n crate itself documents the API with examples; it has no UI strings.
    let skip = ["saysdf/i18n/", "saysdf/lua/saysdf/i18n.lua"];
    for (r, path) in code_files(root) {
        if skip.iter().any(|s| r.starts_with(s)) {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let lua = r.ends_with(".lua");
        let code = if lua { lua_code(&src) } else { rust_code(&src) };
        let (dom_re, t_re, n_re) = if lua {
            (&lua_domain, &lua_t, &lua_tn)
        } else {
            (&rs_domain, &rs_t, &rs_n)
        };
        let domains: BTreeSet<_> = dom_re.captures_iter(&code).map(|c| c[1].to_string()).collect();
        let has_calls = t_re.is_match(&code) || n_re.is_match(&code);
        let domain = match (domains.len(), has_calls) {
            (0, false) => continue,
            (1, _) => domains.into_iter().next().unwrap(),
            (0, true) => {
                errors.push(format!("{r}: translation calls without a domain declaration"));
                continue;
            }
            _ => {
                errors.push(format!("{r}: more than one domain: {domains:?}"));
                continue;
            }
        };
        if lua {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let expect = if stem == "init" { "saysdf".to_string() } else { stem };
            if domain != expect {
                errors.push(format!("{r}: Lua module domain must be {expect:?}, not {domain:?}"));
            }
        }
        if lua && lua_single.is_match(&code) {
            errors.push(format!("{r}: translation keys must be double-quoted literals"));
        }
        let keys = out.entry(domain).or_default();
        for c in t_re.captures_iter(&code) {
            keys.text.entry(unescape(&c[1])).or_default().insert(r.clone());
        }
        for c in n_re.captures_iter(&code) {
            let e = keys
                .plural
                .entry(unescape(&c[1]))
                .or_insert_with(|| (unescape(&c[2]), BTreeSet::new()));
            e.1.insert(r.clone());
        }
    }
    (out, errors)
}

#[test]
fn catalogs_match_sources() {
    let root = repo();
    let (sources, mut errors) = source_keys(&root);
    for lang in COMPLETE_LANGS {
        let mut catalogs: BTreeMap<String, catalog::File> = BTreeMap::new();
        for (l, domain, src) in saysdf_i18n::embedded() {
            if l == lang {
                match catalog::parse(src) {
                    Ok(f) => {
                        catalogs.insert(domain.to_string(), f);
                    }
                    Err(e) => errors.push(format!("{l}/{domain}.toml: {e}")),
                }
            }
        }
        let empty = Keys::default();
        let domains: BTreeSet<&String> = sources.keys().chain(catalogs.keys()).collect();
        for domain in domains {
            let keys = sources.get(domain).unwrap_or(&empty);
            let file = catalogs.get(domain);
            let msgs = file.map(|f| &f.messages);
            let at = format!("saysdf/locales/{lang}/{domain}.toml");
            if file.is_none() {
                errors.push(format!("{at}: missing (domain used in the sources)"));
                continue;
            }
            let msgs = msgs.unwrap();
            for (key, files) in &keys.text {
                if keys.plural.contains_key(key) {
                    errors.push(format!("{domain}: {key:?} is used both with t() and tn()"));
                }
                if let Err(e) = placeholders(key) {
                    errors.push(format!("{files:?}: key {key:?}: {e}"));
                }
                match msgs.get(key) {
                    None => errors.push(format!("{at}: no entry for {key:?} (used in {files:?})")),
                    Some(Msg::Plural(_)) => errors.push(format!("{at}: {key:?} must be a string, not plural forms")),
                    Some(Msg::Text(tr)) => {
                        if placeholders(tr).ok() != placeholders(key).ok() {
                            errors.push(format!("{at}: {key:?}: placeholders differ from the key"));
                        }
                    }
                }
            }
            for (one, (other, files)) in &keys.plural {
                let src_ph: BTreeSet<String> = placeholders(one)
                    .unwrap_or_default()
                    .union(&placeholders(other).unwrap_or_default())
                    .cloned()
                    .chain(["n".to_string()])
                    .collect();
                match msgs.get(one) {
                    None => errors.push(format!("{at}: no entry for {one:?} (plural, used in {files:?})")),
                    Some(Msg::Text(_)) => errors.push(format!("{at}: {one:?} must be a table of plural forms")),
                    Some(Msg::Plural(forms)) => {
                        for cat in plural_categories(lang) {
                            match forms.get(*cat) {
                                None => errors.push(format!("{at}: {one:?}: no plural form {cat:?}")),
                                Some(tr) => {
                                    let ph = placeholders(tr).unwrap_or_default();
                                    if !ph.is_subset(&src_ph) {
                                        errors.push(format!("{at}: {one:?}.{cat}: unknown placeholders"));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            for key in msgs.keys() {
                if !keys.text.contains_key(key) && !keys.plural.contains_key(key) {
                    errors.push(format!("{at}: orphan entry {key:?} (not used in the sources)"));
                }
            }
        }
    }
    assert!(errors.is_empty(), "\n{}\n", errors.join("\n"));
}

fn has_cyrillic(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}'))
}

/// Glob of the release manifest as a regex, same rules as `ConvertTo-GlobRegex` of
/// `release-public.ps1`: `*`/`?` within a segment, `**` across segments, a pattern without `/`
/// matches a name at any depth, a leading `/` anchors it, a directory match covers its contents.
fn glob_regex(glob: &str) -> Regex {
    let g = glob.trim();
    let rooted = g.starts_with('/');
    let g = g.trim_matches('/');
    let chars: Vec<char> = g.chars().collect();
    let mut re = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    re.push_str("(?:.*/)?");
                    i += 3;
                } else {
                    re.push_str(".*");
                    i += 2;
                }
                continue;
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    let prefix = if rooted || g.contains('/') { "^" } else { "(?:^|/)" };
    Regex::new(&format!("{prefix}{re}(?:/.*)?$")).unwrap()
}

/// `[scan] cyrillic_allow` (as regexes) and `cyrillic_allow_marker` of the release manifest.
fn cyrillic_allow(root: &Path) -> (Vec<Regex>, String) {
    let text = std::fs::read_to_string(root.join(MANIFEST)).unwrap();
    let doc: toml::Table = text.parse().unwrap_or_else(|e| panic!("{MANIFEST}: {e}"));
    let scan = doc["scan"].as_table().expect("[scan]");
    let globs = scan["cyrillic_allow"].as_array().expect("cyrillic_allow");
    let globs = globs.iter().map(|g| glob_regex(g.as_str().unwrap())).collect();
    let marker = scan["cyrillic_allow_marker"].as_str().expect("cyrillic_allow_marker");
    (globs, marker.to_string())
}

#[test]
fn manifest_globs_match_like_the_release_script() {
    let m = |g: &str, p: &str| glob_regex(g).is_match(p);
    assert!(m("**/locales/ru/**", "saysdf/locales/ru/help.md"));
    assert!(m("saysdf/keys/**", "saysdf/keys/src/lib.rs"));
    assert!(m("saysdf/cli/src/keys.rs", "saysdf/cli/src/keys.rs"));
    assert!(!m("saysdf/cli/src/keys.rs", "saysdf/cli/src/keys.rs.bak"));
    assert!(m("shots", "saysdf/x/shots/a.png"));
    assert!(!m("/shots", "saysdf/shots"));
    assert!(!m("saysdf/*.rs", "saysdf/cli/main.rs"));
}

#[test]
fn code_has_no_cyrillic() {
    let root = repo();
    let (allow, marker) = cyrillic_allow(&root);
    let mut errors = Vec::new();
    for (r, path) in code_files(&root) {
        if allow.iter().any(|re| re.is_match(&r)) {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let bad: Vec<usize> = src
            .lines()
            .enumerate()
            .filter(|(_, l)| has_cyrillic(l) && !l.contains(&marker))
            .map(|(i, _)| i + 1)
            .collect();
        if !bad.is_empty() {
            errors.push(format!("{r}: Cyrillic on lines {bad:?} (move UI text to a catalog, translate comments, or mark test data with `{marker}`)"));
        }
    }
    assert!(errors.is_empty(), "\n{}\n", errors.join("\n"));
}
