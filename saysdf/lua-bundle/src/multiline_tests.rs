//! Tests of the `saysdf.multiline` module (change multiline-input, task 1.1).
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

#[test]
fn default_binds_shift_and_ctrl_enter_to_esc_cr() {
    let l = lua();
    let got: Vec<String> = l
        .load(
            r#"local c = require('saysdf').setup({}, { keys = { { key = 'a' } } })
              local out = { tostring(#c.keys), c.set_environment_variables.SAYSDFTERM_MULTILINE }
              for i = 2, #c.keys do
                local k = c.keys[i]
                table.insert(out, k.key .. '/' .. k.mods .. '/' .. k.action.action .. '/'
                  .. (k.action.arg == '\27\r' and 'ESC-CR' or k.action.arg))
              end
              return out"#,
        )
        .eval()
        .unwrap();
    assert_eq!(
        got,
        vec![
            "3",
            "1",
            "Enter/SHIFT/SendString/ESC-CR",
            "Enter/CTRL/SendString/ESC-CR"
        ]
    );
}

#[test]
fn options_keys_and_sequence() {
    let l = lua();
    let got: Vec<String> = l
        .load(
            r#"local c = require('saysdf.multiline').setup({ keys = { 'SHIFT' }, sequence = 'x' }, {})
              return { tostring(#c.keys), c.keys[1].mods, c.keys[1].action.arg }"#,
        )
        .eval()
        .unwrap();
    assert_eq!(got, vec!["1", "SHIFT", "x"]);
}

#[test]
fn disabled_adds_no_keys_and_no_env() {
    let l = lua();
    let (keys, env): (bool, bool) = l
        .load(
            "local c = require('saysdf').setup({ multiline = false }, {}) \
             return c.keys == nil, c.set_environment_variables == nil",
        )
        .eval()
        .unwrap();
    assert!(keys);
    assert!(env);
}
