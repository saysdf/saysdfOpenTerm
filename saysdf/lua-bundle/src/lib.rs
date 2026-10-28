//! saysdfTerminal: the fork's Lua modules from `saysdf/lua/`, embedded into the binary
//! (change `bundled-lua-modules`, design D2–D3, D6).
use mlua::{Lua, Table, Value};
use std::io::Write;

pub mod i18n;

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/bundled.rs"));
}

/// Table of `(module name, source)`, sorted by name.
pub type Modules = &'static [(&'static str, &'static str)];

/// Module name in `package.preload` that lists the bundled modules (for `saysdf.setup`).
pub const BUNDLED_LIST_MODULE: &str = "saysdf._bundled";

/// Name of the bundled default config used by `show_module`.
pub const DEFAULT_CONFIG_NAME: &str = "default-config";

/// Source of a bundled module by its full name (`saysdf`, `saysdf.tabs`).
pub fn get(name: &str) -> Option<&'static str> {
    find(bundled::MODULES, name)
}

/// Full names of the bundled modules, in alphabetical order.
pub fn names() -> impl Iterator<Item = &'static str> {
    bundled::MODULES.iter().map(|(name, _)| *name)
}

/// Bundled default config (`saysdf/lua/default_config.lua`).
pub fn default_config() -> &'static str {
    bundled::DEFAULT_CONFIG
}

fn find(modules: Modules, name: &str) -> Option<&'static str> {
    modules
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| modules[i].1)
}

fn chunk_name(name: &str) -> String {
    let file = match name.strip_prefix("saysdf.") {
        Some(rest) => format!("saysdf/{}.lua", rest.replace('.', "/")),
        None => format!("{}/init.lua", name.replace('.', "/")),
    };
    format!("@saysdf-bundled:{file}")
}

/// Installs the bundled-module searcher into `lua`.
pub fn install_searcher(lua: &Lua) -> mlua::Result<()> {
    install_searcher_with(lua, bundled::MODULES)
}

/// Like [`install_searcher`], but with a given set of modules (for tests).
///
/// The searcher is inserted into `package.searchers` at position 3 — after `preload` and the
/// file search along `package.path`, so a user file overrides a bundled module.
/// `modules` must be sorted by name.
pub fn install_searcher_with(lua: &Lua, modules: Modules) -> mlua::Result<()> {
    let package: Table = lua.globals().get("package")?;

    let searcher = lua.create_function(move |lua, name: String| match find(modules, &name) {
        Some(source) => {
            let loader = lua
                .load(source)
                .set_name(chunk_name(&name))
                .into_function()?;
            Ok(Value::Function(loader))
        }
        // Lua 5.4 itself prepends "\n\t" to the message of each searcher
        None => Ok(Value::String(
            lua.create_string(format!("no bundled module '{name}'"))?,
        )),
    })?;
    let searchers: Table = package.get("searchers")?;
    let pos = (searchers.raw_len() + 1).min(3);
    let insert: mlua::Function = lua.globals().get::<_, Table>("table")?.get("insert")?;
    insert.call::<_, ()>((searchers, pos, searcher))?;

    let list = lua.create_function(move |lua, ()| {
        lua.create_sequence_from(modules.iter().map(|(name, _)| *name))
    })?;
    let preload: Table = package.get("preload")?;
    preload.set(BUNDLED_LIST_MODULE, list)?;
    i18n::install(lua)?;
    Ok(())
}

/// `saysdfTerm-cli show-module [NAME] [--list]`: prints the source of a bundled module
/// or the list of modules. Returns the exit code.
pub fn show_module(
    name: Option<&str>,
    list: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    show_module_with(bundled::MODULES, default_config(), name, list, out, err)
}

fn show_module_with(
    modules: Modules,
    default_config: &'static str,
    name: Option<&str>,
    list: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let write_list = |w: &mut dyn Write| -> std::io::Result<()> {
        for (n, _) in modules {
            writeln!(w, "{n}")?;
        }
        writeln!(w, "{DEFAULT_CONFIG_NAME}")
    };

    let result = match (name, list) {
        (_, true) => write_list(out).map(|_| 0),
        (None, false) => writeln!(err, "usage: show-module <NAME> | --list; available:")
            .and_then(|_| write_list(err))
            .map(|_| 1),
        (Some(name), false) => match resolve(modules, default_config, name) {
            Some(source) => out.write_all(source.as_bytes()).map(|_| 0),
            None => writeln!(err, "unknown bundled module '{name}'; available:")
                .and_then(|_| write_list(err))
                .map(|_| 1),
        },
    };
    result.unwrap_or(1)
}

fn resolve(modules: Modules, default_config: &'static str, name: &str) -> Option<&'static str> {
    let name = name.trim();
    if name == DEFAULT_CONFIG_NAME || name == "default_config" {
        return Some(default_config);
    }
    find(modules, name).or_else(|| find(modules, &format!("saysdf.{name}")))
}

#[cfg(test)]
mod shell_tests;

#[cfg(test)]
mod multiline_tests;

#[cfg(test)]
mod hints_tests;

#[cfg(test)]
mod close_tests;
#[cfg(test)]
mod panes_tests; // saysdfTerminal: pane-header

#[cfg(test)]
mod tabs_tests;

#[cfg(test)]
mod notify_tests;

#[cfg(test)]
mod paths_tests;

#[cfg(test)]
mod quickselect_tests;

#[cfg(test)]
mod git_tests;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod viewer_tests;
#[cfg(test)]
mod ssh_tests;

#[cfg(test)]
mod workspaces_tests;
#[cfg(test)]
mod monitor_tests;
#[cfg(test)]
mod db_tests;
#[cfg(test)]
mod ui_bars_tests;
#[cfg(test)]
mod theme_tests;
#[cfg(test)]
mod theme_presets_tests;
#[cfg(test)]
mod keys_tests;
#[cfg(test)]
mod perf_tests;
#[cfg(test)]
mod sidebar_tests;
#[cfg(test)]
mod files_tests; // file-browser
#[cfg(test)]
mod i18n_tests;
#[cfg(test)]
mod i18n_security_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_with(modules: Modules) -> Lua {
        let lua = Lua::new();
        {
            let package: Table = lua.globals().get("package").unwrap();
            package.set("path", "").unwrap();
            package.set("cpath", "").unwrap();
        }
        install_searcher_with(&lua, modules).unwrap();
        lua
    }

    #[test]
    fn bundled_table() {
        assert!(get("saysdf").is_some());
        assert!(get("saysdf.nope").is_none());
        let all: Vec<_> = names().collect();
        let mut sorted = all.clone();
        sorted.sort();
        assert_eq!(all, sorted);
        assert!(default_config().contains("require(\"saysdf\")"));
    }

    #[test]
    fn bundled_sources_compile() {
        let lua = Lua::new();
        for (name, src) in bundled::MODULES {
            lua.load(*src)
                .set_name(chunk_name(name))
                .into_function()
                .unwrap();
        }
        lua.load(default_config()).into_function().unwrap();
    }

    static FAKE: &[(&str, &str)] = &[("saysdf.x", "return { from = 'bundled', arg = ... }")];

    #[test]
    fn require_bundled() {
        let lua = lua_with(FAKE);
        let from: String = lua.load("return require('saysdf.x').from").eval().unwrap();
        assert_eq!(from, "bundled");
        let arg: String = lua.load("return require('saysdf.x').arg").eval().unwrap();
        assert_eq!(arg, "saysdf.x");
        let list: Vec<String> = lua.load("return require('saysdf._bundled')").eval().unwrap();
        assert_eq!(list, vec!["saysdf.x"]);
    }

    #[test]
    fn unknown_module() {
        let lua = lua_with(FAKE);
        let err = lua
            .load("require('saysdf.nope')")
            .exec()
            .unwrap_err()
            .to_string();
        assert!(err.contains("no bundled module 'saysdf.nope'"), "{err}");
    }

    #[test]
    fn file_overrides_bundled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("saysdf")).unwrap();
        std::fs::write(
            dir.path().join("saysdf").join("x.lua"),
            "return { from = 'file' }",
        )
        .unwrap();
        let lua = lua_with(FAKE);
        let package: Table = lua.globals().get("package").unwrap();
        package
            .set("path", format!("{}/?.lua", dir.path().display()))
            .unwrap();
        let from: String = lua.load("return require('saysdf.x').from").eval().unwrap();
        assert_eq!(from, "file");
    }

    const FAKE_WEZTERM: &str = r#"
        calls = {}
        warnings = {}
        package.loaded.wezterm = {
            config_builder = function() return { built = true } end,
            log_warn = function(msg) table.insert(warnings, msg) end,
        }
    "#;

    const MOD_A: &str = "return { setup = function(o, c) calls.a = o; c.a = true end }";
    const MOD_B: &str = "return { setup = function(o, c) calls.b = o; c.b = true end }";

    fn setup_lua() -> Lua {
        let modules: Modules = Box::leak(
            vec![
                ("saysdf", get("saysdf").unwrap()),
                ("saysdf.a", MOD_A),
                ("saysdf.b", MOD_B),
                ("saysdf.lib", "return { helper = true }"),
            ]
            .into_boxed_slice(),
        );
        let lua = lua_with(modules);
        lua.load(FAKE_WEZTERM).exec().unwrap();
        lua
    }

    #[test]
    fn setup_disables_module() {
        let lua = setup_lua();
        lua.load("cfg = require('saysdf').setup({ b = false }, {})")
            .exec()
            .unwrap();
        let (a_empty, b_called, b_loaded, cfg_a, warns): (bool, bool, bool, bool, usize) = lua
            .load(
                "return next(calls.a) == nil, calls.b ~= nil, \
                 package.loaded['saysdf.b'] ~= nil, cfg.a == true, #warnings",
            )
            .eval()
            .unwrap();
        assert!(a_empty);
        assert!(!b_called);
        assert!(!b_loaded);
        assert!(cfg_a);
        assert_eq!(warns, 0);
    }

    #[test]
    fn setup_passes_options_and_builds_config() {
        let lua = setup_lua();
        let (max, built): (i64, bool) = lua
            .load(
                "local c = require('saysdf').setup({ a = { max = 10 } }) \
                 return calls.a.max, c.built",
            )
            .eval()
            .unwrap();
        assert_eq!(max, 10);
        assert!(built);
    }

    #[test]
    fn setup_warns_unknown_key() {
        let lua = setup_lua();
        let (n, msg): (usize, String) = lua
            .load("require('saysdf').setup({ nope = true }, {}) return #warnings, warnings[1]")
            .eval()
            .unwrap();
        assert_eq!(n, 1);
        assert!(msg.contains("nope"), "{msg}");
    }

    static SHOW: &[(&str, &str)] = &[("saysdf", "ROOT"), ("saysdf.tabs", "TABS")];

    fn show(name: Option<&str>, list: bool) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = show_module_with(SHOW, "DEFAULT", name, list, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn show_module_names() {
        assert_eq!(show(Some("tabs"), false), (0, "TABS".into(), "".into()));
        assert_eq!(show(Some("saysdf.tabs"), false).1, "TABS");
        assert_eq!(show(Some("saysdf"), false).1, "ROOT");
        assert_eq!(show(Some("default-config"), false).1, "DEFAULT");

        let (code, out, err) = show(Some("nope"), false);
        assert_eq!(code, 1);
        assert!(out.is_empty());
        assert!(
            err.contains("saysdf.tabs") && err.contains("default-config"),
            "{err}"
        );

        let (code, out, _) = show(None, true);
        assert_eq!(code, 0);
        assert_eq!(out, "saysdf\nsaysdf.tabs\ndefault-config\n");

        assert_eq!(show(None, false).0, 1);
    }
}
