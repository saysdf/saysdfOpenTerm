-- saysdfTerminal: startup banner and help on F1 (change startup-help).
--
-- The banner is a temporary fork bar `banner` outside the pty (not in history or scrollback):
-- window:saysdf_set_bar("banner", { left = { { text = "line 1" }, ... }, right = {} }) --
-- one left element = one line. Shown once per window on every launch (a flag in
-- wezterm.GLOBAL by window_id; a config reload does not repeat it); Rust hides it (Esc, click,
-- first input, timer). While the GUI has no bars there is no banner.
-- F1 runs `saysdf help` in a split; if a TUI is in the foreground, F1 goes to the program.
--
-- Options of saysdf.setup({ help = false | {...} }):
--   banner = true                       -- false: no banner
--   key = { key = "F1", mods = "NONE" } -- false: no F1 binding
--   tui_processes = { ... }             -- as in saysdf.menu
--
-- Pure function (test via mlua): banner_lines(opts, entries) -> up to 5 lines.

local t = require("saysdf.i18n").domain("help")

local M = {}

M.MAX_LINES = 5

local state = { opts = {} }

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

local function find(entries, id)
  for _, e in ipairs(entries or {}) do
    if e.id == id then
      return e
    end
  end
  return nil
end

function M.banner_lines(opts, entries)
  opts = opts or {}
  local version = wezterm().version
  if version == nil or version == "" then
    version = "(version unknown)"
  end
  local lines = { "saysdfTerminal " .. version, t("saysdfTerm is a terminal with git, SSH, databases, a monitor and a file viewer.") }
  local parts = {}
  local function add(id, text)
    local e = find(entries, id)
    if e and e.key then
      table.insert(parts, e.key .. " — " .. text)
    end
  end
  add("menu.open", t("menu"))
  add("help.full", t("full help"))
  add("core.palette", t("command palette"))
  if #parts > 0 then
    table.insert(lines, table.concat(parts, " · "))
  end
  parts = {}
  add("git.menu", "git")
  add("ssh.connect", "SSH")
  add("tabs.rename", t("tab name"))
  if #parts > 0 then
    table.insert(lines, table.concat(parts, " · "))
  end
  table.insert(lines, t("Hints at the bottom change with the context. Esc closes this."))
  table.insert(lines, t("Turn the banner off: help = {{ banner = false }} in saysdf.setup."))
  while #lines > M.MAX_LINES do
    table.remove(lines)
  end
  return lines
end

local function window_id(window)
  local ok, id = pcall(function()
    return window:window_id()
  end)
  return ok and id or nil
end

function M.maybe_banner(window, pane)
  if state.opts.banner == false or window == nil or window.saysdf_set_bar == nil then
    return false
  end
  local wid = window_id(window)
  if wid == nil then
    return false
  end
  local g = wezterm().GLOBAL
  local shown = g.saysdf_banner_shown or {}
  if shown[tostring(wid)] then
    return false
  end
  shown[tostring(wid)] = true
  g.saysdf_banner_shown = shown
  local left = {}
  for _, l in ipairs(M.banner_lines(state.opts, require("saysdf.palette").entries(window, pane))) do
    table.insert(left, { text = l })
  end
  window:saysdf_set_bar("banner", { left = left, right = {} })
  return true
end

function M.open(window, pane)
  local bin = util().find_bin("saysdf")
  if not bin then
    util().log_warn("saysdf.help: the saysdf binary was not found; see `saysdfTerm-cli show-module help`")
    util().notify(window, "saysdfTerm", t("saysdf not found; help is unavailable"))
    return false
  end
  pane:split({ args = { bin, "help" }, direction = "Right", size = 0.5 })
  return true
end

function M.setup(opts, config)
  opts = opts or {}
  state = { opts = opts }
  local w = wezterm()
  local palette = require("saysdf.palette")
  local key = opts.key == nil and { key = "F1", mods = "NONE" } or opts.key
  palette.add({
    id = "help.full", menu = "Help", order = 1, label = t("Full help"), accel = "h",
    key = key and palette.key_text(key) or nil,
    brief = t("saysdfTerm help (saysdf help)"), icon = "md_help_circle",
    hint = { contexts = { "shell", "git", "ssh", "selection" }, label = t("Help"), priority = 2, always = true },
    action = w.action_callback(function(window, pane)
      M.open(window, pane)
    end),
  })
  if key then
    config.keys = config.keys or {}
    table.insert(config.keys, {
      key = key.key, mods = key.mods,
      action = w.action_callback(function(window, pane)
        local info = util().pane_foreground(pane)
        if util().foreground_is_tui(info, opts.tui_processes) then
          window:perform_action(w.action.SendKey({ key = key.key }), pane)
        else
          M.open(window, pane)
        end
      end),
    })
  end
  if opts.banner ~= false then
    require("saysdf.status").hook("help", function(window, pane)
      M.maybe_banner(window, pane)
    end)
  end
  return config
end

M._state = function()
  return state
end

return M
