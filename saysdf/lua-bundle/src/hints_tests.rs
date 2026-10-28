//! Tests of the `saysdf.hints` module (change command-hints, task 4.1): option parsing and env.
use super::*;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf", get("saysdf").unwrap()),
            ("saysdf.hints", get("saysdf.hints").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
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
    lua.load("package.loaded.wezterm = { log_warn = function() end }")
        .exec()
        .unwrap();
    lua
}

fn env_of(l: &Lua, code: &str) -> String {
    l.load(format!(
        r#"local c = {code}
           local env = c.set_environment_variables or {{}}
           local keys = {{}}
           for k, v in pairs(env) do table.insert(keys, k .. '=' .. v) end
           table.sort(keys)
           return table.concat(keys, ' ')"#
    ))
    .eval()
    .unwrap()
}

#[test]
fn defaults_enable_hints() {
    let l = lua();
    let got = env_of(
        &l,
        "require('saysdf.hints').setup({ bin = '/x/saysdf' }, { set_environment_variables = { A = '1' } })",
    );
    assert_eq!(got, "A=1 SAYSDFTERM_HINTS=1 SAYSDFTERM_HINTS_BIN=/x/saysdf");
}

#[test]
fn options_map_to_env() {
    let l = lua();
    let got = env_of(
        &l,
        "require('saysdf.hints').setup({ bin = 'b', disabled = { 'kubectl', 'npm' }, override = true, \
         inline = false, menu = false, commands = { 'git', 'cargo' } }, {})",
    );
    assert_eq!(
        got,
        "SAYSDFTERM_HINTS=1 SAYSDFTERM_HINTS_BIN=b SAYSDFTERM_HINTS_COMMANDS=git,cargo \
         SAYSDFTERM_HINTS_INLINE=0 SAYSDFTERM_HINTS_MENU=0 SAYSDF_HINTS_DISABLED=kubectl,npm \
         SAYSDF_HINTS_OVERRIDE=1"
    );
    let all = env_of(&l, "require('saysdf.hints').setup({ bin = 'b', disabled = true }, {})");
    assert!(all.contains("SAYSDF_HINTS_DISABLED=*"));
}

#[test]
fn module_off_sets_nothing() {
    let l = lua();
    let got = env_of(&l, "require('saysdf').setup({ hints = false }, {})");
    assert!(!got.contains("SAYSDFTERM_HINTS"), "{got}");
}
