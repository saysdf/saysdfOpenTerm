//! Tests of the `saysdf.panes` module and `saysdf.close.close_pane` (change pane-header, tasks 1.1–1.2).
use super::*;

const FAKE: &str = r#"
    handlers = {}
    warnings = {}
    performed = {}
    package.loaded.wezterm = {
        on = function(name, f) handlers[name] = f end,
        log_warn = function(msg) table.insert(warnings, msg) end,
        config_builder = function() return {} end,
        action = {
            TogglePaneZoomState = { name = 'TogglePaneZoomState' },
            CloseCurrentPane = function(args)
                return { name = 'CloseCurrentPane', confirm = args.confirm }
            end,
        },
    }
    function make_window()
        local win = { modes = {} }
        function win:perform_action(action, pane)
            table.insert(performed, { action = action, pane = pane.id })
        end
        function win:saysdf_set_pane_header(m) table.insert(self.modes, m) end
        function win:saysdf_pane_action(pane, action, confirm)
            table.insert(performed, { action = { name = action, confirm = confirm }, pane = pane.id })
        end
        return win
    end
    function make_pane(id)
        local p = { id = id, activated = 0 }
        function p:activate() self.activated = self.activated + 1 end
        return p
    end
    panes = require('saysdf.panes')
    close = require('saysdf.close')
"#;

fn lua() -> Lua {
    // sorted by name: the table lookup is binary
    let modules: Modules = Box::leak(
        vec![
            ("saysdf", get("saysdf").unwrap()),
            ("saysdf.close", get("saysdf.close").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.panes", get("saysdf.panes").unwrap()),
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

#[test]
fn header_option_validation() {
    let l = lua();
    let r: (String, String, String, String, i64) = l
        .load(
            "warnings = {} \
             return panes.header_mode({}), panes.header_mode({ header = 'always' }), \
                    panes.header_mode({ header = 'never' }), panes.header_mode({ header = 'bogus' }), \
                    #warnings",
        )
        .eval()
        .unwrap();
    assert_eq!(
        r,
        ("splits".into(), "always".into(), "never".into(), "splits".into(), 1)
    );
}

#[test]
fn setup_registers_click_and_sends_mode_on_reload() {
    let l = lua();
    let r: (bool, String) = l
        .load(
            "handlers = {} panes.setup({ header = 'always' }, {}) \
             local win = make_window() \
             handlers['window-config-reloaded'](win) \
             return handlers['saysdf-pane-header-click'] ~= nil, win.modes[1]",
        )
        .eval()
        .unwrap();
    assert_eq!(r, (true, "always".into()));
}

#[test]
fn zoom_and_title_buttons() {
    let l = lua();
    let r: (String, String, i64, i64, String) = l
        .load(
            "performed = {} handlers = {} panes.setup({}, {}) \
             local win, p = make_window(), make_pane(7) \
             handlers['saysdf-pane-header-click'](win, p, 'zoom') \
             local a = performed[1].action.name \
             local t = panes.on_click(win, p, 'title') \
             return a, t, #performed, p.activated, tostring(panes.on_click(win, p, 'evil'))",
        )
        .eval()
        .unwrap();
    // zoom -- by the pane id from the hit, without activate + perform_action (review L2); title -- activate
    assert_eq!(r, ("zoom".into(), "title".into(), 1, 1, "nil".into()));
}

#[test]
fn close_button_uses_close_mode() {
    let l = lua();
    let run = |mode: &str| -> (String, bool, i64, i64) {
        l.load(format!(
            "performed = {{}} close.setup({{ close_confirmation = '{mode}', palette = false }}, {{}}) \
             local win, p = make_window(), make_pane(3) \
             panes.on_click(win, p, 'close') \
             return performed[1].action.name, performed[1].action.confirm, performed[1].pane, p.activated"
        ))
        .eval()
        .unwrap()
    };
    // the pane from the hit (pane 3) is closed, the window active pane is not used (review L2)
    assert_eq!(run("always"), ("close".into(), true, 3, 0));
    assert_eq!(run("only-if-running-process"), ("close".into(), true, 3, 0));
    assert_eq!(run("never"), ("close".into(), false, 3, 0));
}

#[test]
fn buttons_never_use_current_pane_actions() {
    // perform_action acts on the window active pane -- header buttons do not call it
    let l = lua();
    let r: i64 = l
        .load(
            "performed = {} local win = make_window()              local used = 0              function win:perform_action() used = used + 1 end              local p = make_pane(5)              panes.on_click(win, p, 'zoom') panes.on_click(win, p, 'close')              return used",
        )
        .eval()
        .unwrap();
    assert_eq!(r, 0);
}

#[test]
fn close_pane_without_pane_is_noop() {
    let l = lua();
    let r: (bool, i64) = l
        .load("performed = {} return close.close_pane(make_window(), nil), #performed")
        .eval()
        .unwrap();
    assert_eq!(r, (false, 0));
}
