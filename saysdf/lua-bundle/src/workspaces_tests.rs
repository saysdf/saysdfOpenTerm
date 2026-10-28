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
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
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
