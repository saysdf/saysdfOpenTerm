//! Tests of `saysdf.theme` (change theme-switcher): the light/dark pair, the presets selection, saving/loading the
//! choice, the respect_config priority. The module list is sorted by name (the searcher uses binary search).
use super::*;

const FAKE: &str = r#"
    package.loaded.wezterm = {
        on = function() end,
        target_triple = 'x86_64-unknown-linux-gnu',
        home_dir = '/home/u',
        log_error = function() end, log_warn = function() end,
        json_parse = function(s)
            if s == 'BAD' then error('bad') end
            local mode = s:match('"mode":"(%w+)"')
            local scheme = s:match('"scheme":"(.-)"}')
            if scheme then scheme = scheme:gsub('\\"', '"') end
            return { mode = mode, scheme = scheme }
        end,
    }
    theme = require('saysdf.theme')
    require('saysdf.i18n').select('en')
    o = theme.merge_opts({})
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.theme", get("saysdf.theme").unwrap()),
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

fn lua_with_presets() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.theme", get("saysdf.theme").unwrap()),
            ("saysdf.theme_presets", get("saysdf.theme_presets").unwrap()),
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
    l.load(code).eval::<String>().unwrap()
}

#[test]
fn light_dark_pair() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"return theme.toggle_target(o.dark, o) .. '|' .. theme.toggle_target(o.light, o)
                .. '|' .. theme.toggle_target('Dracula', o) .. '|' .. theme.toggle_target('Dracula', o, 'dark')
                .. '|' .. theme.appearance_scheme('Light', o) .. '|' .. theme.appearance_scheme('DarkHighContrast', o)
                .. '|' .. theme.merge_opts({ dark = 'X' }).dark .. theme.merge_opts({ dark = 'X' }).light"#
        ),
        "Modus-Operandi|Modus-Vivendi|Modus-Vivendi|Modus-Operandi|Modus-Operandi|Modus-Vivendi|XModus-Operandi"
    );
}

#[test]
fn presets_registered_and_listed() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local p = {
                   groups = {
                       { id = 'contrast', label = 'High contrast', schemes = {
                           { name = 'Modus-Vivendi', label = 'Modus dark', contrast = 21, ansi_min = 4 },
                           { name = 'X (Src)', label = 'X label', contrast = 7.26, ansi_min = 1.5, colors = { foreground = '#000' } },
                       } },
                   },
                   schemes_table = function() return { ['X (Src)'] = { foreground = '#000' }, ['Mine'] = { foreground = '#fff' } } end,
               }
               local cfg = { color_schemes = { Mine = { foreground = '#111' } } }
               local n = theme.register_presets(cfg, p)
               local ch = theme.choices(p, { 'Modus-Vivendi', 'Zed' }, 'Zed')
               local labels = {}
               for _, c in ipairs(ch) do table.insert(labels, c.id .. '=' .. c.label) end
               return n .. '|' .. cfg.color_schemes['X (Src)'].foreground .. cfg.color_schemes.Mine.foreground
                   .. '|' .. theme.register_presets({}, nil) .. '|' .. table.concat(labels, ';')"#
        ),
        "1|#000#111|0|Modus-Vivendi=High contrast · Modus dark  [21.0:1];X (Src)=High contrast · X label  [7.3:1]  ⚠ ANSI;Zed=All · Zed  ✓"
    );
}

#[test]
fn texts_follow_language() {
    let l = lua_with_presets();
    let run = |lang: &str| -> String {
        l.load(format!(
            r#"require('saysdf.i18n').select({lang:?})
               local p = {{ tr = require('saysdf.theme_presets').tr, groups = {{ {{ id = 'contrast', label = 'High contrast', schemes = {{
                   {{ name = 'A', label = 'High contrast: IBM 3270 (dark)', contrast = 9 }} }} }} }} }}
               local ch = theme.choices(p, {{ 'Zed' }}, nil)
               local items = {{}}
               package.loaded['saysdf.palette'] = {{ add = function(e) table.insert(items, e.label .. '/' .. e.brief) end }}
               package.loaded.wezterm.action_callback = function(f) return f end
               theme.setup({{}}, {{}})
               return ch[1].label .. '|' .. ch[2].label .. '|' .. table.concat(items, ';')"#
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(
        run("en"),
        "High contrast · High contrast: IBM 3270 (dark)  [9.0:1]|All · Zed|Theme…/Theme: pick a color scheme;Light/Dark/Theme: light/dark;Follow system theme/Theme: follow system"
    );
    let ru = run("ru");
    run("en");
    assert_eq!(
        ru,
        "Высокий контраст · Высокий контраст — IBM 3270 (тёмная)  [9.0:1]|Все · Zed|Тема…/Тема: выбрать цветовую схему;Светлая/тёмная/Тема: светлая/тёмная;Тема как в системе/Тема: как в системе" // i18n-allow-cyrillic
    );
}

#[test]
fn real_presets_have_default_pair() {
    let l = lua_with_presets();
    assert_eq!(
        s(
            &l,
            r#"local p = theme.presets()
               local a, b = p.find(o.light), p.find(o.dark)
               local cfg = {}
               local n = theme.register_presets(cfg, p)
               return a.kind .. b.kind .. tostring(a.contrast >= 7 and b.contrast >= 7) .. tostring(n > 0)"#
        ),
        "lightdarktruetrue"
    );
}

#[test]
fn save_and_load_roundtrip() {
    let l = lua();
    let dir = std::env::temp_dir().join(format!("saysdf-theme-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("theme.json").to_string_lossy().replace('\\', "/");
    let got = s(
        &l,
        &format!(
            r#"local p = '{path}'
               assert(theme.save({{ mode = 'scheme', scheme = 'My "odd" scheme' }}, p))
               local a = theme.load(p)
               assert(theme.save({{ mode = 'system' }}, p))
               local b = theme.load(p)
               return a.mode .. ':' .. a.scheme .. '|' .. b.mode .. ':' .. tostring(b.scheme)
                   .. '|' .. tostring(theme.decode('BAD')) .. tostring(theme.decode('')) .. tostring(theme.decode('{{"mode":"x"}}'))
                   .. '|' .. tostring(theme.load('{path}.missing'))"#
        ),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got, "scheme:My \"odd\" scheme|system:nil|nilnilnil|nil");
}

#[test]
fn resolve_priority() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local st = { mode = 'scheme', scheme = 'Dracula' }
               local rc = theme.merge_opts({ respect_config = true })
               return tostring(theme.resolve(st, 'Builtin Solarized Light', o, 'Light'))
                   .. '|' .. tostring(theme.resolve(st, 'Builtin Solarized Light', rc, 'Light'))
                   .. '|' .. tostring(theme.resolve(st, nil, rc, 'Light'))
                   .. '|' .. tostring(theme.resolve({ mode = 'system' }, nil, o, 'Dark'))
                   .. '|' .. tostring(theme.resolve(nil, 'X', o, 'Dark'))"#
        ),
        "Dracula|nil|Dracula|Modus-Vivendi|nil"
    );
}

#[test]
fn setup_applies_saved_choice_over_config() {
    let l = lua();
    let dir = std::env::temp_dir().join(format!("saysdf-theme-setup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("theme.json").to_string_lossy().replace('\\', "/");
    let got = s(
        &l,
        &format!(
            r#"theme.save({{ mode = 'scheme', scheme = 'Dracula' }}, '{path}')
               theme.path = function() return '{path}' end
               local c1 = theme.setup({{ palette = false }}, {{ color_scheme = 'Builtin Solarized Light' }})
               local c2 = theme.setup({{ palette = false, respect_config = true }}, {{ color_scheme = 'Builtin Solarized Light' }})
               local c3 = theme.setup({{ palette = false, respect_config = true }}, {{}})
               return c1.color_scheme .. '|' .. c2.color_scheme .. '|' .. c3.color_scheme"#
        ),
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got, "Dracula|Builtin Solarized Light|Dracula");
}

#[test]
fn data_dir_env_override() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local u = require('saysdf.util')
               local env = { triple = 'x86_64-pc-windows-msvc', home = 'C:\\U',
                   getenv = function(n) if n == 'SAYSDFTERM_DATA_DIR' then return 'D:\\tmp\\x' end end }
               local env2 = { triple = 'x86_64-unknown-linux-gnu', home = '/h',
                   getenv = function(n) if n == 'SAYSDFTERM_DATA_DIR' then return '' end end }
               return u.data_dir(env) .. '|' .. u.data_dir(env2)"#
        ),
        "D:\\tmp\\x|/h/.local/share/saysdfTerm"
    );
}
