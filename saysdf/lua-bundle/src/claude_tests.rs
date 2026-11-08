//! Tests of the module `saysdf.claude` (change workspace-restore, task 1.1, design D6).
use super::*;

fn lua() -> Lua {
    let modules: Modules =
        Box::leak(vec![("saysdf.claude", get("saysdf.claude").unwrap())].into_boxed_slice());
    let lua = Lua::new();
    {
        let package: Table = lua.globals().get("package").unwrap();
        package.set("path", "").unwrap();
        package.set("cpath", "").unwrap();
    }
    install_searcher_with(&lua, modules).unwrap();
    // No fake wezterm: the module must load and work without it.
    lua.load("claude = require('saysdf.claude')").exec().unwrap();
    lua
}

fn s(l: &Lua, code: &str) -> String {
    l.load(code).eval().unwrap()
}

fn b(l: &Lua, code: &str) -> bool {
    l.load(code).eval().unwrap()
}

#[test]
fn projects_dir_by_os_and_override() {
    let l = lua();
    assert_eq!(
        s(&l, r#"return claude.projects_dir{ triple = 'x86_64-pc-windows-msvc', home = 'C:\\Users\\me', getenv = function() return nil end }"#),
        r"C:\Users\me\.claude\projects"
    );
    assert_eq!(
        s(&l, r#"return claude.projects_dir{ triple = 'x86_64-unknown-linux-gnu', home = '/home/me/', getenv = function() return '' end }"#),
        "/home/me/.claude/projects"
    );
    assert_eq!(
        s(&l, r#"return claude.projects_dir{ triple = 'aarch64-apple-darwin', home = '/Users/me', getenv = function(n) if n == 'CLAUDE_CONFIG_DIR' then return '/opt/claude/' end end }"#),
        "/opt/claude/projects"
    );
}

#[test]
fn mangle_paths() {
    let l = lua();
    let cases = [
        (r"C:\dev\proj", "C--dev-proj"),
        ("C:/Users/me/project", "C--Users-me-project"),
        (r"C:\Users\me\project\", "C--Users-me-project"),
        ("/home/me/proj", "-home-me-proj"),
        ("/home/me/proj/", "-home-me-proj"),
        ("/Users/me/My Project (old)", "-Users-me-My-Project--old-"),
        ("/home/me/a_b.c", "-home-me-a-b-c"),
        // one "-" per code point, not per byte
        ("/home/me/проект", "-home-me-------"), // i18n-allow-cyrillic
        (r"C:\Users\me\Мой код", "C--Users-me--------"), // i18n-allow-cyrillic
        // OSC 7 reports a trailing separator; both forms map to the same folder
        ("C:/work/dev/proj/", "C--work-dev-proj"),
        (r"C:\work\dev\proj\\","C--work-dev-proj"),
        ("C:/work/dev/proj//", "C--work-dev-proj"),
        ("C:/", "C--"),
        ("/", "-"),
        (r"C:\", "C--"),
    ];
    for (input, want) in cases {
        l.globals().set("p", input).unwrap();
        assert_eq!(s(&l, "return claude.mangle(p)"), want, "mangle({input})");
    }
    assert!(b(&l, "return claude.mangle('') == nil"));
    assert!(b(&l, "return claude.mangle(nil) == nil"));
    assert!(b(&l, "return claude.mangle('/home/\\xff') == nil"));
}

#[test]
fn uuid_strict() {
    let l = lua();
    let good = "0b3c1a2d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
    l.globals().set("g", good).unwrap();
    assert!(b(&l, "return claude.is_uuid(g)"));
    for bad in [
        "0B3C1A2D-4E5F-4A6B-8C7D-9E0F1A2B3C4D",
        "0b3c1a2d-4e5f-4a6b-8c7d-9e0f1a2b3c4D",
        "{0b3c1a2d-4e5f-4a6b-8c7d-9e0f1a2b3c4d}",
        "0b3c1a2d-4e5f-4a6b-8c7d-9e0f1a2b3c4",
        "0b3c1a2d-4e5f-4a6b-8c7d-9e0f1a2b3c4d0",
        "0b3c1a2d4e5f4a6b8c7d9e0f1a2b3c4d",
        "0b3c1a2d-4e5f-4a6b-8c7d-9e0f1a2b3c4g",
        "0b3c1a2d-4e5f-4a6b-8c7d-9e0f1a2b;rm -",
        "0b3c1a2d-4e5f-4a6b-8c7d-9e0f1a2b3c4d\n",
        "../../../../etc/passwd-aaaa-bbbb-cccc",
        "",
    ] {
        l.globals().set("x", bad).unwrap();
        assert!(!b(&l, "return claude.is_uuid(x)"), "is_uuid({bad:?})");
    }
    assert!(!b(&l, "return claude.is_uuid(nil) or claude.is_uuid(42)"));
}

#[test]
fn argv_parsing() {
    let l = lua();
    let id = "0b3c1a2d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
    l.globals().set("id", id).unwrap();
    for code in [
        "return claude.argv_session{ 'claude', '--resume', id }",
        "return claude.argv_session{ 'claude', '-r', id, '--verbose' }",
        "return claude.argv_session{ 'claude', '--session-id', id }",
        "return claude.argv_session{ 'claude', '--resume=' .. id }",
        "return claude.argv_session{ 'claude', '--session-id=' .. id }",
        "return claude.argv_session{ 'claude', '--resume', 'bad', '-r', id }",
    ] {
        assert_eq!(s(&l, code), id, "{code}");
    }
    for code in [
        "return claude.argv_session{ 'claude' } == nil",
        "return claude.argv_session{ 'claude', '--resume' } == nil",
        "return claude.argv_session{ 'claude', '--resume', string.upper(id) } == nil",
        "return claude.argv_session{ 'claude', id } == nil",
        "return claude.argv_session{ 'claude', '--resume=' .. id .. 'x' } == nil",
        "return claude.argv_session(nil) == nil",
    ] {
        assert!(b(&l, code), "{code}");
    }
}

const IDS: &str = r#"
    A = 'aaaaaaaa-0000-4000-8000-000000000001'
    B = 'bbbbbbbb-0000-4000-8000-000000000002'
    C = 'cccccccc-0000-4000-8000-000000000003'
    function f(id, created, modified)
      return { path = '/home/me/.claude/projects/-home-me-proj/' .. id .. '.jsonl',
               created = created, modified = modified or created }
    end
"#;

#[test]
fn match_single_and_none() {
    let l = lua();
    l.load(IDS).exec().unwrap();
    assert!(b(
        &l,
        "local r = claude.match_sessions({ { pane_id = 1, start_time = 100 } }, { f(A, 101, 150) }) return r[1] == A"
    ));
    // file created before the process (minus slack) and not modified since -> no id
    assert!(b(
        &l,
        "local r = claude.match_sessions({ { pane_id = 1, start_time = 100 } }, { f(A, 50, 60) }) return next(r) == nil"
    ));
    // no files at all, and non-UUID names are ignored
    assert!(b(
        &l,
        "return next(claude.match_sessions({ { pane_id = 1, start_time = 100 } }, {})) == nil"
    ));
    assert!(b(&l, "local r = claude.match_sessions({ { pane_id = 1, start_time = 100 } }, { { path = '/x/notes.jsonl', modified = 200 } }) return next(r) == nil"));
}

#[test]
fn match_two_far_apart() {
    let l = lua();
    l.load(IDS).exec().unwrap();
    assert!(b(
        &l,
        r#"local r = claude.match_sessions(
            { { pane_id = 2, start_time = 200 }, { pane_id = 1, start_time = 100 } },
            { f(B, 201, 300), f(A, 101, 300) })
           return r[1] == A and r[2] == B"#
    ));
}

#[test]
fn match_two_close_is_ambiguous() {
    let l = lua();
    l.load(IDS).exec().unwrap();
    assert!(b(
        &l,
        r#"local r = claude.match_sessions(
            { { pane_id = 1, start_time = 100 }, { pane_id = 2, start_time = 101.5 } },
            { f(A, 101, 300), f(B, 102, 300) })
           return next(r) == nil"#
    ));
    // the ambiguous pair still holds its files: a later pane does not take them
    assert!(b(
        &l,
        r#"local r = claude.match_sessions(
            { { pane_id = 1, start_time = 100 }, { pane_id = 2, start_time = 101 },
              { pane_id = 3, start_time = 500 } },
            { f(A, 101, 600), f(B, 102, 600), f(C, 501, 600) })
           return r[1] == nil and r[2] == nil and r[3] == C"#
    ));
}

#[test]
fn match_sticky_argv_and_resume_of_older_file() {
    let l = lua();
    l.load(IDS).exec().unwrap();
    // sticky binding wins while the same process runs and the file exists
    assert!(b(
        &l,
        r#"local r = claude.match_sessions(
            { { pane_id = 1, start_time = 100, bound = { session = B, start_time = 100 } } },
            { f(A, 101, 300), f(B, 50, 60) })
           return r[1] == B"#
    ));
    // a new process (other start_time) drops the binding
    assert!(b(
        &l,
        r#"local r = claude.match_sessions(
            { { pane_id = 1, start_time = 400, bound = { session = B, start_time = 100 } } },
            { f(A, 401, 500), f(B, 50, 60) })
           return r[1] == A"#
    ));
    // argv id with an existing file; it also takes the pane out of the ambiguity check
    assert!(b(
        &l,
        r#"local r = claude.match_sessions(
            { { pane_id = 1, start_time = 100, argv_id = B }, { pane_id = 2, start_time = 101 } },
            { f(A, 101, 300), f(B, 50, 300) })
           return r[1] == B and r[2] == A"#
    ));
    // argv id without a file -> treated like any other pane
    assert!(b(
        &l,
        r#"local r = claude.match_sessions(
            { { pane_id = 1, start_time = 100, argv_id = C } }, { f(A, 101, 300) })
           return r[1] == A"#
    ));
    // resume continuing an older file: created long ago, modified after the start
    assert!(b(
        &l,
        r#"local r = claude.match_sessions(
            { { pane_id = 1, start_time = 500 } }, { f(A, 100, 510), f(B, 100, 400) })
           return r[1] == A"#
    ));
}

#[test]
fn start_time_units_by_os() {
    let l = lua();
    // Windows FILETIME: 100 ns ticks since 1601-01-01
    assert!(b(&l, "return claude.start_unix((1700000000 + 11644473600) * 10000000, 'x86_64-pc-windows-msvc') == 1700000000"));
    // Linux: clock ticks since boot
    assert!(b(&l, "return claude.start_unix(250, 'x86_64-unknown-linux-gnu', 1700000000) == 1700000002.5"));
    assert!(b(&l, "return claude.start_unix(250, 'x86_64-unknown-linux-gnu', nil) == nil"));
    // macOS: already epoch seconds
    assert!(b(&l, "return claude.start_unix(1700000000, 'aarch64-apple-darwin') == 1700000000"));
    assert!(b(&l, "return claude.start_unix(1, 'x86_64-unknown-freebsd') == nil"));
    assert!(b(&l, "return claude.start_unix(nil, 'aarch64-apple-darwin') == nil"));
}

#[test]
fn match_continued_session_and_reasons() {
    let l = lua();
    l.load(IDS).exec().unwrap();
    // continued session: created long before the start, appended after it; older files untouched
    assert!(b(
        &l,
        r#"local r = claude.match_sessions(
            { { pane_id = 1, start_time = 500 } }, { f(A, 100, 510), f(B, 100, 400), f(C, 90, 95) })
           return r[1] == A"#
    ));
    // two files appended after the start: cannot tell which -> no id
    assert_eq!(
        s(
            &l,
            r#"local why = {}
               local r = claude.match_sessions(
                { { pane_id = 1, start_time = 500 } }, { f(A, 100, 510), f(B, 100, 520) }, why)
               return tostring(r[1]) .. ' ' .. why[1]"#
        ),
        "nil ambiguous"
    );
    assert_eq!(
        s(
            &l,
            r#"local a, b, c, d = {}, {}, {}, {}
               claude.match_sessions({ { pane_id = 1, start_time = 500 } }, {}, a)
               claude.match_sessions({ { pane_id = 1, start_time = 500 } }, { f(A, 100, 200) }, b)
               claude.match_sessions({ { pane_id = 1 } }, { f(A, 100, 200) }, c)
               claude.match_sessions({ { pane_id = 1, start_time = 100 }, { pane_id = 2, start_time = 101 } },
                 { f(A, 101, 300), f(B, 102, 300) }, d)
               return a[1] .. '|' .. b[1] .. '|' .. c[1] .. '|' .. d[1] .. '|' .. d[2]"#
        ),
        "no session files|no file modified after start|no start time|ambiguous|ambiguous"
    );
}
