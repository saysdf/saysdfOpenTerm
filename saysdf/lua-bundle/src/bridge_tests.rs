//! Tests of `saysdf.bridge` (change text-annotations, D7): the trusted user-var channel --
//! parsing, token, size limit, foreground check, ownership, allow-list, one-time and multi records.
use super::*;

const FAKE: &str = r#"
    logs = {}
    package.loaded.wezterm = {
        GLOBAL = {},
        target_triple = 'x86_64-unknown-linux-gnu', home_dir = '/home/u',
        log_info = function(s) table.insert(logs, s) end,
        log_error = function(s) table.insert(logs, 'E:' .. s) end,
        log_warn = function() end,
    }
    function mk_pane(id, fg)
        return { id = id, fg = fg,
            pane_id = function(self) return self.id end,
            get_foreground_process_name_cached = function(self) return self.fg end }
    end
    br = require('saysdf.bridge')
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.bridge", get("saysdf.bridge").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
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
fn token_and_parse() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local a, b = br.new_token(), br.new_token()
            local r = { #a, tostring(a:match('^%x+$') ~= nil), tostring(a ~= b) }
            local function p(v, max) local x, y, z = br.parse(v, max) table.insert(r, tostring(x) .. '/' .. tostring(y) .. '/' .. tostring(z)) end
            p('t:k:a:b')                 -- payload keeps its colons
            p('t:k:')                    -- empty payload
            p('t-k')                     -- format
            p(42)                        -- not a string
            p(string.rep('a', 4097))     -- default limit
            p('t:k:abc', 6)              -- custom limit
            return table.concat(r, '|')"#
        ),
        "32|true|true|t/k/a:b|t/k/|nil/format/nil|nil/length/nil|nil/length/nil|nil/length/nil"
    );
}

#[test]
fn foreground_and_check() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local r = {}
            local p = mk_pane(1, 'C:\\bin\\saysdf.exe')
            table.insert(r, tostring(br.foreground_is(p)))
            p.fg = nil table.insert(r, tostring(br.foreground_is(p)))
            p.fg = '/usr/bin/vim' table.insert(r, tostring(br.foreground_is(p)))
            table.insert(r, tostring(br.foreground_is(p, 'vim')))
            local bad = { get_foreground_process_name_cached = function() error('dead') end }
            table.insert(r, tostring(br.foreground_is(bad)))
            local rec = { token = 'T' }
            local function c(v) local x, y = br.check(p, rec, v) table.insert(r, tostring(x) .. '/' .. tostring(y)) end
            p.fg = '/bin/saysdf'
            c('T:pick:a')
            c('X:pick:a')                -- wrong token
            p.fg = nil c('T:pick:a')     -- foreground unknown
            p.fg = 'vim' c('T:pick:a')   -- other foreground
            p.fg = 'saysdf'
            br.spend(rec) c('T:pick:a')  -- spent
            local x, y = br.check(p, {}, ':k:v')   -- record without a token never matches
            table.insert(r, tostring(x) .. '/' .. tostring(y))
            return table.concat(r, '|')"#
        ),
        "true|nil|false|true|nil|T/pick|nil/token|nil/foreground not known yet|nil/foreground is not saysdf|nil/token|nil/token"
    );
}

#[test]
fn consume_once_and_multi() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local r = {}
            local once = { token = 'T' }
            table.insert(r, tostring(br.consume(once, 'v')) .. ',' .. tostring(once.used))
            local multi = { token = 'T', multi = true }
            local function m(v) local ok, why = br.consume(multi, v) table.insert(r, tostring(ok) .. ',' .. tostring(why)) end
            m('a') m('a') m('b') m('a')
            table.insert(r, tostring(multi.used))
            return table.concat(r, '|')"#
        ),
        "true,true|true,nil|nil,repeat|true,nil|true,nil|nil"
    );
}

#[test]
fn channel_checks_and_dispatch() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local owned = mk_pane(1, 'saysdf')
            local other = mk_pane(2, 'saysdf')
            local rec = { token = 'T', multi = true }
            local ran = {}
            local ch = br.channel({
                var = 'SAYSDF_X', name = 'test.x',
                lookup = function(p) if p:pane_id() == 1 then return rec end end,
                not_owned = 'not mine',
                kinds = {
                    go = function(w, p, r, payload, value)
                        if payload == 'bad' then return nil, 'payload' end
                        local ok, why = br.consume(r, value)
                        if not ok then return nil, why end
                        table.insert(ran, payload) return true
                    end,
                    boom = function() error('broken') end,
                },
            })
            local r = {}
            local function h(p, n, v) table.insert(r, tostring(ch.handle(nil, p, n, v))) end
            h(owned, 'OTHER', 'T:go:1')     -- foreign variable
            h(other, 'SAYSDF_X', 'T:go:1')  -- not owned
            h(owned, 'SAYSDF_X', 'X:go:1')  -- token
            h(owned, 'SAYSDF_X', 'T:nope:1') -- kind not in the allow-list
            h(owned, 'SAYSDF_X', 'T:go:bad') -- handler rejects the payload
            h(owned, 'SAYSDF_X', 'T:boom:1') -- handler error is a refusal
            h(owned, 'SAYSDF_X', 'T:go:1')  -- valid
            h(owned, 'SAYSDF_X', 'T:go:1')  -- repeat
            h(owned, 'SAYSDF_X', 'T:go:2')  -- valid (multi)
            h(owned, 'SAYSDF_X', 'T:nope:2') -- same reason again: logged once
            table.insert(r, table.concat(ran, ','))
            local norm = {}
            for _, x in ipairs(logs) do table.insert(norm, (x:gsub('^E:(test%.x: ).*broken$', 'E:%1broken'))) end
            table.insert(r, table.concat(norm, ';'))
            return table.concat(r, '|')"#
        ),
        "false|false|false|false|false|false|true|false|true|false|1,2|\
test.x: result rejected: not mine;test.x: result rejected: token;\
test.x: result rejected: kind;test.x: result rejected: payload;E:test.x: broken;\
test.x: result rejected: repeat"
    );
}
