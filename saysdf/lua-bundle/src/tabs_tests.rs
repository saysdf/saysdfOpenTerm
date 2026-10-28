//! Tests of the `saysdf.tabs` module (change tab-management, tasks 1.2-1.4, 2.1, 2.3, 3.1-3.2, 4.1).
use super::*;

const FAKE: &str = r#"
    handlers = {}
    warnings = {}
    errors = {}
    prompts = {}
    package.loaded.wezterm = {
        on = function(name, f) handlers[name] = f end,
        log_warn = function(msg) table.insert(warnings, msg) end,
        log_error = function(msg) table.insert(errors, msg) end,
        config_builder = function() return {} end,
        action_callback = function(f) return { callback = f } end,
        action = setmetatable({}, { __index = function(_, k)
            return function(arg) return { [k] = arg } end
        end }),
        mux = { all_windows = function() return {} end },
    }
    tabs = require('saysdf.tabs')
"#;

fn lua_with_git(git: Option<&'static str>) -> Lua {
    let mut list = vec![
        ("saysdf", get("saysdf").unwrap()),
        ("saysdf.i18n", get("saysdf.i18n").unwrap()),
        ("saysdf.palette", get("saysdf.palette").unwrap()),
        ("saysdf.tabs", get("saysdf.tabs").unwrap()),
            ("saysdf.util", get("saysdf.util").unwrap()),
    ];
    if let Some(src) = git {
        list.push(("saysdf.git", src));
    }
    list.sort();
    let modules: Modules = Box::leak(list.into_boxed_slice());
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

fn lua() -> Lua {
    lua_with_git(None)
}

fn s(l: &Lua, code: &str) -> String {
    l.load(code).eval().unwrap()
}

fn b(l: &Lua, code: &str) -> bool {
    l.load(code).eval().unwrap()
}

#[test]
fn icons_known_unknown_override_text() {
    let l = lua();
    assert_eq!(s(&l, "return tabs.icon_for('pwsh.exe')"), "\u{f489}");
    assert_eq!(s(&l, "return tabs.icon_for('/usr/bin/zsh')"), "\u{f489}");
    assert_eq!(s(&l, "return tabs.icon_for('C:\\\\bin\\\\nvim.exe')"), "\u{e62b}");
    assert_eq!(s(&l, "return tabs.icon_for('python3')"), "\u{e606}");
    assert_eq!(s(&l, "return tabs.icon_for('mytool.exe')"), "\u{f013}");
    assert_eq!(s(&l, "return tabs.icon_for('')"), "\u{f013}");
    assert_eq!(s(&l, "return tabs.icon_for('htop', { icons = { htop = 'H' } })"), "H");
    assert_eq!(s(&l, "return tabs.icon_for('cargo', { nerd_font = false })"), "rs");
    assert_eq!(s(&l, "return tabs.icon_for('bash', { nerd_font = false })"), ">_");
    assert_eq!(s(&l, "return tabs.icon_for('x', { nerd_font = false })"), "*");
}

#[test]
fn truncate_multibyte() {
    let l = lua();
    assert_eq!(s(&l, "return tabs.truncate('abc', 5)"), "abc");
    assert_eq!(s(&l, "return tabs.truncate('abcdef', 4)"), "abc…");
    assert_eq!(s(&l, "return tabs.truncate('привет мир', 7)"), "привет…"); // i18n-allow-cyrillic
    assert_eq!(s(&l, "return tabs.truncate('привет', 6)"), "привет"); // i18n-allow-cyrillic
    assert_eq!(s(&l, "return tabs.truncate('привет', 1)"), "…"); // i18n-allow-cyrillic
    assert_eq!(s(&l, "return tabs.truncate('привет', 0)"), ""); // i18n-allow-cyrillic
}

#[test]
fn build_title_variants() {
    let l = lua();
    let o = "{ nerd_font = false }";
    // Windows path, Unix path, file:// URL with percent-encoding, Url object
    assert_eq!(
        s(&l, &format!("return tabs.build_title({{}}, {{ foreground_process_name = 'pwsh.exe', current_working_dir = 'C:\\\\dev\\\\saysdfTerminal\\\\' }}, {o}, {{ branch = 'main' }})")),
        ">_ saysdfTerminal main"
    );
    assert_eq!(
        s(&l, &format!("return tabs.build_title({{}}, {{ foreground_process_name = 'zsh', current_working_dir = '/home/u/proj' }}, {o})")),
        ">_ proj"
    );
    assert_eq!(
        s(&l, &format!("return tabs.build_title({{}}, {{ foreground_process_name = 'nvim', current_working_dir = 'file://host/C:/dev/my%20app/' }}, {o})")),
        "vi my app"
    );
    assert_eq!(
        s(&l, &format!("return tabs.build_title({{}}, {{ foreground_process_name = 'bash', current_working_dir = {{ file_path = '/srv/x' }} }}, {o})")),
        ">_ x"
    );
    // without cwd: the program title, no branch, no empty separators
    assert_eq!(
        s(&l, &format!("return tabs.build_title({{}}, {{ foreground_process_name = '', title = 'ssh host' }}, {o}, {{ branch = 'main' }})")),
        "ssh ssh host"
    );
    assert_eq!(s(&l, &format!("return tabs.build_title({{}}, {{}}, {o})")), "*");
    // the manual name wins over everything
    assert_eq!(
        s(&l, &format!("return tabs.build_title({{ tab_title = 'prod-db' }}, {{ title = 'vim README.md', current_working_dir = '/x' }}, {o})")),
        "prod-db"
    );
}

#[test]
fn git_soft_dependency() {
    let l = lua();
    assert!(b(&l, "return tabs.branch_of(nil, {}) == nil"));
    assert!(b(&l, "return tabs.branch_of({}, {}) == nil"));
    assert!(b(&l, "return tabs.branch_of({ info = function() error('x') end }, {}) == nil"));
    assert_eq!(s(&l, "return tabs.branch_of({ info = function() return { branch = 'dev' } end }, {})"), "dev");
    assert_eq!(s(&l, "return tabs.branch_of({ info = function() return 'feat' end }, {})"), "feat");

    // with the saysdf.git module the branch ends up in the title
    let l = lua_with_git(Some("return { info = function(p) return { branch = 'main' } end }"));
    l.load("tabs.setup({ nerd_font = false }, {})").exec().unwrap();
    let t = s(&l, "local items = handlers['format-tab-title']({ tab_id = 1, is_active = true, active_pane = { foreground_process_name = 'bash', current_working_dir = '/p/repo' } }, {}, {}, {}, false, 40) return items[1].Text");
    assert_eq!(t, " >_ repo main ");
}

#[test]
fn indicators() {
    let l = lua();
    let ind = |tab: &str, pane: &str| -> String {
        s(&l, &format!(
            "local r = tabs.indicators_for({tab}, {pane}, {{}}, {{}}) \
             return (r.running and 'R' or '') .. (r.error and 'E' or '') .. (r.unseen and 'U' or '') .. (r.attention and 'A' or '')"
        ))
    };
    assert_eq!(ind("{ tab_id = 1 }", "{ foreground_process_name = 'pwsh.exe' }"), "");
    assert_eq!(ind("{ tab_id = 1 }", "{ foreground_process_name = 'cargo.exe' }"), "R");
    assert_eq!(ind("{ tab_id = 1 }", "{ foreground_process_name = 'bash', user_vars = { SAYSDF_LAST_EXIT = '1' } }"), "E");
    assert_eq!(ind("{ tab_id = 1 }", "{ foreground_process_name = 'bash', user_vars = { SAYSDF_LAST_EXIT = '0' } }"), "");
    // without shell-integration there is no error indicator
    assert_eq!(ind("{ tab_id = 1 }", "{ foreground_process_name = 'bash', user_vars = {} }"), "");
    assert_eq!(ind("{ tab_id = 1 }", "{ has_unseen_output = true }"), "U");
    assert_eq!(ind("{ tab_id = 1, is_active = true }", "{ has_unseen_output = true, user_vars = { SAYSDF_LAST_EXIT = '2' } }"), "");
}

#[test]
fn error_reset_on_activation() {
    let l = lua();
    let r: (bool, bool, bool, bool) = l
        .load(
            r#"
            local st = {}
            local pane = { foreground_process_name = 'bash', user_vars = { SAYSDF_LAST_EXIT = '1', SAYSDF_LAST_DURATION_MS = '5' } }
            local before = tabs.indicators_for({ tab_id = 7 }, pane, st).error
            local changed = tabs.mark_seen(st, { tab_id = 7, is_active = true }, pane)
            local after = tabs.indicators_for({ tab_id = 7 }, pane, st).error
            -- a new failed command (different duration): the indicator again
            pane.user_vars.SAYSDF_LAST_DURATION_MS = '9'
            local again = tabs.indicators_for({ tab_id = 7 }, pane, st).error
            return before, changed, after, again
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(r, (true, true, false, true));
}

#[test]
fn attention_api_and_color_api() {
    let l = lua();
    let r: (bool, bool, String, bool, bool, bool, usize) = l
        .load(
            r#"
            tabs.set_attention(3, 'ssh: disconnected')
            local st = tabs._load()
            local on = tabs.indicators_for({ tab_id = 3 }, {}, st).attention
            tabs.mark_seen(st, { tab_id = 3, is_active = true }, {})
            local off = tabs.indicators_for({ tab_id = 3 }, {}, st).attention
            tabs.set_color({ tab_id = function() return 3 end }, 'red')
            local red = tabs.get_color(3)
            local bad = tabs.set_color(3, 'nope')
            tabs.set_color(3, '#123456')
            local hex = tabs.get_color(3) == '#123456'
            tabs.set_color(3, nil)
            return on, off, red, bad, hex, tabs.get_color(3) == nil, #warnings
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(r, (true, false, "#e06c75".into(), false, true, true, 1));
}

#[test]
fn prune_closed_tabs() {
    let l = lua();
    let r: (bool, bool, bool, bool) = l
        .load(
            r#"
            local st = { colors = { ['1'] = '#fff', ['2'] = '#000' }, seen = { ['2'] = 'x' }, attention = { ['5'] = true } }
            local changed = tabs.prune(st, { 1 })
            return changed, st.colors['1'] ~= nil, st.colors['2'] == nil and st.seen['2'] == nil, st.attention['5'] == nil
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(r, (true, true, true, true));
}

#[test]
fn format_items_color_and_marks() {
    let l = lua();
    let r: (String, String, String, String) = l
        .load(
            r#"
            local a = tabs.format_items('build', { error = true }, '#e06c75', true, { nerd_font = false }, 40)
            local i = tabs.format_items('build', { unseen = true }, '#e06c75', false, { nerd_font = false }, 40)
            local off = tabs.format_items('build', { unseen = true }, nil, false, { nerd_font = false, indicators = { unseen = false } }, 40)
            return a[1].Background.Color, a[3].Text .. a[4].Text, i[1].Foreground.Color, off[#off].Text
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(r, ("#e06c75".into(), " build ! ".into(), "#e06c75".into(), " build ".into()));
    // truncation accounts for the indicator width
    let t = s(&l, "local it = tabs.format_items('abcdefghij', { running = true }, nil, false, { nerd_font = false }, 8) return it[1].Text");
    assert_eq!(t, " abc… ");
}

#[test]
fn setup_registers_handlers_and_keys() {
    let l = lua();
    let r: (bool, bool, bool, usize, String, String) = l
        .load(
            r#"
            local c = tabs.setup({}, {})
            local k = c.keys[1]
            return handlers['format-tab-title'] ~= nil, handlers['saysdf-tab-double-click'] ~= nil,
                handlers['augment-command-palette'] ~= nil, #c.keys, k.key, k.mods
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(
        r,
        (true, true, true, 2, "E".into(), "CTRL|SHIFT".into())
    );
    let n: usize = l
        .load("return #(tabs.setup({ rename_key = false }, { keys = {} }).keys)")
        .eval()
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn format_handler_falls_back_on_error() {
    let l = lua();
    let t = s(
        &l,
        "tabs.setup({}, {}) \
         tabs.build_title = function() error('boom') end \
         return handlers['format-tab-title']({ tab_id = 1, tab_title = '', active_pane = { title = 'pwsh' } }, {}, {}, {}, false, 20)",
    );
    assert_eq!(t, "pwsh");
    assert!(b(&l, "return #errors == 1"));
}

/// Window/tab stubs for renaming and double click.
const UI: &str = r#"
    function make_tab(id, title)
        local t = { id = id, title = title or '' }
        function t:tab_id() return self.id end
        function t:get_title() return self.title end
        function t:set_title(s) self.title = s end
        function t:active_pane() return { get_current_working_dir = function() return '/w/proj' end, get_title = function() return 'x' end } end
        return t
    end
    t0, t1 = make_tab(10, 'old'), make_tab(11)
    performed = {}
    win = {
        mux_window = function() return { tabs = function() return { t0, t1 } end } end,
        active_tab = function() return t0 end,
        perform_action = function(self, a, p) table.insert(performed, a) end,
    }
    pane = { tab = function() return t0 end }
"#;

#[test]
fn rename_prompt_set_cancel_reset() {
    let l = lua();
    l.load(UI).exec().unwrap();
    let r: (String, String, String, String) = l
        .load(
            r#"
            tabs.rename(win, pane)
            local p = performed[1].PromptInputLine
            local init = p.initial_value
            p.action.callback(win, pane, nil)       -- Esc
            local after_esc = t0.title
            p.action.callback(win, pane, 'build')
            local after_set = t0.title
            p.action.callback(win, pane, '')        -- reset
            return init, after_esc, after_set, t0.title
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(r, ("old".into(), "old".into(), "build".into(), "".into()));
}

#[test]
fn double_click_renames_clicked_tab() {
    let l = lua();
    l.load(UI).exec().unwrap();
    let r: (String, String) = l
        .load(
            r#"
            tabs.setup({}, {})
            handlers['saysdf-tab-double-click'](win, pane, 1)
            local p = performed[1].PromptInputLine
            p.action.callback(win, pane, 'second')
            return p.initial_value, t1.title
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(r, ("proj".into(), "second".into()));
    let n: usize = l
        .load(
            "performed = {} tabs.setup({ double_click = false }, {}) \
             handlers['saysdf-tab-double-click'](win, pane, 0) return #performed",
        )
        .eval()
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn color_selector() {
    let l = lua();
    l.load(UI).exec().unwrap();
    let r: (usize, String, bool) = l
        .load(
            r#"
            tabs.choose_color(win, pane)
            local sel = performed[1].InputSelector
            sel.action.callback(win, pane, 'green')
            local c = tabs.get_color(10)
            sel.action.callback(win, pane, '')
            return #sel.choices, c, tabs.get_color(10) == nil
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(r, (9, "#98c379".into(), true));
}

#[test]
fn palette_aggregates_entries_from_all_modules() {
    let l = lua();
    let r: (usize, usize, usize, usize) = l
        .load(
            r#"
            local count = 0
            local orig = package.loaded.wezterm.on
            package.loaded.wezterm.on = function(n, f)
                if n == 'augment-command-palette' then count = count + 1 end
                orig(n, f)
            end
            local p = require('saysdf.palette')
            p.add({ brief = 'A' })
            p.add(function() return { { brief = 'B' }, { brief = 'C' } } end)
            p.add(function() error('boom') end)
            tabs.setup({}, {})
            local list = handlers['augment-command-palette']({}, {})
            local briefs = {}
            for _, e in ipairs(list) do briefs[e.brief] = true end
            return count, #list, (briefs.A and briefs.B and briefs.C) and 1 or 0, #errors
        "#,
        )
        .eval()
        .unwrap();
    // one handler; A, B, C + 3 tabs entries; the source error is logged
    assert_eq!(r, (1, 6, 1, 1));
}

#[test]
fn texts_follow_language() {
    let l = lua();
    l.load(UI).exec().unwrap();
    let run = |lang: &str| -> String {
        l.load(format!(
            "require('saysdf.i18n').select({lang:?})
             tabs.setup({{}}, {{}})
             performed = {{}}
             local e = tabs.palette_entries()
             tabs.rename(win, pane)
             tabs.choose_color(win, pane)
             local sel = performed[2].InputSelector
             return e[1].label .. '|' .. e[1].brief .. '|' .. e[2].label .. '|' .. e[3].label
                 .. '|' .. performed[1].PromptInputLine.description
                 .. '|' .. sel.title .. '|' .. sel.choices[1].label .. '|' .. sel.choices[#sel.choices].label"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(
        run("en"),
        "Rename tab…|Tab: rename tab|Reset tab name|Tab color…|\
         Tab name (empty for automatic, Esc to cancel)|Tab color|Red|Reset color"
    );
    // The Russian UI is unchanged by the migration
    assert_eq!(
        run("ru"),
        concat!(
            "Переименовать…|Tab: переименовать вкладку|Сбросить имя|Цвет…|", // i18n-allow-cyrillic
            "Имя вкладки (пусто — автоматическое, Esc — отмена)|", // i18n-allow-cyrillic
            "Цвет вкладки|Красный|Сбросить цвет" // i18n-allow-cyrillic
        )
    );
}
