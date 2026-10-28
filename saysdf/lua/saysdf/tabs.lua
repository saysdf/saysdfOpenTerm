-- saysdfTerminal: tab titles, renaming, indicators and color (change tab-management, D1-D4).
--
-- setup options (require("saysdf").setup({ tabs = {...} })); tabs = false disables the whole module
-- (the upstream titles remain):
--   rename_key = { key = "E", mods = "CTRL|SHIFT" }  -- false: no rename hotkey
--   color_key = false                                -- e.g. { key = "...", mods = "..." }
--   double_click = true      -- double click on a tab opens the rename prompt
--   palette = true           -- command palette entries (via saysdf.palette)
--   nerd_font = true         -- false: text labels instead of Nerd Font glyphs
--   icons = { htop = "H" }   -- override/extend icons by process name (without .exe)
--   indicators = { running = "...", error = "...", unseen = "...", attention = "..." }
--                            -- custom indicator text, or false to disable it
--   colors = { { name = "red", label = "Red", color = "#e06c75" }, ... }  -- palette
--
-- Data: folder is pane.current_working_dir (OSC 7), exit code is the user var SAYSDF_LAST_EXIT
-- (shell-integration), branch is saysdf.git.info(pane).branch if the git module is present (soft
-- dependency). The manual name is the upstream tab:set_title() (OSC 0/2 do not overwrite it).
--
-- API for other modules:
--   set_color(tab, color)        -- tab: a tab object or tab_id; color: a palette name,
--                                   "#rrggbb" or nil (reset)
--   set_attention(tab, on)       -- attention indicator (true/string: on, false/nil: off);
--                                   cleared when the tab is activated
--   rename(window, pane, tab?)   -- open the name prompt; reset_name(tab) resets to the auto title

local t, _, N_ = require("saysdf.i18n").domain("tabs")

local M = {}

local function wezterm()
  return require("wezterm")
end

-- ---- constants ----

-- Shells: a process from this list counts as "not running". Extended in setup with the
-- close-confirmation skip list (config.skip_close_confirmation_for_processes_named).
M.SHELLS = { "bash", "sh", "zsh", "fish", "nu", "tmux", "cmd", "pwsh", "powershell", "wsl", "conhost" }

M.NERD_ICONS = {
  shell = "\u{f489}",
  vim = "\u{e62b}",
  nvim = "\u{e62b}",
  hx = "\u{f044}",
  nano = "\u{f044}",
  ssh = "\u{f233}",
  git = "\u{e702}",
  lazygit = "\u{e702}",
  claude = "\u{f069}",
  python = "\u{e606}",
  node = "\u{e718}",
  cargo = "\u{e7a8}",
  generic = "\u{f013}",
}

M.TEXT_ICONS = {
  shell = ">_",
  vim = "vi",
  nvim = "vi",
  hx = "hx",
  nano = "ed",
  ssh = "ssh",
  git = "git",
  lazygit = "git",
  claude = "ai",
  python = "py",
  node = "js",
  cargo = "rs",
  generic = "*",
}

-- Process name aliases.
local ALIASES = { python3 = "python", py = "python", ["node.exe"] = "node", rustc = "cargo", vi = "vim" }

M.NERD_INDICATORS = { running = "\u{f110}", error = "\u{f00d}", unseen = "\u{f444}", attention = "\u{f0f3}" }
M.TEXT_INDICATORS = { running = "~", error = "!", unseen = "+", attention = "@" }
M.INDICATOR_COLORS = { running = "#e5c07b", error = "#e06c75", unseen = "#61afef", attention = "#c678dd" }
M.INDICATOR_ORDER = { "attention", "error", "running", "unseen" }

M.DEFAULT_COLORS = {
  { name = "red", label = N_("Red"), color = "#e06c75" },
  { name = "orange", label = N_("Orange"), color = "#d19a66" },
  { name = "yellow", label = N_("Yellow"), color = "#e5c07b" },
  { name = "green", label = N_("Green"), color = "#98c379" },
  { name = "cyan", label = N_("Cyan"), color = "#56b6c2" },
  { name = "blue", label = N_("Blue"), color = "#61afef" },
  { name = "purple", label = N_("Purple"), color = "#c678dd" },
  { name = "gray", label = N_("Gray"), color = "#abb2bf" },
}

M.EXIT_VAR = "SAYSDF_LAST_EXIT"
M.DURATION_VAR = "SAYSDF_LAST_DURATION_MS"
M.DOUBLE_CLICK_EVENT = "saysdf-tab-double-click"

-- ---- pure functions ----

-- Process name: basename without .exe, lowercase; "" for empty.
function M.process_name(p)
  if type(p) ~= "string" or p == "" then
    return ""
  end
  local name = p:match("([^/\\]+)$") or p
  return (name:lower():gsub("%.exe$", ""))
end

-- Process icon: user override > table > generic; nerd_font = false means text.
function M.icon_for(process, opts)
  opts = opts or {}
  local name = M.process_name(process)
  local user = opts.icons or {}
  if name ~= "" and user[name] then
    return user[name]
  end
  local key = ALIASES[name] or name
  local table_ = opts.nerd_font == false and M.TEXT_ICONS or M.NERD_ICONS
  if M.is_shell(name, opts.shells) then
    return user.shell or table_.shell
  end
  return table_[key] or user.generic or table_.generic
end

function M.is_shell(name, shells)
  name = M.process_name(name)
  for _, s in ipairs(shells or M.SHELLS) do
    if M.process_name(s) == name then
      return true
    end
  end
  return false
end

-- Truncate by UTF-8 characters with an ellipsis; invalid UTF-8 is truncated by bytes.
function M.truncate(s, width)
  s = s or ""
  if width == nil or width <= 0 then
    return ""
  end
  local n = utf8.len(s)
  if n == nil then
    if #s <= width then
      return s
    end
    return s:sub(1, math.max(width - 1, 0)) .. "…"
  end
  if n <= width then
    return s
  end
  if width == 1 then
    return "…"
  end
  return s:sub(1, utf8.offset(s, width) - 1) .. "…"
end

local function percent_decode(s)
  return (s:gsub("%%(%x%x)", function(h)
    return string.char(tonumber(h, 16))
  end))
end

-- Path from cwd: a path string, a file:// URL or the upstream Url object (file_path field).
function M.cwd_path(cwd)
  if cwd == nil then
    return nil
  end
  if type(cwd) ~= "string" then
    local ok, fp = pcall(function()
      return cwd.file_path
    end)
    if ok and type(fp) == "string" then
      cwd = fp
    else
      cwd = tostring(cwd)
    end
  end
  if cwd == "" then
    return nil
  end
  local rest = cwd:match("^[Ff][Ii][Ll][Ee]://[^/]*(/.*)$")
  if rest then
    cwd = percent_decode(rest)
  end
  return cwd
end

-- Folder name (basename of cwd); nil if unknown.
function M.folder_name(cwd)
  local path = M.cwd_path(cwd)
  if not path then
    return nil
  end
  local trimmed = path:gsub("[/\\]+$", "")
  if trimmed == "" then
    return "/"
  end
  local name = trimmed:match("([^/\\]+)$") or trimmed
  return name
end

-- Branch from the optional saysdf.git module: info(pane) -> string | { branch = ... }.
function M.branch_of(git, pane)
  if type(git) ~= "table" or type(git.info) ~= "function" then
    return nil
  end
  local ok, info = pcall(git.info, pane)
  if not ok then
    return nil
  end
  if type(info) == "string" and info ~= "" then
    return info
  elseif type(info) == "table" and type(info.branch) == "string" and info.branch ~= "" then
    return info.branch
  end
  return nil
end

-- Auto title without a manual name and icon: "folder branch" or the program title.
function M.auto_name(pane, branch)
  pane = pane or {}
  local folder = M.folder_name(pane.current_working_dir)
  local parts = {}
  if folder then
    table.insert(parts, folder)
    if branch then
      table.insert(parts, branch)
    end
  elseif type(pane.title) == "string" and pane.title ~= "" then
    table.insert(parts, pane.title)
  end
  return table.concat(parts, " ")
end

-- Title: manual name > icon + folder + branch (unknown parts are skipped).
-- extra.branch is the git branch, if known.
function M.build_title(tab, pane, opts, extra)
  tab = tab or {}
  pane = pane or {}
  extra = extra or {}
  if type(tab.tab_title) == "string" and tab.tab_title ~= "" then
    return tab.tab_title
  end
  local process = pane.foreground_process_name
  if process == nil or process == "" then
    process = (type(pane.title) == "string" and pane.title:match("^(%S+)")) or ""
  end
  local icon = M.icon_for(process, opts)
  local name = M.auto_name(pane, extra.branch)
  if name == "" then
    return icon
  end
  return icon .. " " .. name
end

-- Key of the last command result (for "seen").
function M.exit_key(pane)
  local vars = (pane or {}).user_vars or {}
  local code = vars[M.EXIT_VAR]
  if code == nil or code == "" then
    return nil
  end
  return tostring(code) .. "|" .. tostring(vars[M.DURATION_VAR] or "")
end

-- Indicators: { running = bool, error = bool, unseen = bool, attention = bool }.
-- state: { seen = { [tab_id] = exit_key }, attention = { [tab_id] = ... } }.
function M.indicators_for(tab, pane, state, opts)
  tab = tab or {}
  pane = pane or {}
  state = state or {}
  opts = opts or {}
  local id = tostring(tab.tab_id)
  local result = {}

  local process = M.process_name(pane.foreground_process_name)
  result.running = process ~= "" and not M.is_shell(process, opts.shells)

  local vars = pane.user_vars or {}
  local code = vars[M.EXIT_VAR]
  local key = M.exit_key(pane)
  result.error = code ~= nil and code ~= "" and tostring(code) ~= "0"
    and not (tab.is_active or (state.seen or {})[id] == key)

  result.unseen = (pane.has_unseen_output and not tab.is_active) and true or false
  result.attention = ((state.attention or {})[id] ~= nil and not tab.is_active) and true or false
  return result
end

-- State update when the active tab is rendered: error and attention count as seen.
-- Returns true if the state changed.
function M.mark_seen(state, tab, pane)
  if not (tab and tab.is_active) then
    return false
  end
  local id = tostring(tab.tab_id)
  local changed = false
  state.seen = state.seen or {}
  local key = M.exit_key(pane)
  if state.seen[id] ~= key then
    state.seen[id] = key
    changed = true
  end
  if state.attention and state.attention[id] ~= nil then
    state.attention[id] = nil
    changed = true
  end
  return changed
end

-- Removes entries of closed tabs. live is the list of live tab_ids. Returns true on change.
function M.prune(state, live)
  local alive = {}
  for _, id in ipairs(live or {}) do
    alive[tostring(id)] = true
  end
  local changed = false
  for _, field in ipairs({ "colors", "seen", "attention" }) do
    local t = state[field]
    if type(t) == "table" then
      for id in pairs(t) do
        if not alive[id] then
          t[id] = nil
          changed = true
        end
      end
    end
  end
  return changed
end

-- Color: a palette name or "#rrggbb"; nil if unknown.
function M.resolve_color(color, palette)
  if type(color) ~= "string" or color == "" then
    return nil
  end
  for _, c in ipairs(palette or M.DEFAULT_COLORS) do
    if c.name == color then
      return c.color
    end
  end
  if color:match("^#%x%x%x%x%x%x$") or color:match("^#%x%x%x$") then
    return color
  end
  return nil
end

-- FormatItem elements for format-tab-title.
function M.format_items(title, indicators, color, is_active, opts, max_width)
  opts = opts or {}
  local glyphs = opts.nerd_font == false and M.TEXT_INDICATORS or M.NERD_INDICATORS
  local user = opts.indicators or {}
  local marks = {}
  for _, name in ipairs(M.INDICATOR_ORDER) do
    if indicators[name] and user[name] ~= false then
      table.insert(marks, { name = name, text = type(user[name]) == "string" and user[name] or glyphs[name] })
    end
  end

  local marks_width = 0
  for _, m in ipairs(marks) do
    marks_width = marks_width + (utf8.len(m.text) or #m.text) + 1
  end
  local width = (max_width or 1000) - 2 - marks_width
  title = M.truncate(title, math.max(width, 1))

  local items = {}
  if color then
    if is_active then
      table.insert(items, { Background = { Color = color } })
      table.insert(items, { Foreground = { Color = "#1e1e1e" } })
    else
      table.insert(items, { Foreground = { Color = color } })
    end
  end
  table.insert(items, { Text = " " .. title .. " " })
  -- indicators go last so their color needs no attribute reset for the title
  for _, m in ipairs(marks) do
    if not (color and is_active) then
      table.insert(items, { Foreground = { Color = M.INDICATOR_COLORS[m.name] } })
    end
    table.insert(items, { Text = m.text .. " " })
  end
  return items
end

-- ---- state (wezterm.GLOBAL, survives a config reload) ----

local cache = { raw = nil, state = nil }

function M._load()
  local w = wezterm()
  local g = w.GLOBAL
  if g == nil then
    cache.state = cache.state or {}
    return cache.state
  end
  local raw = g.saysdf_tabs
  if type(raw) == "string" then
    if raw ~= cache.raw then
      local ok, parsed = pcall(w.json_parse, raw)
      cache.raw = raw
      cache.state = (ok and type(parsed) == "table") and parsed or {}
    end
    return cache.state
  end
  cache.state = cache.state or {}
  return cache.state
end

function M._save(state)
  local w = wezterm()
  cache.state = state
  if w.GLOBAL ~= nil and w.json_encode then
    local raw = w.json_encode(state)
    cache.raw = raw
    w.GLOBAL.saysdf_tabs = raw
  end
end

local function tab_id_of(tab)
  if type(tab) == "number" or type(tab) == "string" then
    return tostring(tab)
  end
  return tostring(tab:tab_id())
end

-- ---- API ----

local current = { palette = M.DEFAULT_COLORS }

function M.set_color(tab, color)
  local state = M._load()
  state.colors = state.colors or {}
  local id = tab_id_of(tab)
  local resolved = M.resolve_color(color, current.palette)
  if color ~= nil and color ~= "" and resolved == nil then
    require("saysdf.util").log_warn("saysdf.tabs: unknown color '" .. tostring(color) .. "'")
    return false
  end
  state.colors[id] = resolved
  M._save(state)
  return true
end

function M.get_color(tab)
  return (M._load().colors or {})[tab_id_of(tab)]
end

function M.set_attention(tab, on)
  local state = M._load()
  state.attention = state.attention or {}
  local id = tab_id_of(tab)
  if on == nil or on == false then
    state.attention[id] = nil
  else
    state.attention[id] = type(on) == "string" and on or true
  end
  M._save(state)
end

function M.reset_name(tab)
  tab:set_title("")
end

local function tab_of(window, pane, tab)
  if tab then
    return tab
  end
  local ok, t = pcall(function()
    return pane:tab()
  end)
  if ok and t then
    return t
  end
  return window:active_tab()
end

-- Rename: PromptInputLine; empty resets to the auto title, Esc leaves it unchanged.
function M.rename(window, pane, tab)
  local w = wezterm()
  local act = w.action
  tab = tab_of(window, pane, tab)
  if not tab then
    return
  end
  local initial = tab:get_title() or ""
  if initial == "" then
    local ok, name = pcall(function()
      local p = tab:active_pane()
      return M.auto_name({ current_working_dir = p:get_current_working_dir(), title = p:get_title() })
    end)
    initial = ok and name or ""
  end
  window:perform_action(
    act.PromptInputLine({
      description = t("Tab name (empty for automatic, Esc to cancel)"),
      initial_value = initial,
      action = w.action_callback(function(_win, _pane, line)
        if line == nil then
          return
        end
        tab:set_title(line)
      end),
    }),
    pane
  )
end

function M.choose_color(window, pane, tab)
  local w = wezterm()
  local act = w.action
  tab = tab_of(window, pane, tab)
  if not tab then
    return
  end
  local choices = {}
  for _, c in ipairs(current.palette) do
    table.insert(choices, { id = c.name, label = c.label and t(c.label) or c.name })
  end
  table.insert(choices, { id = "", label = t("Reset color") })
  window:perform_action(
    act.InputSelector({
      title = t("Tab color"),
      choices = choices,
      action = w.action_callback(function(_win, _pane, id)
        if id == nil then
          return
        end
        M.set_color(tab, id ~= "" and id or nil)
      end),
    }),
    pane
  )
end

-- Live tab_ids of all windows (for state cleanup).
local function live_tab_ids()
  local ids = {}
  local ok = pcall(function()
    for _, win in ipairs(wezterm().mux.all_windows()) do
      for _, t in ipairs(win:tabs()) do
        table.insert(ids, t:tab_id())
      end
    end
  end)
  return ok and ids or nil
end

local PRUNE_INTERVAL = 10
local last_prune = 0

-- format-tab-title handler (no pcall; the wrapper is in setup).
function M.format(tab, pane, opts, git, max_width)
  local state = M._load()
  local changed = M.mark_seen(state, tab, pane)
  local now = os.time()
  if now - last_prune >= PRUNE_INTERVAL then
    last_prune = now
    local live = live_tab_ids()
    if live and M.prune(state, live) then
      changed = true
    end
  end
  if changed then
    M._save(state)
  end
  local title = M.build_title(tab, pane, opts, { branch = M.branch_of(git, pane) })
  local indicators = M.indicators_for(tab, pane, state, opts)
  local color = (state.colors or {})[tostring(tab.tab_id)]
  return M.format_items(title, indicators, color, tab.is_active, opts, max_width)
end

local function fallback_title(tab)
  if tab and type(tab.tab_title) == "string" and tab.tab_title ~= "" then
    return tab.tab_title
  end
  local p = tab and tab.active_pane
  return (p and p.title) or ""
end

-- ---- setup ----

local function binding(opt, default)
  if opt == false or opt == nil and default == nil then
    return nil
  end
  if type(opt) == "table" then
    return opt
  end
  return default
end

local function add_key(keys, b, action)
  table.insert(keys, { key = b.key, mods = b.mods, action = action })
  -- like upstream for Ctrl+Shift+letter: add the lowercase key too
  if type(b.key) == "string" and b.key:match("^%u$") then
    table.insert(keys, { key = b.key:lower(), mods = b.mods, action = action })
  end
end

-- Entries with action_callback are built once per setup: action_callback registers a new event
-- on every call, while palette.entries is called on every update-status tick (perf-2).
local palette_cache = nil

local registered = false

function M.setup(opts, config)
  opts = opts or {}
  local w = wezterm()
  palette_cache = nil

  local o = {}
  for k, v in pairs(opts) do
    o[k] = v
  end
  if type(opts.colors) == "table" then
    current.palette = opts.colors
  else
    current.palette = M.DEFAULT_COLORS
  end
  local shells = {}
  for _, s in ipairs(M.SHELLS) do
    table.insert(shells, s)
  end
  for _, s in ipairs(config.skip_close_confirmation_for_processes_named or {}) do
    table.insert(shells, s)
  end
  o.shells = shells
  current.opts = o

  local ok_git, git = pcall(require, "saysdf.git")
  current.git = ok_git and type(git) == "table" and git or nil

  if not registered then
    registered = true
    w.on("format-tab-title", function(tab, _tabs, _panes, _config, _hover, max_width)
      local ok, items = pcall(M.format, tab, tab.active_pane, current.opts, current.git, max_width)
      if ok then
        return items
      end
      require("saysdf.util").log_error("saysdf.tabs: format-tab-title: " .. tostring(items))
      return fallback_title(tab)
    end)
    w.on(M.DOUBLE_CLICK_EVENT, function(window, pane, tab_idx)
      if current.opts.double_click == false then
        return
      end
      local tabs = window:mux_window():tabs()
      M.rename(window, pane, tabs[(tab_idx or 0) + 1])
    end)
    if opts.palette ~= false then
      -- upstream calls only the first augment-command-palette handler: go through saysdf.palette
      require("saysdf.palette").add(function(_window, _pane)
        return M.palette_entries()
      end)
    end
  end

  local keys = config.keys or {}
  local rename = binding(opts.rename_key, { key = "E", mods = "CTRL|SHIFT" })
  current.rename_key_text = rename and require("saysdf.palette").key_text(rename) or nil
  if rename then
    add_key(keys, rename, w.action_callback(function(window, pane)
      M.rename(window, pane)
    end))
  end
  local color = binding(opts.color_key, nil)
  if color then
    add_key(keys, color, w.action_callback(function(window, pane)
      M.choose_color(window, pane)
    end))
  end
  config.keys = keys

  return config
end

function M.palette_entries()
  if palette_cache == nil then
    palette_cache = M._build_palette_entries()
  end
  return palette_cache
end

function M._build_palette_entries()
  local w = wezterm()
  return {
    {
      id = "tabs.rename", menu = "Tabs", order = 20, label = t("Rename tab…"), accel = "e",
      key = current.rename_key_text,
      brief = t("Tab: rename tab"),
      icon = "md_rename_box",
      action = w.action_callback(function(window, pane)
        M.rename(window, pane)
      end),
    },
    {
      id = "tabs.reset_name", menu = "Tabs", order = 21, label = t("Reset tab name"), accel = "x",
      brief = t("Tab: reset tab name"),
      action = w.action_callback(function(window, pane)
        M.reset_name(tab_of(window, pane))
      end),
    },
    {
      id = "tabs.color", menu = "Tabs", order = 22, label = t("Tab color…"), accel = "c",
      brief = t("Tab: set tab color"),
      icon = "md_palette",
      action = w.action_callback(function(window, pane)
        M.choose_color(window, pane)
      end),
    },
  }
end

return M
