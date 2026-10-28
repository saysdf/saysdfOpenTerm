//! Tests of the `saysdf.close` module (change close-confirmation, tasks 1.1-1.4).
use super::*;

const FAKE: &str = r#"
    handlers = {}
    warnings = {}
    package.loaded.wezterm = {
        on = function(name, f) handlers[name] = f end,
        log_warn = function(msg) table.insert(warnings, msg) end,
        config_builder = function() return {} end,
    }
    close = require('saysdf.close')
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf", get("saysdf").unwrap()),
            ("saysdf.close", get("saysdf.close").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.palette", get("saysdf.palette").unwrap()),
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

/// (hook answer as a string, window_close_confirmation, number of warnings)
fn run(l: &Lua, opts: &str) -> (String, String, i64) {
    l.load(format!(
        "warnings = {{}} handlers = {{}} \
         local c = close.setup({opts}, {{}}) \
         local v = handlers['mux-is-process-stateful']({{ name = 'pwsh.exe' }}) \
         return tostring(v), c.window_close_confirmation, #warnings"
    ))
    .eval()
    .unwrap()
}

#[test]
fn modes_map_to_hook_and_window_setting() {
    let l = lua();
    assert_eq!(run(&l, "{}"), ("true".into(), "AlwaysPrompt".into(), 0));
    assert_eq!(
        run(&l, "{ close_confirmation = 'always' }"),
        ("true".into(), "AlwaysPrompt".into(), 0)
    );
    assert_eq!(
        run(&l, "{ close_confirmation = 'only-if-running-process' }"),
        ("nil".into(), "AlwaysPrompt".into(), 0)
    );
    assert_eq!(
        run(&l, "{ close_confirmation = 'never' }"),
        ("false".into(), "NeverPrompt".into(), 0)
    );
}

#[test]
fn unknown_mode_warns_and_uses_default() {
    let l = lua();
    assert_eq!(
        run(&l, "{ close_confirmation = 'sometimes' }"),
        ("true".into(), "AlwaysPrompt".into(), 1)
    );
    let w: String = l.load("return warnings[1]").eval().unwrap();
    assert!(w.contains("sometimes"), "{w}");
}

#[test]
fn skip_list_default_untouched_user_list_extended_without_dups() {
    let l = lua();
    let none: bool = l
        .load("return close.setup({}, {}).skip_close_confirmation_for_processes_named == nil")
        .eval()
        .unwrap();
    assert!(none);
    let list: Vec<String> = l
        .load(
            "return close.setup({}, { skip_close_confirmation_for_processes_named = \
             { 'bash', 'pwsh.exe', 'bash' } }).skip_close_confirmation_for_processes_named",
        )
        .eval()
        .unwrap();
    assert_eq!(list, vec!["bash", "pwsh.exe", "cmd.exe", "powershell.exe"]);
}

#[test]
fn wired_into_saysdf_setup_and_can_be_disabled() {
    let l = lua();
    l.load("package.preload['saysdf._bundled'] = function() return { 'saysdf', 'saysdf.close' } end")
        .exec()
        .unwrap();
    let on: String = l
        .load("handlers = {} return require('saysdf').setup({}, {}).window_close_confirmation")
        .eval()
        .unwrap();
    assert_eq!(on, "AlwaysPrompt");
    let off: bool = l
        .load(
            "handlers = {} local c = require('saysdf').setup({ close = false }, {}) \
             return c.window_close_confirmation == nil and handlers['mux-is-process-stateful'] == nil",
        )
        .eval()
        .unwrap();
    assert!(off);
}

#[test]
fn texts_follow_language() {
    let l = lua();
    let run = |lang: &str| -> String {
        l.load(format!(
            "require('saysdf.i18n').select({lang:?})
             require('saysdf.palette')._reset()
             package.loaded.wezterm.action = {{ CloseCurrentTab = function() return {{}} end }}
             close.setup({{}}, {{}})
             local e = require('saysdf.palette').by_id(nil, nil, 'close.tab')
             return e.label .. '|' .. e.brief"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(run("en"), "Close tab|Close tab");
    // The Russian UI is unchanged by the migration
    assert_eq!(run("ru"), "Закрыть вкладку|Закрыть вкладку"); // i18n-allow-cyrillic
}
