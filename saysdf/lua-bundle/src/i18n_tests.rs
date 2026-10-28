//! Tests of `saysdf.i18n` and its native part (change `i18n`).
use super::*;

fn lua() -> Lua {
    let lua = Lua::new();
    {
        let package: Table = lua.globals().get("package").unwrap();
        package.set("path", "").unwrap();
        package.set("cpath", "").unwrap();
    }
    install_searcher(&lua).unwrap();
    lua.load(
        r#"
        warnings = {}
        package.loaded.wezterm = {
            log_warn = function(m) table.insert(warnings, m) end,
            log_error = function(m) table.insert(warnings, m) end,
            log_info = function() end,
        }
        i18n = require("saysdf.i18n")
        "#,
    )
    .exec()
    .unwrap();
    lua
}

fn s(l: &Lua, code: &str) -> String {
    l.load(code).eval().unwrap()
}

#[test]
fn lookups_in_english_and_russian() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            local t, tn, N_ = i18n.domain("menu")
            local r = {}
            local function run(lang)
                table.insert(r, i18n.select(lang))
                table.insert(r, i18n.language())
                table.insert(r, t("View"))
                table.insert(r, t("Not in the catalog: {x}", { x = 5 }))
                table.insert(r, tn("{n} tab", "{n} tabs", 1))
                table.insert(r, tn("{n} tab", "{n} tabs", 3, { extra = true }))
                table.insert(r, N_("Tools"))
            end
            run("en")
            run("ru")
            return table.concat(r, "|") .. "|" .. #warnings
            "#
        ),
        "en|en|View|Not in the catalog: 5|1 tab|3 tabs|Tools|\
         ru|ru|Вид|Not in the catalog: 5|1 tab|3 tabs|Tools|0" // i18n-allow-cyrillic
    );
}

#[test]
fn unknown_language_falls_back_to_english_with_one_warning() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            local lang = i18n.select("xx")
            local t = i18n.domain("menu")
            return lang .. "|" .. t("View") .. "|" .. #warnings .. "|" .. tostring(warnings[1]:find('"xx"', 1, true) ~= nil)
            "#
        ),
        "en|View|1|true"
    );
}

#[test]
fn user_catalogs_override_and_broken_file_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let ru = dir.path().join("locales").join("ru");
    std::fs::create_dir_all(&ru).unwrap();
    std::fs::write(ru.join("menu.toml"), "\"View\" = \"Обзор\"\n[_accel]\n\"core.search\" = \"s\"\n") // i18n-allow-cyrillic
        .unwrap();
    std::fs::write(ru.join("broken.toml"), "\"View\" = [1, 2]").unwrap();
    let eo = dir.path().join("locales").join("eo");
    std::fs::create_dir_all(&eo).unwrap();
    std::fs::write(eo.join("menu.toml"), "\"View\" = \"Vido\"").unwrap();

    let l = lua();
    l.globals()
        .set("DIR", dir.path().to_str().unwrap())
        .unwrap();
    assert_eq!(
        s(
            &l,
            r#"
            local t = i18n.domain("menu")
            local r = { i18n.select("ru", DIR), t("View"), t("Tools"), tostring(i18n.accel("core.search")) }
            table.insert(r, #warnings)
            table.insert(r, tostring(warnings[1]:find("broken.toml", 1, true) ~= nil))
            table.insert(r, i18n.select("eo", DIR))
            table.insert(r, t("View"))
            table.insert(r, table.concat(i18n.languages(DIR), ","))
            return table.concat(r, "|")
            "#
        ),
        "ru|Обзор|Инструменты|s|1|true|eo|Vido|en,eo,ru" // i18n-allow-cyrillic
    );
}

#[test]
fn setup_selects_language_and_passes_it_to_tools() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            package.loaded.wezterm.config_builder = function() return {} end
            local saysdf = require("saysdf")
            -- only the root logic: no feature modules
            saysdf.modules = function() return {} end
            local c1 = saysdf.setup({ language = "ru" }, {})
            local c2 = saysdf.setup({ language = "en" }, { set_environment_variables = { SAYSDF_LANG = "de", X = "1" } })
            return c1.set_environment_variables.SAYSDF_LANG .. "|" .. c2.set_environment_variables.SAYSDF_LANG
                .. c2.set_environment_variables.X .. "|" .. i18n.language() .. "|" .. #warnings
            "#
        ),
        "ru|de1|en|0"
    );
}

/// A rejected user catalog file: besides the log line, one notice per config load, shown on
/// the first status update of a window.
#[test]
fn broken_user_catalog_shows_one_notice() {
    let dir = tempfile::tempdir().unwrap();
    let ru = dir.path().join("locales").join("ru");
    std::fs::create_dir_all(&ru).unwrap();
    std::fs::write(ru.join("broken.toml"), "\"View\" = [1, 2]").unwrap();
    std::fs::write(ru.join("also-broken.toml"), "not toml").unwrap();
    let l = lua();
    l.globals().set("DIR", dir.path().to_str().unwrap()).unwrap();
    assert_eq!(
        s(
            &l,
            r#"
            local hooks, toasts = {}, {}
            package.loaded["saysdf.status"] = {
                hook = function(name, fn) hooks[name] = fn end,
                add = function() end,
                render = function() end,
            }
            package.loaded.wezterm.config_dir = DIR
            local saysdf = require("saysdf")
            saysdf.modules = function() return {} end
            saysdf.setup({ language = "ru" }, {})
            local win = {
                window_id = function() return 1 end,
                active_pane = function() return {} end,
                toast_notification = function(_, title, text) table.insert(toasts, title .. ": " .. text) end,
            }
            local h = hooks["i18n-rejected-catalogs"]
            h(win, {})
            h(win, {})
            local r = { i18n.rejected_user_files, #toasts, toasts[1], tostring(saysdf.notify_rejected_catalogs(0)) }
            i18n.select("en")
            saysdf.notify_rejected_catalogs(1)
            hooks["i18n-rejected-catalogs"](win, {})
            table.insert(r, toasts[2])
            return table.concat(r, "|")
            "#
        ),
        "2|1|saysdfTerm: 2 файла перевода в папке конфига содержат ошибки и пропущены; подробности в журнале|false|saysdfTerm: 1 translation file in the config folder has errors and was ignored; see the log" // i18n-allow-cyrillic
    );
}

// The language is per Lua state: two states on parallel threads keep switching to their own
// language and never see the other one's (select does not touch shared state; only
// apply_global, called by saysdf.setup, sets the process catalog of the Rust code).
#[test]
fn lua_states_have_independent_languages() {
    let run = |lang: &'static str, want: &'static str| {
        std::thread::spawn(move || {
            let l = lua();
            let code = format!(
                r#"
                local t, tn = i18n.domain("menu")
                local out = {{}}
                for i = 1, 200 do
                    if i % 20 == 1 then i18n.select("{lang}") end
                    local got = i18n.language() .. "|" .. t("View")
                    if got ~= "{lang}|{want}" then table.insert(out, i .. ":" .. got) end
                end
                return table.concat(out, ",")
                "#
            );
            for _ in 0..10 {
                assert_eq!(s(&l, &code), "", "{lang}");
            }
        })
    };
    let en = run("en", "View");
    let ru = run("ru", "Вид"); // i18n-allow-cyrillic
    en.join().unwrap();
    ru.join().unwrap();
}
