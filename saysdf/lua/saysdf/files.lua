-- saysdfTerminal: the `saysdf files` file browser in the sidebar (change file-browser, D1, D4, D6).
--
-- The tree, git marks and filesystem operations live in the `saysdf files` process (the GUI thread
-- does not wait). Here:
--   * Ctrl+Shift+B toggles the browser in the sidebar (saysdf.sidebar); with the sidebar disabled
--     it is a plain split on the right (without GUI actions: they have no channel);
--   * the menu entry "File -> File browser" (saysdf.palette);
--   * actions from the browser through the sidebar channel (user var SAYSDF_SIDEBAR, kind action):
--       files.open      -- open a file in the built-in viewer (saysdf.viewer: never executes it);
--       files.cd_pane   -- a folder as the cwd of a new pane;
--       files.copy_path -- the path to the clipboard;
--     the argument is `<seq>:<base64 of the path>`; the path is accepted only if it is absolute,
--     has no `..`, exists and lies under one of this tab's browser roots (the opening root and the
--     cwds passed by the sync); for cd_pane it must be a folder, for open a file (validate);
--   * cwd sync (D6): in saysdf.status.defer, the cwd of the active non-sidebar pane of a tab with
--     an open browser; on change (an in-memory string comparison) write_atomic to
--     runtime_dir()/files-<id>.cwd; the browser watches the file itself (watch.rs).
-- Options: key = Ctrl+Shift+B (false = no hotkey), palette = true, sync = true, width (as sidebar).

local M = {}

local t, _, N_ = require("saysdf.i18n").domain("files")

M.TOOL = "files"
M.MAX_ROOTS = 32
M.DEFAULTS = {
  key = { key = "B", mods = "CTRL|SHIFT" },
  palette = true,
  sync = true,
}

local opts = nil
local by_tab = {} -- [tab_id] = { pane_id, roots = { ... }, cwd_file, last_cwd }
local counter = 0

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

local function merge(o)
  local r = {}
  for k, v in pairs(M.DEFAULTS) do
    r[k] = v
  end
  for k, v in pairs(o or {}) do
    r[k] = v
  end
  return r
end

local function tab_id_of(pane)
  local ok, id = pcall(function()
    return pane:tab():tab_id()
  end)
  return ok and id or nil
end

local function pane_id(p)
  local ok, id = pcall(function()
    return p:pane_id()
  end)
  return ok and id or nil
end

local function is_local(pane)
  local ok, d = pcall(function()
    return pane:get_domain_name()
  end)
  return not ok or d == "local"
end

-- The pane's cwd as a local path (OSC 7 file://); nil if unknown or not a local domain.
function M.cwd_of(pane)
  if not is_local(pane) then
    return nil
  end
  local ok, cwd = pcall(function()
    return pane:get_current_working_dir()
  end)
  if not ok or cwd == nil then
    return nil
  end
  local path = require("saysdf.tabs").cwd_path(cwd)
  if type(path) ~= "string" or path == "" then
    return nil
  end
  return (path:gsub("^/(%a:)", "%1"))
end

-- Normalization for comparison: `/` separators, no trailing `/`, case-insensitive on Windows.
function M.norm(p, windows)
  p = p:gsub("\\", "/")
  local lead = p:match("^//") and "//" or ""
  p = lead .. p:sub(#lead + 1):gsub("/+", "/")
  if #p > 1 and p:sub(-1) == "/" and not p:match("^%a:/$") then
    p = p:sub(1, -2)
  end
  if windows then
    p = p:lower()
  end
  return p
end

local function is_absolute(p)
  return p:match("^/") ~= nil or p:match("^%a:[/\\]") ~= nil or p:match("^\\\\") ~= nil
end

local function default_exists(path)
  return require("saysdf.paths").exists(path)
end

-- Pure check of an action path. want: "dir" | "file" | nil. exists(path) -> ok, is_dir.
-- Returns path, or nil and a reason (an English catalog key; shown through t()).
function M.validate(roots, path, want, exists, windows)
  if type(path) ~= "string" or path == "" or path:find("%z") or path:find("[\r\n]") then
    return nil, N_("path")
  end
  if not is_absolute(path) then
    return nil, N_("not absolute")
  end
  local n = M.norm(path, windows)
  for part in n:gmatch("[^/]+") do
    if part == ".." or part == "." then
      return nil, ".."
    end
  end
  local under = false
  for _, r in ipairs(roots or {}) do
    local nr = M.norm(r, windows)
    local pref = nr:sub(-1) == "/" and nr or (nr .. "/")
    if n == nr or n:sub(1, #pref) == pref then
      under = true
      break
    end
  end
  if not under then
    return nil, N_("outside the root")
  end
  local ok, is_dir = (exists or default_exists)(path)
  if not ok then
    return nil, N_("does not exist")
  end
  if want == "dir" and not is_dir then
    return nil, N_("not a folder")
  end
  if want == "file" and is_dir then
    return nil, N_("is a folder")
  end
  return path
end

local B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
local B64_IDX = {}
for i = 1, #B64 do
  B64_IDX[B64:byte(i)] = i - 1
end

function M.b64decode(s)
  if type(s) ~= "string" or #s % 4 ~= 0 then
    return nil
  end
  local out = {}
  for i = 1, #s, 4 do
    local a, b, c, d = s:byte(i, i + 3)
    local va, vb = B64_IDX[a], B64_IDX[b]
    local vc = c == 61 and 0 or B64_IDX[c]
    local vd = d == 61 and 0 or B64_IDX[d]
    if va == nil or vb == nil or vc == nil or vd == nil then
      return nil
    end
    if (c == 61 or d == 61) and i + 3 ~= #s then
      return nil
    end
    local n = ((va * 64 + vb) * 64 + vc) * 64 + vd
    local b1, b2, b3 = math.floor(n / 65536) % 256, math.floor(n / 256) % 256, n % 256
    table.insert(out, string.char(b1))
    if c ~= 61 then
      table.insert(out, string.char(b2))
    end
    if d ~= 61 then
      table.insert(out, string.char(b3))
    end
  end
  return table.concat(out)
end

-- Action argument: `<seq>:<base64 of the path>` -> the path or nil.
function M.parse_arg(arg)
  local seq, b64 = tostring(arg or ""):match("^(%d+):([A-Za-z0-9+/=]+)$")
  if not seq then
    return nil
  end
  return M.b64decode(b64)
end

-- The tab's record, if this browser is still open in it.
function M.state_for(tid)
  local st = tid and by_tab[tid]
  if not st then
    return nil
  end
  local sst = require("saysdf.sidebar").state_for(tid)
  if not sst or sst.pane_id ~= st.pane_id or sst.tool ~= M.TOOL then
    by_tab[tid] = nil
    return nil
  end
  return st
end

local function add_root(st, dir)
  for _, r in ipairs(st.roots) do
    if r == dir then
      return
    end
  end
  if #st.roots >= M.MAX_ROOTS then
    table.remove(st.roots, 2) -- the first one is the opening root
  end
  table.insert(st.roots, dir)
end

-- Check an action for the tab of the origin pane. -> path, or nil and a reason.
function M.check(origin, arg, want, exists)
  local path = M.parse_arg(arg)
  if not path then
    return nil, N_("argument")
  end
  local st = M.state_for(tab_id_of(origin))
  if not st then
    return nil, N_("browser not open")
  end
  return M.validate(st.roots, path, want, exists, util().is_windows())
end

local function rejected(window, why)
  util().log_info("saysdf.files: action rejected: " .. tostring(why))
  util().notify(window, t("Files"), t("action rejected: {why}", { why = t(tostring(why)) }))
end

local ACTIONS = {
  { id = "files.open", want = "file", run = function(window, origin, path)
    require("saysdf.viewer").open(window, origin, path)
  end },
  { id = "files.cd_pane", want = "dir", run = function(_, origin, path)
    origin:split({ cwd = path, direction = "Bottom" })
  end },
  { id = "files.copy_path", want = nil, run = function(window, _, path)
    window:copy_to_clipboard(path, "Clipboard")
  end },
}

local function bin()
  return util().find_bin_cached("saysdf")
end

-- Open the browser for a pane. true = opened.
function M.open(window, pane)
  local exe = bin()
  if not exe then
    util().notify_missing_bin(window, "saysdf", t("Files"))
    return false
  end
  local root = M.cwd_of(pane) or wezterm().home_dir
  local sidebar = require("saysdf.sidebar")
  if sidebar.options().enabled then
    local u = util()
    counter = counter + 1
    local dir = u.runtime_dir()
    u.ensure_dir(dir)
    local cwd_file = u.join(dir, "files-" .. u.hash(tostring(os.time()) .. "|" .. counter .. "|" .. tostring({})):gsub("-", "") .. ".cwd")
    local ok = sidebar.open(window, pane, {
      tool = M.TOOL,
      args = { "files", "--cwd-file", cwd_file, root },
      multi = true,
    })
    if ok then
      local tid = tab_id_of(pane)
      local sst = tid and sidebar.state_for(tid)
      if sst then
        by_tab[tid] = { pane_id = sst.pane_id, roots = { root }, cwd_file = cwd_file, last_cwd = root }
      end
      return true
    end
  end
  -- without the sidebar: a plain split on the right, GUI actions unavailable (no channel)
  pane:split({ args = { exe, "files", root }, direction = "Right", size = 0.3 })
  return true
end

function M.toggle(window, pane)
  local sidebar = require("saysdf.sidebar")
  if not sidebar.options().enabled then
    return M.open(window, pane)
  end
  local r = sidebar.toggle(window, pane, M.TOOL, function()
    return M.open(window, pane)
  end)
  if r == "closed" then
    by_tab[tab_id_of(pane)] = nil
  end
  return r
end

-- cwd sync (D6): called from status.defer with the window's active pane.
-- true = a new cwd was written.
function M.sync(window, pane)
  local tid = tab_id_of(pane)
  local st = tid and by_tab[tid]
  if not st then
    return false -- fast exit: the tab has no browser
  end
  if pane_id(pane) == st.pane_id then
    return false
  end
  local cwd = M.cwd_of(pane)
  if not cwd or cwd == st.last_cwd then
    return false
  end
  if not M.state_for(tid) then
    return false
  end
  st.last_cwd = cwd
  add_root(st, cwd)
  local ok, err = util().write_atomic(st.cwd_file, cwd)
  if not ok then
    util().log_warn("saysdf.files: " .. tostring(err))
  end
  return ok
end

function M.setup(o, config)
  opts = merge(o)
  local w = wezterm()
  local cb = w.action_callback(function(window, pane)
    M.toggle(window, pane)
  end)
  if opts.key then
    config.keys = config.keys or {}
    table.insert(config.keys, { key = opts.key.key, mods = opts.key.mods, action = cb })
  end
  local palette = require("saysdf.palette")
  if opts.palette then
    palette.add({
      id = "files.toggle", menu = "File", order = 30, label = t("File browser"), accel = "b",
      brief = t("Files: file browser"), icon = "md_folder_open",
      key = opts.key and palette.key_text(opts.key) or nil, action = cb,
    })
  end
  -- Sidebar channel actions: not in the palette or the menu (palette = false, no menu).
  for _, a in ipairs(ACTIONS) do
    palette.add({
      id = a.id, palette = false, sidebar = true, brief = a.id,
      action = w.action_callback(function() end),
      accept = function(arg)
        return M.parse_arg(arg) ~= nil
      end,
      sidebar_run = function(window, origin, arg)
        local path, why = M.check(origin, arg, a.want)
        if not path then
          return rejected(window, why)
        end
        a.run(window, origin, path)
      end,
    })
  end
  if opts.sync then
    local status = require("saysdf.status")
    status.hook("files", function(window, pane)
      -- only a string comparison on the tick; the write happens on change, after the status (rule 5)
      if next(by_tab) == nil then
        return
      end
      status.defer(function()
        M.sync(window, pane)
      end)
    end)
  end
  return config
end

-- For tests.
function M._reset()
  opts, by_tab, counter = nil, {}, 0
end

function M._state()
  return by_tab
end

return M
