//! Speed tests of the hot paths (perf-2): bar memoization, the git.root_of cache, no
//! action_callback registration on every tick, early exit of notify, the profiler, status.defer.
use super::*;

const FAKE: &str = r#"
    callbacks = 0
    handlers = {}
    logs = {}
    spawned = 0
    package.loaded.wezterm = {
        on = function(name, f) handlers[name] = handlers[name] or {}; table.insert(handlers[name], f) end,
        action_callback = function(f) callbacks = callbacks + 1; return { EmitEvent = 'user-defined-' .. callbacks } end,
        log_error = function(m) table.insert(logs, m) end,
        log_warn = function(m) table.insert(logs, m) end,
        log_info = function(m) table.insert(logs, m) end,
        format = function(items)
            local t = {}
            for _, it in ipairs(items) do table.insert(t, it.Text or "") end
            return table.concat(t)
        end,
        run_child_process = function() spawned = spawned + 1; return true, '{}', '' end,
        json_parse = function() return {} end,
        target_triple = 'x86_64-unknown-linux-gnu',
        home_dir = '/home/u',
        GLOBAL = {},
    }
    function make_pane(id, cwd)
        local p = { cwd = cwd, title = 'pwsh', fg_calls = 0 }
        function p:pane_id() return id end
        function p:get_current_working_dir() return self.cwd end
        function p:get_domain_name() return 'local' end
        function p:get_user_vars() return {} end
        function p:is_alt_screen_active() return false end
        function p:get_title() return self.title end
        function p:get_foreground_process_name() self.fg_calls = self.fg_calls + 1; return 'pwsh.exe' end
        p.cached_calls = 0
        function p:get_foreground_process_name_cached() self.cached_calls = self.cached_calls + 1; return self.fg_cached or 'pwsh.exe' end
        return p
    end
    function make_window(id)
        local w = { bars = 0, focused_calls = 0 }
        function w:window_id() return id end
        function w:get_selection_text_for_pane() return '' end
        function w:saysdf_set_bar(kind, spec) self.bars = self.bars + 1; self.last_kind = kind end
        function w:set_right_status() end
        function w:set_left_status() end
        function w:is_focused() self.focused_calls = self.focused_calls + 1; return false end
        return w
    end
"#;

fn lua() -> Lua {
    let names = [
        "saysdf.git",
        "saysdf.hotbar",
        "saysdf.i18n",
        "saysdf.menu",
        "saysdf.notify",
        "saysdf.palette",
        "saysdf.profile",
        "saysdf.ssh",
        "saysdf.status",
        "saysdf.util",
    ];
    let modules: Modules = Box::leak(
        names
            .iter()
            .map(|n| (*n, get(n).unwrap()))
            .collect::<Vec<_>>()
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
    l.load(code).eval().unwrap()
}

#[test]
fn palette_same_entries_same_version() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local palette = require('saysdf.palette')
        palette.add({ id = 'a.x', brief = 'X', hint = { contexts = { 'shell' } } })
        local dyn = 'one'
        palette.add(function() return { { id = 'b.y', brief = dyn } } end)
        local l1 = palette.entries()
        local v1 = palette.version()
        local l2 = palette.entries()
        local same = (l1 == l2) and palette.version() == v1
        dyn = 'two'
        local l3 = palette.entries()
        return tostring(same) .. ',' .. tostring(l3 ~= l1) .. ',' .. (palette.version() - v1)
    "#,
    );
    assert_eq!(r, "true,true,1");
}

#[test]
fn palette_sources_do_not_register_callbacks_per_call() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local palette = require('saysdf.palette')
        require('saysdf.notify').setup({}, {})
        require('saysdf.ssh').setup({}, {})
        palette.entries()
        local after_first = callbacks
        for _ = 1, 10 do palette.entries() end
        return tostring(after_first > 0) .. ',' .. (callbacks - after_first)
    "#,
    );
    assert_eq!(r, "true,0");
}

#[test]
fn hotbar_same_input_is_not_rebuilt() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local palette = require('saysdf.palette')
        palette.add({ id = 'a.x', brief = 'X', hint = { contexts = { 'shell' } } })
        local hotbar = require('saysdf.hotbar')
        hotbar.setup({}, {})
        local win, pane = make_window(1), make_pane(1, nil)
        local right = { { Text = 'main' } }
        hotbar.consume(win, pane, right)
        hotbar.consume(win, pane, right)
        hotbar.consume(win, pane, right)
        local b1, bars1 = hotbar._state().builds, win.bars
        hotbar.consume(win, pane, { { Text = 'main ~1' } })
        return b1 .. ',' .. bars1 .. ',' .. hotbar._state().builds .. ',' .. win.bars
    "#,
    );
    assert_eq!(r, "1,1,2,2");
}

#[test]
fn menu_same_registry_version_skips_build() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local palette = require('saysdf.palette')
        palette.add({ id = 'a.x', brief = 'X', menu = 'File', accel = 'x', action = 'A' })
        local menu = require('saysdf.menu')
        local builds = 0
        local orig = menu.build
        menu.build = function(...) builds = builds + 1; return orig(...) end
        local win, pane = make_window(1), make_pane(1, nil)
        menu.push(win, pane, false)
        menu.push(win, pane, false)
        menu.push(win, pane, false)
        local b1 = builds
        palette.add({ id = 'a.y', brief = 'Y', menu = 'File', accel = 'y', action = 'A' })
        menu.push(win, pane, false)
        return b1 .. ',' .. builds .. ',' .. win.bars
    "#,
    );
    assert_eq!(r, "1,2,2");
}

#[test]
fn git_root_of_cached_by_cwd() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local git = require('saysdf.git')
        git.setup({ keys = false, palette = false }, {})
        local probes = {}
        local has_git = { ['/r/.git'] = true }
        local function exists(p) table.insert(probes, p); return has_git[p] == true end
        local pane = make_pane(1, '/r/a/b')
        local out = {}
        table.insert(out, tostring(git.root_of(pane, true, exists)))
        table.insert(out, #probes)                 -- /r/a/b, /r/a, /r
        git.root_of(pane, true, exists)
        git.root_of(pane, true, exists)
        table.insert(out, #probes)                 -- cache: no filesystem
        pane.cwd = '/r/a/c'
        table.insert(out, tostring(git.root_of(pane, true, exists)))
        table.insert(out, #probes)                 -- only /r/a/c, /r/a is already known
        git.invalidate(pane)                       -- end of a command
        git.root_of(pane, true, exists)
        table.insert(out, #probes)                 -- one check: /r/.git is still there
        has_git['/r/.git'] = nil
        git.invalidate(pane)
        table.insert(out, tostring(git.root_of(pane, true, exists)))
        return table.concat(out, ',')
    "#,
    );
    assert_eq!(r, "/r,3,3,/r,4,5,nil");
}

#[test]
fn notify_short_command_does_not_query_window() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local notify = require('saysdf.notify')
        notify.setup({ palette = false }, {})
        local win, pane = make_window(1), make_pane(1, nil)
        notify.on_command_finished(win, pane, { exit_code = 0, duration_ms = 50 })
        local short = win.focused_calls
        notify.on_command_finished(win, pane, { exit_code = 0, duration_ms = 60000 })
        return short .. ',' .. tostring(win.focused_calls > 0)
    "#,
    );
    assert_eq!(r, "0,true");
}

// perf-3: the process name is a non-blocking fork method, without walking processes on the GUI
// thread and without its own cache in Lua (a new value is visible on the next call).
#[test]
fn foreground_uses_cached_method() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local util = require('saysdf.util')
        local pane = make_pane(7, nil)
        local a = util.pane_foreground(pane).process
        pane.fg_cached = 'ssh.exe'
        local b = util.pane_foreground(pane).process
        return a .. ',' .. b .. ',' .. pane.cached_calls .. ',' .. pane.fg_calls
    "#,
    );
    assert_eq!(r, "pwsh.exe,ssh.exe,2,0");
}

#[test]
fn status_defer_runs_after_render() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local status = require('saysdf.status')
        local order = {}
        status.add('slow', function()
            status.defer(function() table.insert(order, 'deferred') end)
            return 'S'
        end)
        status.hook('bar', function() table.insert(order, 'hook') end)
        handlers['update-status'][1](make_window(1), make_pane(1, nil))
        status.defer(function() table.insert(order, 'outside') end)
        return table.concat(order, ',')
    "#,
    );
    assert_eq!(r, "hook,deferred,outside");
}

#[test]
fn profile_records_handlers_without_child_wait() {
    let l = lua();
    let r = s(
        &l,
        r#"
        local profile = require('saysdf.profile')
        local lines = {}
        assert(profile.enabled(function() return '1' end))
        assert(not profile.enabled(function() return nil end))
        local w = require('wezterm')
        local t = 0
        profile.now_ms = function() return t end
        w.run_child_process = function() t = t + 100; return true, '', '' end
        profile.install(w, { log = function(m) table.insert(lines, m) end, quiet = true })
        w.on('update-status', function() t = t + 2; w.run_child_process({}) end)
        local co = coroutine.create(function() handlers['update-status'][1]() end)
        assert(coroutine.resume(co))
        local st = profile._stats()['ev:update-status@config']
        profile.flush(true)
        return st.n .. ',' .. st.total .. ',' .. st.wait .. ',' .. #lines
    "#,
    );
    assert_eq!(r, "1,2,100,1");
}

// Base menu entries follow the language of the Lua state; the Russian UI is unchanged by the
// migration.
#[test]
fn texts_follow_language() {
    let run = |lang: &str| -> String {
        let l = lua();
        l.load(format!(
            r#"
            require('saysdf.i18n').select({lang:?})
            local callable = setmetatable({{}}, {{ __call = function() return {{}} end }})
            package.loaded.wezterm.action = setmetatable({{}}, {{ __index = function() return callable end }})
            local menu = require('saysdf.menu')
            menu.setup({{}}, {{}})
            local palette = require('saysdf.palette')
            local quit = palette.by_id(nil, nil, 'core.quit')
            local pal = palette.by_id(nil, nil, 'core.palette')
            local titles = {{}}
            for _, m in ipairs(menu.build(palette.entries(nil, nil))) do table.insert(titles, m.title) end
            return quit.label .. '|' .. pal.hint.label .. '|' .. table.concat(titles, ',')
            "#
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(run("en"), "Quit|Palette|File,Tabs,Tools,View,Help");
    // The Russian UI is unchanged by the migration
    assert_eq!(
        run("ru"),
        "Выход|Палитра|Файл,Вкладки,Инструменты,Вид,Справка" // i18n-allow-cyrillic
    );
}
