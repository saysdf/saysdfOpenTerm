//! Tests of `saysdf.db`: options, `args` building, temporary connections TOML, setup.
use super::*;

const FAKE: &str = r#"
    package.loaded.wezterm = {
        on = function() end,
        log_error = function() end,
        log_warn = function() end,
        log_info = function() end,
        action_callback = function(f) return f end,
        action = {},
    }
    db = require('saysdf.db')
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.db", get("saysdf.db").unwrap()),
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

fn s(l: &Lua, code: &str) -> String {
    l.load(code).eval().unwrap()
}

#[test]
fn opts_defaults() {
    let l = lua();
    assert_eq!(
        s(&l, r#"local o = db.parse_opts(nil) return o.key.key .. o.key.mods .. o.target .. tostring(o.keys) .. tostring(o.palette) .. o.split.direction"#),
        "DCTRL|SHIFTsplittruetrueRight"
    );
    assert_eq!(s(&l, r#"return db.parse_opts({ target = "tab", keys = false }).target"#), "tab");
}

#[test]
fn build_args() {
    let l = lua();
    let j = |code: &str| s(&l, &format!("return table.concat({code}, ' ')"));
    assert_eq!(j("db.build_args('saysdf', nil, nil)"), "saysdf db");
    assert_eq!(j("db.build_args('saysdf', 'prod', nil)"), "saysdf db --connection prod");
    assert_eq!(j("db.build_args('saysdf', 'C:/data/app.sqlite', nil)"), "saysdf db -- C:/data/app.sqlite");
    assert_eq!(j("db.build_args('saysdf', 'app.db', nil)"), "saysdf db -- app.db");
    assert_eq!(j("db.build_args('saysdf', { id = 'x' }, '/tmp/c.toml')"), "saysdf db --connections /tmp/c.toml --connection x");
    assert_eq!(j("db.build_args('saysdf', { path = '-rf' }, nil)"), "saysdf db -- -rf");
}

#[test]
fn connections_toml_drops_password() {
    let l = lua();
    let t = s(
        &l,
        r#"return db.connections_toml({
            { id = "pg", kind = "postgres", title = 'Прод "main"', -- i18n-allow-cyrillic
              host = "db", port = 5432, user = "ro", password = "secret", remember_password = true },
            { title = "без id" }, -- i18n-allow-cyrillic
        })"#,
    );
    assert!(t.contains("[[connection]]"), "{t}");
    assert!(t.contains("id = \"pg\""), "{t}");
    assert!(t.contains("title = \"Прод \\\"main\\\"\""), "{t}"); // i18n-allow-cyrillic
    assert!(t.contains("port = 5432"), "{t}");
    assert!(t.contains("remember_password = true"), "{t}");
    assert!(!t.contains("secret"), "{t}");
    assert_eq!(t.matches("[[connection]]").count(), 1);
}

#[test]
fn setup_adds_key() {
    let l = lua();
    assert_eq!(
        s(&l, r#"local c = db.setup({}, {}) return c.keys[1].key .. c.keys[1].mods .. type(c.keys[1].action)"#),
        "DCTRL|SHIFTfunction"
    );
    assert_eq!(s(&l, r#"local c = db.setup({ keys = false, palette = false }, {}) return tostring(c.keys)"#), "nil");
}
