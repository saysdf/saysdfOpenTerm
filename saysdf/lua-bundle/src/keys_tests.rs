//! Tests of the `saysdf.keys` module (change layout-independent-keys).
use super::*;

const FAKE: &str = r#"
    local act = setmetatable({}, { __index = function(_, k)
        return function(arg) return { action = k, arg = arg } end
    end })
    package.loaded.wezterm = { action = act, log_warn = function() end }
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf", get("saysdf").unwrap()),
            ("saysdf.keys", get("saysdf.keys").unwrap()),
            ("saysdf.multiline", get("saysdf.multiline").unwrap()),
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

fn pref(l: &Lua, code: &str) -> String {
    l.load(code).eval::<Option<String>>().unwrap().unwrap_or("nil".into())
}

#[test]
fn default_sets_physical() {
    let l = lua();
    assert_eq!(
        pref(&l, "return require('saysdf').setup({}, {}).key_map_preference"),
        "Physical"
    );
}

#[test]
fn flag_false_leaves_upstream_default() {
    let l = lua();
    assert_eq!(
        pref(&l, "return require('saysdf').setup({ keys = { layout_independent = false } }, {}).key_map_preference"),
        "nil"
    );
    assert_eq!(
        pref(&l, "return require('saysdf').setup({ keys = false }, {}).key_map_preference"),
        "nil"
    );
}

#[test]
fn user_value_is_not_overwritten() {
    let l = lua();
    assert_eq!(
        pref(&l, "return require('saysdf').setup({}, { key_map_preference = 'Mapped' }).key_map_preference"),
        "Mapped"
    );
}

#[test]
fn multiline_enter_bindings_unaffected() {
    let l = lua();
    let got: Vec<String> = l
        .load(
            r#"local c = require('saysdf').setup({}, {})
              local out = {}
              for _, k in ipairs(c.keys) do table.insert(out, k.key .. '/' .. k.mods) end
              return out"#,
        )
        .eval()
        .unwrap();
    assert_eq!(got, vec!["Enter/SHIFT", "Enter/CTRL"]);
}

fn kitty(l: &Lua, opts: &str, config: &str) -> Option<bool> {
    l.load(format!(
        "return require('saysdf').setup({opts}, {config}).enable_kitty_keyboard"
    ))
    .eval()
    .unwrap()
}

#[test]
fn kitty_keyboard_enabled_by_default() {
    assert_eq!(kitty(&lua(), "{}", "{}"), Some(true));
}

#[test]
fn kitty_keyboard_user_false_is_kept() {
    assert_eq!(
        kitty(&lua(), "{}", "{ enable_kitty_keyboard = false }"),
        Some(false)
    );
}

#[test]
fn kitty_keyboard_untouched_when_keys_disabled() {
    let l = lua();
    assert_eq!(kitty(&l, "{ keys = false }", "{}"), None);
    assert_eq!(
        kitty(&l, "{ keys = { layout_independent = false } }", "{}"),
        None
    );
}

#[test]
fn util_missing_bin_message_follows_language() {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
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
    lua.load("package.loaded.wezterm = {}").exec().unwrap();
    let run = |lang: &str| -> String {
        lua.load(format!(
            "require('saysdf.i18n').select({lang:?}) \
             local msg \
             local w = {{ window_id = function() return 1 end, active_pane = function() end, \
                         toast_notification = function(_, title, text) msg = text end }} \
             require('saysdf.util').notify_missing_bin(w, 'saysdf') \
             return msg"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(run("en"), "saysdf not found (next to saysdfTerm or in PATH)");
    assert_eq!(run("ru"), "saysdf не найден (рядом с saysdfTerm или в PATH)"); // i18n-allow-cyrillic
}
