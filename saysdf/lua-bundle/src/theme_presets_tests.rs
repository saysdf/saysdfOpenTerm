//! Tests of `saysdf.theme_presets` (the scheme catalog for "View -> Theme"): the module is pure data,
//! every scheme has the required fields, non-built-in ones have a full #rrggbb palette,
//! the declared contrast matches the recomputed one and is not below the group threshold.
use super::*;
use std::collections::HashSet;

fn presets(lua: &Lua) -> Table<'_> {
    let modules: Modules = Box::leak(
        vec![("saysdf.i18n", get("saysdf.i18n").unwrap())].into_boxed_slice(),
    );
    install_searcher_with(lua, modules).unwrap();
    lua.load(get("saysdf.theme_presets").unwrap())
        .set_name(chunk_name("saysdf.theme_presets"))
        .eval()
        .unwrap()
}

fn is_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn lum(hex: &str) -> f64 {
    let ch = |i: usize| {
        let v = u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f64 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(1) + 0.7152 * ch(3) + 0.0722 * ch(5)
}

fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[test]
fn has_no_setup_and_no_side_effects() {
    let lua = Lua::new();
    let before: usize = lua.globals().pairs::<Value, Value>().count();
    let p = presets(&lua);
    let after: usize = lua.globals().pairs::<Value, Value>().count();
    assert_eq!(before, after, "the module must not touch global variables");
    assert!(matches!(p.get::<_, Value>("setup").unwrap(), Value::Nil));
}

#[test]
fn every_scheme_is_well_formed() {
    let lua = Lua::new();
    let p = presets(&lua);
    let groups: Table = p.get("groups").unwrap();
    let mut names = HashSet::new();
    let mut total = 0;
    let mut group_ids = Vec::new();
    for g in groups.sequence_values::<Table>() {
        let g = g.unwrap();
        let gid: String = g.get("id").unwrap();
        let glabel: String = g.get("label").unwrap();
        assert!(!glabel.is_empty());
        group_ids.push(gid.clone());
        let schemes: Table = g.get("schemes").unwrap();
        assert!(schemes.raw_len() > 0, "empty group {gid}");
        for s in schemes.sequence_values::<Table>() {
            let s = s.unwrap();
            total += 1;
            let name: String = s.get("name").unwrap();
            assert!(names.insert(name.clone()), "duplicate name {name}");
            let label: String = s.get("label").unwrap();
            assert!(!label.is_empty(), "{name}: empty label");
            let group: String = s.get("group").unwrap();
            assert_eq!(group, gid, "{name}: group does not match the group id");
            let kind: String = s.get("kind").unwrap();
            assert!(kind == "light" || kind == "dark", "{name}: kind {kind}");
            let source: String = s.get("source").unwrap();
            assert!(!source.is_empty(), "{name}: no source");
            let c: f64 = s.get("contrast").unwrap();
            let limit = if gid == "contrast" { 7.0 } else { 4.5 };
            assert!(c >= limit && c <= 21.0, "{name}: contrast {c} < {limit}");
            let _ansi_min: f64 = s.get("ansi_min").unwrap();

            if let Value::Table(colors) = s.get::<_, Value>("colors").unwrap() {
                for key in [
                    "foreground",
                    "background",
                    "cursor_bg",
                    "cursor_fg",
                    "cursor_border",
                    "selection_fg",
                    "selection_bg",
                ] {
                    let v: String = colors
                        .get(key)
                        .unwrap_or_else(|e| panic!("{name}: no {key}: {e}"));
                    assert!(is_hex(&v), "{name}: {key} = {v}");
                }
                for key in ["ansi", "brights"] {
                    let list: Vec<String> = colors.get(key).unwrap();
                    assert_eq!(list.len(), 8, "{name}: {key} is not 8 colors");
                    assert!(list.iter().all(|v| is_hex(v)), "{name}: {key} {list:?}");
                }
                let fg: String = colors.get("foreground").unwrap();
                let bg: String = colors.get("background").unwrap();
                let real = contrast(&fg, &bg);
                assert!((real - c).abs() < 0.01, "{name}: declared {c}, by colors {real:.2}");
                let want_kind = if lum(&bg) > 0.4 { "light" } else { "dark" };
                assert_eq!(kind, want_kind, "{name}: kind by background");
            }
        }
    }
    assert_eq!(
        group_ids,
        ["windows", "macos", "ubuntu", "contrast", "light", "dark", "popular"]
    );
    assert!(total >= 40, "fewer than 40 schemes: {total}");
}

#[test]
fn find_list_and_schemes_table() {
    let lua = Lua::new();
    let p = presets(&lua);
    lua.globals().set("p", p).unwrap();
    let (n, label): (usize, String) = lua
        .load("return #p.list(), p.find('Campbell (Windows Terminal)').label")
        .eval()
        .unwrap();
    assert!(n >= 40);
    assert!(label.contains("Campbell"));
    let missing: bool = lua.load("return p.find('nope') == nil").eval().unwrap();
    assert!(missing);
    // schemes_table: only non-built-in schemes, as copies (editing does not corrupt the catalog)
    let ok: bool = lua
        .load(
            r#"
            local t = p.schemes_table()
            local n = 0
            for name, c in pairs(t) do
                n = n + 1
                assert(p.find(name).colors, name)
                assert(#c.ansi == 8 and #c.brights == 8)
            end
            assert(t['Modus-Vivendi'] == nil, 'a built-in scheme is not registered')
            t['Campbell (Windows Terminal)'].ansi[1] = 'x'
            assert(p.find('Campbell (Windows Terminal)').colors.ansi[1] ~= 'x')
            return n > 0
            "#,
        )
        .eval()
        .unwrap();
    assert!(ok);
}

#[test]
fn texts_follow_language() {
    let lua = Lua::new();
    let p = presets(&lua);
    lua.globals().set("p", p).unwrap();
    let run = |lang: &str| -> String {
        lua.load(format!(
            "require('saysdf.i18n').select({lang:?})
             local s = p.find('Campbell (Windows Terminal)')
             return p.tr(p.groups[4].label) .. '|' .. p.tr(s.label)"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(run("en"), "High contrast|Windows Terminal: Campbell (dark, default)");
    let ru = run("ru");
    run("en");
    assert_eq!(ru, "Высокий контраст|Windows Terminal — Campbell (тёмная, по умолчанию)"); // i18n-allow-cyrillic
}
