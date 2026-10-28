//! Native part of `saysdf.i18n` (change `i18n`, design D6): `package.preload["saysdf._i18n"]`
//! exposes the `saysdf-i18n` crate, so Lua and Rust share one implementation of catalogs,
//! language detection, plurals and placeholders.
//!
//! The catalog lives in the Lua state (app data): `init` and every lookup touch only that
//! state, so two Lua states (config evaluations, parallel tests) use different languages at
//! the same time. A Lua state where `init` was never called loads `$SAYSDF_LANG`/`auto` on
//! first use. Rust code of the GUI uses the process catalog; only `apply_global` (called by
//! `saysdf.setup`) copies the state's catalog there, so a test `select` never changes it.
use mlua::{Lua, Table, Value};
use saysdf_i18n::Catalog;
use std::path::PathBuf;
use std::sync::Arc;

/// Name of the native module in `package.preload`.
pub const NATIVE_MODULE: &str = "saysdf._i18n";

struct LuaCatalog(Arc<Catalog>);

fn catalog(lua: &Lua) -> Arc<Catalog> {
    if let Some(c) = lua.app_data_ref::<LuaCatalog>() {
        return Arc::clone(&c.0);
    }
    let requested = std::env::var(saysdf_i18n::LANG_ENV).unwrap_or_default();
    let (cat, _) = saysdf_i18n::load(&requested, None);
    lua.set_app_data(LuaCatalog(Arc::clone(&cat)));
    cat
}

fn vars(lua: &Lua, t: Option<Table>) -> mlua::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    if let Some(t) = t {
        for pair in t.pairs::<Value, Value>() {
            let (k, v) = pair?;
            let Value::String(k) = k else { continue };
            let v = match v {
                Value::Boolean(b) => b.to_string(),
                other => match lua.coerce_string(other)? {
                    Some(s) => s.to_str()?.to_string(),
                    None => continue,
                },
            };
            out.push((k.to_str()?.to_string(), v));
        }
    }
    Ok(out)
}

fn user_dir(config_dir: Option<String>) -> Option<PathBuf> {
    config_dir
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join(saysdf_i18n::USER_LOCALES_DIR))
}

fn borrow(v: &[(String, String)]) -> Vec<(&str, &str)> {
    v.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

fn create(lua: &Lua) -> mlua::Result<Table<'_>> {
    let m = lua.create_table()?;
    // init(requested, config_dir|nil) -> language, { warnings... }, rejected: the catalog of
    // this Lua state; user catalogs: <config_dir>/locales/<lang>/*.toml; rejected = number of
    // broken user catalog files that were skipped
    m.set(
        "init",
        lua.create_function(|lua, (requested, config_dir): (Option<String>, Option<String>)| {
            let (cat, init) = saysdf_i18n::load(
                requested.as_deref().unwrap_or(saysdf_i18n::AUTO),
                user_dir(config_dir).as_deref(),
            );
            let rejected = cat.rejected_user_files;
            lua.set_app_data(LuaCatalog(cat));
            Ok((init.language, lua.create_sequence_from(init.warnings)?, rejected))
        })?,
    )?;
    // apply_global() -> language: makes this state's catalog the process catalog of the Rust
    // code (saysdf_i18n::Domain); saysdf.setup calls it for the evaluated config
    m.set(
        "apply_global",
        lua.create_function(|lua, ()| {
            let cat = catalog(lua);
            let lang = cat.lang.clone();
            saysdf_i18n::set_current(cat);
            Ok(lang)
        })?,
    )?;
    m.set(
        "language",
        lua.create_function(|lua, ()| Ok(catalog(lua).lang.clone()))?,
    )?;
    m.set(
        "languages",
        lua.create_function(|_, config_dir: Option<String>| {
            Ok(saysdf_i18n::languages(user_dir(config_dir).as_deref()))
        })?,
    )?;
    m.set(
        "system_language",
        lua.create_function(|_, ()| Ok(saysdf_i18n::detect::system_language()))?,
    )?;
    m.set("env", saysdf_i18n::LANG_ENV)?;
    // single_line(text) -> text without control and invisible/bidi characters (each run becomes
    // one space): saysdf.util.notify uses it, one set with the GUI bars and catalogs
    m.set(
        "single_line",
        lua.create_function(|_, s: mlua::String| {
            Ok(saysdf_i18n::text::single_line(&s.to_string_lossy()))
        })?,
    )?;
    // t(domain, key, vars|nil) -> string
    m.set(
        "t",
        lua.create_function(|lua, (domain, key, v): (String, String, Option<Table>)| {
            let v = vars(lua, v)?;
            Ok(catalog(lua).text(&domain, &key, &borrow(&v)))
        })?,
    )?;
    // tn(domain, one, other, n, vars|nil) -> string
    m.set(
        "tn",
        lua.create_function(
            |lua, (domain, one, other, n, v): (String, String, String, f64, Option<Table>)| {
                let v = vars(lua, v)?;
                let count = if n.is_finite() { n.abs().floor() as u64 } else { 0 };
                Ok(catalog(lua).plural(&domain, &one, &other, count, &borrow(&v)))
            },
        )?,
    )?;
    // accel(entry_id) -> key|nil
    m.set(
        "accel",
        lua.create_function(|lua, id: String| Ok(catalog(lua).accel.get(&id).cloned()))?,
    )?;
    // accel_key(spec) -> canonical key ("q" -> "Q") | nil if the menu bar would reject it;
    // saysdf.menu compares accelerators by it when it applies catalog overrides
    m.set(
        "accel_key",
        lua.create_function(|_, spec: String| Ok(saysdf_i18n::catalog::accel_key(&spec)))?,
    )?;
    Ok(m)
}

/// Registers the native module in `package.preload`.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let package: Table = lua.globals().get("package")?;
    let preload: Table = package.get("preload")?;
    preload.set(NATIVE_MODULE, lua.create_function(|lua, ()| create(lua))?)?;
    Ok(())
}
