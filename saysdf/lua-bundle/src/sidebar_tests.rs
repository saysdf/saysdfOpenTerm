//! Tests of `saysdf.sidebar` (change tool-sidebar): open/toggle/replace/narrow tab on mocks,
//! checks of the result channel (D3) -- forgeries are rejected, a valid value is executed.
use super::*;

const FAKE: &str = r#"
    panes = {}
    actions = {}
    emitted = {}
    written = {}
    next_id = 10
    local function mk_tab(id, cols) return { id = id, cols = cols,
        tab_id = function(self) return self.id end,
        get_size = function(self) return { cols = self.cols, rows = 30 } end,
        set_zoomed = function() end,
        active_pane = function(self) return panes[1] end } end
    tab = mk_tab(1, 200)
    function mk_pane(id, t, fg)
        local p = { id = id, t = t, fg = fg,
            pane_id = function(self) return self.id end,
            tab = function(self) return self.t end,
            activate = function(self) active = self.id end,
            get_foreground_process_name_cached = function(self) return self.fg end,
            split = function(self, a)
                next_id = next_id + 1
                last_split = a
                return mk_pane(next_id, self.t, 'C:\\bin\\saysdf.exe')
            end }
        panes[id] = p
        return p
    end
    main = mk_pane(1, tab, 'pwsh.exe')
    other = mk_pane(2, tab, 'pwsh.exe')
    window = {
        perform_action = function(self, a, p)
            table.insert(actions, a)
            if a.close then panes[active] = nil end
        end,
        toast_notification = function() end,
    }
    package.loaded.wezterm = {
        GLOBAL = {},
        on = function() end,
        target_triple = 'x86_64-unknown-linux-gnu', home_dir = '/home/u',
        log_info = function() end, log_error = function() end, log_warn = function() end,
        json_encode = function(t) return 'JSON' .. #t end,
        emit = function(...) table.insert(emitted, { ... }) end,
        action_callback = function(f) return { cb = f } end,
        action = {
            CloseCurrentPane = function(o) return { close = true } end,
            InputSelector = function(s) return { input_selector = s } end,
        },
        mux = { get_pane = function(id) return panes[id] end, get_tab = function() return tab end },
    }
    util = require('saysdf.util')
    util.find_bin_cached = function() return '/bin/saysdf' end
    util.ensure_dir = function() end
    util.write_atomic = function(path, text) written[path] = text return true end
    require('saysdf.i18n').select('en')
    sb = require('saysdf.sidebar')
    sb.setup({}, {})
    picked = {}
    spec = { tool = 't', title = 'T', choices = { { id = 'a', label = 'A' }, { id = 'b', label = 'B' } },
        on_choice = function(w, p, id, label) table.insert(picked, { pane = p:pane_id(), id = id, label = label }) end }
    function open_pick() return sb.selector(spec).cb(window, main) end
    function sp() local tid, st = next(sb._state()) return st end
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.palette", get("saysdf.palette").unwrap()),
            ("saysdf.sidebar", get("saysdf.sidebar").unwrap()),
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

fn s(l: &Lua, code: &str) -> String {
    l.load(code).eval::<String>().unwrap()
}

#[test]
fn open_toggle_replace() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"open_pick()
            local st = sp()
            local a = last_split
            local r = {}
            table.insert(r, tostring(st.pane_id) .. ',' .. a.direction .. ',' .. a.size .. ',' .. tostring(a.top_level)
                .. ',' .. a.domain .. ',' .. a.args[1] .. ',' .. a.args[2] .. ',' .. #st.token
                .. ',' .. tostring(a.set_environment_variables.SAYSDF_SIDEBAR_TOKEN == st.token))
            -- list file: the name has the token, --items is in args
            table.insert(r, tostring(written[a.args[#a.args]] ~= nil) .. ',' .. a.args[#a.args - 1])
            -- repeat from the origin pane: focus the sidebar
            table.insert(r, sb.toggle(window, main, 't'))
            -- repeat from the sidebar: close it, focus goes back
            table.insert(r, sb.toggle(window, panes[st.pane_id] or main, 't') .. ',' .. tostring(active))
            table.insert(r, tostring(next(sb._state())))
            -- replace: 't' is open, another tool is called
            open_pick()
            local first = sp().pane_id
            sb.selector({ tool = 'u', title = 'U', choices = {}, on_choice = function() end }).cb(window, main)
            table.insert(r, tostring(panes[first] == nil) .. ',' .. sp().tool .. ',' .. tostring(sb.is_sidebar(panes[sp().pane_id])))
            return table.concat(r, '|')"#
        ),
        "11,Right,40,true,DefaultDomain,/bin/saysdf,pick,32,true|true,--items|focused|closed,1|nil|true,u,true"
    );
}

/// A config reload recreates the module: the state comes from wezterm.GLOBAL, a second
/// sidebar is not opened (toggle focuses/closes the existing one).
#[test]
fn state_survives_config_reload() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"open_pick()
            local first = sp().pane_id
            local splits = next_id
            -- config reload: a new module instance
            package.loaded['saysdf.sidebar'] = nil
            sb = require('saysdf.sidebar')
            sb.setup({}, {})
            local r = { tostring(next(sb._state())) }
            -- repeat from the origin pane: focus the open one, no new split
            table.insert(r, tostring(sb.toggle(window, main, 't')) .. ',' .. tostring(next_id == splits)
                .. ',' .. tostring(active == first) .. ',' .. tostring(sb.is_sidebar(panes[first])))
            -- a pick result after the reload is rejected (callback lost), the token did not leak
            table.insert(r, tostring(sb.handle_user_var(window, panes[first], 'SAYSDF_SIDEBAR',
                sp().token .. ':pick:a')))
            -- repeat from the sidebar: close it, the record is removed from GLOBAL too
            table.insert(r, sb.toggle(window, panes[first], 't') .. ',' .. tostring(panes[first] == nil)
                .. ',' .. tostring(package.loaded.wezterm.GLOBAL.saysdf_sidebar_1))
            -- open again and close the pane by hand: the record is not restored after a reload
            open_pick()
            local second = sp().pane_id
            panes[second] = nil
            package.loaded['saysdf.sidebar'] = nil
            sb = require('saysdf.sidebar')
            sb.setup({}, {})
            table.insert(r, tostring(sb.state_for(1)) .. ',' .. tostring(package.loaded.wezterm.GLOBAL.saysdf_sidebar_1))
            return table.concat(r, '|')"#
        ),
        "nil|focused,true,true,true|false|closed,true,nil|nil,nil"
    );
}

#[test]
fn narrow_and_fallback() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local r = { tostring(sb.width_for(200)), tostring(sb.width_for(60)), tostring(sb.width_for(39)) }
            tab.cols = 30
            open_pick()
            table.insert(r, tostring(actions[#actions].input_selector ~= nil) .. ',' .. tostring(next(sb._state())))
            tab.cols = 200
            util.find_bin_cached = function() return nil end
            open_pick()
            table.insert(r, tostring(actions[#actions].input_selector.title))
            sb._reset() sb.setup({ enabled = false }, {})
            table.insert(r, tostring(sb.selector(spec).input_selector ~= nil))
            sb._reset() -- saysdf.setup({ sidebar = false }): setup was not called
            table.insert(r, tostring(sb.selector(spec).input_selector ~= nil))
            return table.concat(r, '|')"#
        ),
        "40|30|nil|true,nil|T|true|true"
    );
}

#[test]
fn channel_rejects_forgeries() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"open_pick()
            local st = sp()
            local side = panes[st.pane_id]
            local tok = st.token
            local V = 'SAYSDF_SIDEBAR'
            local r = {}
            local function try(p, name, v) table.insert(r, tostring(sb.handle_user_var(window, p, name, v))) end
            try(other, V, tok .. ':pick:a')                -- foreign pane
            try(side, 'OTHER', tok .. ':pick:a')           -- foreign variable
            try(side, V, 'deadbeef:pick:a')                -- wrong token
            try(side, V, tok .. ':pick:zzz')               -- id not in the list
            try(side, V, tok .. ':pick:' .. string.rep('a', 5000)) -- length
            try(side, V, tok .. ':action:theme.x:1')       -- no palette entry
            side.fg = nil  try(side, V, tok .. ':pick:a')  -- foreground unknown
            side.fg = '/usr/bin/vim' try(side, V, tok .. ':pick:a') -- not saysdf
            table.insert(r, #picked)
            side.fg = 'C:\\x\\saysdf.exe'
            try(side, V, tok .. ':pick:b')                 -- valid
            try(side, V, tok .. ':pick:b')                 -- token reuse
            table.insert(r, #picked .. ',' .. picked[1].id .. ',' .. picked[1].label .. ',' .. picked[1].pane)
            return table.concat(r, '|')"#
        ),
        "false|false|false|false|false|false|false|false|0|true|false|1,b,B,1"
    );
}

#[test]
fn cancel_and_palette_action() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local palette = require('saysdf.palette')
            ran = {}
            palette.add({ id = 'x.plain', brief = 'p', action = 'A' })
            palette.add({ id = 'x.ok', brief = 'ok', sidebar = true, action = 'B',
                accept = function(arg) return arg == 'good' end,
                sidebar_run = function(w, p, arg) table.insert(ran, arg .. '@' .. p:pane_id()) end })
            local r = {}
            open_pick()
            local st = sp()
            local side = panes[st.pane_id]
            table.insert(r, tostring(sb.handle_user_var(window, side, 'SAYSDF_SIDEBAR', st.token .. ':pick:')))
            table.insert(r, #picked .. ',' .. tostring(picked[1].id))
            panes[st.pane_id] = nil -- saysdf exited, the pane closed
            open_pick()
            st = sp() side = panes[st.pane_id]
            local V = 'SAYSDF_SIDEBAR'
            table.insert(r, tostring(sb.handle_user_var(window, side, V, st.token .. ':action:x.plain:good')))
            table.insert(r, tostring(sb.handle_user_var(window, side, V, st.token .. ':action:x.ok:bad')))
            table.insert(r, tostring(sb.handle_user_var(window, side, V, st.token .. ':action:x.ok:good')))
            table.insert(r, table.concat(ran, ','))
            return table.concat(r, '|')"#
        ),
        "true|1,nil|false|false|true|good@1"
    );
}

#[test]
fn plain_labels_and_nested_selector_after_pick() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local r = { sb.plain_label('\27[1mgit\27[0m \27]8;;x\7host\27]8;;\7 ok') }
            open_pick()
            local first = sp().pane_id
            -- the choice is delivered (used): the same tool from the callback opens a new pane, not the departing one's focus
            sp().used = true
            open_pick()
            table.insert(r, tostring(sp().pane_id ~= first) .. ',' .. tostring(sp().used))
            return table.concat(r, '|')"#
        ),
        "git host ok|true,false"
    );
}

#[test]
fn texts_follow_language() {
    let l = lua();
    let run = |lang: &str| -> String {
        l.load(format!(
            "require('saysdf.i18n').select({lang:?})
             local palette = require('saysdf.palette')
             palette._reset()
             sb._reset()
             sb.setup({{ test_entry = true }}, {{}})
             local labels = {{}}
             for _, e in ipairs(palette.entries(window, main)) do
                 if e.id == 'sidebar.toggle' or e.id == 'sidebar.test' then
                     table.insert(labels, e.label .. '/' .. e.brief)
                 end
             end
             table.sort(labels)
             local c = sb.test_choices()
             local notified = nil
             util.notify = function(_, _, text) notified = text end
             sb.toggle_last(window, main)
             return table.concat(labels, ',') .. '|' .. c[1].label .. '|' .. c[3].label .. '|' .. notified"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(
        run("en"),
        "Sidebar test/Sidebar test,Sidebar/Sidebar: last tool|First item|\
         Third item: a long label to check truncation|Sidebar: no tool has been opened yet"
    );
    // The Russian UI is unchanged by the migration
    assert_eq!(
        run("ru"),
        concat!(
            "Боковая панель/Боковая панель: последний инструмент,Тест боковой панели/Тест боковой панели|", // i18n-allow-cyrillic
            "Первый пункт|Третий пункт: длинная подпись для проверки обрезки|", // i18n-allow-cyrillic
            "Боковая панель: инструмент ещё не открывался" // i18n-allow-cyrillic
        )
    );
    l.load("require('saysdf.i18n').select('en')").exec().unwrap();
}
