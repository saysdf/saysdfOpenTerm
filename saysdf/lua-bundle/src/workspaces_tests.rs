//! Tests of `saysdf.workspaces` and `saysdf.util` (change project-workspaces, tasks 1.1-1.3, 4.1-4.2).
use super::*;

const FAKE: &str = r#"
    package.loaded.wezterm = {
        on = function() end,
        target_triple = 'x86_64-unknown-linux-gnu',
        home_dir = '/home/u',
        json_parse = function(s) if s == 'BAD' then error('bad') end return decoded end,
    }
    util = require('saysdf.util')
    ws = require('saysdf.workspaces')
    -- the language of this Lua state only (not the system one): English expectations
    require('saysdf.i18n').select('en')
    function show(n)
        if n.pane then return n.pane.cwd or '?' end
        return n.split .. n.ratio .. '(' .. show(n.a) .. ',' .. show(n.b) .. ')'
    end
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.claude", get("saysdf.claude").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.status", get("saysdf.status").unwrap()),
            ("saysdf.util", get("saysdf.util").unwrap()),
            ("saysdf.workspaces", get("saysdf.workspaces").unwrap()),
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

fn s(l: &Lua, code: &str) -> String {
    l.load(code).eval::<String>().unwrap()
}

#[test]
fn data_dir_three_os() {
    let l = lua();
    let env = |t: &str, home: &str| {
        format!(
            "{{ triple = '{t}', home = '{home}', getenv = function(n) \
             if n == 'LOCALAPPDATA' then return 'C:\\\\Users\\\\u\\\\AppData\\\\Local' end end }}"
        )
    };
    assert_eq!(
        s(&l, &format!("return util.data_dir({})", env("x86_64-pc-windows-msvc", "C:\\\\Users\\\\u"))),
        "C:\\Users\\u\\AppData\\Local\\saysdfTerm"
    );
    assert_eq!(
        s(&l, &format!("return util.data_dir({})", env("aarch64-apple-darwin", "/Users/u"))),
        "/Users/u/Library/Application Support/saysdfTerm"
    );
    assert_eq!(
        s(&l, &format!("return util.data_dir({})", env("x86_64-unknown-linux-gnu", "/home/u"))),
        "/home/u/.local/share/saysdfTerm"
    );
}

#[test]
fn layout_single_pane() {
    let l = lua();
    assert_eq!(
        s(&l, "return show(ws.layout_from_panes({{left=0,top=0,width=80,height=24,cwd='/a'}}))"),
        "/a"
    );
}

#[test]
fn layout_grid_2x2_and_plan() {
    let l = lua();
    let code = "local L = ws.layout_from_panes({
        {left=0,top=0,width=40,height=12,cwd='a'}, {left=41,top=0,width=39,height=12,cwd='b'},
        {left=0,top=13,width=40,height=11,cwd='c'}, {left=41,top=13,width=39,height=11,cwd='d'} })";
    assert_eq!(s(&l, &format!("{code} return show(L)")), "h0.5(v0.5(a,c),v0.5(b,d))");
    // plan: 1 is the original; 2 is to the right of 1; 1 splits down (3), 2 splits down (4)
    assert_eq!(
        s(
            &l,
            &format!(
                "{code} local p = ws.plan_splits(L) local r = {{p.root.cwd}} \
                 for _, st in ipairs(p.steps) do table.insert(r, st.from..st.direction..st.new..st.cwd..st.size) end \
                 return table.concat(r, ' ')"
            )
        ),
        "a 1Right2b0.5 1Bottom3c0.5 2Bottom4d0.5"
    );
}

#[test]
fn layout_nested_proportions() {
    let l = lua();
    // a narrow full-height column on the left; top and bottom on the right (the bottom is split again)
    let code = "return show(ws.layout_from_panes({
        {left=0,top=0,width=20,height=24,cwd='l'},
        {left=21,top=0,width=79,height=8,cwd='t'},
        {left=21,top=9,width=40,height=15,cwd='bl'}, {left=62,top=9,width=38,height=15,cwd='br'} }))";
    assert_eq!(s(&l, code), "h0.2(l,v0.333(t,h0.506(bl,br)))");
}

#[test]
fn serialize_roundtrip_and_validation() {
    let l = lua();
    let out = s(
        &l,
        "local w = { name = 'p', tabs = { { title = 'x', layout = { split = 'h', ratio = 0.5,
            a = { pane = { cwd = '/a', args = {'rm','-rf'} } }, b = { pane = { cwd = '/b' } } } } } }
         local d = ws.to_data(w)
         assert(d.version == 1 and d.tabs[1].layout.a.pane.args == nil, 'commands not stored')
         local back = assert(ws.from_data(d))
         return back.name .. ' ' .. back.tabs[1].title .. ' ' .. show(back.tabs[1].layout)",
    );
    assert_eq!(out, "p x h0.5(/a,/b)");
    let errs = s(
        &l,
        "local r = {}
         for _, d in ipairs({ 'str', { name = 'p', tabs = {} },
             { version = 99, name = 'p', tabs = {} },
             { version = 1, name = 'p', tabs = { { layout = { split = 'h', ratio = 2, a = {pane={}}, b = {pane={}} } } } } }) do
           local v, e = ws.from_data(d) table.insert(r, e) end
         decoded = nil
         table.insert(r, select(2, ws.decode('BAD')))
         return table.concat(r, '|')",
    );
    assert_eq!(errs, "not an object|no version|future version 99|bad layout in tab 1|bad json");
}

#[test]
fn project_cwd_normalization() {
    let l = lua();
    assert_eq!(s(&l, "return ws.normalize_cwd('/p/', 'a/./b')"), "/p/a/b");
    assert_eq!(s(&l, "return ws.normalize_cwd('C:\\\\P', 'srv', true)"), "C:\\P\\srv");
    assert_eq!(s(&l, "return select(2, ws.normalize_cwd('/p', '../x'))"), "outside project");
    assert_eq!(s(&l, "return select(2, ws.normalize_cwd('/p', '/etc'))"), "absolute path");
    assert_eq!(s(&l, "return select(2, ws.normalize_cwd('C:\\\\p', 'D:\\\\x', true))"), "absolute path");
    assert_eq!(s(&l, "return ws.trust_key('C:/Users/U/Proj/', true)"), "c:\\users\\u\\proj");
}

#[test]
fn trust_bound_to_exact_commands() {
    let l = lua();
    let out = s(
        &l,
        "local cmds = { { cwd = 'srv', args = {'cargo','run'} }, { cwd = '.', args = {'npm','test'} } }
         local store = {}
         local r = { tostring(ws.is_trusted(store, '/p', cmds)) }            -- new repo
         ws.trust(store, '/p', cmds)
         table.insert(r, tostring(ws.is_trusted(store, '/p', cmds)))         -- match
         local changed = { { cwd = 'srv', args = {'cargo','run','--release'} }, cmds[2] }
         table.insert(r, tostring(ws.is_trusted(store, '/p', changed)))     -- command changed
         table.insert(r, tostring(ws.is_trusted(store, '/p', { cmds[1] }))) -- command removed
         table.insert(r, tostring(ws.is_trusted(store, '/other', cmds)))    -- other repo
         table.insert(r, tostring(ws.commands_hash(cmds) ~= ws.commands_hash(changed)))
         -- Windows: path case does not matter
         ws.trust(store, 'C:\\\\Proj', cmds, true)
         table.insert(r, tostring(ws.is_trusted(store, 'c:/proj', cmds, true)))
         return table.concat(r, ' ')",
    );
    assert_eq!(out, "false true false false false true true");
}

#[test]
fn parse_project_collects_commands() {
    let l = lua();
    let out = s(
        &l,
        "local p = assert(ws.parse_project('/p', { tabs = { { cwd = 'srv', command = 'make' }, { title = 'sh' } } }))
         return p.name .. ' ' .. #p.commands .. ' ' .. p.tabs[1].cwd .. ' ' .. p.tabs[2].cwd",
    );
    assert_eq!(out, "p 1 /p/srv /p");
    assert_eq!(
        s(&l, "return select(2, ws.parse_project('/p', { tabs = { { cwd = '../x' } } }))"),
        "tab 1: outside project"
    );
}

#[test]
fn autosave_debounce_and_change() {
    let l = lua();
    let out = s(
        &l,
        "local st, r = {}, {}
         for _, c in ipairs({ {0,'A'}, {10,'B'}, {30,'A'}, {60,'A'}, {95,'C'} }) do
           table.insert(r, tostring(ws.autosave_decide(st, c[1], c[2], 30))) end
         return table.concat(r, ' ')",
    );
    // 0: first write; 10: too early; 30: 30 s passed but the text is the same; 60: unchanged; 95: changed
    assert_eq!(out, "true false false false true");
}

#[test]
fn startup_mode_choice() {
    let l = lua();
    let out = s(
        &l,
        "local r = {}
         for _, m in ipairs({ 'ask', 'always', 'never' }) do
           table.insert(r, ws.startup_action(m, true) .. '/' .. ws.startup_action(m, false)) end
         table.insert(r, tostring(ws.session_names({ version = 1, workspaces = { 'a', '', 5 } })[1]))
         table.insert(r, tostring(ws.session_names({ version = 9, workspaces = { 'a' } })))
         return table.concat(r, ' ')",
    );
    assert_eq!(out, "ask/none restore/none none/none a nil");
}

#[test]
fn startup_action_always_is_default() {
    // change workspace-restore D2/D9: default "always", wss is the session read into memory.
    let l = lua();
    let out = s(
        &l,
        "local one = { pane = { cwd = '/a' } }
         local tab1 = { { tabs = { { layout = one } } } }
         local tab2 = { { tabs = { { layout = one }, { layout = one } } } }
         local r = {}
         for _, m in ipairs({ 'always', 'ask', 'never', 'none' }) do
           table.insert(r, m .. '=' .. ws.startup_action(m, tab1) .. '/' .. ws.startup_action(m, tab2)
             .. '/' .. ws.startup_action(m, {}) .. '/' .. ws.startup_action(m, { { tabs = {} } })) end
         table.insert(r, 'nil=' .. ws.startup_action(nil, tab1) .. '/' .. ws.startup_action(nil, nil))
         return table.concat(r, ' ')",
    );
    assert_eq!(
        out,
        "always=restore/restore/none/none ask=none/ask/none/none never=none/none/none/none \
         none=none/none/none/none nil=restore/none"
    );
}

#[test]
fn session_weight_counts_panes_and_claude() {
    let l = lua();
    let out = s(
        &l,
        "local c = { pane = { cwd = '/c', claude = { session = '0f8fad5b-d9cb-469f-a165-70867728950e' } } }
         local p = { pane = { cwd = '/p' } }
         local bad = { pane = { cwd = '/x', claude = { session = '' } } }
         local split = { split = 'h', ratio = 0.5, a = c, b = { split = 'v', ratio = 0.5, a = p, b = bad } }
         local r = {}
         for _, wss in ipairs({
           {},
           { { tabs = {} } },
           { { tabs = { { layout = p } } } },
           { { tabs = { { layout = split } } } },
           { { tabs = { { layout = split }, { layout = c } } }, { tabs = { { layout = p } } } },
           { { tabs = { { layout = 'junk' }, 5 } }, 'junk' },
         }) do local w = ws.session_weight(wss); table.insert(r, w.panes .. ':' .. w.claude) end
         return table.concat(r, ' ')",
    );
    assert_eq!(out, "0:0 0:0 1:0 3:1 5:2 0:0");
}

#[test]
fn commit_decision_shrink_guard() {
    let l = lua();
    let out = s(
        &l,
        "local function W(p, c) return { panes = p, claude = c or 0 } end
         local r = {}
         local function d(st, now, n, o, opts) table.insert(r, ws.commit_decision(st, now, n, o, opts)) end
         -- empty mux: never, not even manual or with nothing saved
         d({}, 0, W(0), W(7)); d({}, 0, W(0), nil); d({}, 0, nil, W(1), { manual = true })
         table.insert(r, '|')
         -- nothing saved yet, growth, same weight: at once
         d({}, 0, W(1), nil); d({}, 0, W(8), W(7)); d({}, 0, W(7, 2), W(7, 2)); d({}, 0, W(7, 3), W(7, 2))
         table.insert(r, '|')
         -- shrink: held for < 90 s, committed at 90 s (default delay)
         local st = {}
         d(st, 100, W(2), W(7)); d(st, 150, W(2), W(7)); d(st, 189, W(1), W(7)); d(st, 190, W(1), W(7))
         table.insert(r, tostring(st.shrink_since))
         table.insert(r, '|')
         -- fewer Claude sessions is a shrink too, even with more panes
         st = {}
         d(st, 0, W(9, 1), W(7, 2)); d(st, 30, W(9, 1), W(7, 2), 30)
         table.insert(r, '|')
         -- regrow resets the timer: the next shrink waits the full delay again
         st = {}
         d(st, 0, W(2), W(7)); d(st, 60, W(7), W(7)); d(st, 100, W(2), W(7)); d(st, 180, W(2), W(7))
         d(st, 190, W(2), W(7))
         table.insert(r, '|')
         -- manual save bypasses the guard and resets it
         st = {}
         d(st, 0, W(2), W(7)); d(st, 10, W(2), W(7), { manual = true })
         table.insert(r, tostring(st.shrink_since))
         d(st, 20, W(2), W(7), { delay = 90 })
         return table.concat(r, ' ')",
    );
    assert_eq!(
        out,
        "skip skip skip | write write write write | wait wait wait write nil | wait write | \
         wait write wait wait write | wait write nil wait"
    );
}

#[test]
fn trivial_session_is_not_offered_for_restore() {
    // Orchestrator decision: ask only if the previous session had >1 tabs or splits.
    let l = lua();
    let out = s(
        &l,
        "local one = { pane = { cwd = '/a' } }
         local split = { split = 'h', ratio = 0.5, a = one, b = { pane = { cwd = '/b' } } }
         local r = {}
         for _, wss in ipairs({
           {},                                                   -- no workspaces
           { { tabs = {} } },                                    -- empty
           { { tabs = { { layout = one } } } },                  -- one tab, one pane
           { { tabs = { { title = 'mine', layout = one } } } },  -- a custom name does not count
           { { tabs = { { layout = split } } } },                -- split
           { { tabs = { { layout = one }, { layout = one } } } },-- two tabs
           { { tabs = { { layout = one } } }, { tabs = { { layout = one } } } }, -- two workspaces
         }) do table.insert(r, tostring(ws.session_worth_restoring(wss))) end
         return table.concat(r, ' ')",
    );
    assert_eq!(out, "false false false false true true true");
}

#[test]
fn commands_reach_spawn_only_with_flag() {
    let l = lua();
    let out = s(
        &l,
        "local log = {}
         local function pane() return { split = function(_, a) table.insert(log, a.args and table.concat(a.args, ' ') or '-') return pane() end } end
         local window = { spawn_tab = function(_, a) table.insert(log, a.args and table.concat(a.args, ' ') or '-') return { set_title = function() end }, pane() end }
         package.loaded.wezterm.mux = {
           spawn_window = function(a) table.insert(log, a.args and table.concat(a.args, ' ') or '-')
             return { set_title = function() end }, pane(), window end,
           set_active_workspace = function() end,
         }
         local layout = { split = 'h', ratio = 0.5, a = { pane = { cwd = '/a', args = {'htop'} } }, b = { pane = { cwd = '/b', args = {'top'} } } }
         local function run(flag)
           log = {}
           local back = assert(ws.from_data(ws.to_data({ name = 'p', allow_commands = flag, tabs = { { layout = layout } } })))
           ws.restore(back, false)
           return table.concat(log, ',')
         end
         return run(nil) .. ' | ' .. run(true)",
    );
    assert_eq!(out, "-,- | htop,top");
}

// perf-1: the mkdir process only for a missing folder and only once; perf-3: the foreground
// process name without the fork's method -- the upstream fallback call.
#[test]
fn ensure_dir_spawns_only_for_missing_dir_once() {
    let l = lua();
    let r = s(
        &l,
        r#"
        util._forget_dirs()
        local n = 0
        local exists = { ['/have'] = true }
        local env = {
            windows = false,
            execute = function() n = n + 1 end,
            read_dir = function(d) if not exists[d] then error('no') end return {} end,
        }
        util.ensure_dir('/have', env)
        util.ensure_dir('/have', env)
        local a = n
        util.ensure_dir('/new', env)
        util.ensure_dir('/new', env)
        util.ensure_dir('/new', env)
        -- the async path (run_child_process) takes priority, os.execute is left alone
        local argv
        env.run = function(a) argv = a end
        util.ensure_dir('/async', env)
        util.ensure_dir('/async', env)
        -- outside a coroutine run fails -- fallback to os.execute
        env.run = function() error('yield outside coroutine') end
        util.ensure_dir('/fallback', env)
        return a .. ',' .. n .. ',' .. table.concat(argv, ' ')
    "#,
    );
    assert_eq!(r, "0,2,mkdir -p /async");
}

#[test]
fn pane_foreground_falls_back_without_cached_method() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local calls = 0
        local p = {
            pane_id = function() return 1 end,
            is_alt_screen_active = function() return true end,
            get_foreground_process_name = function() calls = calls + 1 return 'p1' end,
        }
        local i = util.pane_foreground(p)
        return tostring(i.process) .. ',' .. tostring(i.alt_screen) .. ',' .. calls
    "#,
    );
    assert_eq!(r, "p1,true,1");
}

#[test]
fn texts_follow_language() {
    let l = lua();
    let root = std::env::temp_dir().join(format!("saysdf-ws-i18n-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".saysdf")).unwrap();
    std::fs::write(root.join(".saysdf").join("workspace.json"), "{}").unwrap();
    let root = root.to_string_lossy().replace('\\', "/");
    let run = |lang: &str| -> String {
        l.load(format!(
            r#"
            require('saysdf.i18n').select({lang:?})
            local w = package.loaded.wezterm
            w.action = {{ InputSelector = function(a) return a end }}
            w.action_callback = function(f) return f end
            decoded = {{ tabs = {{ {{ cwd = 'srv', command = 'make' }} }} }}
            local performed
            local window = {{ perform_action = function(_, a) performed = a end }}
            ws._state.opts = {{}}
            ws.open_project(window, {{}}, {root:?})
            local out = {{ performed.title }}
            for _, c in ipairs(performed.choices) do table.insert(out, c.label) end
            local entries = {{}}
            package.loaded['saysdf.palette'] = {{
                add = function(e) table.insert(entries, e) end,
                key_text = function() return 'K' end,
            }}
            ws.setup({{ autosave = false, restore = 'never', key = false }}, {{}})
            table.insert(out, entries[1].label)
            table.insert(out, entries[1].hint.label)
            table.insert(out, entries[1].brief)
            table.insert(out, entries[4].brief)
            return table.concat(out, '|')
            "#
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(
        run("en"),
        concat!(
            "Project commands: srv: make|Run (remember for this command list)|Shells only|Cancel|",
            "Workspaces…|Projects|Workspaces: list|",
            "Workspaces: open the project of the current folder (.saysdf/workspace.json)"
        )
    );
    // The Russian UI is unchanged by the migration
    assert_eq!(
        run("ru"),
        concat!(
            "Команды проекта: srv: make|Запустить (запомнить для этого списка команд)|", // i18n-allow-cyrillic
            "Только шеллы|Отмена|", // i18n-allow-cyrillic
            "Рабочие пространства…|Проекты|Рабочие пространства: список|", // i18n-allow-cyrillic
            "Рабочие пространства: открыть проект текущей папки (.saysdf/workspace.json)" // i18n-allow-cyrillic
        )
    );
    let _ = std::fs::remove_dir_all(root);
}

// ---- mask_args and preview_md (change workspace-restore, W-3) ----

const MASK: &str = "function m(...) local r, h = ws.mask_args({...}); return table.concat(r, ' ') .. '|' .. tostring(h) end";

#[test]
fn mask_args_rules() {
    let l = lua();
    l.load(MASK).exec().unwrap();
    let case = |args: &str| s(&l, &format!("return m({args})"));
    // flag value, separate and attached by '='
    assert_eq!(case("'deploy','--api-key','abc123'"), "deploy --api-key ***|true");
    assert_eq!(case("'npm','run','--token=abc'"), "npm run --token=***|true");
    assert_eq!(case("'x','--PASSWORD','p'"), "x --PASSWORD ***|true");
    // NAME=value argument
    assert_eq!(case("'env','DB_PASSWORD=hunter2','run'"), "env DB_PASSWORD=*** run|true");
    // URL password
    assert_eq!(
        case("'psql','postgres://app:hunter2@203.0.113.5/db'"),
        "psql postgres://app:***@203.0.113.5/db|true"
    );
    // opaque string of 32+ characters
    let long = "A".repeat(20) + &"b1".repeat(8);
    assert_eq!(case(&format!("'curl','{long}'")), "curl ***|true");
    // negatives: ordinary flags, short values, URL without password, nothing to mask, plain NAME=value
    assert_eq!(case("'ls','-la','--color=auto','src'"), "ls -la --color=auto src|false");
    assert_eq!(case("'git','commit','-m','fix'"), "git commit -m fix|false");
    assert_eq!(case("'git','clone','https://example.com/a/b.git'"), "git clone https://example.com/a/b.git|false");
    assert_eq!(case("'x','--token'"), "x --token|false");
    assert_eq!(case("'x','--key','--verbose'"), "x --key --verbose|false");
    assert_eq!(case("'x','HOME=/home/me'"), "x HOME=/home/me|false");
    // a long path is not opaque; the program path is never masked
    assert_eq!(case(&format!("'x','/very/long/{long}/path'")), format!("x /very/long/{long}/path|false"));
    assert_eq!(case(&format!("'/opt/{long}/token'")), format!("/opt/{long}/token|false"));
    // `--resume <uuid>` and a bare UUID are untouched
    let uuid = "0b3c1f2a-1111-4222-8333-444455556666";
    assert_eq!(case(&format!("'claude','--resume','{uuid}'")), format!("claude --resume {uuid}|false"));
    assert_eq!(case(&format!("'x','{uuid}'")), format!("x {uuid}|false"));
}

const PREVIEW_WS: &str = "
    pv = {
      name = 'default',
      tabs = {
        { title = 'api', layout = { split = 'h', ratio = 0.5,
            a = { pane = { cwd = 'C:/Users/me/proj/api', claude = { session = '0b3c1f2a-1111-4222-8333-444455556666' } } },
            b = { pane = { cwd = 'C:/Users/me/proj/api', args = { 'npm', 'run', 'dev' } } } } },
        { layout = { pane = { cwd = '/srv/prodhost', args = { 'deploy', '--api-key', 'abc123' } } } },
      },
    }
    pv_opts = { home = 'C:\\\\Users\\\\me', file = 'C:/Users/me/AppData/Local/saysdfTerm/workspaces/default.json', time = '2026-11-08 14:03' }
";

#[test]
fn preview_structure_two_tabs_with_split() {
    let l = lua();
    l.load(PREVIEW_WS).exec().unwrap();
    let md = s(&l, "return ws.preview_md('default', pv, pv_opts)");
    for want in [
        "# Workspace \"default\" saved 2026-11-08 14:03",
        "File: `~/AppData/Local/saysdfTerm/workspaces/default.json`",
        "Commands: not allowed",
        "## Tab 1 - \"api\"",
        "- Pane 1 - `~/proj/api`",
        "  - Claude Code: resume session `0b3c1f2a...`",
        "- Pane 2 - `~/proj/api`",
        "  - command: `npm run dev`",
        "## Tab 2",
        "- Pane 1 - `/srv/prodhost`",
        "## Not saved",
    ] {
        assert!(md.contains(want), "missing {want:?} in:\n{md}");
    }
    // a command with a secret is not shown at all, nor is the secret
    assert!(md.contains("command not saved: looks like it contains a secret"), "{md}");
    assert!(!md.contains("abc123") && !md.contains("deploy"), "{md}");
    assert!(md.find("## Tab 1").unwrap() < md.find("## Tab 2").unwrap());
}

#[test]
fn preview_in_russian() {
    let l = lua();
    l.load(PREVIEW_WS).exec().unwrap();
    let md = s(&l, "require('saysdf.i18n').select('ru'); return ws.preview_md('default', pv, pv_opts)");
    for want in [
        "# Рабочее пространство \"default\" сохранено 2026-11-08 14:03", // i18n-allow-cyrillic
        "Файл: `~/AppData/Local/saysdfTerm/workspaces/default.json`", // i18n-allow-cyrillic
        "## Вкладка 1 - \"api\"", // i18n-allow-cyrillic
        "- Панель 1 - `~/proj/api`", // i18n-allow-cyrillic
        "команда: `npm run dev`", // i18n-allow-cyrillic
        "команда не сохранена: похоже, содержит секрет", // i18n-allow-cyrillic
        "## Не сохраняется", // i18n-allow-cyrillic
    ] {
        assert!(md.contains(want), "missing {want:?} in:\n{md}");
    }
}

// ---- change workspace-restore W-5: startup gate, in-memory restore, deferred autosave ----

// A fake mux, clock, file system and JSON for the startup/autosave flow. Example data only.
const MUX: &str = r#"
    local w = package.loaded.wezterm
    handlers, fs, writes, logs, spy = {}, {}, {}, {}, {}
    w.on = function(n, f) handlers[n] = f end
    -- JSON: Lua literals (enough for the module's tables)
    local function enc(v)
      if type(v) == 'table' then
        local out = {}
        for k, x in pairs(v) do
          local key = type(k) == 'number' and ('[' .. k .. ']') or ('[' .. string.format('%q', k) .. ']')
          table.insert(out, key .. '=' .. enc(x))
        end
        table.sort(out)
        return '{' .. table.concat(out, ',') .. '}'
      elseif type(v) == 'string' then return string.format('%q', v)
      else return tostring(v) end
    end
    w.json_encode = enc
    w.json_parse = function(s) local f = assert(load('return ' .. s, 'j', 't', {})) return f() end
    w.format = function() return '' end
    w.action = { InputSelector = function(t) return t end }
    w.action_callback = function(f) return f end
    -- util.join uses the host separator: keys are normalized to '/'
    local function N(p) return (p:gsub('\\', '/')) end
    util.read_file = function(p) return fs[N(p)] end
    util.write_atomic = function(p, text) fs[N(p)] = text table.insert(writes, N(p)) return true end
    util.ensure_dir = function() end
    util.log_warn = function(m) table.insert(logs, m) end
    util.log_error = function(m) table.insert(logs, m) end
    os.remove = function(p) fs[N(p)] = nil return true end
    os.rename = function(a, b) fs[N(b)] = fs[N(a)] fs[N(a)] = nil return true end
    clock = 1000
    ws._clock = function() return clock end
    ws._forget_caches()
    -- mux
    windows, next_id, selector, spawned = {}, 0, nil, {}
    function new_pane(cwd, fg, args)
      next_id = next_id + 1
      -- what each spawn ran: 'shell' or the argv joined with '|'
      table.insert(spawned, args and table.concat(args, '|') or 'shell')
      local id = next_id
      return {
        pane_id = function() return id end,
        get_current_working_dir = function() return cwd and { file_path = cwd } or nil end,
        is_alt_screen_active = function() return false end,
        get_foreground_process_name_cached = function() return fg or '/bin/bash' end,
        get_foreground_process_info = function() table.insert(spy, 'info') return { argv = { fg or 'bash', '-x' } } end,
      }
    end
    local function new_tab(win, a)
      local tab = { pane = new_pane(a.cwd, a.fg, a.args) }
      tab.get_title = function() return '' end
      tab.set_title = function() end
      tab.panes_with_info = function() return { { left = 0, top = 0, width = 80, height = 24, pane = tab.pane } } end
      tab.pane.split = function(_, b) return new_pane(b.cwd, nil, b.args) end
      table.insert(win._tabs, tab)
      return tab, tab.pane
    end
    function new_window(a)
      local win = { _tabs = {}, _ws = a.workspace or 'default' }
      win.get_workspace = function() return win._ws end
      win.tabs = function() return win._tabs end
      win.spawn_tab = function(_, b) return new_tab(win, b) end
      win.gui_window = function() return { perform_action = function(_, act) selector = act end } end
      table.insert(windows, win)
      local tab, pane = new_tab(win, a)
      return tab, pane, win
    end
    w.mux = {
      all_windows = function() return windows end,
      spawn_window = function(a) return new_window(a or {}) end,
      set_active_workspace = function() end,
      get_workspace_names = function()
        local seen, out = {}, {}
        for _, x in ipairs(windows) do if not seen[x._ws] then seen[x._ws] = true table.insert(out, x._ws) end end
        return out
      end,
    }
    function tabs_total() local n = 0 for _, x in ipairs(windows) do n = n + #x._tabs end return n end
    function cwds() local r = {} for _, x in ipairs(windows) do for _, t in ipairs(x._tabs) do
      local u = t.pane.get_current_working_dir() table.insert(r, u and u.file_path or '?') end end
      return table.concat(r, ',') end
    -- previous session on disk: { {name, n one-pane tabs /p1../pn | raw file text} }
    function put_session(list)
      local names = {}
      for _, e in ipairs(list) do
        table.insert(names, e[1])
        if type(e[2]) == 'string' then fs['/d/' .. e[1] .. '.json'] = e[2]
        else
          local tabs = {}
          for i = 1, e[2] do table.insert(tabs, { layout = { pane = { cwd = '/p' .. i } } }) end
          fs['/d/' .. e[1] .. '.json'] = ws.encode({ name = e[1], tabs = tabs })
        end
      end
      fs['/d/last-session.json'] = w.json_encode({ version = 1, workspaces = names })
    end
    function saved_tabs(name) local t = ws.decode(fs['/d/' .. name .. '.json'] or 'BAD') return t and #t.tabs or -1 end
"#;

fn mux_lua() -> Lua {
    let l = lua();
    l.load(MUX).exec().unwrap();
    l
}

#[test]
fn restore_uses_memory_copy_despite_overwrite() {
    // The diagnosis race: an autosave would have overwritten default.json with the fresh one-tab
    // window before the restore re-read it. Now the restore uses the copy read at gui-startup.
    let l = mux_lua();
    let out = s(
        &l,
        "put_session({ { 'default', 7 } })
         ws.setup({ dir = '/d', restore = 'ask', key = false, palette = false })
         assert(ws._state.gate == 'startup')
         handlers['gui-startup'](nil)
         local before = #writes
         for _, c in ipairs({ 1000, 1010, 1100 }) do clock = c assert(not ws.autosave_tick()) end
         assert(#writes == before, 'no write while gated')
         put_session({ { 'default', 1 } }) -- the file is overwritten anyway
         selector.action(nil, nil, 'yes')
         return tabs_total() .. ' ' .. cwds() .. ' ' .. ws._state.gate",
    );
    assert_eq!(out, "8 ?,/p1,/p2,/p3,/p4,/p5,/p6,/p7 open");
}

#[test]
fn always_is_default_and_gate_opens_after_grace() {
    let l = mux_lua();
    let out = s(
        &l,
        "put_session({ { 'default', 7 } })
         ws.setup({ dir = '/d', key = false, palette = false })
         local r = {}
         clock = 900 table.insert(r, tostring(ws.autosave_tick())) -- before gui-startup: closed
         clock = 1000
         handlers['gui-startup'](nil)
         table.insert(r, tabs_total())
         clock = 1004 table.insert(r, tostring(ws.autosave_tick())) -- grace
         clock = 1005 table.insert(r, tostring(ws.autosave_tick())) -- open: same weight, written
         table.insert(r, saved_tabs('default'))
         return table.concat(r, ' ')",
    );
    assert_eq!(out, "false 7 false true 7");
}

#[test]
fn ask_keeps_gate_closed_and_shrink_is_held_90s() {
    let l = mux_lua();
    let out = s(
        &l,
        "put_session({ { 'default', 7 } })
         ws.setup({ dir = '/d', restore = 'ask', key = false, palette = false })
         handlers['gui-startup'](nil)
         local r = {}
         for _, c in ipairs({ 1000, 1050, 1150 }) do clock = c table.insert(r, tostring(ws.autosave_tick())) end
         table.insert(r, ws._state.gate)
         clock = 1200 selector.action(nil, nil, 'no')
         -- the fresh one-tab session is a shrink against the 7 tabs on disk: held for 90 s
         for _, c in ipairs({ 1205, 1235, 1265, 1294, 1295 }) do
           clock = c table.insert(r, tostring(ws.autosave_tick()) .. ':' .. saved_tabs('default')) end
         return table.concat(r, ' ')",
    );
    assert_eq!(out, "false false false restoring false:7 false:7 false:7 false:7 true:1");
}

#[test]
fn cancel_of_ask_opens_gate() {
    let l = mux_lua();
    let out = s(
        &l,
        "put_session({ { 'default', 3 } })
         ws.setup({ dir = '/d', restore = 'ask', key = false, palette = false })
         handlers['gui-startup'](nil)
         selector.action(nil, nil, nil)
         return ws._state.gate .. ' ' .. tabs_total()",
    );
    assert_eq!(out, "open 1");
}

#[test]
fn empty_mux_is_never_written() {
    let l = mux_lua();
    let out = s(
        &l,
        "ws.setup({ dir = '/d', restore = 'never', key = false, palette = false })
         clock = 2000
         local a = tostring(ws.autosave_tick())
         -- a workspace without panes
         windows = { { _ws = 'x', _tabs = {}, get_workspace = function() return 'x' end, tabs = function() return {} end } }
         clock = 2100
         local b = tostring(ws.autosave_tick())
         return a .. ' ' .. b .. ' ' .. #writes",
    );
    assert_eq!(out, "false false 0");
}

#[test]
fn update_status_tick_defers_foreground_info_and_caches_it() {
    let l = mux_lua();
    let out = s(
        &l,
        "fs['/d/default.json'] = ws.encode({ name = 'default', allow_commands = true, tabs = { { layout = { pane = { cwd = '/a' } } } } })
         package.loaded.wezterm.mux.spawn_window({ cwd = '/a', fg = '/usr/bin/htop' })
         package.loaded.wezterm.mux.spawn_window({ cwd = '/b' }) -- a shell: info never asked
         ws.setup({ dir = '/d', key = false, palette = false })
         assert(ws._state.gate == 'open', 'live windows: config reload path')
         local status = require('saysdf.status')
         status.add('x', function() return 'x' end)
         local window = { set_right_status = function() table.insert(spy, 'status') end,
                          set_left_status = function() end }
         clock = 3000 status.render(window, nil)
         clock = 3100 status.render(window, nil)
         return table.concat(spy, ',') .. ' ' .. #writes",
    );
    // the status is set first, the foreground info comes after it, once per pane and name
    assert_eq!(out, "status,info,status 2");
}

#[test]
fn attach_does_not_restore_and_opens_gate() {
    let l = mux_lua();
    let out = s(
        &l,
        "put_session({ { 'default', 7 } })
         ws.setup({ dir = '/d', key = false, palette = false })
         local g1 = ws._state.gate
         package.loaded.wezterm.mux.spawn_window({ cwd = '/srv' }) -- the server's pane
         handlers['gui-attached']()
         return g1 .. ' ' .. ws._state.gate .. ' ' .. tabs_total()",
    );
    assert_eq!(out, "startup open 1");
}

#[test]
fn startup_gate_times_out_without_gui_startup() {
    let l = mux_lua();
    let out = s(
        &l,
        "ws.setup({ dir = '/d', key = false, palette = false })
         package.loaded.wezterm.mux.spawn_window({ cwd = '/a' })
         local r = {}
         for _, c in ipairs({ 100, 219, 220, 225 }) do clock = c table.insert(r, tostring(ws.autosave_tick())) end
         return table.concat(r, ' ') .. ' ' .. ws._state.gate",
    );
    assert_eq!(out, "false false false true open");
}

#[test]
fn explicit_command_opens_on_top_of_restored_session() {
    let l = mux_lua();
    let out = s(
        &l,
        "put_session({ { 'default', 2 } })
         ws.setup({ dir = '/d', key = false, palette = false })
         handlers['gui-startup']({ cwd = '/here' })
         return cwds() .. ' ' .. #windows",
    );
    assert_eq!(out, "/p1,/p2,/here 2");
}

#[test]
fn corrupt_workspace_file_is_renamed_bad_and_skipped() {
    let l = mux_lua();
    let out = s(
        &l,
        "put_session({ { 'a', 2 }, { 'b', 'BAD{' } })
         ws.setup({ dir = '/d', key = false, palette = false })
         handlers['gui-startup'](nil)
         return cwds() .. ' ' .. tostring(fs['/d/b.json']) .. ' ' .. tostring(fs['/d/b.json.bad'])
           .. ' ' .. tostring(logs[1]:find('corrupt b', 1, true) ~= nil)",
    );
    assert_eq!(out, "/p1,/p2 nil BAD{ true");
}

// ---- change workspace-restore W-6: Claude Code session detection and storage ----

// Fake Claude Code projects folder and claude panes on top of MUX. Example data only.
const CLAUDE: &str = r#"
    local w = package.loaded.wezterm
    dirs = {} -- [mangled cwd] = entries of saysdf_dir_entries
    w.saysdf_dir_entries = function(dir)
      table.insert(spy, 'dir')
      local m = (dir:gsub('\\', '/')):match('([^/]+)$')
      return dirs[m] or {}
    end
    ws._claude_projects_dir = function() return '/home/me/.claude/projects' end
    -- start times in these tests are already epoch seconds
    REAL_START_UNIX = ws._process_start_unix
    ws._process_start_unix = function(t) return t end
    U1 = '11111111-1111-4111-8111-111111111111'
    U2 = '22222222-2222-4222-8222-222222222222'
    U0 = '00000000-0000-4000-8000-000000000000'
    EXE = '/home/me/.local/bin/claude'
    function file(id, t) return { path = '/home/me/.claude/projects/x/' .. id .. '.jsonl', modified = t, created = t } end
    function claude_pane(cwd, info)
      local _, p = w.mux.spawn_window({ cwd = cwd, fg = EXE })
      p.get_foreground_process_info = function() table.insert(spy, 'info') return info end
      return p
    end
    -- the claude field of every pane of the default workspace, in order
    function snap_ids()
      local r = {}
      for _, t in ipairs(ws.snapshot('default', false).tabs) do
        local c = t.layout.pane.claude
        table.insert(r, c and c.session:sub(1, 1) or '-')
      end
      return table.concat(r, ',')
    end
"#;

fn claude_lua() -> Lua {
    let l = mux_lua();
    l.load(CLAUDE).exec().unwrap();
    l
}

#[test]
fn claude_session_from_argv_is_stored_and_weighted() {
    let l = claude_lua();
    let out = s(
        &l,
        "dirs['-w-proj'] = { file(U2, 900), file(U1, 100) }
         claude_pane('/w/proj', { argv = { EXE, '--resume', U1 }, executable = EXE, start_time = 500 })
         local snap = ws.snapshot('default', false)
         local c = snap.tabs[1].layout.pane.claude
         local back = ws.decode(ws.encode(snap)).tabs[1].layout.pane.claude
         return c.session .. ' ' .. c.exe .. ' ' .. back.session .. ' ' .. ws.session_weight({ snap }).claude",
    );
    assert_eq!(
        out,
        "11111111-1111-4111-8111-111111111111 /home/me/.local/bin/claude 11111111-1111-4111-8111-111111111111 1"
    );
}

#[test]
fn claude_session_from_dir_entries_by_start_time() {
    let l = claude_lua();
    let out = s(
        &l,
        "dirs['-w-proj'] = { file(U2, 601), file(U1, 501) }
         claude_pane('/w/proj', { argv = { EXE }, executable = EXE, start_time = 500 })
         claude_pane('/w/proj', { argv = { EXE }, executable = EXE, start_time = 600 })
         package.loaded.wezterm.mux.spawn_window({ cwd = '/w/proj' }) -- a shell: no claude field
         return snap_ids()",
    );
    assert_eq!(out, "1,2,-");
}

// Live check: Windows FILETIME start time, cwd with a trailing slash from OSC 7, a continued
// session (file created long before the start, appended after it), one log line per process.
#[test]
fn claude_continued_session_with_filetime_start_and_trailing_slash() {
    let l = claude_lua();
    let out = s(
        &l,
        r#"local w = package.loaded.wezterm
         w.target_triple = 'x86_64-pc-windows-msvc'
         ws._process_start_unix = REAL_START_UNIX
         util.log_info = function(m) table.insert(logs, m) end
         dirs['C--w-proj'] = { { path = 'x/' .. U1 .. '.jsonl', created = 100, modified = 1700000100 },
                               { path = 'x/' .. U2 .. '.jsonl', created = 100, modified = 200 } }
         -- 1700000000 s after the Unix epoch as a FILETIME
         local ft = (1700000000 + 11644473600) * 10000000
         claude_pane('C:/w/proj/', { argv = { EXE }, executable = EXE, start_time = ft })
         dirs['C--w-other'] = { { path = 'x/' .. U0 .. '.jsonl', created = 100, modified = 200 } }
         claude_pane('C:/w/other', { argv = { EXE }, executable = EXE, start_time = ft })
         local r = snap_ids()
         clock = 5000 snap_ids()
         local n = 0
         for _, m in ipairs(logs) do
           if m:find('claude session not found (no file modified after start) in C--w-other', 1, true) then n = n + 1 end
         end
         return r .. " " .. n"#,
    );
    assert_eq!(out, "1,- 1");
}

#[test]
fn claude_ambiguous_start_gets_no_id_and_node_exe_is_not_stored() {
    let l = claude_lua();
    let out = s(
        &l,
        "dirs['-w-proj'] = { file(U2, 502), file(U1, 501) }
         dirs['-w-other'] = { file(U0, 501) }
         claude_pane('/w/proj', { argv = { EXE }, executable = EXE, start_time = 500 })
         claude_pane('/w/proj', { argv = { EXE }, executable = EXE, start_time = 501 })
         claude_pane('/w/other', { argv = { 'node', 'cli.js' }, executable = '/usr/bin/node', start_time = 500 })
         return snap_ids()",
    );
    assert_eq!(out, "-,-,-");
}

#[test]
fn claude_binding_is_sticky_and_detection_is_throttled() {
    let l = claude_lua();
    let out = s(
        &l,
        "dirs['-w-proj'] = { file(U1, 501) }
         local p = claude_pane('/w/proj', { argv = { EXE }, executable = EXE, start_time = 500 })
         local r = {}
         clock = 1000 table.insert(r, snap_ids() .. ':' .. #spy)
         -- an earlier file appears: by start time it would win, the binding keeps U1
         dirs['-w-proj'] = { file(U1, 501), file(U0, 500) }
         clock = 1010 table.insert(r, snap_ids() .. ':' .. #spy) -- throttled: no info, no dir
         clock = 1030 table.insert(r, snap_ids() .. ':' .. #spy) -- interval passed: re-checked, sticky
         -- claude left the foreground and came back (a new process): detected afresh at once
         p.get_foreground_process_name_cached = function() return '/bin/bash' end
         clock = 1031 table.insert(r, snap_ids() .. ':' .. #spy)
         p.get_foreground_process_name_cached = function() return EXE end
         clock = 1032 table.insert(r, snap_ids() .. ':' .. #spy)
         return table.concat(r, ' ')",
    );
    // without the binding the new process claims the earliest file >= start - 2 s (U0)
    assert_eq!(out, "1:2 1:2 1:4 -:4 0:6");
}

#[test]
fn claude_detection_runs_only_in_deferred_phase() {
    let l = claude_lua();
    let out = s(
        &l,
        "dirs['-w-proj'] = { file(U1, 501) }
         claude_pane('/w/proj', { argv = { EXE }, executable = EXE, start_time = 500 })
         ws.setup({ dir = '/d', key = false, palette = false })
         local status = require('saysdf.status')
         status.add('x', function() return 'x' end)
         local window = { set_right_status = function() table.insert(spy, 'status') end,
                          set_left_status = function() end }
         clock = 3000 status.render(window, nil)
         local saved = ws.decode(fs['/d/default.json']).tabs[1].layout.pane.claude
         return table.concat(spy, ',') .. ' ' .. saved.session:sub(1, 1) .. ' ' .. ws._state.autosave.saved_w.claude",
    );
    assert_eq!(out, "status,info,dir 1 1");
}

#[test]
fn invalid_claude_field_is_dropped_on_load() {
    let l = claude_lua();
    let out = s(
        &l,
        r#"local function pane(c) return { layout = { pane = { cwd = '/a', claude = c } } } end
         local text = package.loaded.wezterm.json_encode({ version = 1, name = 'd', tabs = {
           pane({ session = U1, exe = EXE }),
           pane({ session = 'not-a-uuid', exe = EXE }),
           pane({ session = string.upper(U1:gsub('1', 'a')), exe = EXE }),
           pane({ session = U1, exe = 'claude' }),
           pane({ session = U1, exe = '/usr/bin/node' }),
           pane('x'),
         } })
         local d = ws.decode(text)
         local r = {}
         for _, t in ipairs(d.tabs) do table.insert(r, t.layout.pane.claude and 'y' or 'n') end
         local n = 0
         for _, m in ipairs(logs) do if m:find('claude field dropped', 1, true) then n = n + 1 end end
         -- a Windows path is accepted
         local win = ws.valid_claude({ session = U1, exe = 'C:\\Users\\me\\.local\\bin\\claude.exe' })
         return table.concat(r, '') .. ' ' .. n .. ' ' .. tostring(win ~= nil)"#,
    );
    assert_eq!(out, "ynnnnn 5 true");
}

#[test]
fn project_file_cannot_carry_claude() {
    let l = claude_lua();
    let out = s(
        &l,
        "local p = ws.parse_project('/r', { tabs = { { cwd = '.', claude = { session = U1, exe = EXE } } } }, false)
         return tostring(p.tabs[1].claude) .. ' ' .. #p.commands",
    );
    assert_eq!(out, "nil 0");
}

#[test]
fn trust_store_programs_roundtrip() {
    let l = claude_lua();
    let out = s(
        &l,
        r#"local exe = 'C:\\Users\\me\\.local\\bin\\claude.exe'
         local store = ws.trust({}, 'C:\\Users\\me\\proj', { { cwd = '.', args = { 'make' } } }, true)
         ws.trust_program(store, exe, 'claude-resume', true)
         ws.trust_program(store, 'claude', 'claude-resume', true) -- relative: ignored
         ws.save_trust_store(store)
         local back = ws.trust_store()
         return table.concat({
           tostring(ws.is_program_trusted(back, 'c:/users/me/.local/bin/CLAUDE.exe', 'claude-resume', true)),
           tostring(ws.is_program_trusted(back, exe, 'other', true)),
           tostring(ws.is_program_trusted(back, 'claude', 'claude-resume', true)),
           tostring(ws.is_program_trusted({}, exe, 'claude-resume', true)),
           tostring(ws.is_trusted(back, 'C:\\Users\\me\\proj', { { cwd = '.', args = { 'make' } } }, true)),
         }, ' ')"#,
    );
    assert_eq!(out, "true false false false true");
}

// ---- change workspace-restore W-7: resume Claude Code sessions, ask once per executable ----

// A saved workspace with Claude panes, executables on a fake disk, the trust store. Example data only.
const RESUME: &str = r#"
    local w = package.loaded.wezterm
    exists = { [EXE] = true }
    ws._exists = function(p) return exists[p] == true end
    dirs['-w-proj'] = { file(U1, 1), file(U2, 2) }
    -- tabs: { cwd, session, exe?, args? } or { split = { a, b } }; written raw (not through to_data)
    function put_claude(tabs, allow)
      local function leafp(x) return { pane = { cwd = x[1], args = x[4], claude = { session = x[2], exe = x[3] or EXE } } } end
      local out = {}
      for _, e in ipairs(tabs) do
        if e.split then
          table.insert(out, { layout = { split = 'h', ratio = 0.5, a = leafp(e.split[1]), b = leafp(e.split[2]) } })
        else
          table.insert(out, { layout = leafp(e) })
        end
      end
      fs['/d/default.json'] = w.json_encode({ version = 1, name = 'default', allow_commands = allow, tabs = out })
      fs['/d/last-session.json'] = w.json_encode({ version = 1, workspaces = { 'default' } })
    end
    function trust(exe, kind)
      local st = ws.trust_store()
      ws.trust_program(st, exe, kind or 'claude-resume', false)
      ws.save_trust_store(st)
    end
    function kind_of(exe)
      local st = ws.trust_store()
      local e = st.programs and st.programs[ws.trust_key(exe, false)]
      return e and e.kind or 'none'
    end
    -- a fresh startup: what was spawned, joined with ' '
    function start(opts)
      opts = opts or {}
      opts.dir, opts.key, opts.palette = '/d', false, false
      windows, spawned, selector = {}, {}, nil
      ws.setup(opts)
      handlers['gui-startup'](nil)
      return table.concat(spawned, ' ')
    end
    function skipped()
      local n = 0
      for _, m in ipairs(logs) do
        if m:find('claude resume skipped', 1, true) or m:find('claude field dropped', 1, true) then n = n + 1 end
      end
      return n
    end
"#;

fn resume_lua() -> Lua {
    let l = claude_lua();
    l.load(RESUME).exec().unwrap();
    l
}

const A1: &str = "/home/me/.local/bin/claude|--resume|11111111-1111-4111-8111-111111111111";
const A2: &str = "/home/me/.local/bin/claude|--resume|22222222-2222-4222-8222-222222222222";

#[test]
fn trusted_exe_resumes_with_exact_argv_in_tabs_and_splits() {
    let l = resume_lua();
    let out = s(
        &l,
        "trust(EXE)
         -- saved args (allow_commands on) never reach a Claude pane's argv
         put_claude({ { '/w/proj', U1, nil, { 'rm', '-rf', '/' } }, { split = { { '/w/proj', U2 }, { '/w/proj', U1 } } } }, true)
         local r = start()
         return r .. ' ' .. tostring(selector) .. ' ' .. ws._state.gate",
    );
    assert_eq!(out, format!("{A1} {A2} {A1} nil open"));
}

#[test]
fn unknown_exe_is_asked_once_and_allow_is_remembered() {
    let l = resume_lua();
    let out = s(
        &l,
        "put_claude({ { '/w/proj', U1 }, { '/w/proj', U2 } })
         local r = { start(), ws._state.gate, tostring(selector.title:find(EXE, 1, true) ~= nil) }
         local sel = selector
         selector = nil
         sel.action(nil, nil, 'allow')
         -- one prompt for both panes: no second question
         table.insert(r, tostring(selector))
         table.insert(r, table.concat(spawned, ' '))
         table.insert(r, ws._state.gate)
         table.insert(r, kind_of(EXE))
         -- the next startup: no question, resumed at once
         table.insert(r, start())
         table.insert(r, tostring(selector))
         return table.concat(r, '\\n')",
    );
    assert_eq!(
        out,
        format!("shell\nrestoring\ntrue\nnil\nshell {A1} {A2}\nopen\nclaude-resume\n{A1} {A2}\nnil")
    );
}

#[test]
fn deny_is_remembered_and_reset_by_commands_toggle() {
    let l = resume_lua();
    let out = s(
        &l,
        "put_claude({ { '/w/proj', U1 }, { '/w/proj', U2 } })
         local r = { start() }
         selector.action(nil, nil, 'deny')
         table.insert(r, table.concat(spawned, ' '))
         table.insert(r, kind_of(EXE))
         -- no nagging: the next startup asks nothing and opens shells
         table.insert(r, start())
         table.insert(r, tostring(selector))
         -- palette 'allow/forbid commands' forgets the answer: asked again
         ws.toggle_commands('default')
         table.insert(r, kind_of(EXE))
         put_claude({ { '/w/proj', U1 }, { '/w/proj', U2 } }) -- the toggle saved the live (shell) layout
         start()
         table.insert(r, tostring(selector ~= nil))
         -- cancel: shells now, nothing stored, asked next time
         selector.action(nil, nil, nil)
         table.insert(r, kind_of(EXE) .. ':' .. ws._state.gate)
         start()
         table.insert(r, tostring(selector ~= nil))
         return table.concat(r, ' ')",
    );
    assert_eq!(
        out,
        "shell shell shell shell claude-resume-denied shell shell nil none true none:open true"
    );
}

#[test]
fn every_failed_check_opens_a_shell_and_logs() {
    // { file pane, startup options }; the exes are trusted, so only the named check fails
    let cases = [
        ("{ '/w/proj', 'x; rm -rf ~' }", "{}"),             // bad uuid
        ("{ '/w/proj', U0 }", "{}"),                        // no session file
        ("{ '/w/proj', U1, 'claude' }", "{}"),              // relative exe
        ("{ '/w/proj', U1, '/usr/bin/node' }", "{}"),       // wrong basename
        ("{ '/w/proj', U1, '/opt/claude' }", "{}"),         // exe does not exist
        ("{ '/w/proj', U1 }", "{ claude_resume = false }"), // option off
    ];
    for (pane, opts) in cases {
        let l = resume_lua();
        let out = s(
            &l,
            &format!(
                "trust(EXE) trust('/usr/bin/node') trust('/opt/claude')
                 put_claude({{ {pane} }}, true)
                 local r = start({opts})
                 for _, m in ipairs(logs) do assert(not m:find('rm -rf', 1, true), 'saved value in the log') end
                 return r .. ' ' .. skipped() .. ' ' .. tostring(selector)"
            ),
        );
        assert_eq!(out, "shell 1 nil", "case {pane} {opts}");
    }
}

#[test]
fn resume_args_is_the_only_argv_shape() {
    let l = resume_lua();
    let out = s(
        &l,
        "trust(EXE)
         local ctx = ws.resume_context()
         local c = { session = U1, exe = EXE, args = { '--dangerously-skip-permissions' }, extra = 'x' }
         local argv = ws.resume_args(c, '/w/proj', ctx)
         local _, pending = ws.resume_args({ session = U1, exe = EXE }, '/w/proj', { store = {}, sessions = {} })
         return #argv .. ' ' .. table.concat(argv, '|') .. ' ' .. pending",
    );
    assert_eq!(out, format!("3 {A1} pending"));
}

// ---- change workspace-restore W-8: manual save with preview ----

const MANUAL: &str = r#"
    notices, opened = {}, {}
    util.notify = function(_, title, text) table.insert(notices, text) end
    viewer_ok = true
    package.loaded['saysdf.viewer'] = { open = function(_, _, path) table.insert(opened, path) return viewer_ok end }
    ws.setup({ dir = '/d', key = false, palette = false, restore = 'ask' })
    -- one tab, two panes side by side (30% / 70%): a psql with a password and a plain tool
    function manual_ws()
      windows = {}
      local tab, pane, win = new_window({ cwd = '/w/api', fg = '/usr/bin/psql' })
      win.active_workspace = function() return 'default' end
      pane.get_foreground_process_info = function()
        return { argv = { '/usr/bin/psql', '--password=hunter2' }, executable = '/usr/bin/psql' }
      end
      pane.get_foreground_process_name_cached = function() return '/usr/bin/psql' end
      local second = pane.split(pane, { cwd = '/w/web', args = nil })
      second.get_foreground_process_info = function()
        return { argv = { '/usr/bin/htop', '-d', '5' }, executable = '/usr/bin/htop' }
      end
      second.get_foreground_process_name_cached = function() return '/usr/bin/htop' end
      tab.panes_with_info = function() return {
        { left = 0, top = 0, width = 30, height = 24, pane = pane },
        { left = 30, top = 0, width = 70, height = 24, pane = second } } end
      return win, pane
    end
"#;

#[test]
fn manual_save_masks_secret_command_writes_preview_and_opens_it() {
    let l = mux_lua();
    l.load(MANUAL).exec().unwrap();
    let out = s(
        &l,
        "ws.save('default', true) -- allow_commands on
         local win, pane = manual_ws()
         local r = ws.save_manual(win, pane)
         local json = fs['/d/default.json']
         local md = fs['/d/default.preview.md']
         return table.concat({
           tostring(json:find('hunter2', 1, true)), tostring(md:find('hunter2', 1, true)),
           tostring(json:find('htop', 1, true) ~= nil), tostring(json:find('psql', 1, true)),
           tostring(md:find('command not saved', 1, true) ~= nil),
           tostring(md:find('(left, 30%)', 1, true) ~= nil), tostring(md:find('(right, 70%)', 1, true) ~= nil),
           tostring(md:find('Restore at startup: ask', 1, true) ~= nil),
           opened[1]:gsub('\\\\', '/'), notices[1], #notices, r.tabs, r.panes, r.claude }, '|')",
    );
    assert_eq!(
        out,
        "nil|nil|true|nil|true|true|true|true|/d/default.preview.md|Saved: 1 tabs, 2 panes, 0 Claude sessions|1|1|2|0"
    );
}

#[test]
fn manual_save_without_viewer_notifies_the_preview_path() {
    let l = mux_lua();
    l.load(MANUAL).exec().unwrap();
    let out = s(
        &l,
        "viewer_ok = false
         local win, pane = manual_ws()
         ws.save_manual(win, pane)
         return #opened .. '|' .. #notices .. '|' .. notices[2]:gsub('\\\\', '/')",
    );
    assert_eq!(out, "1|2|Preview: /d/default.preview.md");
}

#[test]
fn manual_save_bypasses_the_shrink_guard() {
    let l = mux_lua();
    l.load(MANUAL).exec().unwrap();
    let out = s(
        &l,
        "put_session({ { 'default', 5 } })
         ws._state.autosave = { saved_w = ws.session_weight({ ws.decode(fs['/d/default.json']) }), saved_w_known = true }
         local win, pane = manual_ws()
         ws.save_manual(win, pane)
         return saved_tabs('default') .. ''",
    );
    assert_eq!(out, "1");
}
