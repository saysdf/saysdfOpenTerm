//! Tests of the `saysdf.quickselect` module (change quick-select).
use super::*;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf", get("saysdf").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.paths", get("saysdf.paths").unwrap()),
            ("saysdf.quickselect", get("saysdf.quickselect").unwrap()),
        ]
        .into_boxed_slice(),
    );
    let lua = Lua::new();
    {
        let package: Table = lua.globals().get("package").unwrap();
        package.set("path", "").unwrap();
        package.set("cpath", "").unwrap();
    }
    install_searcher_with(&lua, modules).unwrap();
    lua.load("package.loaded.wezterm = { config_builder = function() return {} end }; qs = require('saysdf.quickselect')")
        .exec()
        .unwrap();
    lua
}

fn patterns(l: &Lua, expr: &str) -> Vec<regex::Regex> {
    let v: Vec<String> = l.load(format!("return {expr}")).eval().unwrap();
    v.iter().map(|p| regex::Regex::new(p).unwrap()).collect()
}

/// Matches: group 1 if present, else the whole match (as quick select does).
fn found(rs: &[regex::Regex], line: &str) -> Vec<String> {
    let mut out = vec![];
    for r in rs {
        for c in r.captures_iter(line) {
            out.push(c.get(1).or(c.get(0)).unwrap().as_str().to_string());
        }
    }
    out
}

#[test]
fn patterns_find_paths_hashes_ips() {
    let l = lua();
    let rs = patterns(&l, "qs.patterns()");
    for (line, want) in [
        ("--> src/main.rs:10:5", "src/main.rs:10:5"),
        ("abc1234 fix bug", "abc1234"),
        ("host 10.0.0.1 up", "10.0.0.1"),
        ("listen 192.168.1.5:8080 now", "192.168.1.5:8080"),
    ] {
        let got = found(&rs, line);
        assert!(got.iter().any(|g| g == want), "{line:?}: want {want:?}, got {got:?}");
    }
}

#[test]
fn setup_user_patterns_first_and_keeps_existing() {
    let l = lua();
    let v: Vec<String> = l
        .load(
            "local c = { quick_select_patterns = { 'OLD' } } \
             qs.setup({ patterns = { 'MINE' }, open_key = false }, c) return c.quick_select_patterns",
        )
        .eval()
        .unwrap();
    assert_eq!(&v[..2], ["OLD", "MINE"]);
    let n: usize = l.load("return #qs.patterns()").eval().unwrap();
    assert_eq!(v.len(), 2 + n);
    // paths come before IPs and hashes
    assert!(v[2].contains("A-Za-z"), "{}", v[2]);
    assert!(v[v.len() - 1].contains("0-9a-f"));
}

#[test]
fn decide_and_open_selected() {
    let l = lua();
    for (text, want) in [
        ("https://x", "url"),
        ("http://a.b/c?d=1", "url"),
        ("src/a.rs:3", "path"),
        ("main.rs:10", "path"),
        (r"C:\a\b.rs:3", "path"),
        ("file:///etc/passwd", "path"),
        ("10.0.0.1", "copy"),
        ("10.0.0.1:8080", "copy"),
        ("abc1234def", "copy"),
        ("ssh://h/x", "copy"),
    ] {
        let got: String = l.load(format!("return qs.decide({text:?})")).eval().unwrap();
        assert_eq!(got, want, "{text}");
    }
    let r: String = l
        .load(
            "local log = {} \
             local d = { open_with = function(u) table.insert(log, 'b:'..u) end, \
               open_path = function(t) table.insert(log, 'p:'..t) end, \
               toast = function(m) table.insert(log, 't') end } \
             for _, t in ipairs({ 'https://x', 'src/a.rs:3', '10.0.0.1', 'file:///etc/passwd' }) do \
               qs.open_selected(t, d) end \
             return table.concat(log, ',')",
        )
        .eval()
        .unwrap();
    assert_eq!(r, "b:https://x,p:src/a.rs:3,t,p:file:///etc/passwd");
}

#[test]
fn toast_text_follows_language() {
    let l = lua();
    let run = |lang: &str| -> String {
        l.load(format!(
            "require('saysdf.i18n').select({lang:?})              local msg              qs.open_selected('10.0.0.1', {{ toast = function(m) msg = m end }})              return msg"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(run("en"), "Copied, cannot be opened: 10.0.0.1");
    // The Russian UI is unchanged by the migration
    assert_eq!(run("ru"), "скопировано, не открывается: 10.0.0.1"); // i18n-allow-cyrillic
}
