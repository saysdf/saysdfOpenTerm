-- saysdfTerminal: shared aggregator of command palette entries.
--
-- Upstream calls only the FIRST handler of the `augment-command-palette` event, so modules that
-- registered their entries separately would displace each other. saysdf modules add their
-- entries here, and the single handler returns them all.
--
-- API:
--   require("saysdf.palette").add(entry_or_fn)
--       entry is a palette entry { brief, icon?, action, doc?, ... };
--       fn(window, pane) -> list of entries (or nil).
--   require("saysdf.palette").entries(window, pane) -> shared list of entries (errors of the
--       sources are logged and skipped).
--       If the content did not change since the previous call (signature of the visible
--       fields), the same list object is returned and version() does not grow: consumers on the
--       update-status tick (menu, hotbar) skip rebuilding by version() (perf-2).
--   require("saysdf.palette").version() -> version number of the entry list
--   require("saysdf.palette").by_id(window, pane, id) -> entry or nil
--   require("saysdf.palette").key_text({ key = "S", mods = "CTRL|SHIFT" }) -> "Ctrl+Shift+S"
--
-- Single command registry (change menu-bar D4): the palette, the menu (saysdf.menu) and the
-- status bar (saysdf.hotbar) show the same entries. Optional entry fields:
--   id    = "<module>.<name>"   -- for bar actions (event saysdf-bar-action)
--   menu  = "File" | "Tabs" | "Git" | "Tools" | "View" | "Help"   (ids; titles are translated by saysdf.menu)
--   label = "Commit…"           -- caption in the menu (brief otherwise)
--   accel = "c"                 -- required with menu: physical key of the item in the open menu
--                               -- (the language's catalog may override it: saysdf.menu applies
--                               -- i18n.accel(id) to free slots when it builds the menu)
--   order = N                   -- order inside the menu (default 100)
--   key   = "Ctrl+Shift+S"      -- hotkey text to show (from the same table as the binding)
--   hint  = { contexts = { "git", "shell" }, label = "Git", priority = 10 }  -- status bar
--   palette = false             -- do not show in the palette (menu/status bar only)
-- Only brief/doc/icon/action go to the upstream palette.
--
-- The user's own handler: if the config already has `wezterm.on("augment-command-palette", ...)`
-- registered EARLIER, upstream calls only it, and the palette handler does not run.
-- Then merge by hand:
--   wezterm.on("augment-command-palette", function(window, pane)
--     local list = { { brief = "Mine", action = ... } }
--     for _, e in ipairs(require("saysdf.palette").entries(window, pane)) do table.insert(list, e) end
--     return list
--   end)
-- The module registers its handler lazily, on the first add(), exactly once.
--
-- Event saysdf-bar-action (clicks on the menu/status bars): upstream calls the handlers in
-- registration order, and a handler that returns false stops the rest. A user's own
-- wezterm.on("saysdf-bar-action", ...) registered earlier that returns false disables all bar
-- actions; do not return false from such a handler.

local M = {}

local sources = {}
local static_taken = {}
local notified = {}
local memo = { sig = nil, list = nil, version = 0 }
local registered = false

local function wezterm()
  return require("wezterm")
end

local function claim(taken, e)
  if type(e) ~= "table" or e.menu == nil then
    return nil
  end
  local id, menu = tostring(e.id), tostring(e.menu)
  if type(e.accel) ~= "string" or e.accel == "" then
    return string.format(
      "saysdf.palette.add: entry %q in menu %q has no accel (menu entries need an explicit physical key, e.g. accel = \"q\")",
      id, menu)
  end
  local slot = menu .. "\1" .. e.accel
  if taken[slot] and taken[slot] ~= id then
    return string.format("saysdf.palette.add: entry %q in menu %q: accel %q is already taken by entry %q",
      id, menu, e.accel, taken[slot])
  end
  taken[slot] = id
  return nil
end

local function release(taken, e)
  if type(e) == "table" and e.menu ~= nil and type(e.accel) == "string" then
    local slot = tostring(e.menu) .. "\1" .. e.accel
    if taken[slot] == tostring(e.id) then
      taken[slot] = nil
    end
  end
end

local function report_rejected(window, e, err)
  local key = tostring(e.id)
  if notified[key] then
    return
  end
  notified[key] = true
  local util = require("saysdf.util")
  util.log_error(err)
  local t = require("saysdf.i18n").domain("palette")
  util.notify(window, t("saysdfTerm: menu entry rejected"), err)
end

function M.entries(window, pane)
  local out = {}
  local dyn = {} -- entries of function sources: only they change without add() (perf-2)
  local taken = setmetatable({}, { __index = static_taken })
  for _, src in ipairs(sources) do
    if type(src) == "function" then
      local ok, list = pcall(src, window, pane)
      if ok then
        for _, e in ipairs(list or {}) do
          local err = claim(taken, e)
          if err then
            report_rejected(window, e, err)
          else
            table.insert(out, e)
            table.insert(dyn, e)
          end
        end
      else
        require("saysdf.util").log_error("saysdf.palette: " .. tostring(list))
      end
    else
      table.insert(out, src)
    end
  end
  -- Static entries do not change after add() (add resets the memo): the signature has only their
  -- count and the order of sources; the full signature covers the dynamic ones.
  local sig = #out .. "#" .. M.signature(dyn)
  if sig == memo.sig and memo.list ~= nil then
    return memo.list
  end
  memo.sig, memo.list, memo.version = sig, out, memo.version + 1
  return out
end

-- Signature of the visible fields of entries (what the palette, menu and status bar show).
function M.signature(list)
  local parts = {}
  for i, e in ipairs(list) do
    local h = type(e.hint) == "table" and e.hint or nil
    local hc = h and type(h.contexts) == "table" and table.concat(h.contexts, ",") or ""
    parts[i] = table.concat({
      tostring(e.id), tostring(e.brief), tostring(e.label), tostring(e.menu), tostring(e.order),
      tostring(e.key), tostring(e.icon), tostring(e.palette), tostring(e.doc),
      h and (hc .. ";" .. tostring(h.label) .. ";" .. tostring(h.priority) .. ";" .. tostring(h.always)) or "",
    }, "\1")
  end
  return table.concat(parts, "\2")
end

function M.version()
  return memo.version
end

function M.by_id(window, pane, id)
  if id == nil then
    return nil
  end
  for _, e in ipairs(M.entries(window, pane)) do
    if e.id == id then
      return e
    end
  end
  return nil
end

local MOD_NAMES = { CTRL = "Ctrl", SHIFT = "Shift", ALT = "Alt", SUPER = "Super", META = "Alt", CMD = "Cmd" }
local KEY_NAMES = { UpArrow = "Up", DownArrow = "Down", LeftArrow = "Left", RightArrow = "Right" }

function M.key_text(b)
  if type(b) ~= "table" or b.key == nil then
    return nil
  end
  local parts = {}
  for m in tostring(b.mods or ""):gmatch("[^|%s]+") do
    if m:upper() ~= "NONE" then -- mods = "NONE" means no modifiers: "F9", not "NONE+F9"
      table.insert(parts, MOD_NAMES[m:upper()] or m)
    end
  end
  local k = tostring(b.key)
  table.insert(parts, KEY_NAMES[k] or (#k == 1 and k:upper() or k))
  return table.concat(parts, "+")
end

-- Entries for the upstream palette: only the UserPaletteEntry fields, without palette = false.
function M.palette_list(window, pane)
  local out = {}
  for _, e in ipairs(M.entries(window, pane)) do
    if e.palette ~= false then
      table.insert(out, { brief = e.brief, doc = e.doc, icon = e.icon, action = e.action })
    end
  end
  return out
end

-- Registers the single palette handler (idempotent).
function M.ensure_handler()
  if not registered then
    registered = true
    wezterm().on("augment-command-palette", function(window, pane)
      return M.palette_list(window, pane)
    end)
  end
end

function M.add(entry_or_fn)
  if entry_or_fn == nil then
    return
  end
  local replaced
  if type(entry_or_fn) == "table" and entry_or_fn.id ~= nil then
    for i, src in ipairs(sources) do
      if type(src) == "table" and src.id == entry_or_fn.id then
        replaced = i
        release(static_taken, src)
        break
      end
    end
  end
  if type(entry_or_fn) == "table" then
    local err = claim(static_taken, entry_or_fn)
    if err then
      if replaced then
        claim(static_taken, sources[replaced])
      end
      error(err, 0)
    end
  end
  if replaced then
    sources[replaced] = entry_or_fn
  else
    table.insert(sources, entry_or_fn)
  end
  memo.sig, memo.list = nil, nil
  M.ensure_handler()
end

-- Run a registry entry by id (actions of the menu/status bars). An unknown id is logged.
function M.run(window, pane, id)
  local e = M.by_id(window, pane, id)
  if not e or e.action == nil then
    require("saysdf.util").log_warn("saysdf.palette: no command with id '" .. tostring(id) .. "'")
    return false
  end
  window:perform_action(e.action, pane)
  return true
end

-- The single handler of the bar event `saysdf-bar-action(window, pane, id)` (menu-bar D3);
-- saysdf.menu and saysdf.hotbar enable it, it is registered once.
M.BAR_ACTION_EVENT = "saysdf-bar-action"
local bar_registered = false
function M.ensure_bar_action()
  if bar_registered then
    return
  end
  bar_registered = true
  wezterm().on(M.BAR_ACTION_EVENT, function(window, pane, id)
    local ok, err = pcall(M.run, window, pane, id)
    if not ok then
      require("saysdf.util").log_error("saysdf.palette: " .. tostring(err))
    end
  end)
end

-- For tests: reset the sources.
function M._reset()
  sources = {}
  static_taken = {}
  notified = {}
  memo = { sig = nil, list = nil, version = 0 }
  registered = false
  bar_registered = false
end

return M
