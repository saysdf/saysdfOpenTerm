//! Malicious catalog samples of security review `security-reviews/2026-10-21-i18n.md` against
//! the Lua side: the loader drops the file, the menu bar escapes its markup, catalog
//! accelerators only take free slots, and a rejected bar update does not raise.
use super::*;

const FAKE: &str = r#"
    logs = {}
    notes = {}
    local function ctor(name)
        return setmetatable({ name = name }, { __call = function(self, arg) return { name = name, arg = arg } end })
    end
    package.loaded.wezterm = {
        on = function() end,
        log_error = function(m) table.insert(logs, m) end,
        log_warn = function(m) table.insert(logs, m) end,
        log_info = function() end,
        action_callback = function(f) return f end,
        action = setmetatable({}, { __index = function(_, k) return ctor(k) end }),
        GLOBAL = {},
        home_dir = "C:/Users/u",
    }
    i18n = require('saysdf.i18n')
    i18n.select('en')
    palette = require('saysdf.palette')
    menu = require('saysdf.menu')
    util = require('saysdf.util')
    util.notify = function(window, title, text) table.insert(notes, title .. ": " .. text) end
"#;

fn lua() -> Lua {
    let lua = Lua::new();
    {
        let package: Table = lua.globals().get("package").unwrap();
        package.set("path", "").unwrap();
        package.set("cpath", "").unwrap();
    }
    install_searcher(&lua).unwrap();
    lua.load(FAKE).exec().unwrap();
    lua
}

fn s(l: &Lua, code: &str) -> String {
    l.load(code).eval().unwrap()
}

#[test]
fn disguised_user_catalogs_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let eo = dir.path().join("locales").join("eo");
    std::fs::create_dir_all(&eo).unwrap();
    // M1: RLO in a label; L1: a tab (fake hotkey caption); L2: a repeated placeholder;
    // L4: an accelerator the menu bar would reject. Each file is dropped as a whole.
    std::fs::write(eo.join("menu.toml"), "\"View\" = \"Vido\\u202Eevil\"\n").unwrap();
    std::fs::write(eo.join("palette.toml"), "\"Tools\" = \"Iloj\\tCtrl+Q\"\n").unwrap();
    std::fs::write(
        eo.join("quickselect.toml"),
        "\"Copied: {text}\" = \"{text}{text}{text}{text}{text}\"\n",
    )
    .unwrap();
    std::fs::write(eo.join("git.toml"), "[_accel]\n\"core.quit\" = \"Ctrl+Nope+Q\"\n").unwrap();
    std::fs::write(eo.join("tabs.toml"), "\"Tabs\" = \"Langetoj\"\n").unwrap();

    let l = lua();
    l.globals().set("DIR", dir.path().to_str().unwrap()).unwrap();
    assert_eq!(
        s(
            &l,
            r#"
            logs = {}
            local lang = i18n.select("eo", DIR)
            local t = i18n.domain("menu")
            local tabs = i18n.domain("tabs")
            local w = logs[1] or ""
            local r = { lang, t("View"), tabs("Tabs"), tostring(i18n.accel("core.quit")), #logs }
            for _, f in ipairs({ "menu.toml", "palette.toml", "quickselect.toml", "git.toml" }) do
                table.insert(r, tostring(w:find(f, 1, true) ~= nil))
            end
            table.insert(r, tostring(w:find("tabs.toml", 1, true) ~= nil))
            return table.concat(r, "|")
            "#
        ),
        "eo|View|Langetoj|nil|1|true|true|true|true|false"
    );
}

#[test]
fn menu_bar_escapes_markup_of_labels_and_titles() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            local tree = {
                { title = "&File", items = { { label = "Save & quit\tCtrl+Q", id = "x.a", accel = "q" } } },
            }
            local bar = menu.to_bar(tree)
            local it = bar.left[1].children[1]
            return bar.left[1].text .. "|" .. it.text .. "|" .. tostring(it.key)
            "#
        ),
        "&&File|Save && quit Ctrl+Q|nil"
    );
}

#[test]
fn catalog_accelerators_only_take_free_slots() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            logs = {}
            local act = function() end
            local entries = {
                { id = "x.a", menu = "File", label = "A", accel = "q", action = act, order = 1 },
                { id = "x.b", menu = "File", label = "B", accel = "w", action = act, order = 2 },
                { id = "x.c", menu = "File", label = "C", accel = "e", action = act, order = 3 },
                { id = "x.d", menu = "File", label = "D", accel = "r", action = act, order = 4 },
                { id = "x.e", menu = "File", label = "E", accel = "t", action = act, order = 5 },
                { id = "x.f", menu = "File", label = "F", accel = "y", action = act, order = 6 },
                { id = "x.g", menu = "File", label = "G", accel = "Ctrl+k", action = act, order = 7 },
                { id = "x.h", menu = "File", label = "H", accel = "u", action = act, order = 8 },
            }
            local over = {
                ["x.a"] = "w",          -- taken by x.b in code: x.a keeps q
                ["x.b"] = "q",          -- taken by x.a: x.b keeps w
                ["x.c"] = "z",          -- free: applied
                ["x.d"] = "z",          -- now taken by x.c's override: keeps r
                ["x.e"] = "Ctrl+Nope",  -- invalid: ignored
                ["x.f"] = "y",          -- its own key: nothing changes
                ["x.h"] = "CTRL+k",     -- x.g's key written differently: taken, keeps u
            }
            local tree = menu.build(entries, { accel_of = function(id) return over[id] end })
            local r = {}
            for _, it in ipairs(tree[1].items) do table.insert(r, it.id .. "=" .. it.accel) end
            -- one warning per refused override, not one per build
            menu.build(entries, { accel_of = function(id) return over[id] end })
            table.insert(r, tostring(#logs))
            -- an accel_of that raises does not break the menu
            local tree2 = menu.build(entries, { accel_of = function() error("boom") end })
            table.insert(r, tree2[1].items[1].accel)
            return table.concat(r, " ")
            "#
        ),
        "x.a=q x.b=w x.c=z x.d=r x.e=t x.f=y x.g=Ctrl+k x.h=u 4 q"
    );
}

#[test]
fn rejected_bar_update_gives_one_notice_and_does_not_raise() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"
            palette.add({ id = "x.a", menu = "File", label = "A", accel = "q", action = function() end })
            local calls = 0
            local window = {
                window_id = function() return 1 end,
                saysdf_set_bar = function(self, kind, spec)
                    calls = calls + 1
                    error("saysdf_set_bar: menu entry \"x.a\": invalid accel")
                end,
            }
            local r1 = menu.push(window, {}, true)
            local r2 = menu.push(window, {}, true)
            return tostring(r1) .. "|" .. tostring(r2) .. "|" .. calls .. "|" .. #notes .. "|"
                .. tostring(notes[1]:find("invalid accel", 1, true) ~= nil)
            "#
        ),
        "false|false|2|1|true"
    );
}
