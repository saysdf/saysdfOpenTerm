-- saysdfTerminal: hand annotations to the agent (change text-annotations, design D7).
--
-- The viewer (`saysdf view`/`md`/annotations list) sends the user var SAYSDF_VIEWER =
-- `<token>:handoff-file:<path>` / `<token>:handoff-folder:<dir>` through saysdf.bridge. The pane
-- must be a recorded viewer pane (saysdf.viewer.viewer_record(pane)). The phrase with the path is
-- pasted (pane:send_paste, NO Enter) into the most recently active pane of the window that runs
-- the agent. Nothing from the terminal output decides where the text goes: the target is chosen
-- here, the payload is only a path that must exist and carry no control characters.
--
-- MRU: a saysdf.status hook (update-status tick) records the active pane id of the window: O(1),
-- no window/process calls (rule 5). The agent is looked up only on a hand-off request, from the
-- cached foreground name.
--
-- Options (saysdf.setup({ annotations = {...} })):
--   enabled = true
--   agent_processes = { "claude" }        -- foreground process basenames
--   agent_titles = { "Claude Code" }      -- fallback: pane title contains
--   palette = true                        -- palette/F9 entries (list, hand folder, viewer keys)
--   phrase_file / phrase_folder           -- text with {path}; default: localized UI strings
--
-- API: handoff(window, pane_from, kind, path) -> true | nil, reason; find_agent(window) -> pane|nil;
--      mru(window) -> { pane_id... }; handle_user_var(window, pane, name, value) -> bool

local M = {}

M.VAR = "SAYSDF_VIEWER"
M.MRU_MAX = 16

local t, _, N_ = require("saysdf.i18n").domain("annotations")

local opts = nil
local mru_by_window = {}
local handler_registered = false
local selector_action = nil
local pending = {} -- window id -> text for the selector
local channel = nil

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

local function wid(window)
  local ok, id = pcall(function()
    return window:window_id()
  end)
  return ok and id or 0
end

local function default_opts(o)
  o = o or {}
  return {
    enabled = o.enabled ~= false,
    agent_processes = o.agent_processes or { "claude" },
    agent_titles = o.agent_titles or { "Claude Code" },
    phrase_file = o.phrase_file,
    phrase_folder = o.phrase_folder,
    palette = o.palette ~= false,
  }
end

-- Status tick: remember the active pane of the window (most recent first).
function M.record(window, pane)
  if pane == nil then
    return
  end
  local okp, id = pcall(function()
    return pane:pane_id()
  end)
  if not okp or id == nil then
    return
  end
  local k = wid(window)
  local list = mru_by_window[k]
  if list == nil then
    list = {}
    mru_by_window[k] = list
  end
  if list[1] == id then
    return
  end
  for i = #list, 1, -1 do
    if list[i] == id then
      table.remove(list, i)
    end
  end
  table.insert(list, 1, id)
  while #list > M.MRU_MAX do
    table.remove(list)
  end
end

function M.mru(window)
  return mru_by_window[wid(window)] or {}
end

local function window_panes(window)
  local out = {}
  pcall(function()
    for _, tab in ipairs(window:mux_window():tabs()) do
      for _, p in ipairs(tab:panes()) do
        table.insert(out, p)
      end
    end
  end)
  return out
end

local function is_agent(pane)
  local o = opts or default_opts()
  local base = util().process_basename(util().pane_foreground(pane).process)
  if base then
    for _, name in ipairs(o.agent_processes) do
      if util().process_basename(tostring(name)) == base then
        return true
      end
    end
  end
  local okt, title = pcall(function()
    return pane:get_title()
  end)
  if okt and type(title) == "string" then
    for _, pat in ipairs(o.agent_titles) do
      if title:find(tostring(pat), 1, true) then
        return true
      end
    end
  end
  return false
end

-- The most recently active agent pane of the window's MRU; else the only agent pane; else nil.
function M.find_agent(window, exclude_id)
  local by_id, order = {}, {}
  for _, p in ipairs(window_panes(window)) do
    local ok, id = pcall(function()
      return p:pane_id()
    end)
    if ok then
      by_id[id] = p
    end
  end
  local seen = {}
  for _, id in ipairs(M.mru(window)) do
    if by_id[id] then
      table.insert(order, id)
      seen[id] = true
    end
  end
  -- the most recently active agent pane of the MRU wins
  for _, id in ipairs(order) do
    if id ~= exclude_id and is_agent(by_id[id]) then
      return by_id[id]
    end
  end
  -- none in the MRU: a lone agent pane of the window is used, several -> the caller asks
  local found = nil
  local ids = {}
  for id in pairs(by_id) do
    if not seen[id] and id ~= exclude_id then
      table.insert(ids, id)
    end
  end
  table.sort(ids)
  for _, id in ipairs(ids) do
    if is_agent(by_id[id]) then
      if found then
        return nil
      end
      found = by_id[id]
    end
  end
  return found
end

local function norm(p)
  p = tostring(p):gsub("\\", "/")
  p = p:gsub("^/(%a:/)", "%1")
  return p
end

-- Path relative to the target pane's cwd when it is inside it.
local function display_path(pane, path)
  local ok, cwd = pcall(function()
    return pane:get_current_working_dir()
  end)
  local dir = nil
  if ok and cwd then
    dir = type(cwd) == "table" and cwd.file_path or tostring(cwd)
  end
  if type(dir) ~= "string" or dir == "" then
    return path
  end
  local d = norm(dir):gsub("/+$", "")
  local p = norm(path)
  local dl, pl = d, p
  if util().is_windows() then
    dl, pl = d:lower(), p:lower()
  end
  if pl:sub(1, #dl + 1) == dl .. "/" and #p > #d + 1 then
    return p:sub(#d + 2)
  end
  return path
end

function M.phrase(kind, path)
  local o = opts or default_opts()
  local p = path
  if p:find("%s") then
    p = '"' .. p .. '"'
  end
  local folder = kind == "handoff-folder"
  local custom = folder and o.phrase_folder or o.phrase_file
  if type(custom) ~= "string" or custom == "" then
    if folder then
      return t("Review the annotations in folder {path}", { path = p })
    end
    return t("Review the annotations in {path}", { path = p })
  end
  return (custom:gsub("{path}", function()
    return p
  end))
end

local function paste_into(window, target, text)
  target:send_paste(text) -- no Enter: the user reviews and sends
  pcall(function()
    target:activate()
  end)
  util().notify(window, "saysdfTerm", t("Annotations handed to the agent pane"))
end

-- One action_callback for the whole session (it registers an event per call, rule 5).
local function get_selector()
  if selector_action == nil then
    local w = wezterm()
    local cb = w.action_callback(function(win, _, id)
      local text = pending[wid(win)]
      pending[wid(win)] = nil
      if id == nil or text == nil then
        return
      end
      for _, p in ipairs(window_panes(win)) do
        local ok, pid = pcall(function()
          return p:pane_id()
        end)
        if ok and tostring(pid) == tostring(id) then
          paste_into(win, p, text)
          return
        end
      end
    end)
    selector_action = function(choices)
      return w.action.InputSelector({
        title = t("Choose the agent pane"),
        choices = choices,
        action = cb,
      })
    end
  end
  return selector_action
end

local function exists(kind, path)
  if kind == "handoff-folder" then
    local ok, list = pcall(function()
      return wezterm().read_dir(path)
    end)
    return ok and list ~= nil
  end
  local f = io.open(path, "rb")
  if f then
    f:close()
    return true
  end
  return false
end

-- A path with control characters (newline, ESC...) could carry commands into the agent.
local function path_ok(path)
  return type(path) == "string" and path ~= "" and #path <= 2048 and not path:find("%c")
end

function M.handoff(window, from_pane, kind, path)
  if kind ~= "handoff-file" and kind ~= "handoff-folder" then
    return nil, "kind"
  end
  if not path_ok(path) then
    return nil, "path"
  end
  if not exists(kind, path) then
    return nil, "path does not exist"
  end
  local from_id = nil
  pcall(function()
    from_id = from_pane:pane_id()
  end)
  local target = M.find_agent(window, from_id)
  if target then
    paste_into(window, target, M.phrase(kind, display_path(target, path)))
    return true
  end
  -- no agent pane: choose one of the window's panes
  local choices = {}
  for _, p in ipairs(window_panes(window)) do
    local ok, pid = pcall(function()
      return p:pane_id()
    end)
    if ok and pid ~= from_id then
      local okt, title = pcall(function()
        return p:get_title()
      end)
      table.insert(choices, { id = tostring(pid), label = tostring(pid) .. ": " .. util().single_line(okt and title or "") })
    end
  end
  if #choices == 0 then
    util().notify(window, "saysdfTerm", t("No pane to hand the annotations to"))
    return nil, "no panes"
  end
  pending[wid(window)] = M.phrase(kind, path)
  window:perform_action(get_selector()(choices), from_pane)
  return true
end

-- The viewer registry (saysdf.viewer, step L-2): record of a viewer pane or nil.
local function lookup(pane)
  local ok, viewer = pcall(require, "saysdf.viewer")
  if not ok or type(viewer) ~= "table" or type(viewer.viewer_record) ~= "function" then
    return nil
  end
  local okr, rec = pcall(viewer.viewer_record, pane)
  return okr and rec or nil
end

local function kind_handler(kind)
  return function(window, pane, rec, payload, value)
    if not path_ok(payload) then
      return nil, "path"
    end
    if not exists(kind, payload) then
      return nil, "path does not exist"
    end
    local fresh, why = require("saysdf.bridge").consume(rec, value)
    if not fresh then
      return nil, why
    end
    if M.handoff(window, pane, kind, payload) then
      return true
    end
    return nil, "handoff failed"
  end
end

local function get_channel()
  if channel == nil then
    channel = require("saysdf.bridge").channel({
      var = M.VAR,
      name = "saysdf.annotations",
      lookup = lookup,
      not_owned = "not a viewer pane",
      kinds = {
        ["handoff-file"] = kind_handler("handoff-file"),
        ["handoff-folder"] = kind_handler("handoff-folder"),
      },
    })
  end
  return channel
end

-- Palette / F9 entries (design D5, D10). Never an own augment-command-palette handler.
M.TOOL = "annotations"
M.OPEN_AT = "viewer.open_at"

-- Help lines of "Viewer keys": English source strings (ru in the catalog), one choice each.
local KEY_HELP = {
  N_("Shift+arrows or mouse drag — select text; Ctrl+Space — selection mode on/off; Esc — clear"),
  N_("Ctrl: R substitute · D delete · N insert · K comment (on a selection)"),
  N_("Alt+Down / Alt+Up — next / previous annotation; Ctrl: E amend · X remove (on an annotation)"),
  N_("Ctrl: G hand the file to the agent · L annotations of the project folder · F diff against HEAD · W wrap"),
  N_("Markdown: Ctrl+S — source / rendered view; ] / [ — next / previous heading"),
  N_("Input: Enter — save; Ctrl+J or Alt+Enter — new line; Esc — cancel"),
}

function M.help_choices()
  local out = {}
  for i, line in ipairs(KEY_HELP) do
    out[i] = { id = tostring(i), label = t(line) }
  end
  return out
end

-- Project root for the list/hand-off: git top level of the pane cwd, else the cwd.
local function project_root(pane)
  local okg, git = pcall(require, "saysdf.git")
  local root = okg and git.root_of and git.root_of(pane) or nil
  if root and root ~= "" then
    return root
  end
  local okf, files = pcall(require, "saysdf.files")
  return okf and files.cwd_of and files.cwd_of(pane) or nil
end

-- Argument of the viewer.open_at sidebar action: "<absolute path>:<line>" (the path may hold ':').
function M.parse_open_at(arg)
  if type(arg) ~= "string" then
    return nil
  end
  local path, line = arg:match("^(.+):(%d+)$")
  line = tonumber(line)
  if not path or not line or line < 1 or line > 100000000 or not path_ok(path) then
    return nil
  end
  if not (path:match("^/") or path:match("^%a:[/\\]")) then
    return nil
  end
  return path, line
end

function M.open_list(window, pane)
  local u = util()
  local root = project_root(pane)
  if not root then
    u.notify(window, "saysdfTerm", t("Unknown folder of the pane"))
    return false
  end
  local bin = u.find_bin("saysdf")
  if not bin then
    u.notify_missing_bin(window, "saysdf", t("Annotations"))
    return false
  end
  -- CLI per design D10: `saysdf annotations <root>` (step E-2); a picker run as a sidebar tool.
  return require("saysdf.sidebar").open(window, pane, {
    tool = M.TOOL,
    args = { "annotations", root },
    multi = true,
  })
end

function M.handoff_folder(window, pane)
  local root = project_root(pane)
  if not root then
    util().notify(window, "saysdfTerm", t("Unknown folder of the pane"))
    return nil
  end
  return M.handoff(window, pane, "handoff-folder", root)
end

local function add_entries(w)
  local palette = require("saysdf.palette")
  palette.add({
    id = "annotations.list", menu = "Tools", order = 60, label = t("Annotations in project…"), accel = "a",
    brief = t("Annotations: list in project folder"), icon = "md_comment_text_multiple",
    action = w.action_callback(M.open_list),
  })
  palette.add({
    id = "annotations.handoff_folder", menu = "Tools", order = 61, label = t("Hand annotations to the agent"), accel = "g",
    brief = t("Annotations: hand folder to agent"), icon = "md_robot",
    action = w.action_callback(M.handoff_folder),
  })
  palette.add({
    id = "annotations.keys", menu = "Help", order = 20, label = t("Viewer keys"), accel = "k",
    brief = t("Viewer keys: selection and annotations"), icon = "md_keyboard",
    action = w.action.InputSelector({
      title = t("Viewer keys (Esc closes)"),
      choices = M.help_choices(),
      action = w.action_callback(function() end),
    }),
  })
  -- Sidebar channel action (not in the palette): the list opens a file in the shared viewer.
  palette.add({
    id = M.OPEN_AT, palette = false, sidebar = true, brief = M.OPEN_AT,
    action = w.action_callback(function() end),
    accept = function(arg)
      return M.parse_open_at(arg) ~= nil
    end,
    sidebar_run = function(window, origin, arg)
      local path, line = M.parse_open_at(arg)
      if path and exists("handoff-file", path) then
        require("saysdf.viewer").open(window, origin, path, line)
      end
    end,
  })
end

function M.handle_user_var(window, pane, name, value)
  return get_channel().handle(window, pane, name, value)
end

function M.setup(o, config)
  opts = default_opts(o)
  if not opts.enabled then
    return config
  end
  if not handler_registered then
    handler_registered = true
    wezterm().on("user-var-changed", function(window, pane, name, value)
      if name == M.VAR then
        M.handle_user_var(window, pane, name, value)
      end
    end)
    require("saysdf.status").hook("annotations", M.record)
  end
  if opts.palette then
    add_entries(wezterm())
  end
  return config
end

-- For tests.
function M._reset()
  opts, mru_by_window, handler_registered, selector_action, pending, channel = nil, {}, false, nil, {}, nil
end

return M
