//! Tests of the Lua part of the bars (menu-bar, startup-help, hotkey-statusbar): registry, menu model,
//! accelerators, foreground_is_tui, status bar context, banner.
use super::*;

const FAKE: &str = r#"
    handlers = {}
    logs = {}
    local function ctor(name)
        return setmetatable({ name = name }, { __call = function(self, arg) return { name = name, arg = arg } end })
    end
    package.loaded.wezterm = {
        on = function(name, f) handlers[name] = handlers[name] or {}; table.insert(handlers[name], f) end,
        log_error = function(m) table.insert(logs, m) end,
        log_warn = function(m) table.insert(logs, m) end,
        log_info = function() end,
        action_callback = function(f) return f end,
        action = setmetatable({}, { __index = function(_, k) return ctor(k) end }),
        GLOBAL = {},
        default_hyperlink_rules = function() return {} end,
        config_builder = function() return {} end,
        format = function(items)
            local t = {}
            for _, it in ipairs(items) do table.insert(t, it.Text or "") end
            return table.concat(t)
        end,
        version = "20261019-abc",
        target_triple = "x86_64-pc-windows-msvc",
        home_dir = "C:/Users/u",
    }
    palette = require('saysdf.palette')
    menu = require('saysdf.menu')
    util = require('saysdf.util')
    hotbar = require('saysdf.hotbar')
    help = require('saysdf.help')
    performed = {}
    bars = {}
    window = {
        window_id = function() return 7 end,
        perform_action = function(self, a, p) table.insert(performed, a) end,
        saysdf_set_bar = function(self, name, spec) bars[name] = spec; bars.n = (bars.n or 0) + 1 end,
        set_right_status = function(self, t) right = t end,
        set_left_status = function() end,
        get_selection_text_for_pane = function() return sel or "" end,
    }
    -- English UI regardless of the machine's language (change i18n)
    require('saysdf.i18n').select('en')
"#;

fn lua() -> Lua {
    let lua = Lua::new();
    {
        let package: Table = lua.globals().get("package").unwrap();
        package.set("path", "").unwrap();
        package.set("cpath", "").unwrap();
    }
    install_searcher(&lua).unwrap();
    lua.load(FAKE).exec().unwrap();
    lua
}

fn s(l: &Lua, code: &str) -> String {
    l.load(code).eval().unwrap()
}

#[test]
fn foreground_is_tui() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"return tostring(util.foreground_is_tui({ alt_screen = true, process = "pwsh" }))
            .. tostring(util.foreground_is_tui({ process = "C:\\Tools\\HTOP.EXE" }))
            .. tostring(util.foreground_is_tui({ process = "/usr/bin/claude" }))
            .. tostring(util.foreground_is_tui({ process = "/bin/bash" }))
            .. tostring(util.foreground_is_tui({ process = "ssh" }))
            .. tostring(util.foreground_is_tui({}))
            .. tostring(util.foreground_is_tui({ process = "bash" }, { "bash" }))"#
        ),
        "truetruetruefalsefalsefalsetrue"
    );
}

fn err(l: &Lua, code: &str) -> String {
    l.load(code).exec().unwrap_err().to_string()
}

#[test]
fn menu_entry_without_accel_is_an_error() {
    let l = lua();
    let e = err(&l, r#"palette.add({ id = "t.one", menu = "File", brief = "One", action = 1 })"#);
    assert!(e.contains("\"t.one\"") && e.contains("\"File\"") && e.contains("no accel"), "{}", e);
    l.load(r#"palette.add({ id = "t.free", brief = "no menu", action = 1 })"#).exec().unwrap();
}

#[test]
fn menu_entry_with_taken_accel_is_an_error() {
    let l = lua();
    let e = err(
        &l,
        r#"
        palette.add({ id = "t.one", menu = "File", accel = "q", brief = "One", action = 1 })
        palette.add({ id = "t.other", menu = "View", accel = "q", brief = "Other menu", action = 1 })
        palette.add({ id = "t.two", menu = "File", accel = "q", brief = "Two", action = 1 })
        "#,
    );
    assert!(e.contains("\"t.two\"") && e.contains("\"File\"") && e.contains("taken by entry \"t.one\""), "{}", e);
}

#[test]
fn dynamic_menu_entry_rejected_is_skipped_and_notified_once() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            local notes = {}
            util.notify = function(w, title, text) table.insert(notes, text) end
            palette.add({ id = "t.one", menu = "File", accel = "q", brief = "One", action = 1 })
            palette.add(function() return {
                { id = "t.dyn", menu = "File", accel = "q", brief = "Dup", action = 1 },
                { id = "t.ok", menu = "File", accel = "k", brief = "Good", action = 1 },
                { id = "t.none", menu = "File", brief = "No key", action = 1 },
            } end)
            local ids = {}
            for _, e in ipairs(palette.entries(window, {})) do table.insert(ids, e.id) end
            palette.entries(window, {})
            palette.entries(window, {})
            return table.concat(ids, ",") .. "|" .. #notes
                .. "|" .. tostring(notes[1]:match("\"t.dyn\" in menu \"File\".*taken by entry \"t.one\"") ~= nil)
                .. "|" .. tostring(notes[2]:match("\"t.none\" in menu \"File\" has no accel") ~= nil)
            "#
        ),
        "t.one,t.ok|2|true|true"
    );
}

#[test]
fn readding_same_id_replaces_entry_and_frees_accel() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            palette.add({ id = "t.x", menu = "File", accel = "q", brief = "X1", action = 1 })
            palette.add({ id = "t.x", menu = "File", accel = "q", brief = "X2", action = 1 })
            local n1 = #palette.entries(window, {})
            palette.add({ id = "t.x", menu = "File", accel = "w", brief = "X3", action = 1 })
            palette.add({ id = "t.y", menu = "File", accel = "q", brief = "Y", action = 1 })
            local list = palette.entries(window, {})
            local ok = pcall(palette.add, { id = "t.y", menu = "File", accel = "w", brief = "Y2", action = 1 })
            local still = palette.entries(window, {})
            return n1 .. "|" .. #list .. list[1].brief .. "|" .. tostring(ok) .. "|" .. still[2].brief .. still[2].accel
            "#
        ),
        "1|2X3|false|Yq"
    );
}

#[test]
fn bundled_default_config_menu_has_accels() {
    let l = lua();
    l.globals().set("DEFAULT_CONFIG", default_config()).unwrap();
    assert_eq!(
        s(
            &l,
            r#"
            local f = load(DEFAULT_CONFIG)
            f()
            local entries = palette.entries(window, {})
            local missing = {}
            local n = 0
            for _, e in ipairs(entries) do
                if e.menu then
                    n = n + 1
                    if not e.accel then table.insert(missing, e.id) end
                end
            end
            local errors = {}
            for _, m in ipairs(logs) do
                if tostring(m):find("palette", 1, true) then table.insert(errors, m) end
            end
            local spec = menu.to_bar(menu.build(entries))
            return tostring(n > 30) .. "|" .. table.concat(missing, ",") .. "|" .. table.concat(errors, ";") .. "|" .. tostring(#spec.left)
            "#
        ),
        "true|||6"
    );
}

#[test]
fn build_orders_and_hides_empty() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            local tree = menu.build({
                { id = "a.b", menu = "View", accel = "b", brief = "B", order = 2, action = 1 },
                { id = "a.a", menu = "View", accel = "a", brief = "A", order = 1, action = 1, key = "F5" },
                { id = "g.x", menu = "Git", accel = "x", brief = "X", action = 1 },
                { brief = "no id", menu = "Git", action = 1 },
                { id = "n.m", brief = "no menu", action = 1 },
            })
            local t = {}
            for _, m in ipairs(tree) do
                local it = {}
                for _, i in ipairs(m.items) do table.insert(it, i.label .. (i.key and ("/" .. i.key) or "")) end
                table.insert(t, m.title .. ":" .. table.concat(it, ";"))
            end
            local bar = menu.to_bar(tree)
            local c = bar.left[2].children[1]
            return table.concat(t, "|") .. "@" .. c.text .. "/" .. c.key .. "/" .. c.accel .. "@" .. bar.left[1].text
        "#
        ),
        "Git:X|View:A/F5;B@A/F5/a@Git"
    );
}

#[test]
fn registry_entry_in_palette_and_menu_and_action() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            palette.add({ id = "t.one", menu = "File", accel = "o", brief = "One", key = "Ctrl+X", action = "ACT" })
            palette.add({ id = "t.hidden", menu = "File", accel = "h", brief = "Hidden", palette = false, action = "H" })
            palette.ensure_bar_action()
            local pl = palette.palette_list(window, {})
            local tree = menu.build(palette.entries(window, {}))
            handlers["saysdf-bar-action"][1](window, {}, "t.one")
            handlers["saysdf-bar-action"][1](window, {}, "no.such")
            return #pl .. pl[1].brief .. tostring(pl[1].id) .. "|" .. #tree[1].items .. "|" .. performed[1]
                .. "|" .. #handlers["augment-command-palette"] .. #handlers["saysdf-bar-action"] .. "|" .. #logs
                .. "|" .. palette.key_text({ key = "s", mods = "CTRL|SHIFT" })
                .. "|" .. palette.key_text({ key = "F9", mods = "NONE" })
        "#
        ),
        "1Onenil|2|ACT|11|1|Ctrl+Shift+S|F9"
    );
}

#[test]
fn module_disabled_has_no_menu_items() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            require('saysdf').setup({ git = false, ssh = false, workspaces = false, monitor = false }, { keys = {} })
            local ids = {}
            for _, e in ipairs(palette.entries(window, {})) do if e.id then ids[e.id] = true end end
            return tostring(ids["git.menu"]) .. tostring(ids["ssh.connect"]) .. tostring(ids["db.open"])
                .. tostring(ids["menu.open"]) .. tostring(ids["help.full"]) .. tostring(ids["tabs.rename"])
                .. tostring(ids["viewer.open"])
        "#
        ),
        "nilniltruetruetruetruetrue"
    );
}

#[test]
fn hotbar_context_priority() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            local c = hotbar.context
            return table.concat({
                c({ db_mode = "write", tool = "db", selection = true, git = true, process = "C:\\bin\\saysdf.exe", domain = "local" }),
                c({ tool = "view", selection = true, process = "saysdf" }),
                c({ process = "saysdf.exe" }),
                c({ selection = true, git = true, process = "ssh" }),
                c({ process = "ssh.exe", git = true }),
                c({ domain = "SSHMUX:host", git = true }),
                c({ git = true, process = "pwsh" }),
                c({}),
            }, ",")
        "#
        ),
        "db_danger,tool,tool,selection,ssh,ssh,git,shell"
    );
}

#[test]
fn hotbar_user_vars_only_from_local_saysdf() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            local c = hotbar.context
            return table.concat({
                -- a program over SSH or a background process: user vars are ignored
                c({ db_mode = "write", tool = "db", process = "ssh.exe", git = true }),
                c({ db_mode = "write", tool = "db", process = "pwsh" }),
                c({ db_mode = "write", tool = "db" }),
                c({ db_mode = "write", process = "saysdf", domain = "SSHMUX:host" }),
                c({ tool = "view", process = "bash", selection = true }),
                -- local saysdf
                c({ db_mode = "write", process = "/usr/bin/saysdf", domain = "local" }),
                c({ db_mode = "read", process = "saysdf.exe", domain = "local" }),
                tostring(hotbar.local_saysdf({ process = "saysdf-other.exe" })),
            }, ",")
        "#
        ),
        "ssh,shell,shell,tool,selection,db_danger,tool,false"
    );
}

#[test]
fn hotbar_hints_and_bar() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            local entries = {
                { id = "m", key = "F9", hint = { label = "Menu", priority = 1, always = true }, action = 1 },
                { id = "g", key = "Ctrl+Shift+S", hint = { contexts = { "git" }, label = "Git", priority = 10 }, action = 1 },
                { id = "c", hint = { contexts = { "selection" }, label = "Copy", priority = 5 }, action = 1 },
            }
            local function txt(list) local t = {} for _, h in ipairs(list) do table.insert(t, h.text) end return table.concat(t, ";") end
            local a = txt(hotbar.hints("git", entries))
            local b = txt(hotbar.hints("selection", entries))
            local d = txt(hotbar.hints("db_danger", entries))
            hotbar.setup({}, {})
            local pane = { get_user_vars = function() return {} end }
            hotbar.consume(window, pane, { { Text = "main" } })
            hotbar.consume(window, pane, { { Text = "main" } })
            return a .. "|" .. b .. "|" .. d .. "|" .. bars.bottom.right[1].text .. bars.n
        "#
        ),
        "F9 Menu;Ctrl+Shift+S Git|F9 Menu;Copy|⚠ DB: WRITE ENABLED;F9 Menu|main1"
    );
}

#[test]
fn hotbar_fallback_right_status() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            hotbar.setup({}, {})
            local w = {
                perform_action = function() end,
                get_selection_text_for_pane = function() return "" end,
                set_right_status = function(self, t) right = t end,
            }
            palette.add({ id = "m", key = "F9", hint = { label = "Menu", always = true }, action = 1 })
            local ok = hotbar.consume(w, {}, { { Text = "main" } })
            return tostring(ok) .. right
        "#
        ),
        "true F9 Menu  │ main "
    );
}

#[test]
fn banner_lines_and_once_per_window() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            local lines = help.banner_lines({}, {
                { id = "menu.open", key = "F9" }, { id = "help.full", key = "F1" }, { id = "git.menu", key = "Ctrl+Shift+S" },
            })
            help.setup({}, { keys = {} })
            local a = help.maybe_banner(window, {})
            local b = help.maybe_banner(window, {})
            return #lines .. "|" .. lines[1] .. "|" .. lines[3] .. "|" .. tostring(a) .. tostring(b) .. "|" .. #bars.banner.left
        "#
        ),
        "5|saysdfTerminal 20261019-abc|F9 — menu · F1 — full help|truefalse|5"
    );
}

#[test]
fn banner_empty_version_is_shown_as_unknown() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            package.loaded.wezterm.version = ""
            return help.banner_lines({}, {})[1]
        "#
        ),
        "saysdfTerminal (version unknown)"
    );
}

/// Menu accelerators stay unique in every language with a catalog (they may be overridden per
/// language by the `_accel` table), and menu titles are translated (change i18n).
#[test]
fn default_config_menu_in_every_language() {
    let langs: Vec<String> = lua()
        .load("return require('saysdf._i18n').languages(nil)")
        .eval()
        .unwrap();
    assert!(langs.contains(&"en".to_string()) && langs.contains(&"ru".to_string()), "{langs:?}");
    for lang in langs {
        let l = lua();
        let cfg = default_config().replace(
            "setup({})",
            &format!("setup({{ language = {lang:?} }})"),
        );
        assert_ne!(cfg, default_config(), "default config no longer calls setup({{}})");
        l.globals().set("DEFAULT_CONFIG", cfg).unwrap();
        let got = s(
            &l,
            r#"
            local c = load(DEFAULT_CONFIG)()
            local errors = {}
            for _, m in ipairs(logs) do
                if tostring(m):find("palette", 1, true) or tostring(m):find("i18n", 1, true) then
                    table.insert(errors, m)
                end
            end
            local spec = menu.to_bar(menu.build(palette.entries(window, {})))
            local titles = {}
            for _, m in ipairs(spec.left) do table.insert(titles, m.text) end
            return table.concat(errors, ";") .. "|" .. table.concat(titles, ",")
                .. "|" .. tostring(c.set_environment_variables.SAYSDF_LANG)
            "#,
        );
        let want_titles = match lang.as_str() {
            "ru" => "Файл,Вкладки,Git,Инструменты,Вид,Справка", // i18n-allow-cyrillic
            _ => "File,Tabs,Git,Tools,View,Help",
        };
        assert_eq!(got, format!("|{want_titles}|{lang}"), "language {lang}");
    }
}

/// The hotbar hints and the startup banner follow the language of the Lua state; the Russian UI
/// is unchanged by the migration.
#[test]
fn texts_follow_language() {
    let run = |lang: &str| -> String {
        let l = lua();
        l.load(format!(
            r#"
            require('saysdf.i18n').select({lang:?})
            local lines = help.banner_lines({{}}, {{
                {{ id = "menu.open", key = "F9" }}, {{ id = "help.full", key = "F1" }},
                {{ id = "core.palette", key = "Ctrl+Shift+P" }}, {{ id = "tabs.rename", key = "F2" }},
            }})
            local function txt(list) local t = {{}} for _, h in ipairs(list) do table.insert(t, h.text) end return table.concat(t, ";") end
            return txt(hotbar.hints("db_danger", {{}})) .. "|" .. txt(hotbar.hints("tool", {{}})) .. "|" .. table.concat(lines, "|")
            "#
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(
        run("en"),
        "⚠ DB: WRITE ENABLED|q/Esc exit|saysdfTerminal 20261019-abc|saysdfTerm is a terminal with git, SSH, databases, a monitor and a file viewer.|F9 — menu · F1 — full help · Ctrl+Shift+P — command palette|F2 — tab name|Hints at the bottom change with the context. Esc closes this."
    );
    assert_eq!(
        run("ru"),
        "⚠ БД: ЗАПИСЬ ВКЛЮЧЕНА|q/Esc выход|saysdfTerminal 20261019-abc|saysdfTerm — терминал с git, SSH, БД, монитором и просмотром файлов.|F9 — меню · F1 — справка · Ctrl+Shift+P — палитра команд|F2 — имя вкладки|Подсказки внизу меняются по контексту. Esc — закрыть." // i18n-allow-cyrillic
    );
}

/// The banner's last hint shows the real config snippet: `{{`/`}}` in the catalog key and the
/// translation render as literal braces.
#[test]
fn banner_off_hint_shows_literal_braces() {
    let run = |lang: &str| -> String {
        let l = lua();
        l.load(format!(
            r#"
            require('saysdf.i18n').select({lang:?})
            local lines = help.banner_lines({{}}, {{}})
            return lines[#lines]
            "#
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(run("en"), "Turn the banner off: help = { banner = false } in saysdf.setup.");
    assert_eq!(
        run("ru"),
        "Выключить баннер: help = { banner = false } в saysdf.setup." // i18n-allow-cyrillic
    );
}
