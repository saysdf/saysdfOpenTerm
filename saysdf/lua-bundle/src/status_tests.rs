//! Tests of the `saysdf.status` module (shared status bar compositor).
use super::*;

const FAKE: &str = r#"
    handlers = {}
    logs = {}
    package.loaded.wezterm = {
        on = function(name, f) handlers[name] = f end,
        log_error = function(msg) table.insert(logs, msg) end,
        format = function(items)
            local t = {}
            for _, it in ipairs(items) do table.insert(t, it.Text or "") end
            return table.concat(t)
        end,
    }
    status = require('saysdf.status')
    right, left = nil, nil
    window = {
        set_right_status = function(self, t) right = t end,
        set_left_status = function(self, t) left = t end,
    }
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(vec![("saysdf.status", get("saysdf.status").unwrap()), ("saysdf.util", get("saysdf.util").unwrap())].into_boxed_slice());
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
fn two_segments_in_order() {
    let l = lua();
    l.load(
        r#"
        status.add("b", function() return "B" end, { order = 20 })
        status.add("a", function() return { { Text = "A" } } end, { order = 10 })
        handlers["update-status"](window, {})
    "#,
    )
    .exec()
    .unwrap();
    assert_eq!(s(&l, "return right"), " A │ B ");
}

#[test]
fn failing_segment_does_not_break_others() {
    let l = lua();
    l.load(
        r#"
        status.add("bad", function() error("boom") end, { order = 1 })
        status.add("ok", function() return "OK" end, { order = 2 })
        status.add("empty", function() return "" end, { order = 3 })
        status.render(window, {})
    "#,
    )
    .exec()
    .unwrap();
    assert_eq!(s(&l, "return right"), " OK ");
    assert!(s(&l, "return logs[1]").contains("bad"));
}

#[test]
fn left_side_and_single_handler() {
    let l = lua();
    l.load(
        r#"
        n = 0
        local orig = package.loaded.wezterm.on
        package.loaded.wezterm.on = function(name, f) n = n + 1; orig(name, f) end
        status.add("l", function() return "L" end, { side = "left" })
        status.add("r", function() return "R" end)
        status.render(window, {})
    "#,
    )
    .exec()
    .unwrap();
    assert_eq!(s(&l, "return left"), " L ");
    assert_eq!(s(&l, "return right"), " R ");
    assert_eq!(s(&l, "return tostring(n)"), "1");
}

#[test]
fn util_notify_shows_message_in_status_and_toast() {
    let l = lua();
    l.load(
        r#"
        package.loaded.wezterm.log_info = function() end
        package.loaded.wezterm.log_warn = function() end
        local util = require('saysdf.util')
        util._forget_notices()
        t = 100
        util._now = function() return t end
        toasts = {}
        window.window_id = function() return 7 end
        window.active_pane = function() return {} end
        window.toast_notification = function(self, title, text, url, ms, pid)
            table.insert(toasts, title .. "|" .. text .. "|" .. tostring(pid))
        end
        status.add("x", function() return "X" end, { order = 50 })
        ok = util.notify(window, "git", "lazygit\nnot found", { pane_id = 3 })
        shown = right
        t = 104
        status.render(window, {})
        still = right
        t = 105
        status.render(window, {})
        gone = right
        -- toast broken (notifications off / API error) - the message is still in the bar
        window.toast_notification = function() error("off") end
        ok2 = util.notify(window, "saysdf", "msg2")
        broken = right
    "#,
    )
    .exec()
    .unwrap();
    assert_eq!(s(&l, "return shown"), " ● git: lazygit not found │ X ");
    assert_eq!(s(&l, "return toasts[1]"), "git|lazygit not found|3");
    assert!(l.load("return ok").eval::<bool>().unwrap());
    assert_eq!(s(&l, "return still"), " ● git: lazygit not found │ X ");
    assert_eq!(s(&l, "return gone"), " X ");
    assert!(!l.load("return ok2").eval::<bool>().unwrap());
    assert_eq!(s(&l, "return broken"), " ● saysdf: msg2 │ X ");
}

/// util.notify cleans untrusted text (security review 2026-10-21-i18n, I5): escapes, control,
/// bidi and zero-width characters never reach the status bar or the toast.
#[test]
fn util_notify_sanitizes_text() {
    let l = lua();
    l.load(
        r#"
        package.loaded.wezterm.log_info = function() end
        package.loaded.wezterm.log_warn = function() end
        local util = require('saysdf.util')
        util._forget_notices()
        util._now = function() return 100 end
        toasts = {}
        window.window_id = function() return 8 end
        window.active_pane = function() return {} end
        window.toast_notification = function(self, title, text)
            table.insert(toasts, title .. "|" .. text)
        end
        util.notify(window, "fi\u{200B}les", "copied: \u{1b}]52;c;eA==\u{7}ok\u{202E}txt.exe\r\nend")
        notice = util.notice_text(window)
        "#,
    )
    .exec()
    .unwrap();
    assert_eq!(s(&l, "return notice"), "fi les: copied:  ]52;c;eA== ok txt.exe end");
    assert_eq!(s(&l, "return toasts[1]"), "fi les|copied:  ]52;c;eA== ok txt.exe end");
}
