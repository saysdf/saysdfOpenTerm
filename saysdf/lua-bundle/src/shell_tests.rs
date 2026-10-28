//! Tests of the `saysdf.shell` module (change shell-integration, tasks 5.1–5.4).
use super::*;

const FAKE: &str = r#"
    handlers = {}
    emitted = {}
    warnings = {}
    files = {}
    env = {}
    local act = setmetatable({}, { __index = function(_, k)
        return function(arg) return { action = k, arg = arg } end
    end })
    package.loaded.wezterm = {
        target_triple = TRIPLE,
        executable_dir = EXE_DIR,
        action = act,
        action_callback = function(f) return { callback = f } end,
        on = function(name, f) handlers[name] = f end,
        emit = function(name, ...) table.insert(emitted, { name = name, args = { ... } }) end,
        log_warn = function(msg) table.insert(warnings, msg) end,
    }
    shell = require('saysdf.shell')
    shell._getenv = function(k) return env[k] end
    shell._exists = function(p) return files[p] == true end
"#;

fn lua(triple: &str, exe_dir: &str) -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf", get("saysdf").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.palette", get("saysdf.palette").unwrap()),
            ("saysdf.shell", get("saysdf.shell").unwrap()),
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
    lua.globals().set("TRIPLE", triple).unwrap();
    lua.globals().set("EXE_DIR", exe_dir).unwrap();
    lua.load(FAKE).exec().unwrap();
    lua
}

const WIN: &str = "x86_64-pc-windows-msvc";
const LINUX: &str = "x86_64-unknown-linux-gnu";
const WIN_EXE: &str = r"C:\Program Files\it's";
const WIN_DIR: &str = r"C:\Program Files\it's\shell-integration";

fn win() -> Lua {
    let l = lua(WIN, WIN_EXE);
    l.globals()
        .get::<_, Table>("files")
        .unwrap()
        .set(format!(r"{WIN_DIR}\saysdfterm.ps1"), true)
        .unwrap();
    l
}

#[test]
fn scripts_dir_next_to_exe_and_env_override() {
    let l = win();
    let dir: String = l.load("return shell.scripts_dir()").eval().unwrap();
    assert_eq!(dir, WIN_DIR);

    let l = lua(LINUX, "/opt/st/bin");
    let none: Option<String> = l.load("return shell.scripts_dir()").eval().unwrap();
    assert_eq!(none, None);
    let dir: String = l
        .load(
            "files['/opt/st/bin/../share/saysdfterm/shell-integration/saysdfterm.ps1'] = true \
             files['/dev/si/saysdfterm.ps1'] = true \
             local a = shell.scripts_dir() \
             env.SAYSDFTERM_SHELL_INTEGRATION_DIR = '/dev/si' \
             return a .. '|' .. shell.scripts_dir()",
        )
        .eval()
        .unwrap();
    assert_eq!(dir, "/opt/st/bin/../share/saysdfterm/shell-integration|/dev/si");
}

#[test]
fn powershell_command_escapes_path() {
    let l = win();
    let argv: Vec<String> = l.load("return shell.command('pwsh.exe')").eval().unwrap();
    assert_eq!(
        argv,
        vec![
            "pwsh.exe",
            "-NoLogo",
            "-NoExit",
            "-Command",
            r". ([scriptblock]::Create([IO.File]::ReadAllText('C:\Program Files\it''s\shell-integration\saysdfterm.ps1')))",
        ]
    );
}

#[test]
fn unix_commands_escape_path() {
    let l = lua(LINUX, "/x");
    let fish: Vec<String> = l
        .load(r"return shell.command('/usr/bin/fish', [[/o p/it's\x]])")
        .eval()
        .unwrap();
    assert_eq!(
        fish,
        vec![
            "/usr/bin/fish",
            "-l",
            "--init-command",
            r"source '/o p/it\'s\\x/saysdfterm.fish'",
        ]
    );
    let bash: Vec<String> = l
        .load("return shell.command('bash', '/o p')")
        .eval()
        .unwrap();
    assert_eq!(bash, vec!["bash", "--rcfile", "/o p/bash/saysdfterm.bashrc", "-i"]);
    let zsh: Vec<String> = l
        .load("env.ZDOTDIR = '/h/z' return shell.command('/bin/zsh', '/o p')")
        .eval()
        .unwrap();
    assert_eq!(
        zsh,
        vec![
            "/usr/bin/env",
            "SAYSDFTERM_ORIG_ZDOTDIR=/h/z",
            "ZDOTDIR=/o p/zsh",
            "/bin/zsh",
            "-l"
        ]
    );
    let unknown: Option<Vec<String>> = l
        .load("return shell.command('/bin/tcsh', '/o p')")
        .eval()
        .unwrap();
    assert_eq!(unknown, None);
}

#[test]
fn default_prog_rules() {
    // pwsh in PATH -> pwsh with integration, bindings and env
    let l = win();
    let (prog, env, keys, mouse): (String, String, usize, String) = l
        .load(
            r"env.PATH = [[C:\pw;C:\Windows]]
              files[ [[C:\pw\pwsh.exe]] ] = true
              local c = require('saysdf').setup({}, {})
              return c.default_prog[1], c.set_environment_variables.SAYSDFTERM_SHELL_INTEGRATION,
                #c.keys, c.mouse_bindings[1].action.arg",
        )
        .eval()
        .unwrap();
    assert_eq!(prog, "pwsh.exe");
    assert_eq!(env, "1");
    assert_eq!(keys, 4);
    assert_eq!(mouse, "SemanticZone");

    // no pwsh -> powershell.exe
    let l = win();
    let prog: String = l
        .load("local c = shell.setup({}, {}) return c.default_prog[1]")
        .eval()
        .unwrap();
    assert_eq!(prog, "powershell.exe");

    // the user own default_prog is left alone
    let l = win();
    let prog: Vec<String> = l
        .load("local c = { default_prog = { 'cmd.exe' } } shell.setup({}, c) return c.default_prog")
        .eval()
        .unwrap();
    assert_eq!(prog, vec!["cmd.exe"]);

    // unknown shell -> no integration, default_prog not set
    let l = lua(LINUX, "/opt/st");
    let none: bool = l
        .load(
            "files['/opt/st/shell-integration/saysdfterm.ps1'] = true env.SHELL = '/bin/tcsh' \
             local c = {} shell.setup({}, c) return c.default_prog == nil",
        )
        .eval()
        .unwrap();
    assert!(none);

    // no scripts dir -> warning, default_prog not set
    let l = lua(LINUX, "/opt/st");
    let (none, warns): (bool, usize) = l
        .load(
            "env.SHELL = '/bin/bash' local c = {} shell.setup({}, c) \
             return c.default_prog == nil, #warnings",
        )
        .eval()
        .unwrap();
    assert!(none);
    assert_eq!(warns, 1);
}

#[test]
fn disabled_module_adds_nothing() {
    let l = win();
    let (empty, handler): (bool, bool) = l
        .load(
            "local c = require('saysdf').setup({ shell = false }, {}) \
             local env = c.set_environment_variables              local only_lang = next(c) == 'set_environment_variables'                and next(c, 'set_environment_variables') == nil                and next(env) == 'SAYSDF_LANG' and next(env, 'SAYSDF_LANG') == nil              return only_lang, handlers['user-var-changed'] ~= nil",
        )
        .eval()
        .unwrap();
    // setup() itself only passes SAYSDF_LANG to the child tools; the module adds nothing
    assert!(empty);
    assert!(!handler);

    let l = win();
    let (keys, mouse): (bool, bool) = l
        .load("local c = shell.setup({ keys = false }, {}) return c.keys == nil, c.mouse_bindings == nil")
        .eval()
        .unwrap();
    assert!(keys && mouse);
}

#[test]
fn user_var_handler_records_last_command() {
    let l = win();
    let (code, dur, ev, ev_code, none, dur2): (i64, i64, String, i64, bool, i64) = l
        .load(
            r#"shell.setup({}, {})
              local vars = { SAYSDF_LAST_EXIT = '3', SAYSDF_LAST_DURATION_MS = '120' }
              local pane = { pane_id = function() return 7 end, get_user_vars = function() return vars end }
              local other = { pane_id = function() return 8 end, get_user_vars = function() return {} end }
              local h = handlers['user-var-changed']
              h('win', pane, 'WEZTERM_PROG', 'ls')
              h('win', pane, 'SAYSDF_LAST_EXIT', '3')
              local i = shell.last_command(pane)
              local d1 = i.duration_ms
              local e = emitted[1]
              h('win', pane, 'SAYSDF_LAST_DURATION_MS', '150')
              h('win', other, 'SAYSDF_LAST_EXIT', 'junk')
              return i.exit_code, d1, e.name, e.args[3].exit_code,
                shell.last_command(other) == nil and #emitted == 1, shell.last_command(7).duration_ms"#,
        )
        .eval()
        .unwrap();
    assert_eq!(code, 3);
    assert_eq!(dur, 120);
    assert_eq!(ev, "saysdf-command-finished");
    assert_eq!(ev_code, 3);
    assert!(none);
    assert_eq!(dur2, 150);
}

#[test]
fn texts_follow_language() {
    let l = lua(WIN, r"C:\app");
    let run = |lang: &str| -> String {
        l.load(format!(
            "require('saysdf.i18n').select({lang:?})
             require('saysdf.palette')._reset()
             shell.setup({{}}, {{}})
             local msg
             package.loaded['saysdf.util'].notify = function(_, _, m) msg = m end
             shell.copy_last_output({{ copy_to_clipboard = function() end }}, {{ get_semantic_zones = function() return {{}} end }})
             return require('saysdf.palette').by_id(nil, nil, 'shell.copy_output').label .. '|' .. msg"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(run("en"), "Copy command output|No output from the last command");
    // The Russian UI is unchanged by the migration
    assert_eq!(run("ru"), "Скопировать вывод команды|Нет вывода последней команды"); // i18n-allow-cyrillic
}
