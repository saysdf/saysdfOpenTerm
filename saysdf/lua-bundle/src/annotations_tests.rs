//! Tests of `saysdf.annotations` (change text-annotations, D7): MRU from the status tick, agent
//! detection (process / title), paste without Enter, selector fallback, rejected requests.
//! `saysdf.viewer` (viewer_record) and `saysdf.status` are stubbed.
use super::*;

const FAKE: &str = r#"
    logs = {}
    pasted = {}
    actions = {}
    activated = nil
    status_hook = nil
    recs = {}
    panes = {}
    package.loaded.wezterm = {
        GLOBAL = {},
        target_triple = 'x86_64-unknown-linux-gnu', home_dir = '/home/u',
        log_info = function(s) table.insert(logs, s) end,
        log_error = function(s) table.insert(logs, 'E:' .. s) end,
        log_warn = function() end,
        on = function(name, f) handlers = handlers or {}; handlers[name] = f end,
        action_callback = function(f) return { cb = f } end,
        action = { InputSelector = function(s) return { input_selector = s } end },
        read_dir = function(d) if d == FOLDER then return {} end error('no dir') end,
    }
    package.loaded['saysdf.status'] = { hook = function(n, f) status_hook = f end }
    package.loaded['saysdf.viewer'] = { viewer_record = function(p) return recs[p:pane_id()] end }
    function mk_pane(id, fg, title, cwd)
        local p = { id = id, fg = fg, title = title or '', cwd = cwd,
            pane_id = function(self) return self.id end,
            get_foreground_process_name_cached = function(self) return self.fg end,
            is_alt_screen_active = function() return false end,
            get_title = function(self) return self.title end,
            get_current_working_dir = function(self) return self.cwd and { file_path = self.cwd } or nil end,
            activate = function(self) activated = self.id end,
            send_paste = function(self, text) table.insert(pasted, { id = self.id, text = text }) end }
        panes[id] = p
        return p
    end
    viewer = mk_pane(1, '/bin/saysdf', 'view', nil)
    shell = mk_pane(2, '/bin/bash', 'bash', nil)
    agent = mk_pane(3, 'C:\\Users\\me\\claude.exe', 'x', '/proj')
    tab = { panes = function() return { viewer, shell, agent } end }
    window = {
        window_id = function() return 7 end,
        mux_window = function() return { tabs = function() return { tab } end } end,
        perform_action = function(self, a, p) table.insert(actions, a) end,
        toast_notification = function() end,
    }
    entries = {}
    package.loaded['saysdf.palette'] = { add = function(e) table.insert(entries, e) end }
    sidebar_specs = {}
    package.loaded['saysdf.sidebar'] = { open = function(w, p, spec) table.insert(sidebar_specs, spec); return true end }
    package.loaded['saysdf.git'] = { root_of = function(p) return p.cwd end }
    package.loaded['saysdf.files'] = { cwd_of = function(p) return p.cwd end }
    function entry(id) for _, e in ipairs(entries) do if e.id == id then return e end end end
    util = require('saysdf.util')
    util.find_bin = function() return '/bin/saysdf' end
    require('saysdf.i18n').select('en')
    ann = require('saysdf.annotations')
    ann.setup({}, {})
    recs[1] = { token = 'tok', used = false, multi = true }
    function send(value) return ann.handle_user_var(window, viewer, 'SAYSDF_VIEWER', value) end
"#;

fn lua() -> Lua {
    let modules: Modules = Box::leak(
        vec![
            ("saysdf.annotations", get("saysdf.annotations").unwrap()),
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
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.md");
    std::fs::write(&file, "x").unwrap();
    let dir_path = Box::leak(Box::new(dir)).path().to_string_lossy().replace('\\', "/");
    lua.globals().set("FILE", format!("{dir_path}/note.md")).unwrap();
    lua.globals().set("FOLDER", dir_path).unwrap();
    lua.load(FAKE).exec().unwrap();
    lua
}

fn s(l: &Lua, code: &str) -> String {
    l.load(code).eval::<String>().unwrap()
}

#[test]
fn mru_order_and_cap() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"status_hook(window, shell); status_hook(window, agent); status_hook(window, shell)
            status_hook(window, shell)
            return table.concat(ann.mru(window), ',')"#
        ),
        "2,3"
    );
    assert_eq!(
        s(
            &l,
            r#"for i = 10, 40 do status_hook(window, mk_pane(i, 'x')) end
            return tostring(#ann.mru(window))"#
        ),
        "16"
    );
}

#[test]
fn agent_by_process_and_by_title() {
    let l = lua();
    assert_eq!(s(&l, "return tostring(ann.find_agent(window):pane_id())"), "3");
    // not by process: change the foreground, title decides
    assert_eq!(
        s(
            &l,
            r#"agent.fg = '/bin/node'
            local r = { tostring(ann.find_agent(window)) }
            agent.title = 'Claude Code - repo'
            table.insert(r, tostring(ann.find_agent(window):pane_id()))
            return table.concat(r, ',')"#
        ),
        "nil,3"
    );
    // the viewer pane itself is never the target
    assert_eq!(s(&l, "viewer.fg = '/bin/claude'; agent.fg = '/bin/bash'; agent.title = 'x'; return tostring(ann.find_agent(window, 1))"), "nil");
}

#[test]
fn most_recent_agent_wins() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local a2 = mk_pane(4, '/bin/claude', '', nil)
            tab.panes = function() return { viewer, shell, agent, a2 } end
            status_hook(window, a2); status_hook(window, agent); status_hook(window, shell)
            return tostring(ann.find_agent(window):pane_id())"#
        ),
        "3"
    );
}

#[test]
fn handoff_pastes_without_enter_into_agent() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local r = { tostring(send('tok:handoff-file:' .. FILE)) }
            table.insert(r, #pasted .. ':' .. pasted[1].id)
            table.insert(r, tostring(pasted[1].text:find('[\r\n]') == nil))
            table.insert(r, tostring(pasted[1].text == 'Review the annotations in ' .. FILE))
            table.insert(r, tostring(activated))
            return table.concat(r, ',')"#
        ),
        "true,1:3,true,true,3"
    );
    // folder kind
    assert_eq!(
        s(
            &l,
            r#"pasted = {}
            send('tok:handoff-folder:' .. FOLDER)
            return tostring(pasted[1].text == 'Review the annotations in folder ' .. FOLDER)"#
        ),
        "true"
    );
}

#[test]
fn relative_path_and_custom_phrase() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"agent.cwd = FOLDER
            send('tok:handoff-file:' .. FILE)
            ann.setup({ phrase_file = 'See {path}!' }, {}); recs[1].last_value = nil
            send('tok:handoff-file:' .. FILE)
            return pasted[1].text .. '|' .. pasted[2].text"#
        ),
        "Review the annotations in note.md|See note.md!"
    );
}

#[test]
fn no_agent_falls_back_to_selector() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"agent.fg = '/bin/bash'
            local r = { tostring(send('tok:handoff-file:' .. FILE)) }
            table.insert(r, #pasted .. ':' .. #actions)
            local sel = actions[1].input_selector
            table.insert(r, #sel.choices .. ':' .. sel.choices[1].id)
            sel.action.cb(window, viewer, '2', 'x')
            table.insert(r, #pasted .. ':' .. pasted[1].id)
            recs[1].last_value = nil
            send('tok:handoff-file:' .. FILE)
            sel.action.cb(window, viewer, nil, nil)
            table.insert(r, tostring(#pasted))
            return table.concat(r, ',')"#
        ),
        "true,0:1,2:2,1:2,1"
    );
}

#[test]
fn forged_unknown_and_missing_requests_are_rejected() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local r = {}
            local function try(v, pane) table.insert(r, tostring(ann.handle_user_var(window, pane or viewer, 'SAYSDF_VIEWER', v))) end
            try('bad:handoff-file:' .. FILE)                 -- wrong token
            try('tok:unknown-kind:' .. FILE)                 -- kind outside the allow-list
            try('tok:handoff-file:' .. FILE .. '.nope')      -- path does not exist
            try('tok:handoff-folder:' .. FILE)               -- a file is not a folder
            try('tok:handoff-file:' .. FILE .. '\nrm -rf /') -- control characters
            try('tok:handoff-file:')                         -- empty path
            try('tok:handoff-file:' .. FILE, shell)          -- not a viewer pane
            viewer.fg = '/bin/bash'
            try('tok:handoff-file:' .. FILE)                 -- foreground is not saysdf
            table.insert(r, tostring(#pasted))
            return table.concat(r, ',')"#
        ),
        "false,false,false,false,false,false,false,false,0"
    );
}

#[test]
fn status_tick_does_not_call_foreground() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local calls = 0
            shell.get_foreground_process_name_cached = function() calls = calls + 1 end
            shell.get_foreground_process_name = function() calls = calls + 1 end
            shell.is_alt_screen_active = function() calls = calls + 1 end
            for i = 1, 5 do status_hook(window, shell) end
            return tostring(calls)"#
        ),
        "0"
    );
}

#[test]
fn repeated_handoffs_with_different_paths_are_accepted() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local r = { tostring(send('tok:handoff-file:' .. FILE)), tostring(send('tok:handoff-folder:' .. FOLDER)) }
            table.insert(r, tostring(#pasted))
            return table.concat(r, ',')"#
        ),
        "true,true,2"
    );
}

#[test]
fn unseen_agents_lone_is_used_several_ask() {
    let l = lua();
    // pane 3 never entered the MRU: it is the only agent pane
    assert_eq!(s(&l, "status_hook(window, shell); return tostring(ann.find_agent(window):pane_id())"), "3");
    // a second unseen agent: ambiguous -> selector
    assert_eq!(
        s(
            &l,
            r#"local a2 = mk_pane(4, '/bin/claude', '', nil)
            tab.panes = function() return { viewer, shell, agent, a2 } end
            local r = { tostring(ann.find_agent(window)) }
            table.insert(r, tostring(send('tok:handoff-file:' .. FILE)))
            table.insert(r, #pasted .. ':' .. #actions)
            return table.concat(r, ',')"#
        ),
        "nil,true,0:1"
    );
}

#[test]
fn palette_entries_are_registered_with_menu_slots() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local r = {}
            for _, id in ipairs({ 'annotations.list', 'annotations.handoff_folder', 'annotations.keys', 'viewer.open_at' }) do
                local e = entry(id)
                table.insert(r, id .. '=' .. tostring(e and (e.menu or (e.sidebar and 'sidebar')) or 'MISSING'))
            end
            return table.concat(r, ',')"#
        ),
        "annotations.list=Tools,annotations.handoff_folder=Tools,annotations.keys=Help,viewer.open_at=sidebar"
    );
    // palette = false: no entries
    assert_eq!(s(&l, "entries = {}; ann._reset(); ann.setup({ palette = false }, {}); return tostring(#entries)"), "0");
}

#[test]
fn viewer_keys_help_lists_all_keys_in_the_selector() {
    let l = lua();
    assert_eq!(s(&l, "return tostring(#ann.help_choices())"), "6");
    let all = s(&l, "local r = {} for _, c in ipairs(ann.help_choices()) do r[#r+1] = c.label end return table.concat(r, '|')");
    for k in ["Ctrl+Space", "Ctrl: R substitute", "Alt+Down / Alt+Up", "G hand the file", "L annotations", "F diff", "W wrap", "Ctrl+S — source", "] / [", "Ctrl+J"] {
        assert!(all.contains(k), "missing {k}");
    }
    assert_eq!(s(&l, "return tostring(entry('annotations.keys').action.input_selector ~= nil)"), "true");
}

#[test]
fn list_entry_runs_the_annotations_tool_in_the_sidebar_for_the_project_root() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"agent.cwd = '/proj'
            entry('annotations.list').action.cb(window, agent)
            local sp = sidebar_specs[1]
            return sp.tool .. ':' .. table.concat(sp.args, ' ') .. ':' .. tostring(sp.multi)"#
        ),
        "annotations:annotations /proj:true"
    );
}

#[test]
fn handoff_folder_entry_uses_the_project_root() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"shell.cwd = FOLDER
            status_hook(window, shell)
            entry('annotations.handoff_folder').action.cb(window, shell)
            return #pasted .. ':' .. (pasted[1] and pasted[1].id or 0) .. ':' .. tostring(pasted[1].text:find('folder', 1, true) ~= nil)"#
        ),
        "1:3:true"
    );
}

#[test]
fn open_at_action_validates_the_argument() {
    let l = lua();
    assert_eq!(
        s(
            &l,
            r#"local e = entry('viewer.open_at')
            local r = {}
            for _, a in ipairs({ '/a/b.md:12', 'C:/a/b.md:3', 'C:' .. string.char(92) .. 'a:b.md:3', 'rel.md:1', '/a/b.md:0', '/a/b.md', '/a/b' .. string.char(10) .. '.md:1' }) do
                table.insert(r, tostring(e.accept(a)))
            end
            local p, line = ann.parse_open_at('C:/x:y/z.md:42')
            table.insert(r, p .. '@' .. line)
            return table.concat(r, ',')"#
        ),
        "true,true,true,false,false,false,false,C:/x:y/z.md@42"
    );
    assert_eq!(
        s(
            &l,
            r#"local got
            package.loaded['saysdf.viewer'].open = function(w, p, path, line) got = path .. ':' .. line end
            entry('viewer.open_at').sidebar_run(window, shell, FILE .. ':7')
            return got == FILE .. ':7' and 'ok' or tostring(got)"#
        ),
        "ok"
    );
}
