-- saysdfTerminal: top menu bar in the spirit of Norton Commander (change menu-bar).
--
-- The menu model is built from the single saysdf.palette registry (fields id, menu, label,
-- order, key): the palette and the menu show the same commands. Rust (the fork's bars,
-- saysdf_bars.rs) keeps only a data snapshot; choosing an item fires the event
-- `saysdf-bar-action(window, pane, id)` -> here the registry entry by id ->
-- window:perform_action(entry.action, pane).
--
-- Options of saysdf.setup({ menu = false | {...} }):
--   tui_processes = { ... }   -- processes that get F9 (default saysdf.util.TUI_PROCESSES)
--   key = { key = "F9", mods = "NONE" }            -- false: no binding
--   force_key = { key = "F9", mods = "CTRL|SHIFT" } -- open the menu even over a TUI; false: none
--
-- Bar data format (window:saysdf_set_bar("top", spec)):
--   spec = { left = { BarItem... }, right = {} }
--   BarItem = { text = "File", children = { { text = "Commit…", id = "git.commit", accel = "c", key = "Ctrl+Shift+S" } } }
--   accel is the physical key of the item (required, set by the registry entry), key is the
--   hotkey caption on the right (format: see wezterm-gui/src/termwindow/saysdf_bars.rs).
--
-- Pure functions (tests: mlua in saysdf-lua-bundle): build(entries, opts) -> tree,
-- to_bar(tree) -> spec, hash(spec).
--
-- Event saysdf-bar-action (clicks on the menu/status bars): upstream calls the handlers in
-- registration order, and a handler that returns false stops the rest. A user's own
-- wezterm.on("saysdf-bar-action", ...) registered earlier that returns false disables all bar
-- actions; do not return false from such a handler.

local M = {}

local t, _, N_ = require("saysdf.i18n").domain("menu")
local i18n = require("saysdf.i18n")

-- Menu ids in bar order; entries refer to them (palette.add{ menu = "Tools" }), the bar shows
-- their translations.
M.MENUS = { N_("File"), N_("Tabs"), N_("Git"), N_("Tools"), N_("View"), N_("Help") }
M.EVENT = "saysdf-bar-action"

local state = { opts = {}, last_hash = {} }

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

local override_warned = {}

-- Accelerator overrides of the language's catalog (table _accel, values already validated by
-- the loader): an override is applied only to a free slot of its menu, i.e. a key that no
-- other entry of the menu has in code or got from an earlier override; otherwise the code
-- default stays (security review 2026-10-21-i18n, L4).
local function apply_accel_overrides(items, accel_of)
  local function canon(spec)
    return type(spec) == "string" and (i18n.accel_key(spec) or spec) or nil
  end
  local used = {}
  for _, it in ipairs(items) do
    local k = canon(it.accel)
    if k then
      used[k] = true
    end
  end
  for _, it in ipairs(items) do
    local ok, o = pcall(accel_of, it.id)
    local k = ok and type(o) == "string" and i18n.accel_key(o) or nil
    if k and k ~= canon(it.accel) then
      if used[k] then
        local w = tostring(it.id) .. "\1" .. o
        if not override_warned[w] then
          override_warned[w] = true
          pcall(function()
            util().log_warn(string.format(
              "saysdf.menu: catalog accelerator %q of entry %q is taken in its menu; keeping %q",
              o, tostring(it.id), tostring(it.accel)))
          end)
        end
      else
        used[k] = true
        it.accel = o
      end
    end
  end
end

-- entries are registry entries; opts.menus is the menu order; opts.accel_of(id) -> key|nil
-- gives the catalog accelerators (default saysdf.i18n.accel). An entry without id or menu is
-- skipped. tree = { { title, items = { { label, id, key, accel } } } }; empty menus are hidden.
function M.build(entries, opts)
  opts = opts or {}
  local order = opts.menus or M.MENUS
  local accel_of = opts.accel_of or i18n.accel
  local by_menu = {}
  local seq = 0
  for _, e in ipairs(entries or {}) do
    if type(e) == "table" and e.id and e.menu and e.action ~= nil then
      seq = seq + 1
      by_menu[e.menu] = by_menu[e.menu] or {}
      table.insert(by_menu[e.menu], { e = e, seq = seq })
    end
  end
  local tree = {}
  for _, title in ipairs(order) do
    local list = by_menu[title]
    if list and #list > 0 then
      table.sort(list, function(a, b)
        local oa, ob = tonumber(a.e.order) or 100, tonumber(b.e.order) or 100
        if oa ~= ob then
          return oa < ob
        end
        return a.seq < b.seq
      end)
      local items = {}
      for _, it in ipairs(list) do
        table.insert(items, { label = it.e.label or it.e.brief or it.e.id, id = it.e.id, key = it.e.key, accel = it.e.accel })
      end
      apply_accel_overrides(items, accel_of)
      table.insert(tree, { title = t(title), items = items })
    end
  end
  return tree
end

-- Item text of the bar is markup: "\t" starts the hotkey caption, "&X" marks a display
-- accelerator, "&&" is a literal "&". Titles and labels (translations included) are plain
-- text, so a tab becomes a space and "&" is escaped (security review 2026-10-21-i18n, L1).
-- "&" is escaped rather than rejected: "Save & quit" is a fine label.
function M.plain_text(s)
  local r = tostring(s or ""):gsub("\t", " ")
  r = r:gsub("&", "&&")
  return r
end

function M.to_bar(tree)
  local left = {}
  for _, m in ipairs(tree) do
    local children = {}
    for _, it in ipairs(m.items) do
      local key = (it.key and it.key ~= "") and it.key or nil
      table.insert(children, { text = M.plain_text(it.label), id = it.id, key = key, accel = it.accel })
    end
    table.insert(left, { text = M.plain_text(m.title), children = children })
  end
  return { left = left, right = {} }
end

-- Stable hash of the spec (so it is sent to Rust only when it changes).
function M.hash(spec)
  local parts = {}
  local function walk(items, depth)
    for _, it in ipairs(items or {}) do
      table.insert(parts, depth .. "|" .. tostring(it.text) .. "|" .. tostring(it.id) .. "|" .. tostring(it.accel))
      walk(it.children, depth + 1)
    end
  end
  walk(spec.left, 0)
  table.insert(parts, "--")
  walk(spec.right, 0)
  return util().hash(table.concat(parts, "\n"))
end

-- ---- window link ----

local function window_id(window)
  local ok, id = pcall(function()
    return window:window_id()
  end)
  return ok and id or 0
end

function M.tree(window, pane)
  return M.build(require("saysdf.palette").entries(window, pane), state.opts)
end

-- Send the bar to Rust if the tree changed. No method (old GUI build): nothing happens.
function M.push(window, pane, force)
  if window == nil or window.saysdf_set_bar == nil then
    return false
  end
  local palette = require("saysdf.palette")
  local entries = palette.entries(window, pane)
  local wid = window_id(window)
  -- The tree depends only on the registry entries: same version, nothing to rebuild (perf-2).
  local ver = palette.version and palette.version() or nil
  state.last_version = state.last_version or {}
  if not force and ver ~= nil and state.last_version[wid] == ver then
    return true
  end
  state.last_version[wid] = ver
  local spec = M.to_bar(M.build(entries, state.opts))
  local h = M.hash(spec)
  if force or state.last_hash[wid] ~= h then
    state.last_hash[wid] = h
    -- A bad entry (e.g. an accelerator the bar rejects) must not stop the status update and
    -- the other handlers: one notice per distinct error, the bar keeps its last good state
    -- (security review 2026-10-21-i18n, L4)
    local ok, err = pcall(function()
      window:saysdf_set_bar("top", spec)
    end)
    if not ok then
      M.report_bar_error(window, err)
      return false
    end
  end
  return true
end

local bar_errors = {}

function M.report_bar_error(window, err)
  local msg = tostring(err)
  if bar_errors[msg] then
    return
  end
  bar_errors[msg] = true
  pcall(function()
    util().log_error("saysdf.menu: " .. msg)
    util().notify(window, t("Menu bar"), t("The menu bar was not updated: {error}", { error = msg }))
  end)
end

-- Fallback while the GUI has no bars: all items in an InputSelector.
local function fallback_selector(window, pane)
  local w = wezterm()
  local choices = {}
  for _, m in ipairs(M.tree(window, pane)) do
    for _, it in ipairs(m.items) do
      local label = it.label
      if it.key then
        label = label .. "  (" .. it.key .. ")"
      end
      table.insert(choices, { id = it.id, label = m.title .. ": " .. label })
    end
  end
  window:perform_action(
    w.action.InputSelector({
      title = t("Menu"),
      choices = choices,
      fuzzy = true,
      action = w.action_callback(function(win, p, id)
        if id then
          M.run(win, p, id)
        end
      end),
    }),
    pane
  )
end

function M.open(window, pane, index)
  if window.saysdf_open_menu ~= nil then
    M.push(window, pane, false)
    window:saysdf_open_menu(index or 1)
  else
    fallback_selector(window, pane)
  end
end

-- Run a registry entry by id (shared logic: saysdf.palette.run).
function M.run(window, pane, id)
  return require("saysdf.palette").run(window, pane, id)
end

-- F9 handler: a TUI in the foreground gets the key, otherwise the menu opens.
function M.on_key(window, pane, key, open)
  local info = util().pane_foreground(pane)
  if util().foreground_is_tui(info, state.opts.tui_processes) then
    window:perform_action(wezterm().action.SendKey({ key = key }), pane)
  else
    open(window, pane)
  end
end

-- Base upstream commands in the menu (the upstream palette already has them, so palette = false).
-- Texts are translated here, when the entries are built (setup), not at module load.
local function core_entries()
  local w = wezterm()
  local act = w.action
  local palette = require("saysdf.palette")
  local function add(e)
    e.palette = false
    e.brief = e.brief or e.label
    palette.add(e)
  end
  add({ id = "core.new_window", menu = "File", order = 1, label = t("New window"), accel = "n", action = act.SpawnWindow })
  add({ id = "core.quit", menu = "File", order = 99, label = t("Quit"), accel = "q", action = act.QuitApplication })
  add({ id = "core.new_tab", menu = "Tabs", order = 1, label = t("New tab"), accel = "t", key = "Ctrl+Shift+T", action = act.SpawnTab("CurrentPaneDomain") })
  add({ id = "core.split_h", menu = "Tabs", order = 5, label = t("Split right"), accel = "r", action = act.SplitHorizontal({ domain = "CurrentPaneDomain" }) })
  add({ id = "core.split_v", menu = "Tabs", order = 6, label = t("Split down"), accel = "d", action = act.SplitVertical({ domain = "CurrentPaneDomain" }) })
  add({ id = "core.next_tab", menu = "Tabs", order = 10, label = t("Next tab"), accel = "n", key = "Ctrl+Tab", action = act.ActivateTabRelative(1) })
  add({ id = "core.prev_tab", menu = "Tabs", order = 11, label = t("Previous tab"), accel = "p", key = "Ctrl+Shift+Tab", action = act.ActivateTabRelative(-1) })
  add({ id = "core.palette", menu = "View", order = 1, label = t("Command palette"), accel = "p", key = "Ctrl+Shift+P",
    hint = { contexts = { "shell", "git", "ssh", "selection" }, label = t("Palette"), priority = 95 }, action = act.ActivateCommandPalette })
  add({ id = "core.search", menu = "View", order = 2, label = t("Search"), accel = "f", key = "Ctrl+Shift+F", action = act.Search({ CaseInSensitiveString = "" }) })
  add({ id = "core.fullscreen", menu = "View", order = 10, label = t("Full screen"), accel = "s", key = "Alt+Enter", action = act.ToggleFullScreen })
  add({ id = "core.font_up", menu = "View", order = 20, label = t("Larger font"), accel = "=", key = "Ctrl+=", action = act.IncreaseFontSize })
  add({ id = "core.font_down", menu = "View", order = 21, label = t("Smaller font"), accel = "-", key = "Ctrl+-", action = act.DecreaseFontSize })
  add({ id = "core.font_reset", menu = "View", order = 22, label = t("Default font size"), accel = "0", key = "Ctrl+0", action = act.ResetFontSize })
  add({ id = "core.copy", menu = "View", order = 30, label = t("Copy selection"), accel = "c", key = "Ctrl+Shift+C", brief = t("Copy"),
    hint = { contexts = { "selection" }, label = t("Copy"), priority = 10 }, action = act.CopyTo("Clipboard") })
  palette.add({
    id = "menu.modules", menu = "Tools", order = 90, label = t("Module options…"), accel = "p",
    brief = t("saysdf: modules and their options (show-module --list)"),
    action = w.action_callback(function(window, pane)
      local bin = util().find_bin_cached("saysdfTerm-cli")
      if not bin then
        util().log_warn("saysdf.menu: saysdfTerm-cli not found")
        util().notify(window, "saysdfTerm", t("saysdfTerm-cli not found (next to saysdfTerm or in PATH)"))
        return
      end
      pane:split({ args = { bin, "show-module", "--list" }, direction = "Bottom", size = 0.4 })
    end),
  })
end

function M.setup(opts, config)
  opts = opts or {}
  state = { opts = opts, last_hash = {} }
  local w = wezterm()
  local palette = require("saysdf.palette")
  local key = opts.key == nil and { key = "F9", mods = "NONE" } or opts.key
  local force = opts.force_key == nil and { key = "F9", mods = "CTRL|SHIFT" } or opts.force_key
  palette.add({
    id = "menu.open", menu = "Help", order = 50, label = t("Open menu"), accel = "m",
    key = key and palette.key_text(key) or nil, palette = false, brief = t("Open menu"),
    hint = { contexts = { "shell", "git", "ssh", "selection", "tool", "db_danger" }, label = t("Menu"), priority = 1, always = true },
    action = w.action_callback(function(window, pane)
      M.open(window, pane, 1)
    end),
  })
  core_entries()
  config.keys = config.keys or {}
  if key then
    table.insert(config.keys, {
      key = key.key, mods = key.mods,
      action = w.action_callback(function(window, pane)
        M.on_key(window, pane, key.key, function(win, p)
          M.open(win, p, 1)
        end)
      end),
    })
  end
  if force then
    table.insert(config.keys, {
      key = force.key, mods = force.mods,
      action = w.action_callback(function(window, pane)
        M.open(window, pane, 1)
      end),
    })
  end
  -- One bar action handler for both the menu and the status bar hints (hotbar).
  palette.ensure_bar_action()
  require("saysdf.status").hook("menu", function(window, pane)
    M.push(window, pane, false)
  end)
  return config
end

M._state = function()
  return state
end

return M
