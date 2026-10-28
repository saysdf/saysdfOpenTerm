//! Tests of module `saysdf.paths` (change clickable-file-paths, tasks 1.1–3.3).
use super::*;

const FAKE: &str = r#"
    handlers = {}
    warnings = {}
    toasts = {}
    spawned = {}
    viewed = {}
    package.loaded.wezterm = {
        on = function(name, f) handlers[name] = f end,
        log_warn = function(msg) table.insert(warnings, msg) end,
        config_builder = function() return {} end,
        default_hyperlink_rules = function() return { { regex = 'URL', format = '$0' } } end,
        home_dir = '/home/u',
        read_dir = function(p) if dirs[p] then return {} end error('not a dir') end,
        background_child_process = function(args) table.insert(spawned, args) end,
    }
    dirs = {}
    paths = require('saysdf.paths')
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf", get("saysdf").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.paths", get("saysdf.paths").unwrap()),
            ("saysdf.util", get("saysdf.util").unwrap()),
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
    lua.load(FAKE).exec().unwrap();
    lua
}

fn rules(l: &Lua, func: &str) -> Vec<regex::Regex> {
    let pats: Vec<String> = l
        .load(format!(
            "local r = {{}} for _, x in ipairs(paths.{func}()) do \
             table.insert(r, type(x) == 'table' and x.regex or x) end return r"
        ))
        .eval()
        .unwrap();
    pats.iter().map(|p| regex::Regex::new(p).unwrap()).collect()
}

/// All highlighted pieces (group 1) across all rules.
fn matches(rs: &[regex::Regex], line: &str) -> Vec<String> {
    let mut out = vec![];
    for r in rs {
        for c in r.captures_iter(line) {
            out.push(c.get(1).unwrap().as_str().to_string());
        }
    }
    out
}

/// (output line, what must be highlighted, what must not)
const SAMPLES: &[(&str, &[&str], &[&str])] = &[
    ("  --> src/main.rs:10:5", &["src/main.rs:10:5"], &[]),
    ("error[E0425]: cannot find value `x` in this scope", &[], &[]),
    ("a.c:3:5: error: expected ';'", &["a.c:3:5"], &[]),
    ("lib/foo.cpp:120:7: warning: unused variable", &["lib/foo.cpp:120:7"], &[]),
    (r"C:\p\a.cpp(12,5): error C2065: 'x': undeclared", &[r"C:\p\a.cpp(12,5)"], &[]),
    ("src/a.rs:42:fn main() {", &["src/a.rs:42"], &[]),
    ("tests/t.py:12: AssertionError", &["tests/t.py:12"], &[]),
    ("/home/u/app/src/index.js:3:10  error  'x' is not defined  no-undef", &["/home/u/app/src/index.js:3:10"], &[]),
    ("● Update(src/app.rs)", &["src/app.rs"], &[]),
    ("see src/app.rs:42 for details.", &["src/app.rs:42"], &[]),
    (r"C:\a\b.rs:3:4", &[r"C:\a\b.rs:3:4"], &[]),
    (r"\\server\share\dir\x.txt:7", &[r"\\server\share\dir\x.txt:7"], &[]),
    ("--- a/x/y.rs", &["a/x/y.rs"], &[]),
    ("main.rs:10", &["main.rs:10"], &[]),
    ("Read README.md and version 1.2/3.4 on 2026/10/17.", &[], &["1.2/3.4", "README.md", "2026/10/17"]),
    ("~/notes/todo.md:5", &["~/notes/todo.md:5"], &[]),
];

#[test]
fn hyperlink_rules_and_quick_select_match_samples() {
    let l = lua();
    for func in ["hyperlink_rules", "quick_select_patterns"] {
        let rs = rules(&l, func);
        for (line, want, deny) in SAMPLES {
            let got = matches(&rs, line);
            for w in *want {
                assert!(got.iter().any(|g| g == w), "{func}: {line:?}: want {w:?}, got {got:?}");
            }
            for d in *deny {
                assert!(!got.iter().any(|g| g == d), "{func}: {line:?}: deny {d:?}, got {got:?}");
            }
            if want.is_empty() && deny.is_empty() {
                assert!(got.is_empty(), "{func}: {line:?}: got {got:?}");
            }
        }
    }
    let fmt: String = l.load("return paths.hyperlink_rules()[1].format").eval().unwrap();
    assert_eq!(fmt, "saysdf-path:$1");
}

#[test]
fn url_is_not_a_full_path_match() {
    let l = lua();
    let rs = rules(&l, "hyperlink_rules");
    let got = matches(&rs, "https://example.com/a/b.html");
    assert!(got.is_empty(), "{got:?}");
}

fn parse(l: &Lua, s: &str) -> (String, Option<i64>, Option<i64>) {
    l.load(format!("local t = paths.parse({s:?}) return t.path, t.line, t.col"))
        .eval()
        .unwrap()
}

#[test]
fn parse_line_col() {
    let l = lua();
    let cases: &[(&str, &str, Option<i64>, Option<i64>)] = &[
        ("src/a.rs", "src/a.rs", None, None),
        ("src/a.rs:10", "src/a.rs", Some(10), None),
        ("src/a.rs:10:5", "src/a.rs", Some(10), Some(5)),
        ("a.cpp(12)", "a.cpp", Some(12), None),
        ("a.cpp(12,5)", "a.cpp", Some(12), Some(5)),
        ("a.cpp(12, 5)", "a.cpp", Some(12), Some(5)),
        ("saysdf-path:src/a.rs:3:4", "src/a.rs", Some(3), Some(4)),
        (r"C:\a\b.rs:3:4", r"C:\a\b.rs", Some(3), Some(4)),
        ("file:///C:/My%20Dir/x.rs", "C:/My Dir/x.rs", None, None),
        ("file://host/home/u/x.rs", "/home/u/x.rs", None, None),
        (r"saysdf-path:\\srv\sh\x.txt:7", r"\\srv\sh\x.txt", Some(7), None),
    ];
    for (input, path, line, col) in cases {
        assert_eq!(parse(&l, input), (path.to_string(), *line, *col), "{input}");
    }
}

#[test]
fn resolve_with_cwd_fallbacks() {
    let l = lua();
    let r = |args: &str| -> (Option<String>, Option<String>) {
        l.load(format!("return paths.resolve_with({args})")).eval().unwrap()
    };
    assert_eq!(r("'file://h/home/u/p', nil, '/home/u', 'src/a.rs'").0.unwrap(), "/home/u/p/src/a.rs");
    assert_eq!(r("'file://h/C:/w/p', nil, nil, 'src/a.rs'").0.unwrap(), "C:/w/p/src/a.rs");
    assert_eq!(r(r"nil, 'C:\\w', nil, 'a.rs'").0.unwrap(), r"C:\w\a.rs");
    assert_eq!(r("nil, nil, '/home/u', '~/x.md'").0.unwrap(), "/home/u/x.md");
    assert_eq!(r("nil, nil, nil, '/etc/hosts'").0.unwrap(), "/etc/hosts");
    assert_eq!(r(r"nil, nil, nil, '\\\\srv\\x'").0.unwrap(), r"\\srv\x");
    assert_eq!(r("nil, nil, nil, './a.rs'").1.unwrap(), "unknown cwd");
}

#[test]
fn executables_open_as_text_in_viewer_never_spawned() {
    let l = lua();
    let dir = tempfile::tempdir().unwrap();
    let names = ["run.exe", "x.bat", "s.ps1", "a.sh", "notes.md", "d.mmd", "p.png", "m.rs"];
    for n in names {
        std::fs::write(dir.path().join(n), "x").unwrap();
    }
    let cwd = dir.path().to_string_lossy().replace('\\', "/");
    let kinds: Vec<String> = l
        .load(format!(
            "package.loaded['saysdf.viewer'] = {{ open = function(w, p, path, line, col) \
               table.insert(viewed, path) end }} \
             local pane = {{ get_current_working_dir = function() return {cwd:?} end }} \
             local r = {{}} \
             for _, n in ipairs({{ {} }}) do \
               assert(paths.open(nil, pane, 'saysdf-path:' .. n .. ':1', nil)) \
               table.insert(r, paths.classify(n)) end \
             assert(#spawned == 0) assert(#viewed == {}) return r",
            names.iter().map(|n| format!("{n:?}")).collect::<Vec<_>>().join(","),
            names.len()
        ))
        .eval()
        .unwrap();
    assert_eq!(kinds, ["text", "text", "text", "text", "markdown", "mermaid", "image", "text"]);
}

#[test]
fn missing_file_and_missing_viewer_toast() {
    let l = lua();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "x").unwrap();
    let cwd = dir.path().to_string_lossy().replace('\\', "/");
    let (missing, noviewer, n): (bool, bool, i64) = l
        .load(format!(
            "require('saysdf.i18n').select('en') package.loaded['saysdf.viewer'] = nil \
             local pane = {{ get_current_working_dir = function() return {cwd:?} end }} \
             local a = paths.open(nil, pane, 'saysdf-path:nope.rs:1') \
             local b = paths.open(nil, pane, 'saysdf-path:b/a.rs:1') \
             return a, b, #warnings"
        ))
        .eval()
        .unwrap();
    assert!(!missing && !noviewer);
    assert_eq!(n, 2);
    let w: String = l.load("return warnings[2]").eval().unwrap();
    assert!(w.contains("viewer is not installed"), "{w}");
    // The Russian UI is unchanged by the migration
    let w_ru: String = l
        .load(
            "require('saysdf.i18n').select('ru')              local pane = { get_current_working_dir = function() return '/nonexistent-dir-xyz' end }              paths.open(nil, pane, 'saysdf-path:nope.rs:1')              return warnings[#warnings]",
        )
        .eval()
        .unwrap();
    assert!(w_ru.contains("файл не найден"), "{w_ru}"); // i18n-allow-cyrillic
}

#[test]
fn setup_appends_rules_and_open_uri_handles_only_own_schemes() {
    let l = lua();
    let (n, http, editor): (i64, Option<bool>, String) = l
        .load(
            "local c = paths.setup({ mouse = false, editor = { 'ed', '+{line}', '{path}' } }, {}) \
             local http = handlers['open-uri'](nil, nil, 'https://example.com') \
             local args = paths.editor_args(paths._state.opts.editor, '/a b.rs', 7, 2) \
             return #c.hyperlink_rules, http, table.concat(args, '|')",
        )
        .eval()
        .unwrap();
    assert_eq!(n, 5);
    assert_eq!(http, None);
    assert_eq!(editor, "ed|+7|/a b.rs");
}
