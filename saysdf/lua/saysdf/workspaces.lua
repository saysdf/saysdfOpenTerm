-- saysdfTerminal: project workspaces (change project-workspaces).
--
-- setup options (require("saysdf").setup({ workspaces = {...} })); workspaces = false disables:
--   key = { key = "A", mods = "CTRL|SHIFT" }  -- workspace list; false -- no hotkey
--   palette = true                            -- palette entries (via saysdf.palette)
--   dir = nil                                 -- storage folder; default <data>/workspaces
--   autosave = true, autosave_interval = 30   -- autosave (update-status, debounce, only on change)
--   restore = "ask"                           -- at startup: "ask" | "always" | "never" (gui-startup)
-- A workspace's allow_commands flag (palette "allow/forbid commands") lives in its file;
-- without the flag commands are neither saved nor passed to spawn.
--
-- User data (outside the repo): <data>/workspaces/<name>.json, <data>/workspace-trust.json
-- (<data> is saysdf.util.data_dir()). Restoring does NOT run commands: only shells in the
-- saved folders. Commands from the project's `.saysdf/workspace.json` run only after
-- confirmation; the approval is bound to the exact command list (and its hash) -- if the list
-- changes, ask again.
--
-- `.saysdf/workspace.json` format:
--   { "name": "proj", "tabs": [ { "title": "srv", "cwd": "server", "command": ["cargo", "run"] } ] }
--   cwd is relative to the project root (absolute paths and ".." are rejected); command is a string or an array.
--
-- Pure functions (tested): layout_from_panes, plan_splits, to_data, from_data,
-- normalize_cwd, parse_project, commands_hash, is_trusted, trust.

local util = require("saysdf.util")
local t, _, _ = require("saysdf.i18n").domain("workspaces")

local M = {}
M.VERSION = 1
M._state = { opts = {} }

local function wezterm()
  return require("wezterm")
end

-- Choosing happens in the sidebar (`saysdf.sidebar.selector`), with InputSelector as the fallback.
local function sidebar()
  return require("saysdf.sidebar")
end

-- ---- layout: pane geometry <-> split tree ----
-- pane: { left, top, width, height, cwd?, args? }
-- tree: { pane = {cwd, args} } | { split = "h"|"v", ratio = 0..1, a = node, b = node }
--   "h" is a cut by a vertical line (a on the left, b on the right), "v" by a horizontal one (a on top).

local function try_cut(panes, lo, size)
  local min, max = math.huge, -math.huge
  for _, p in ipairs(panes) do
    min = math.min(min, p[lo])
    max = math.max(max, p[lo] + p[size])
  end
  local cuts = {}
  for _, p in ipairs(panes) do
    local x = p[lo] + p[size]
    if x < max then
      table.insert(cuts, x)
    end
  end
  table.sort(cuts)
  for _, x in ipairs(cuts) do
    local a, b, ok = {}, {}, true
    for _, p in ipairs(panes) do
      if p[lo] + p[size] <= x then
        table.insert(a, p)
      elseif p[lo] >= x then
        table.insert(b, p)
      else
        ok = false
        break
      end
    end
    if ok and #a > 0 and #b > 0 then
      return a, b, (x - min) / (max - min)
    end
  end
end

local function leaf(p)
  return { pane = { cwd = p.cwd, args = p.args } }
end

local function round3(x)
  return math.floor(x * 1000 + 0.5) / 1000
end

function M.layout_from_panes(panes)
  if #panes == 0 then
    return nil
  end
  if #panes == 1 then
    return leaf(panes[1])
  end
  local kind = "h"
  local a, b, r = try_cut(panes, "left", "width")
  if not a then
    kind = "v"
    a, b, r = try_cut(panes, "top", "height")
  end
  if not a then
    -- not a guillotine layout: stack the panes top to bottom
    a, b, r, kind = { panes[1] }, { table.unpack(panes, 2) }, 1 / #panes, "v"
  end
  return { split = kind, ratio = round3(r), a = M.layout_from_panes(a), b = M.layout_from_panes(b) }
end

local function first_leaf(n)
  while not n.pane do
    n = n.a
  end
  return n.pane
end

-- Plan: { root = {cwd, args}, steps = { {from = i, new = j, direction = "Right"|"Bottom",
-- size = share of the new pane, cwd, args} } }; pane 1 is the original one.
function M.plan_splits(layout)
  local steps, count = {}, 1
  local function walk(n, idx)
    if n.pane then
      return
    end
    count = count + 1
    local new = count
    local p = first_leaf(n.b)
    table.insert(steps, {
      from = idx,
      new = new,
      direction = n.split == "h" and "Right" or "Bottom",
      size = round3(1 - n.ratio),
      cwd = p.cwd,
      args = p.args,
    })
    walk(n.a, idx)
    walk(n.b, new)
  end
  walk(layout, 1)
  return { root = first_leaf(layout), steps = steps }
end

-- ---- serialization ----

local function valid_node(n, depth)
  if type(n) ~= "table" or depth > 64 then
    return false
  end
  if n.pane ~= nil then
    return type(n.pane) == "table" and (n.pane.cwd == nil or type(n.pane.cwd) == "string")
  end
  return (n.split == "h" or n.split == "v")
    and type(n.ratio) == "number" and n.ratio > 0 and n.ratio < 1
    and valid_node(n.a, depth + 1) and valid_node(n.b, depth + 1)
end

-- Copy of the tree; commands (args) are kept only with keep (the workspace's allow_commands flag).
local function strip(n, keep)
  if n.pane then
    local args = nil
    if keep and type(n.pane.args) == "table" then
      args = {}
      for _, a in ipairs(n.pane.args) do
        table.insert(args, tostring(a))
      end
    end
    return { pane = { cwd = n.pane.cwd, args = args } }
  end
  return { split = n.split, ratio = n.ratio, a = strip(n.a, keep), b = strip(n.b, keep) }
end

-- ws: { name, allow_commands?, tabs = { {title?, layout} } } -> table for JSON
function M.to_data(ws)
  local keep = ws.allow_commands == true
  local tabs = {}
  for _, t in ipairs(ws.tabs or {}) do
    table.insert(tabs, { title = t.title, layout = strip(t.layout, keep) })
  end
  return { version = M.VERSION, name = ws.name, allow_commands = keep or nil, tabs = tabs }
end

-- -> ws | nil, err
function M.from_data(t)
  if type(t) ~= "table" then
    return nil, "not an object"
  end
  if type(t.version) ~= "number" then
    return nil, "no version"
  end
  if t.version > M.VERSION then
    return nil, "future version " .. t.version
  end
  if type(t.name) ~= "string" or t.name == "" then
    return nil, "no name"
  end
  if type(t.tabs) ~= "table" then
    return nil, "no tabs"
  end
  local keep = t.allow_commands == true
  local tabs = {}
  for i, tab in ipairs(t.tabs) do
    if type(tab) ~= "table" or not valid_node(tab.layout, 0) then
      return nil, "bad layout in tab " .. i
    end
    table.insert(tabs, { title = type(tab.title) == "string" and tab.title or nil, layout = strip(tab.layout, keep) })
  end
  return { name = t.name, allow_commands = keep, tabs = tabs }
end

-- ---- autosave and startup ----

-- Autosave decision: at most once per debounce seconds and only when the serialization changed.
-- st: { last_check?, last_text? } (mutated). -> true if text should be written.
function M.autosave_decide(st, now, text, debounce)
  debounce = debounce or 30
  if st.last_check and now - st.last_check < debounce then
    return false
  end
  st.last_check = now
  if text == st.last_text then
    return false
  end
  st.last_text = text
  return true
end

-- Action at GUI startup: mode = "ask"|"always"|"never"; has_session -- last-session exists.
-- -> "restore" | "ask" | "none"
function M.startup_action(mode, has_session)
  if not has_session or mode == "never" then
    return "none"
  end
  if mode == "always" then
    return "restore"
  end
  return "ask"
end

-- Whether to ask about restoring: the previous session is non-trivial -- more than one tab
-- (across all workspaces together) or a tab with a split. One tab with one pane is a
-- silent normal start. wss is the list of loaded workspaces ({ tabs = {{layout}} }).
function M.session_worth_restoring(wss)
  local tabs = 0
  for _, ws in ipairs(wss or {}) do
    for _, t in ipairs(ws.tabs or {}) do
      tabs = tabs + 1
      if tabs > 1 or (type(t.layout) == "table" and t.layout.pane == nil) then
        return true
      end
    end
  end
  return false
end

-- last-session.json data: { version, workspaces = {names} } -> list of names | nil
function M.session_names(t)
  if type(t) ~= "table" or type(t.workspaces) ~= "table" or (tonumber(t.version) or 0) > M.VERSION then
    return nil
  end
  local out = {}
  for _, n in ipairs(t.workspaces) do
    if type(n) == "string" and n ~= "" then
      table.insert(out, n)
    end
  end
  return #out > 0 and out or nil
end

function M.encode(ws)
  return wezterm().json_encode(M.to_data(ws))
end

function M.decode(text)
  local ok, t = pcall(wezterm().json_parse, text)
  if not ok then
    return nil, "bad json"
  end
  return M.from_data(t)
end

-- ---- project file and trust ----

-- cwd from `.saysdf/workspace.json` inside the root: absolute paths and ".." are forbidden.
-- -> absolute path | nil, err
function M.normalize_cwd(root, rel, windows)
  rel = rel or "."
  if rel:match("^[/\\]") or rel:match("^%a:") or rel:match("^~") then
    return nil, "absolute path"
  end
  local parts = {}
  for seg in rel:gmatch("[^/\\]+") do
    if seg == ".." then
      return nil, "outside project"
    elseif seg ~= "." then
      table.insert(parts, seg)
    end
  end
  local sep = windows and "\\" or "/"
  root = root:gsub("[/\\]+$", "")
  if #parts == 0 then
    return root
  end
  return root .. sep .. table.concat(parts, sep)
end

function M.trust_key(root, windows)
  local k = root:gsub("[/\\]+$", "")
  if windows then
    k = k:gsub("/", "\\"):lower()
  end
  return k
end

-- Project commands { {cwd, args = {...}} } -> canonical strings
local function canon(commands)
  local out = {}
  for _, c in ipairs(commands or {}) do
    local args = type(c.args) == "table" and c.args or { tostring(c.args or "") }
    table.insert(out, (c.cwd or ".") .. "\0" .. table.concat(args, "\0"))
  end
  return out
end

function M.commands_hash(commands)
  return util.hash(table.concat(canon(commands), "\n"))
end

-- store: { [trust_key] = { hash, commands = {canonical strings} } }
function M.is_trusted(store, root, commands, windows)
  local e = store and store[M.trust_key(root, windows)]
  if type(e) ~= "table" or e.hash ~= M.commands_hash(commands) then
    return false
  end
  local c = canon(commands)
  if type(e.commands) ~= "table" or #e.commands ~= #c then
    return false
  end
  for i = 1, #c do
    if e.commands[i] ~= c[i] then
      return false
    end
  end
  return true
end

function M.trust(store, root, commands, windows)
  store[M.trust_key(root, windows)] = { hash = M.commands_hash(commands), commands = canon(commands) }
  return store
end

-- -> { name, tabs = {{title, cwd(abs), args?}}, commands = {{cwd(rel), args}} } | nil, err
function M.parse_project(root, t, windows)
  if type(t) ~= "table" or type(t.tabs) ~= "table" then
    return nil, "no tabs"
  end
  local res = {
    name = type(t.name) == "string" and t.name or root:match("([^/\\]+)[/\\]*$"),
    tabs = {},
    commands = {},
  }
  for i, tab in ipairs(t.tabs) do
    local cwd, err = M.normalize_cwd(root, tab.cwd, windows)
    if not cwd then
      return nil, "tab " .. i .. ": " .. err
    end
    local args = tab.command
    if type(args) == "string" then
      args = { args }
    end
    table.insert(res.tabs, { title = tab.title, cwd = cwd, args = args })
    if args then
      table.insert(res.commands, { cwd = tab.cwd or ".", args = args })
    end
  end
  return res
end

-- ---- GUI work (checked by hand) ----

function M.dir()
  return M._state.opts.dir or util.join(util.data_dir(), "workspaces")
end

local function safe_name(name)
  return (name:gsub("[^%w%-_. ]", "_"))
end

local function cwd_of(pane)
  local u = pane:get_current_working_dir()
  if u == nil then
    return nil
  end
  local ok, p = pcall(function()
    return u.file_path
  end)
  if ok and p then
    if util.is_windows() then
      p = p:gsub("^/(%a:)", "%1")
    end
    return p
  end
  return tostring(u)
end

local SHELLS = { bash = 1, sh = 1, zsh = 1, fish = 1, nu = 1, cmd = 1, pwsh = 1, powershell = 1, wsl = 1, conhost = 1 }

-- argv of the pane's process if it is not a shell (for workspaces with allow_commands).
local function args_of(pane)
  local ok, info = pcall(function()
    return pane:get_foreground_process_info()
  end)
  if not ok or type(info) ~= "table" or type(info.argv) ~= "table" or #info.argv == 0 then
    return nil
  end
  local exe = tostring(info.argv[1]):match("([^/\\]+)$") or ""
  exe = exe:lower():gsub("%.exe$", "")
  if SHELLS[exe] then
    return nil
  end
  return info.argv
end

-- The "allow commands" flag is stored in the workspace's saved file (user data).
function M.commands_allowed(name)
  local ws = M.load(name)
  return ws ~= nil and ws.allow_commands == true
end

-- Workspace snapshot: windows of this workspace -> tabs -> layout; commands only with the flag.
function M.snapshot(name, allow_commands)
  if allow_commands == nil then
    allow_commands = M.commands_allowed(name)
  end
  local ws = { name = name, allow_commands = allow_commands, tabs = {} }
  for _, win in ipairs(wezterm().mux.all_windows()) do
    if win:get_workspace() == name then
      for _, tab in ipairs(win:tabs()) do
        local panes = {}
        for _, info in ipairs(tab:panes_with_info()) do
          table.insert(panes, {
            left = info.left, top = info.top, width = info.width, height = info.height,
            cwd = cwd_of(info.pane),
            args = allow_commands and args_of(info.pane) or nil,
          })
        end
        local title = tab:get_title()
        table.insert(ws.tabs, { title = title ~= "" and title or nil, layout = M.layout_from_panes(panes) })
      end
    end
  end
  return ws
end

function M.save(name, allow_commands, text)
  util.ensure_dir(M.dir())
  text = text or M.encode(M.snapshot(name, allow_commands))
  local ok, err = util.write_atomic(util.join(M.dir(), safe_name(name) .. ".json"), text)
  if not ok then
    util.log_error("saysdf.workspaces: save failed: " .. tostring(err))
  end
  return ok
end

function M.load(name)
  local text = util.read_file(util.join(M.dir(), safe_name(name) .. ".json"))
  if not text then
    return nil, "not found"
  end
  return M.decode(text)
end

function M.saved_names()
  local out = {}
  local ok, files = pcall(wezterm().read_dir, M.dir())
  if ok then
    for _, f in ipairs(files) do
      local n = f:match("([^/\\]+)%.json$")
      if n then
        table.insert(out, n)
      end
    end
  end
  table.sort(out)
  return out
end

-- Restore; commands only with allow_commands (approved project commands) or the saved
-- workspace's ws.allow_commands flag.
function M.restore(ws, allow_commands)
  allow_commands = allow_commands == true or ws.allow_commands == true
  local mux = wezterm().mux
  local window
  for i, t in ipairs(ws.tabs) do
    local plan = M.plan_splits(t.layout)
    local root_args = allow_commands and plan.root.args or nil
    local tab, pane
    if i == 1 then
      tab, pane, window = mux.spawn_window({ workspace = ws.name, cwd = plan.root.cwd, args = root_args })
    else
      tab, pane = window:spawn_tab({ cwd = plan.root.cwd, args = root_args })
    end
    if t.title then
      tab:set_title(t.title)
    end
    local made = { pane }
    for _, s in ipairs(plan.steps) do
      made[s.new] = made[s.from]:split({
        direction = s.direction,
        size = s.size,
        cwd = s.cwd,
        args = allow_commands and s.args or nil,
      })
    end
  end
  mux.set_active_workspace(ws.name)
end

local TRUST_FILE = "workspace-trust.json"

function M.trust_store()
  local text = util.read_file(util.join(util.data_dir(), TRUST_FILE))
  if not text then
    return {}
  end
  local ok, t = pcall(wezterm().json_parse, text)
  return (ok and type(t) == "table") and t or {}
end

-- Open the project's `.saysdf/workspace.json` in root; commands only after confirmation.
function M.open_project(window, pane, root)
  local w = wezterm()
  local win_os = util.is_windows()
  local text = util.read_file(util.join(root, ".saysdf", "workspace.json"))
  if not text then
    return util.log_warn("saysdf.workspaces: no .saysdf/workspace.json in " .. root)
  end
  local okj, parsed = pcall(w.json_parse, text)
  local proj, err = M.parse_project(root, okj and parsed or nil, win_os)
  if not proj then
    return util.log_error("saysdf.workspaces: " .. tostring(err))
  end
  local ws = { name = proj.name, tabs = {} }
  for _, tab in ipairs(proj.tabs) do
    table.insert(ws.tabs, { title = tab.title, layout = { pane = { cwd = tab.cwd, args = tab.args } } })
  end
  local store = M.trust_store()
  if #proj.commands == 0 or M.is_trusted(store, root, proj.commands, win_os) then
    return M.restore(ws, true)
  end
  local lines = {}
  for _, c in ipairs(proj.commands) do
    table.insert(lines, c.cwd .. ": " .. table.concat(c.args, " "))
  end
  window:perform_action(w.action.InputSelector({
    title = t("Project commands: {commands}", { commands = table.concat(lines, "; ") }),
    choices = {
      { id = "run", label = t("Run (remember for this command list)") },
      { id = "shells", label = t("Shells only") },
      { id = "cancel", label = t("Cancel") },
    },
    action = w.action_callback(function(_, _, id)
      if id == "run" then
        M.trust(store, root, proj.commands, win_os)
        util.ensure_dir(util.data_dir())
        util.write_atomic(util.join(util.data_dir(), TRUST_FILE), w.json_encode(store))
        M.restore(ws, true)
      elseif id == "shells" then
        M.restore(ws, false)
      end
    end),
  }), pane)
end

-- Workspace list: open + saved, fuzzy.
function M.choose(window, pane)
  local w = wezterm()
  local seen, choices = {}, {}
  for _, n in ipairs(w.mux.get_workspace_names()) do
    seen[n] = true
    table.insert(choices, { id = "open:" .. n, label = t("{name}  (open)", { name = n }) })
  end
  for _, n in ipairs(M.saved_names()) do
    if not seen[n] then
      table.insert(choices, { id = "saved:" .. n, label = t("{name}  (saved)", { name = n }) })
    end
  end
  window:perform_action(sidebar().selector({
    tool = "workspaces",
    title = t("Workspaces"),
    fuzzy = true,
    choices = choices,
    on_choice = function(win, p, id)
      if not id then
        return
      end
      local kind, name = id:match("^(%a+):(.*)$")
      if kind == "open" then
        win:perform_action(w.action.SwitchToWorkspace({ name = name }), p)
      else
        local ws, err = M.load(name)
        if ws then
          M.restore(ws, false)
        else
          util.log_error("saysdf.workspaces: " .. tostring(err))
        end
      end
    end,
  }), pane)
end

local SESSION_FILE = "last-session.json"

-- One autosave pass (called from update-status): all open workspaces + last-session.
function M.autosave_tick(now)
  local w = wezterm()
  local st = M._state
  st.autosave = st.autosave or {}
  now = now or os.time()
  local last = st.autosave.last_check
  if last and now - last < (st.opts.autosave_interval or 30) then
    return false -- no snapshot more often than the debounce
  end
  local names = w.mux.get_workspace_names()
  local parts = {}
  for _, n in ipairs(names) do
    parts[n] = M.encode(M.snapshot(n))
    table.insert(parts, parts[n])
  end
  local all = table.concat(names, "\n") .. "\n" .. table.concat(parts, "\n")
  if not M.autosave_decide(st.autosave, now, all, st.opts.autosave_interval) then
    return false
  end
  for _, n in ipairs(names) do
    M.save(n, nil, parts[n])
  end
  util.write_atomic(util.join(M.dir(), SESSION_FILE), w.json_encode({ version = M.VERSION, workspaces = names }))
  return true
end

function M.last_session()
  local text = util.read_file(util.join(M.dir(), SESSION_FILE))
  if not text then
    return nil
  end
  local ok, t = pcall(wezterm().json_parse, text)
  return ok and M.session_names(t) or nil
end

-- Restore the previous session's workspaces (commands only for workspaces with the flag).
function M.restore_session(names)
  for _, n in ipairs(names or {}) do
    local ws, err = M.load(n)
    if ws then
      M.restore(ws, false)
    else
      util.log_warn("saysdf.workspaces: skip " .. n .. ": " .. tostring(err))
    end
  end
end

function M.on_startup(cmd)
  local w = wezterm()
  local names = M.last_session()
  local act = M.startup_action(M._state.opts.restore or "ask", names ~= nil)
  if act == "ask" then
    local wss = {}
    for _, n in ipairs(names) do
      local ws = M.load(n)
      if ws then
        table.insert(wss, ws)
      end
    end
    if not M.session_worth_restoring(wss) then
      act = "none"
    end
  end
  if act == "restore" then
    return M.restore_session(names)
  end
  local _, pane, window = w.mux.spawn_window(cmd or {})
  if act == "ask" then
    local gui = window:gui_window()
    if gui then
      gui:perform_action(w.action.InputSelector({
        title = t("Restore previous session ({names})?", { names = table.concat(names, ", ") }),
        choices = { { id = "yes", label = t("Restore (without running commands)") }, { id = "no", label = t("No") } },
        action = w.action_callback(function(_, _, id)
          if id == "yes" then
            M.restore_session(names)
          end
        end),
      }), pane)
    end
  end
end

function M.toggle_commands(name)
  local allowed = not M.commands_allowed(name)
  M.save(name, allowed)
  util.log_info("saysdf.workspaces: commands for " .. name .. ": " .. tostring(allowed))
  return allowed
end

function M.setup(opts, config)
  opts = opts or {}
  M._state.opts = opts
  local w = wezterm()
  if opts.autosave ~= false then
    w.on("update-status", function()
      local ok, err = pcall(M.autosave_tick)
      if not ok then
        util.log_error("saysdf.workspaces: autosave: " .. tostring(err))
      end
    end)
  end
  if (opts.restore or "ask") ~= "never" then
    w.on("gui-startup", M.on_startup)
  end
  if opts.key ~= false and config then
    local k = opts.key or { key = "A", mods = "CTRL|SHIFT" }
    config.keys = config.keys or {}
    table.insert(config.keys, { key = k.key, mods = k.mods, action = w.action_callback(M.choose) })
  end
  if opts.palette ~= false then
    local palette = require("saysdf.palette")
    local kt = opts.key ~= false and palette.key_text(opts.key or { key = "A", mods = "CTRL|SHIFT" }) or nil
    palette.add({ id = "workspaces.choose", menu = "File", order = 10, label = t("Workspaces…"), accel = "w", key = kt,
      hint = { contexts = { "shell", "git" }, label = t("Projects"), priority = 70 },
      brief = t("Workspaces: list"), icon = "md_view_dashboard", action = w.action_callback(M.choose) })
    palette.add({
      id = "workspaces.save", menu = "File", order = 11, accel = "s",
      brief = t("Workspaces: save current"),
      icon = "md_content_save",
      action = w.action_callback(function(win)
        M.save(win:active_workspace())
      end),
    })
    palette.add({
      id = "workspaces.commands", menu = "File", order = 13, accel = "c",
      brief = t("Workspaces: allow/forbid commands of the current one"),
      icon = "md_play_circle_outline",
      action = w.action_callback(function(win)
        M.toggle_commands(win:active_workspace())
      end),
    })
    palette.add({
      id = "workspaces.open_project", menu = "File", order = 12, accel = "o",
      brief = t("Workspaces: open the project of the current folder (.saysdf/workspace.json)"),
      icon = "md_folder_open",
      action = w.action_callback(function(win, pane)
        local cwd = cwd_of(pane)
        if cwd then
          M.open_project(win, pane, cwd)
        end
      end),
    })
  end
  return config
end

return M
