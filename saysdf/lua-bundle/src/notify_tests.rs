//! Tests of the `saysdf.notify` module (change command-notifications, tasks 1.1-1.2, 3.2, 4.1).
use super::*;

const FAKE: &str = r#"
    handlers = {}
    logs = {}
    toasts = {}
    attention = {}
    palette_sources = {}
    package.loaded.wezterm = {
        on = function(name, f) handlers[name] = f end,
        log_warn = function(msg) table.insert(logs, msg) end,
        log_info = function(msg) table.insert(logs, msg) end,
        log_error = function(msg) table.insert(logs, msg) end,
        action_callback = function(f) return { callback = f } end,
        GLOBAL = {},
    }
    package.loaded["saysdf.palette"] = { add = function(f) table.insert(palette_sources, f) end }
    package.loaded["saysdf.tabs"] = { set_attention = function(tab, on) attention[tab.id] = on end }
    notify = require('saysdf.notify')

    function make(focused, active_tab)
        local tab = { id = 7, tab_id = function(self) return self.id end, get_title = function() return "build" end }
        local other = { id = 1, tab_id = function(self) return self.id end }
        local pane = {
            pane_id = function() return 42 end,
            tab = function() return tab end,
            get_title = function() return "pane" end,
            get_user_vars = function() return {} end,
        }
        local window = {
            is_focused = function() return focused end,
            active_tab = function() return active_tab and tab or other end,
            active_pane = function() return pane end,
            toast_notification = function(self, t, b, url, timeout, pid)
                table.insert(toasts, { t = t, b = b, pid = pid })
            end,
        }
        return window, pane
    end
"#;

fn lua() -> Lua {
    let mut list = vec![
        ("saysdf.notify", get("saysdf.notify").unwrap()),
        ("saysdf.util", get("saysdf.util").unwrap()),
        ("saysdf.i18n", get("saysdf.i18n").unwrap()),
    ];
    list.sort();
    let modules: Modules = Box::leak(list.into_boxed_slice());
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

fn b(l: &Lua, code: &str) -> bool {
    l.load(code).eval().unwrap()
}

fn i(l: &Lua, code: &str) -> i64 {
    l.load(code).eval().unwrap()
}

#[test]
fn decide_threshold_focus_error_dnd() {
    let l = lua();
    let bg = "{ window_focused = true, tab_active = false, pane_active = true }";
    // short
    assert!(!b(&l, &format!("return notify.should_notify({{ duration_ms = 2000, exit_code = 0 }}, {bg})")));
    // long, in the background - both success and failure
    assert!(b(&l, &format!("return notify.should_notify({{ duration_ms = 40000, exit_code = 0 }}, {bg})")));
    assert!(b(&l, &format!("return notify.should_notify({{ duration_ms = 40000, exit_code = 1 }}, {bg})")));
    // focused - no
    assert!(!b(&l, "return notify.should_notify({ duration_ms = 40000 }, { window_focused = true, tab_active = true, pane_active = true })"));
    // minimized/inactive window - yes
    assert!(b(&l, "return notify.should_notify({ duration_ms = 40000 }, { window_focused = false, tab_active = true, pane_active = true })"));
    // threshold and on = error
    assert!(!b(&l, &format!("return notify.should_notify({{ duration_ms = 40000 }}, {bg}, {{ min_duration_s = 60 }})")));
    assert!(!b(&l, &format!("return notify.should_notify({{ duration_ms = 40000, exit_code = 0 }}, {bg}, {{ on = 'error' }})")));
    // no duration - no
    assert!(!b(&l, &format!("return notify.should_notify({{ exit_code = 1 }}, {bg})")));
    // DND suppresses the toast, keeps attention
    assert!(!b(&l, &format!("return notify.decide({{ duration_ms = 40000 }}, {bg}, {{ dnd = true }}).toast")));
    assert!(b(&l, &format!("return notify.decide({{ duration_ms = 40000 }}, {bg}, {{ dnd = true }}).attention")));
}

#[test]
fn format_duration_and_toast() {
    let l = lua();
    assert_eq!(s(&l, "return notify.format_duration(40000)"), "40s");
    assert_eq!(s(&l, "return notify.format_duration(65000)"), "1m 05s");
    assert_eq!(s(&l, "return notify.format_duration(3725000)"), "1h 02m");
    assert_eq!(
        s(&l, "return (notify.format_toast({ duration_ms = 40000, exit_code = 1 }, 'build'))"),
        "✗ build — 40s, exit 1"
    );
    assert_eq!(
        s(&l, "return (notify.format_toast({ duration_ms = 65000, exit_code = 0 }, 'build'))"),
        "✓ build — 1m 05s"
    );
}

#[test]
fn sanitize_length_control_bidi() {
    let l = lua();
    assert_eq!(s(&l, "local t, b = notify.sanitize(nil, 'hi\\27[31mx\\7') return t .. '|' .. b"), "hi[31mx|");
    assert_eq!(s(&l, "local t, b = notify.sanitize('a\\u{202E}b', 'c\\u{2066}d') return t .. '|' .. b"), "ab|cd");
    assert_eq!(i(&l, "local t, b = notify.sanitize(string.rep('я', 100), string.rep('x', 1000)) return utf8.len(t) * 10000 + #b"), 64 * 10000 + 256); // i18n-allow-cyrillic
    assert_eq!(s(&l, "local t = notify.sanitize('ok\\255\\1z', '') return t"), "okz");
}

#[test]
fn token_bucket_spam_and_refill() {
    let l = lua();
    assert_eq!(i(&l, "local st, n = {}, 0 for k = 1, 100 do if notify.take_token(st, 'p', 0) then n = n + 1 end end return n"), 3);
    assert_eq!(
        i(&l, "local st, n = {}, 0 for k = 1, 3 do notify.take_token(st, 'p', 0) end
               if notify.take_token(st, 'p', 5) then n = n + 1 end
               if notify.take_token(st, 'p', 5) then n = n + 1 end
               if notify.take_token(st, 'q', 5) then n = n + 10 end return n"),
        11
    );
}

#[test]
fn handlers_toast_attention_dnd_and_cancel() {
    let l = lua();
    l.load("notify.setup({}, {})").exec().unwrap();
    assert_eq!(i(&l, "return #palette_sources"), 1);
    // background tab: toast + attention, the click knows pane_id
    l.load("local w, p = make(true, false) handlers['saysdf-command-finished'](w, p, { duration_ms = 40000, exit_code = 1 })").exec().unwrap();
    assert_eq!(s(&l, "return toasts[1].t"), "✗ build — 40s, exit 1");
    assert_eq!(i(&l, "return toasts[1].pid"), 42);
    assert!(b(&l, "return attention[7] == true"));
    // DND: no toast, attention present
    assert!(b(&l, "return notify.toggle_dnd()"));
    l.load("attention = {} local w, p = make(true, false) handlers['saysdf-command-finished'](w, p, { duration_ms = 40000, exit_code = 0 })").exec().unwrap();
    assert_eq!(i(&l, "return #toasts"), 1);
    assert!(b(&l, "return attention[7] == true"));
    assert!(!b(&l, "return notify.toggle_dnd()"));
    // OSC: always false (upstream does not show it), spam is limited
    assert!(!b(&l, "local w, p = make(false, true) return handlers['saysdf-app-notification'](w, p, nil, 'hello')"));
    assert_eq!(s(&l, "return toasts[2].t"), "hello");
    l.load("local w, p = make(false, true) for k = 1, 100 do handlers['saysdf-app-notification'](w, p, 'T', 'B') end").exec().unwrap();
    assert_eq!(i(&l, "return #toasts"), 4);
    assert!(b(&l, "return #notify.palette_entries() == 1"));
}

#[test]
fn texts_follow_language() {
    let l = lua();
    // Both languages in one chunk: the language is process-global and other tests also select it.
    let both: Vec<String> = l
        .load(
            "local r = {}
             for _, lang in ipairs({ 'en', 'ru' }) do
               require('saysdf.i18n').select(lang)
               local _, body = notify.format_toast({ duration_ms = 1000, exit_code = 0 }, 'x')
               local _, ebody = notify.format_toast({ duration_ms = 1000, exit_code = 2 }, 'x')
               notify.set_dnd(false)
               table.insert(r, body .. '|' .. ebody .. '|' .. notify.palette_entries()[1].brief)
             end
             require('saysdf.i18n').select('en')
             return r",
        )
        .eval()
        .unwrap();
    assert_eq!(both[0], "Command finished|Command failed|Notify: turn on do not disturb");
    // The Russian UI is unchanged by the migration
    assert_eq!(
        both[1],
        "Команда завершилась|Команда завершилась с ошибкой|Notify: включить «не беспокоить»" // i18n-allow-cyrillic
    );
}
