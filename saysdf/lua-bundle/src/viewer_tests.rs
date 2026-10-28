//! Tests of module `saysdf.viewer` (change code-viewer, tasks 3.1–3.2) and `util.find_bin`.
use super::*;

const FAKE: &str = r#"
    warnings = {}
    package.loaded.wezterm = {
        log_warn = function(msg) table.insert(warnings, msg) end,
        on = function() end,
        config_builder = function() return {} end,
        action_callback = function(f) return f end,
        action = { PromptInputLine = function(t) return t end },
        home_dir = '/home/u',
        target_triple = 'x86_64-unknown-linux-gnu',
        executable_dir = '/opt/saysdfterm',
    }
    viewer = require('saysdf.viewer')
    util = require('saysdf.util')
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf", get("saysdf").unwrap()),
            ("saysdf.i18n", get("saysdf.i18n").unwrap()),
            ("saysdf.palette", get("saysdf.palette").unwrap()),
            ("saysdf.paths", get("saysdf.paths").unwrap()),
            ("saysdf.util", get("saysdf.util").unwrap()),
            ("saysdf.viewer", get("saysdf.viewer").unwrap()),
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
fn build_args_with_position_and_editor() {
    let l = lua();
    assert_eq!(
        s(&l, "return table.concat(viewer.build_args('/b/saysdf', 'view', '/a b.rs', 42, 7, nil), '|')"),
        "/b/saysdf|view|--line|42|--col|7|--|/a b.rs"
    );
    assert_eq!(
        s(&l, "return table.concat(viewer.build_args('s', 'view', 'x', nil, nil, { 'code', '-g', '{path}:{line}' }), '|')"),
        r#"s|view|--editor-cmd|["code","-g","{path}:{line}"]|--|x"#
    );
}

#[test]
fn subcommand_table() {
    let l = lua();
    assert_eq!(s(&l, "return viewer.subcommand_for('a/b.RS')"), "view");
    assert_eq!(s(&l, "return viewer.subcommand_for('README.md')"), "md");
    assert_eq!(s(&l, "return viewer.subcommand_for('a/b.Markdown')"), "md");
    assert_eq!(s(&l, "viewer.SUBCOMMANDS.md = 'markdown-preview' return viewer.subcommand_for('x.MD')"), "markdown-preview");
    assert_eq!(s(&l, "return viewer.subcommand_for('noext')"), "view");
    assert_eq!(s(&l, "return viewer.subcommand_for('flow.mmd')"), "mermaid");
    assert_eq!(s(&l, "return viewer.subcommand_for('a/b.Mermaid')"), "mermaid");
}

#[test]
fn mermaid_options_build_flags() {
    let l = lua();
    let args = |opts: &str| {
        s(&l, &format!(
            "local sub = viewer.subcommand_for('a.mmd')              return table.concat(viewer.build_args('s', sub, 'a.mmd', nil, nil, nil, viewer.subcommand_flags(sub, {opts})), '|')"
        ))
    };
    assert_eq!(args("{ mermaid = { ascii = true, zoom = 'compact' } }"), "s|mermaid|--ascii|--zoom|compact|--|a.mmd");
    assert_eq!(args("{ mermaid = { zoom = 'huge' } }"), "s|mermaid|--|a.mmd");
    assert_eq!(args("nil"), "s|mermaid|--|a.mmd");
}

#[test]
fn markdown_images_option_builds_no_images_flag() {
    let l = lua();
    let args = |opts: &str| {
        s(&l, &format!(
            "local sub = viewer.subcommand_for('a.md')              return table.concat(viewer.build_args('s', sub, 'a.md', 3, nil, nil, viewer.subcommand_flags(sub, {opts})), '|')"
        ))
    };
    assert_eq!(args("{ markdown = { images = false } }"), "s|md|--line|3|--no-images|--|a.md");
    assert_eq!(args("{ markdown = { images = true } }"), "s|md|--line|3|--|a.md");
    assert_eq!(args("nil"), "s|md|--line|3|--|a.md");
    assert_eq!(
        s(&l, "return #viewer.subcommand_flags('view', { markdown = { images = false } })"),
        "0"
    );
}

#[test]
fn find_bin_prefers_executable_dir_then_path() {
    let l = lua();
    let code = |present: &str| {
        format!(
            "local present = {{ {present} }} \
             return tostring(util.find_bin('saysdf', {{ triple = 'x86_64-unknown-linux-gnu', home = '/h', \
               executable_dir = '/opt/t', getenv = function() return '/usr/bin:/x/bin' end, \
               open = function(p) if present[p] then return {{ close = function() end }} end end }}))"
        )
    };
    assert_eq!(s(&l, &code("['/opt/t/saysdf'] = true, ['/usr/bin/saysdf'] = true")), "/opt/t/saysdf");
    assert_eq!(s(&l, &code("['/x/bin/saysdf'] = true")), "/x/bin/saysdf");
    assert_eq!(s(&l, &code("")), "nil");
    let win = "local B = string.char(92)         local hit = 'C:' .. B .. 'b' .. B .. 'saysdf.exe'         return tostring(util.find_bin('saysdf', { triple = 'x86_64-pc-windows-msvc', home = 'h',         executable_dir = 'C:' .. B .. 't', getenv = function() return 'C:' .. B .. 'a;C:' .. B .. 'b' .. B end,         open = function(p) if p == hit then return { close = function() end } end end }))";
    let want = format!("C:{0}b{0}saysdf.exe", char::from(92u8));
    assert_eq!(s(&l, win), want);
}

#[test]
fn open_splits_local_only() {
    let l = lua();
    l.load(
        r#"
        viewer.setup({ bin = '/b/saysdf', editor = { 'nvim', '+{line}', '{path}' } }, {})
        splits = {}
        function mkpane(domain)
          return { get_domain_name = function() return domain end,
                   split = function(_, t) table.insert(splits, t) end }
        end
        r1 = viewer.open(nil, mkpane('local'), '/p/a.rs', 3, 4)
        r2 = viewer.open(nil, mkpane('SSH:host'), '/p/a.rs', 3, 4)
    "#,
    )
    .exec()
    .unwrap();
    assert_eq!(s(&l, "return tostring(r1) .. tostring(r2) .. #splits"), "truefalse1");
    assert_eq!(
        s(&l, "return table.concat(splits[1].args, '|')"),
        r#"/b/saysdf|view|--line|3|--col|4|--editor-cmd|["nvim","+{line}","{path}"]|--|/p/a.rs"#
    );
}

#[test]
fn setup_registers_key() {
    let l = lua();
    assert_eq!(
        s(&l, "local c = {} viewer.setup({}, c) return c.keys[1].key .. c.keys[1].mods"),
        "GCTRL|SHIFT"
    );
    assert_eq!(s(&l, "local c = {} viewer.setup({ keys = false }, c) return tostring(c.keys)"), "nil");
}

// change inline-graphics 3.2: an image from saysdf.paths -> `saysdfTerm-cli imgcat --hold` in a split
#[test]
fn open_image_uses_imgcat_hold() {
    let l = lua();
    l.load(
        r#"
        viewer.setup({ bin = '/b/saysdf', cli_bin = '/b/saysdfTerm-cli' }, {})
        splits = {}
        local pane = { get_domain_name = function() return 'local' end,
                       split = function(_, t) table.insert(splits, t) end }
        r = viewer.open(nil, pane, '/p/Shot.PNG', 3, nil)
    "#,
    )
    .exec()
    .unwrap();
    assert_eq!(s(&l, "return tostring(r) .. #splits"), "true1");
    assert_eq!(
        s(&l, "return table.concat(splits[1].args, '|')"),
        "/b/saysdfTerm-cli|imgcat|--hold|/p/Shot.PNG"
    );
    assert_eq!(
        s(&l, "return tostring(viewer.is_image('a.webp')) .. tostring(viewer.is_image('a.rs'))"),
        "truefalse"
    );
}
