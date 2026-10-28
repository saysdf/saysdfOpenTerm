//! Tests of the `saysdf.ssh` module (change ssh-connections, tasks 3.1–3.3).
use super::*;

const FAKE: &str = r#"
    HOSTS = {
        build = { hostname = 'b.corp', user = 'ci', port = '22' },
        web = { hostname = 'w' },
        db = { hostname = 'd' },
    }
    META = {
        build = { title = 'Build server', description = 'CI\nand artifacts', group = 'work', color = 'red' },
        db = { title = 'Database', group = 'prod' },
    }
    package.loaded.wezterm = {
        on = function() end,
        action_callback = function(f) return f end,
        action = {},
        GLOBAL = {},
        home_dir = '/home/u',
        config_dir = '/home/u/.config/saysdfTerm',
        target_triple = 'x86_64-unknown-linux-gnu',
        enumerate_ssh_hosts = function() return HOSTS end,
        json_parse = function(s) assert(s == 'META') return META end,
        format = function(items)
            local out = {}
            for _, it in ipairs(items) do
                if type(it) == 'table' and it.Text then table.insert(out, it.Text) end
            end
            return table.concat(out)
        end,
    }
    require('saysdf.util').read_file = function() return 'META' end
    ssh = require('saysdf.ssh')
    -- the language of this Lua state only (not the system one): English expectations
    require('saysdf.i18n').select('en')
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.palette", get("saysdf.palette").unwrap()),
            ("saysdf.sidebar", get("saysdf.sidebar").unwrap()),
            ("saysdf.ssh", get("saysdf.ssh").unwrap()),
            ("saysdf.tabs", get("saysdf.tabs").unwrap()),
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

#[test]
fn list_merges_metadata_and_sorts_by_group() {
    let l = lua();
    l.load("ssh.setup({ enabled = true, keys = false, palette = false, hosts = { web = { title = 'Site' } } }, {})")
        .exec()
        .unwrap();
    assert_eq!(
        s(&l, "local o = {} for _, e in ipairs(ssh.list()) do table.insert(o, e.alias .. '=' .. e.title .. '/' .. e.group) end return table.concat(o, '|')"),
        "db=Database/prod|build=Build server/work|web=Site/"
    );
    // the description is sanitized, the ssh config fields are in place
    assert_eq!(s(&l, "local e = ssh.list()[2] return e.description .. '|' .. e.hostname .. '|' .. e.user"), "CI and artifacts|b.corp|ci");
    // cache in GLOBAL: changes to HOSTS are not visible until invalidate
    assert_eq!(s(&l, "HOSTS.extra = {} local n = #ssh.list() ssh.invalidate() return n .. '/' .. #ssh.list()"), "3/4");
}

#[test]
fn choices_label_columns_and_add_item() {
    let l = lua();
    l.load("ssh.setup({ enabled = true, keys = false, palette = false, desc_width = 8 }, {})").exec().unwrap();
    let rows = s(&l, "local o = {} for _, c in ipairs(ssh.choices(ssh.list())) do table.insert(o, c.id .. '=' .. c.label) end return table.concat(o, '\\n')");
    let lines: Vec<&str> = rows.lines().collect();
    assert_eq!(lines[0], "db=● [prod] Database (db)");
    assert_eq!(lines[1], "build=● [work] Build server (build)  CI and …");
    assert_eq!(lines[2], "web=● web");
    assert_eq!(lines[3], "__saysdf_ssh_add__=+ Add connection…");
    assert_eq!(
        s(&l, "return ssh.row_text({ alias = 'a', title = 'T', group = '', description = 'd' }, 8, 60)"),
        "T (a)     d"
    );
    assert_eq!(s(&l, "return ssh.choices({})[1].label"), "+ Add connection… (no hosts in ~/.ssh/config)");
}

#[test]
fn build_action_backends() {
    let l = lua();
    assert_eq!(
        s(&l, "local a = ssh.build_action({ alias = 'build' }, { hold_on_error = false }, '/usr/bin/ssh') return a.kind .. '|' .. table.concat(a.args, ' ')"),
        "tab|/usr/bin/ssh build"
    );
    assert_eq!(s(&l, "return ssh.build_action({ alias = 'b' }, { split = true }, 'ssh').kind"), "split");
    assert_eq!(s(&l, "return ssh.build_action({ alias = 'b' }, { backend = 'domain' }).domain"), "SSH:b");
    assert_eq!(
        s(&l, "return table.concat(ssh.add_args('saysdf', '/r/x.json', '/c/m.json', '/h/.ssh'), ' ')"),
        "saysdf ssh add --existing-file /r/x.json --meta-file /c/m.json --ssh-dir /h/.ssh"
    );
}

#[test]
fn setup_enabled_by_default_and_hotkey() {
    let l = lua();
    assert_eq!(s(&l, "local c = ssh.setup({ palette = false }, {}) return c.keys[1].key"), "H");
    assert_eq!(s(&l, "local c = ssh.setup({ enabled = false }, {}) return tostring(c.keys)"), "nil");
    assert_eq!(
        s(&l, "local c = ssh.setup({ enabled = true, palette = false }, {}) return c.keys[1].key .. ' ' .. c.keys[1].mods"),
        "H CTRL|SHIFT"
    );
}

// An ssh error must not close the tab silently (the upstream exit_behavior defaults to "Close").
#[test]
fn ssh_args_hold_tab_on_error() {
    let l = lua();
    let unix = s(&l, "return table.concat(ssh.build_action({ alias = 'build' }, {}, '/usr/bin/ssh', false).args, '|')");
    assert!(unix.starts_with("/bin/sh|-c|"), "{unix}");
    assert!(unix.ends_with("|/usr/bin/ssh|build"), "{unix}");
    assert_eq!(
        s(&l, r"return table.concat(ssh.ssh_args([[C:\Windows\System32\OpenSSH\ssh.exe]], 'prodhost', true, true), ' ')"),
        r"cmd.exe /d /c C:\Windows\System32\OpenSSH\ssh.exe prodhost || pause"
    );
    // alias/path unsafe for cmd.exe: no wrapper
    assert_eq!(s(&l, "return table.concat(ssh.ssh_args('ssh', 'a&calc', true, true), ' ')"), "ssh a&calc");
    assert_eq!(s(&l, r"return table.concat(ssh.ssh_args([[C:\Program Files\ssh.exe]], 'h', true, true), '|')"), r"C:\Program Files\ssh.exe|h");
    assert_eq!(s(&l, "return table.concat(ssh.ssh_args('ssh', '-oProxyCommand=x', true, true), ' ')"), "ssh -oProxyCommand=x");
}

// A choice in InputSelector (click or arrows+Enter: upstream passes the same id) -> the right host.
#[test]
fn resolve_choice_maps_ids_to_hosts_and_add() {
    let l = lua();
    l.load("ssh.setup({ enabled = true, keys = false, palette = false }, {})").exec().unwrap();
    // each choices item id maps to the list entry at the same index; the last one is "add"
    assert_eq!(
        s(&l, r#"
            local list = ssh.list(true)
            local ch = ssh.choices(list)
            assert(#ch == #list + 1, 'add item')
            local out = {}
            for i, c in ipairs(ch) do
              local r = ssh.resolve_choice(c.id, list)
              if r.kind == 'add' then
                assert(i == #ch)
                table.insert(out, 'ADD')
              else
                assert(r.entry == list[i], 'entry ' .. i)
                table.insert(out, r.entry.alias)
              end
            end
            return table.concat(out, ',')
        "#),
        "db,build,web,ADD"
    );
    assert_eq!(s(&l, "return tostring(ssh.resolve_choice(nil))"), "nil");
    assert_eq!(s(&l, "return ssh.resolve_choice('db').entry.title"), "Database");
    // the host vanished from the list after the choice: open by alias, do not "add"
    assert_eq!(s(&l, "local r = ssh.resolve_choice('gone') return r.kind .. ':' .. r.entry.alias"), "host:gone");
}

// The pick callback opens a tab with ssh to the chosen host; "add" opens the form.
#[test]
fn pick_callback_spawns_selected_host() {
    let l = lua();
    l.load(r#"
        local w = package.loaded.wezterm
        w.action.InputSelector = function(t) return { selector = t } end
        w.action.SpawnCommandInNewTab = function(t) return { spawn = t } end
        ssh.setup({ enabled = true, keys = false, palette = false, ssh_bin = 'ssh', hold_on_error = false }, {})
        SPAWNED, ADDED = nil, 0
        ssh.add = function() ADDED = ADDED + 1 end
        local sel
        WIN = {
          perform_action = function(_, a) if a.selector then sel = a.selector elseif a.spawn then SPAWNED = table.concat(a.spawn.args, ' ') end end,
          active_tab = function() return nil end,
        }
        ssh.pick(WIN, {})
        SEL = sel
    "#).exec().unwrap();
    assert_eq!(s(&l, "SEL.action(WIN, {}, SEL.choices[2].id) return SPAWNED"), "ssh build");
    assert_eq!(s(&l, "SEL.action(WIN, {}, SEL.choices[#SEL.choices].id) return tostring(ADDED)"), "1");
    assert_eq!(s(&l, "SPAWNED = nil SEL.action(WIN, {}, nil) return tostring(SPAWNED) .. tostring(ADDED)"), "nil1");
}

#[test]
fn texts_follow_language() {
    let l = lua();
    let run = |lang: &str| -> String {
        l.load(format!(
            "require('saysdf.i18n').select({lang:?})
             ssh.setup({{ enabled = true, keys = false, palette = false }}, {{}})
             local e = ssh.palette_entries()[1]
             return e.label .. '|' .. ssh.choices({{}})[1].label .. '|' .. ssh.ssh_args('ssh', 'prodhost', true, false)[3]:match('%[ssh:[^%]]*%]')"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(
        run("en"),
        "SSH: connect…|+ Add connection… (no hosts in ~/.ssh/config)|[ssh: exit code %s — press Enter to close the tab]"
    );
    // The Russian UI is unchanged by the migration
    assert_eq!(
        run("ru"),
        "SSH: подключиться…|+ Добавить подключение… (в ~/.ssh/config нет хостов)|[ssh: код %s — Enter закроет вкладку]" // i18n-allow-cyrillic
    );
}
