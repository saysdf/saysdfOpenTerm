//! Tests of `saysdf.monitor`: options and the status segment from the JSON status file.
use super::*;

const FAKE: &str = r#"
    handlers = {}
    logs = {}
    package.loaded.wezterm = {
        on = function(name, f) handlers[name] = f end,
        log_error = function(msg) table.insert(logs, msg) end,
        log_warn = function(msg) table.insert(logs, msg) end,
        action_callback = function(f) return f end,
        -- minimal JSON parser for tests: an object with numbers/null, otherwise an error
        json_parse = function(s)
            if not s:match("^%s*{.*}%s*$") then error("bad json") end
            local t = {}
            for k, v in s:gmatch('"([%w_]+)"%s*:%s*([%-%d%.]+)') do t[k] = tonumber(v) end
            return t
        end,
    }
    mon = require('saysdf.monitor')
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.monitor", get("saysdf.monitor").unwrap()),
            ("saysdf.palette", get("saysdf.palette").unwrap()),
            ("saysdf.status", get("saysdf.status").unwrap()),
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

const G: u64 = 1024 * 1024 * 1024;

#[test]
fn opts_defaults_and_clamp() {
    let l = lua();
    assert_eq!(
        s(&l, r#"local o = mon.parse_opts(nil) return o.interval .. o.key.key .. o.key.mods .. o.split_key.key .. tostring(o.keys) .. tostring(o.enabled)"#),
        "2ICTRL|SHIFTJtruefalse"
    );
    assert_eq!(s(&l, r#"return tostring(mon.parse_opts({ interval = 0.2, keys = false }).interval)"#), "1");
    assert_eq!(s(&l, r#"return tostring(mon.parse_opts({ keys = false }).keys)"#), "false");
}

#[test]
fn fresh_sample() {
    let l = lua();
    let code = format!(
        r#"return (mon.format_status({{ ts = 100, cpu = 12.3, mem_used = {}, mem_total = {}, net_rx = 1258291, net_tx = 40960, disk_read = 3145728, disk_write = 0 }}, 102, 2))"#,
        G * 71 / 10,
        16 * G
    );
    assert_eq!(s(&l, &code), "CPU 12% RAM 7.1/16G ↓1.2M ↑40K D 3.0M/s");
}

#[test]
fn stale_and_broken() {
    let l = lua();
    assert_eq!(s(&l, r#"return (mon.format_status({ ts = 100, cpu = 1 }, 107, 2))"#), "mon: n/a");
    assert_eq!(s(&l, r#"return (mon.format_status(nil, 107, 2))"#), "mon: n/a");
    assert_eq!(s(&l, r#"return (mon.format_status({ cpu = "x" }, 107, 2))"#), "mon: n/a");
    assert_eq!(s(&l, r#"return (mon.format_status({ ts = 100, cpu = 5, gpu = 40 }, 100, 2))"#), "CPU 5% RAM 0.0/0G ↓0B ↑0B D 0B/s GPU 40%");
}

#[test]
fn segment_reads_status_file() {
    let l = lua();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("mon.json");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    std::fs::write(&p, format!(r#"{{"v":1,"ts":{now},"cpu":50.0,"mem_used":0,"mem_total":0,"net_rx":0,"net_tx":0,"disk_read":0,"disk_write":0,"gpu":null}}"#)).unwrap();
    l.globals().set("PATH", p.to_string_lossy().to_string()).unwrap();
    l.load(r#"mon.setup({ keys = false, palette = false }, {})
        local st = mon._state(); st.enabled = true; st.path = PATH"#)
        .exec()
        .unwrap();
    assert_eq!(s(&l, "return mon.segment()"), "CPU 50% RAM 0.0/0G ↓0B ↑0B D 0B/s");
    std::fs::write(&p, "{broken").unwrap();
    assert_eq!(s(&l, "return mon.segment()"), "mon: n/a");
    assert_eq!(s(&l, "return tostring(#logs > 0)"), "true");
    // disabled: the segment is hidden
    assert_eq!(s(&l, "mon._state().enabled = false return tostring(mon.segment())"), "nil");
}

#[test]
fn setup_adds_keys_and_palette() {
    let l = lua();
    assert_eq!(
        s(&l, r#"local c = mon.setup({}, {}) local p = require('saysdf.palette').entries(nil, nil)
            return #c.keys .. ":" .. c.keys[1].key .. c.keys[2].key .. ":" .. #p"#),
        "2:IJ:2"
    );
}
