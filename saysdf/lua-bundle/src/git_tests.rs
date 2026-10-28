//! Tests of the module `saysdf.git` (change git-integration, tasks 2.2, 2.3, 2.6, 3.3).
use super::*;

const FAKE: &str = r#"
    spawned = 0
    package.loaded.wezterm = {
        log_warn = function() end,
        log_info = function() end,
        run_child_process = function() spawned = spawned + 1; return true, '{}', '' end,
        json_parse = function() return {} end,
        target_triple = 'x86_64-unknown-linux-gnu',
        home_dir = '/home/u',
    }
    git = require('saysdf.git')
    require('saysdf.i18n').select('en')
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.git", get("saysdf.git").unwrap()),
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
    l.load(code).eval().unwrap()
}

fn b(l: &Lua, code: &str) -> bool {
    l.load(code).eval().unwrap()
}

#[test]
fn find_root_dir_and_gitfile() {
    let l = lua();
    l.load(
        r#"exists = function(p)
            return p == '/r/repo/.git' or p == [[C:\w\wt/.git]]
        end"#,
    )
    .exec()
    .unwrap();
    assert_eq!(s(&l, "return git.find_root('/r/repo/src/a', exists)"), "/r/repo");
    assert_eq!(s(&l, "return git.find_root('/r/repo', exists)"), "/r/repo");
    assert_eq!(s(&l, r"return git.find_root([[C:\w\wt\sub]], exists)"), r"C:\w\wt");
    assert!(b(&l, "return git.find_root('/other/x', exists) == nil"));
}

#[test]
fn cache_policy() {
    let l = lua();
    assert!(b(&l, "return git.should_refresh(nil, 0, 3000)"));
    // inflight: a second request does not start, even if stale
    assert!(!b(&l, "return git.should_refresh({ inflight = true, stale = true }, 99999, 3000)"));
    assert!(b(&l, "return git.should_refresh({ fetched_at = 1000, stale = true }, 1001, 3000)"));
    assert!(!b(&l, "return git.should_refresh({ fetched_at = 1000 }, 3999, 3000)"));
    assert!(b(&l, "return git.should_refresh({ fetched_at = 1000 }, 4000, 3000)"));
    // the entry's own (adaptive) ttl takes priority
    assert!(!b(&l, "return git.should_refresh({ fetched_at = 0, ttl = 9000 }, 5000, 3000)"));
    assert!(b(&l, "return git.next_ttl(3000, 200) == 3000"));
    assert!(b(&l, "return git.next_ttl(3000, 1500) == 9000"));
    assert!(b(&l, "return git.next_ttl(20000, 5000) == 30000"));
    assert!(b(&l, "return git.next_ttl(100, 0) == 1000"));
}

#[test]
fn format_and_info_without_cache() {
    let l = lua();
    assert_eq!(
        s(&l, "return git.format_status({ root = '/r', branch = 'main', staged = 1, modified = 2, untracked = 0, conflicts = 0, ahead = 1, behind = 0, submodules_dirty = 1 })"),
        "main +1 ~2 ↑1 sub:1"
    );
    assert_eq!(
        s(&l, "return git.format_status({ root = '/r', detached = true, oid = '0123456789ab', label = 'REBASE' })"),
        "@0123456 REBASE"
    );
    // info without a cache: nil and no processes
    l.load(
        r#"pane = { get_domain_name = function() return 'local' end,
                     get_current_working_dir = function() return '/r/repo' end,
                     pane_id = function() return 1 end }"#,
    )
    .exec()
    .unwrap();
    assert!(b(&l, "return git.info(pane) == nil and spawned == 0"));
}

#[test]
fn push_plan_args_and_text() {
    let l = lua();
    assert_eq!(
        s(&l, "local a, t = git.push_plan({ branch = 'main', upstream = 'origin/main', ahead = 2, behind = 0 }); return table.concat(a, ' ') .. '|' .. t"),
        "push|Push main → origin/main?\nCommits to push: 2"
    );
    assert_eq!(
        s(&l, "local a, t = git.push_plan({ branch = 'feat' }, 'up', { { path = 'vendor/x', sha = 'abcdef0123' } }); return table.concat(a, ' ') .. '|' .. t"),
        "push -u up feat|Push feat → up/feat (new upstream)?\nWarning: submodule vendor/x — commit abcdef0 is not pushed"
    );
    assert!(b(&l, "return git.push_plan({ detached = true, oid = 'x' }) == nil"));
}

// "Open lazygit/gitui" without the tools: a toast with an install hint, nothing is started;
// the PATH lookup is cached (rule 5 -- no PATH walk on every call in the GUI thread).
#[test]
fn ui_missing_toasts_install_hint_and_caches_lookup() {
    let l = lua();
    l.load(r#"
        local util = require('saysdf.util')
        util._forget_bins()
        CALLS = 0
        util.find_bin = function() CALLS = CALLS + 1 return nil end
        git.root_of = function() return '/r' end
        TOASTS, SPLITS = {}, 0
        WIN = { toast_notification = function(_, _, msg) table.insert(TOASTS, msg) end }
        PANE = { split = function() SPLITS = SPLITS + 1 end }
        git.act(WIN, PANE, 'ui')
        git.act(WIN, PANE, 'ui')
    "#).exec().unwrap();
    assert_eq!(s(&l, "return TOASTS[1]"), "lazygit is not installed; install: winget install JesseDuffield.lazygit (or scoop install lazygit)");
    assert_eq!(s(&l, "return #TOASTS .. ' ' .. SPLITS .. ' ' .. CALLS"), "2 0 2");
    // found: a split opens, the path is cached
    l.load(r#"
        local util = require('saysdf.util')
        util._forget_bins()
        util.find_bin = function(n) CALLS = CALLS + 1 if n == 'gitui' then return '/bin/gitui' end end
        CALLS = 0
        git.act(WIN, PANE, 'ui')
        git.act(WIN, PANE, 'ui')
    "#).exec().unwrap();
    assert_eq!(s(&l, "return SPLITS .. ' ' .. CALLS"), "2 2");
}

/// `run_child_process` does not pass SAYSDF_LANG, so `saysdf` gets the language as `--lang`
/// (gap of F-1 task 4).
#[test]
fn saysdf_calls_pass_language() {
    let l = lua();
    let argv = |lang: &str| -> String {
        l.load(format!(
            "require('saysdf.i18n').select({lang:?})
             local w = require('wezterm')
             local seen
             w.run_child_process = function(a) seen = a; return true, '{{}}', '' end
             git.find_bin = function() return '/bin/saysdf' end
             git.refresh('/r/{lang}')
             return table.concat(seen, ' ')"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(argv("ru"), "/bin/saysdf --lang ru git-status --json /r/ru");
    assert_eq!(argv("en"), "/bin/saysdf --lang en git-status --json /r/en");
    assert_eq!(
        s(&l, "return table.concat(git.saysdf_argv('s', 'git-status', '--submodules-push-check', '/r'), ' ')"),
        "s --lang en git-status --submodules-push-check /r"
    );
}

#[test]
fn texts_follow_language() {
    let l = lua();
    let run = |lang: &str| -> String {
        l.load(format!(
            "require('saysdf.i18n').select({lang:?})
             local _, text = git.push_plan({{ branch = 'main', upstream = 'origin/main', ahead = 2, behind = 0 }})
             local label = require('saysdf.i18n').domain('git')(git.ITEMS[6].label)
             return text .. '|' .. git.ui_missing() .. '|' .. label"
        ))
        .eval()
        .unwrap()
    };
    assert_eq!(
        run("en"),
        "Push main → origin/main?\nCommits to push: 2|lazygit is not installed; install: winget install JesseDuffield.lazygit (or scoop install lazygit)|Push… (with confirmation)"
    );
    // The Russian UI is unchanged by the migration
    let ru = run("ru");
    run("en");
    assert_eq!(
        ru,
        "Push main → origin/main?\nКоммитов к отправке: 2|lazygit не установлен; установить: winget install JesseDuffield.lazygit (или scoop install lazygit)|Push… (с подтверждением)" // i18n-allow-cyrillic
    );
}
