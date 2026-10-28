//! Tests of `saysdf.files` (change file-browser): the action path validator (outside the root, `..`,
//! nonexistent, prefix name), registration of the hotkey/menu/channel actions, and the cwd sync
//! writing the file only on change.
use super::*;

const FAKE: &str = r#"
    written = {}
    hooks = {}
    viewer_opened = {}
    splits = {}
    sbstate = {}
    package.loaded.wezterm = {
        on = function() end,
        target_triple = 'x86_64-unknown-linux-gnu', home_dir = '/home/u',
        log_info = function() end, log_error = function() end, log_warn = function() end,
        action_callback = function(f) return { cb = f } end,
    }
    package.loaded['saysdf.sidebar'] = {
        options = function() return { enabled = true } end,
        state_for = function(tid) return sbstate[tid] end,
        open = function(w, p, spec) last_open = spec sbstate[1] = { pane_id = 50, tool = spec.tool } return true end,
        toggle = function(w, p, tool, reopen) return reopen() and 'opened' end,
    }
    package.loaded['saysdf.tabs'] = { cwd_path = function(c) return c end }
    package.loaded['saysdf.viewer'] = { open = function(w, p, path) table.insert(viewer_opened, path) end }
    package.loaded['saysdf.status'] = { hook = function(n, f) hooks[n] = f end, defer = function(f) f() end }
    util = require('saysdf.util')
    util.find_bin_cached = function() return '/bin/saysdf' end
    util.ensure_dir = function() end
    util.notify = function() end
    util.write_atomic = function(p, t) table.insert(written, t) return true end
    local tab = { tab_id = function() return 1 end }
    function mk_pane(id, cwd)
        return { id = id, cwd = cwd,
            pane_id = function(self) return self.id end,
            tab = function() return tab end,
            get_domain_name = function() return 'local' end,
            get_current_working_dir = function(self) return self.cwd end,
            split = function(self, a) table.insert(splits, a) return mk_pane(99) end }
    end
    main = mk_pane(1, '/proj')
    side = mk_pane(50, nil)
    window = { copy_to_clipboard = function(self, t) copied = t end }
    files = require('saysdf.files')
    cfg = files.setup({}, {})
    fs = { ['/proj'] = true, ['/proj/src'] = true, ['/proj/src/main.rs'] = false, ['/other/x'] = false,
           ['/proj2/f'] = false, ['/next'] = true, ['/next/a.txt'] = false }
    function exists(p) local d = fs[p] if d == nil then return false end return true, d end
    function b64(s)
        local c = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
        return ((s:gsub('.', function(x)
            local r, b = '', x:byte()
            for i = 8, 1, -1 do r = r .. (b % 2 ^ i - b % 2 ^ (i - 1) > 0 and '1' or '0') end
            return r
        end) .. '0000'):gsub('%d%d%d?%d?%d?%d?', function(x)
            if #x < 6 then return '' end
            local n = 0
            for i = 1, 6 do n = n + (x:sub(i, i) == '1' and 2 ^ (6 - i) or 0) end
            return c:sub(n + 1, n + 1)
        end) .. ({ '', '==', '=' })[#s % 3 + 1])
    end
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.files", get("saysdf.files").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.palette", get("saysdf.palette").unwrap()),
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
fn validator_rejects_outside_dotdot_missing() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local roots = { '/proj' }
            local function v(p, want, win, r)
                local ok, why = files.validate(r or roots, p, want, exists, win)
                return ok and 'ok' or why
            end
            return table.concat({
                v('/proj/src/main.rs', 'file'),
                v('/proj/src', 'dir'),
                v('/proj', 'dir'),
                v('/other/x'),
                v('/proj/../other/x'),
                v('/proj/./src', 'dir'),
                v('/proj2/f'),
                v('/proj/nope'),
                v('src/main.rs'),
                v('/proj/src', 'file'),
                v('/proj/src/main.rs', 'dir'),
                v('C:\\Proj\\Src', 'dir', true, { 'c:/proj' }),
                v('/proj/a\nb'),
            }, ',')"#
        ),
        "ok,ok,ok,outside the root,..,..,outside the root,does not exist,not absolute,is a folder,not a folder,does not exist,path"
    );
}

#[test]
fn arg_decoding() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"return table.concat({
                tostring(files.parse_arg('3:' .. b64('/proj/файл.txt'))), -- i18n-allow-cyrillic
                tostring(files.parse_arg('3:' .. b64('/a:b'))),
                tostring(files.parse_arg(b64('/x'))),
                tostring(files.parse_arg('1:@@@@')),
                tostring(files.b64decode('YQ==')) .. tostring(files.b64decode('YWI=')) .. tostring(files.b64decode('YWJj')),
            }, '|')"#
        ),
        "/proj/файл.txt|/a:b|nil|nil|aababc" // i18n-allow-cyrillic
    );
}

#[test]
fn registration() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local k = cfg.keys[1]
            local p = require('saysdf.palette')
            local t = p.by_id(window, main, 'files.toggle')
            local o = p.by_id(window, main, 'files.open')
            local listed = false
            for _, e in ipairs(p.palette_list(window, main)) do
                if e.brief == 'files.open' then listed = true end
            end
            return table.concat({ k.key, k.mods, t.menu, t.key, tostring(o.sidebar), tostring(o.palette),
                tostring(listed), tostring(hooks.files ~= nil),
                tostring(p.by_id(window, main, 'files.cd_pane').sidebar),
                tostring(p.by_id(window, main, 'files.copy_path').accept('1:' .. b64('/proj'))),
                tostring(o.accept('/proj')) }, ',')"#
        ),
        "B,CTRL|SHIFT,File,Ctrl+Shift+B,true,false,false,true,true,true,false"
    );
}

#[test]
fn actions_checked_against_tab_root() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local p = require('saysdf.palette')
            files.toggle(window, main)
            local r = { last_open.args[1], last_open.args[2], last_open.args[4], tostring(last_open.multi) }
            -- sidebar_run checks through paths.exists: stub the filesystem
            package.loaded['saysdf.paths'] = { exists = exists }
            p.by_id(window, main, 'files.open').sidebar_run(window, main, '1:' .. b64('/other/x'))
            p.by_id(window, main, 'files.open').sidebar_run(window, main, '2:' .. b64('/proj/src'))
            p.by_id(window, main, 'files.open').sidebar_run(window, main, '3:' .. b64('/proj/src/main.rs'))
            table.insert(r, #viewer_opened .. ':' .. tostring(viewer_opened[1]))
            p.by_id(window, main, 'files.cd_pane').sidebar_run(window, main, '4:' .. b64('/proj/src'))
            table.insert(r, tostring(splits[1] and splits[1].cwd))
            p.by_id(window, main, 'files.copy_path').sidebar_run(window, main, '5:' .. b64('/proj/src/main.rs'))
            table.insert(r, tostring(copied))
            -- the browser is closed (another tool in the sidebar): rejected
            sbstate[1].tool = 'pick'
            p.by_id(window, main, 'files.open').sidebar_run(window, main, '6:' .. b64('/proj/src/main.rs'))
            table.insert(r, tostring(#viewer_opened))
            return table.concat(r, ',')"#
        ),
        "files,--cwd-file,/proj,true,1:/proj/src/main.rs,/proj/src,/proj/src/main.rs,1"
    );
}

#[test]
fn sync_writes_only_on_change() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"package.loaded['saysdf.paths'] = { exists = exists }
            local r = {}
            hooks.files(window, main) -- browser not open: nothing
            table.insert(r, #written)
            files.toggle(window, main)
            hooks.files(window, main) -- cwd = the opening root: no write
            hooks.files(window, side) -- the sidebar itself: no write
            table.insert(r, #written)
            main.cwd = '/next'
            hooks.files(window, main)
            hooks.files(window, main)
            hooks.files(window, main)
            table.insert(r, #written .. ':' .. written[1])
            -- the new cwd became a root: an action under it is accepted
            require('saysdf.palette').by_id(window, main, 'files.open').sidebar_run(window, main, '1:' .. b64('/next/a.txt'))
            table.insert(r, tostring(viewer_opened[1]))
            main.cwd = '/proj'
            hooks.files(window, main)
            table.insert(r, #written)
            return table.concat(r, ',')"#
        ),
        "0,0,1:/next,/next/a.txt,2"
    );
}

#[test]
fn texts_follow_language() {
    let l = lua();
    let run = |lang: &str| -> String {
        l.load(format!(
            "require('saysdf.i18n').select({lang:?})
             local p = require('saysdf.palette')
             p._reset()
             files._reset()
             files.setup({{}}, {{}})
             package.loaded['saysdf.paths'] = {{ exists = exists }}
             local r = {{}}
             util.find_bin_cached = function() return '/bin/saysdf' end
             util.notify = function(_, title, m) table.insert(r, title .. ': ' .. m) end
             files.toggle(window, main)
             p.by_id(window, main, 'files.open').sidebar_run(window, main, '1:' .. b64('/other/x'))
             table.insert(r, p.by_id(window, main, 'files.toggle').label)
             util.find_bin_cached = function() return nil end
             files.open(window, main)
             return table.concat(r, '|')"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(
        run("en"),
        "Files: action rejected: outside the root|File browser|Files: saysdf not found (next to saysdfTerm or in PATH)"
    );
    // The Russian UI is unchanged by the migration
    assert_eq!(
        run("ru"),
        "Файлы: действие отклонено: вне корня|Файловый браузер|Файлы: saysdf не найден (рядом с saysdfTerm или в PATH)" // i18n-allow-cyrillic
    );
}
