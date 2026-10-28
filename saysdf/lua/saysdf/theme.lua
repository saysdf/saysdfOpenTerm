-- saysdfTerminal: theme menu (change theme-switcher).
--
-- Items of the "View" menu (via saysdf.palette, also visible in the command palette):
--   "Theme…"               - a selection of contrasting schemes from saysdf.theme_presets (group, label,
--                             WCAG contrast; "⚠ ANSI" when ansi_min < 2), then all the other built-in
--                             schemes; fuzzy search (InputSelector fuzzy); the chosen scheme is applied to
--                             all windows at once (set_config_overrides). Upstream cannot preview while
--                             moving through the list - the scheme is applied on choice (Enter), Esc changes nothing.
--   "Light/Dark"           - switch between opts.light and opts.dark.
--   "Follow system theme"  - the scheme by wezterm.gui.get_appearance(): Light -> light, Dark -> dark;
--                             after that it follows the system automatically (an OS appearance change
--                             reloads the config and sends window-config-reloaded).
--
-- The choice is saved to <data>/theme.json (<data> is saysdf.util.data_dir()):
--   { "mode": "scheme", "scheme": "<name>" }  or  { "mode": "system" }
-- and applied at startup in setup(): config.color_scheme = the saved scheme.
--
-- Priority. A menu choice is the user's later decision, so by default it OVERRIDES config.color_scheme
-- from the config (if setup() is called after the assignment, as in the config template). To let the
-- config win at startup:
--   require("saysdf").setup({ theme = { respect_config = true } }, config)
-- Then the saved choice is applied only if config.color_scheme is not set; the menu items still change
-- the scheme in the current session. To reset the choice, delete <data>/theme.json.
--
-- Non-built-in schemes of the selection (presets.schemes_table()) are registered in config.color_schemes
-- in setup(); the user's schemes with the same name are not overwritten.
--
-- Options: light = "Modus-Operandi", dark = "Modus-Vivendi" (contrast 21:1; Solarized was dropped as
--        low-contrast), respect_config = false, palette = true (menu/palette items).
-- No hotkeys (see saysdf/keymap.md).

local M = {}

local t = require("saysdf.i18n").domain("theme")

M.DEFAULTS = {
  light = "Modus-Operandi",
  dark = "Modus-Vivendi",
  respect_config = false,
  palette = true,
}
M.FILE = "theme.json"

local opts = nil
local state = nil -- last choice { mode, scheme }

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

-- The selection goes to the sidebar (`saysdf.sidebar.selector`); the fallback is InputSelector.
local function sidebar()
  return require("saysdf.sidebar")
end

-- The scheme catalog (saysdf.theme_presets); without it only the built-in schemes are offered.
function M.presets()
  local ok, p = pcall(require, "saysdf.theme_presets")
  if ok and type(p) == "table" and type(p.groups) == "table" then
    return p
  end
  return nil
end

function M.merge_opts(o)
  local r = {}
  for k, v in pairs(M.DEFAULTS) do
    r[k] = v
  end
  for k, v in pairs(o or {}) do
    r[k] = v
  end
  return r
end

-- The scheme for the system appearance ("Light", "Dark", "LightHighContrast", "DarkHighContrast").
function M.appearance_scheme(appearance, o)
  if tostring(appearance or ""):find("Dark") then
    return o.dark
  end
  return o.light
end

-- Where "Light/Dark" switches to: dark -> light, light -> dark; any other scheme by its kind
-- ('dark' -> light), an unknown one -> dark.
function M.toggle_target(current, o, kind)
  if current == o.dark then
    return o.light
  elseif current == o.light then
    return o.dark
  elseif kind == "dark" then
    return o.light
  end
  return o.dark
end

-- Registers the non-built-in schemes of the selection in config.color_schemes (the user's own
-- schemes are left alone). Returns the number added.
function M.register_presets(config, presets)
  if not presets or type(presets.schemes_table) ~= "function" then
    return 0
  end
  local cs = {}
  local okr, existing = pcall(function()
    return config.color_schemes
  end)
  if okr and type(existing) == "table" then
    for k, v in pairs(existing) do
      cs[k] = v
    end
  end
  local n = 0
  for name, colors in pairs(presets.schemes_table()) do
    if cs[name] == nil then
      cs[name] = colors
      n = n + 1
    end
  end
  if n > 0 then
    config.color_schemes = cs
  end
  return n
end

-- Selector items: the selection by group, then the other built-in schemes (builtin is an array of names).
-- Group and scheme labels are catalog keys (N_ in saysdf.theme_presets); presets.tr() translates them here,
-- where they are shown (their catalog is that of saysdf.theme_presets).
function M.choices(presets, builtin, current)
  local out, seen = {}, {}
  local function tr(key)
    return presets and type(presets.tr) == "function" and presets.tr(key) or key
  end
  local function mark(name, text)
    return (name == current) and (text .. "  ✓") or text
  end
  if presets then
    for _, g in ipairs(presets.groups) do
      for _, sc in ipairs(g.schemes or {}) do
        if not seen[sc.name] then
          seen[sc.name] = true
          local text = string.format("%s · %s  [%.1f:1]", g.label and tr(g.label) or g.id, sc.label and tr(sc.label) or sc.name, sc.contrast or 0)
          if type(sc.ansi_min) == "number" and sc.ansi_min < 2 then
            text = text .. "  ⚠ ANSI"
          end
          table.insert(out, { id = sc.name, label = mark(sc.name, text) })
        end
      end
    end
  end
  for _, name in ipairs(builtin or {}) do
    if not seen[name] then
      seen[name] = true
      table.insert(out, { id = name, label = mark(name, t("All · {name}", { name = name })) })
    end
  end
  return out
end

-- Which scheme to set at startup; nil means leave the config alone.
function M.resolve(st, config_scheme, o, appearance)
  if type(st) ~= "table" then
    return nil
  end
  if o.respect_config and config_scheme ~= nil and config_scheme ~= "" then
    return nil
  end
  if st.mode == "system" then
    return M.appearance_scheme(appearance, o)
  end
  if st.mode == "scheme" and type(st.scheme) == "string" and st.scheme ~= "" then
    return st.scheme
  end
  return nil
end

local function json_str(s)
  return '"' .. tostring(s):gsub('[%c"\\]', function(c)
    if c == '"' then
      return '\\"'
    elseif c == "\\" then
      return "\\\\"
    end
    return string.format("\\u%04x", c:byte())
  end) .. '"'
end

function M.encode(st)
  if st.mode == "system" then
    return '{"mode":"system"}'
  end
  return '{"mode":"scheme","scheme":' .. json_str(st.scheme) .. "}"
end

-- Parses theme.json; garbage gives nil.
function M.decode(text)
  if type(text) ~= "string" or text == "" then
    return nil
  end
  local ok, v = pcall(wezterm().json_parse, text)
  if not ok or type(v) ~= "table" then
    return nil
  end
  if v.mode == "system" then
    return { mode = "system" }
  end
  if v.mode == "scheme" and type(v.scheme) == "string" and v.scheme ~= "" then
    return { mode = "scheme", scheme = v.scheme }
  end
  return nil
end

function M.path(env)
  local u = util()
  return u.join(u.data_dir(env), M.FILE)
end

function M.load(path)
  return M.decode(util().read_file(path or M.path()))
end

function M.save(st, path)
  local u = util()
  if not path then
    u.ensure_dir(u.data_dir())
    path = M.path()
  end
  local ok, err = u.write_atomic(path, M.encode(st))
  if not ok then
    u.log_error("saysdf.theme: could not save " .. tostring(path) .. ": " .. tostring(err))
  end
  return ok
end

local function appearance()
  local w = wezterm()
  if w.gui and w.gui.get_appearance then
    local ok, a = pcall(w.gui.get_appearance)
    if ok then
      return a
    end
  end
  return "Dark"
end

local function set_override(window, scheme)
  local o = window:get_config_overrides() or {}
  if o.color_scheme ~= scheme then
    o.color_scheme = scheme
    window:set_config_overrides(o)
  end
end

-- All GUI windows (or just the given one if the list is unavailable).
local function each_window(window, fn)
  local w = wezterm()
  local list = nil
  if w.gui and w.gui.gui_windows then
    local ok, l = pcall(w.gui.gui_windows)
    if ok then
      list = l
    end
  end
  if not list or #list == 0 then
    list = { window }
  end
  for _, win in ipairs(list) do
    local ok, err = pcall(fn, win)
    if not ok then
      util().log_warn("saysdf.theme: " .. tostring(err))
    end
  end
end

function M.apply(window, scheme, st)
  state = st
  M.save(st)
  each_window(window, function(win)
    set_override(win, scheme)
  end)
end

local function current_scheme(window)
  local ok, c = pcall(function()
    return window:effective_config().color_scheme
  end)
  return ok and c or nil
end

function M.scheme_names()
  local w = wezterm()
  local get = (w.color and w.color.get_builtin_schemes) or w.get_builtin_color_schemes
  local names = {}
  if get then
    local ok, schemes = pcall(get)
    if ok and type(schemes) == "table" then
      for name in pairs(schemes) do
        table.insert(names, name)
      end
    end
  end
  table.sort(names, function(a, b)
    return a:lower() < b:lower()
  end)
  return names
end

function M.select(window, pane)
  local choices = M.choices(M.presets(), M.scheme_names(), current_scheme(window))
  window:perform_action(
    sidebar().selector({
      tool = "theme",
      title = t("Theme (color scheme)"),
      description = t("Enter: apply, Esc: cancel, /: search"),
      fuzzy_description = t("Theme: "),
      fuzzy = true,
      choices = choices,
      on_choice = function(win, _, id)
        if id then
          M.apply(win, id, { mode = "scheme", scheme = id })
        end
      end,
    }),
    pane
  )
end

function M.toggle(window, _)
  local cur = current_scheme(window)
  local p = M.presets()
  local preset = p and p.find and cur and p.find(cur) or nil
  local target = M.toggle_target(cur, opts or M.DEFAULTS, preset and preset.kind or nil)
  M.apply(window, target, { mode = "scheme", scheme = target })
end

function M.follow_system(window, _)
  M.apply(window, M.appearance_scheme(appearance(), opts or M.DEFAULTS), { mode = "system" })
end

function M.setup(o, config)
  opts = M.merge_opts(o)
  local w = wezterm()
  state = M.load()
  M.register_presets(config, M.presets())

  local okc, config_scheme = pcall(function()
    return config.color_scheme
  end)
  local scheme = M.resolve(state, okc and config_scheme or nil, opts, appearance())
  if scheme then
    config.color_scheme = scheme
  end

  -- Following the system automatically: an OS appearance change reloads the config; windows with an
  -- override from a previous choice are pulled to the new scheme.
  w.on("window-config-reloaded", function(window)
    if state and state.mode == "system" then
      local ov = window:get_config_overrides()
      if ov and ov.color_scheme then
        set_override(window, M.appearance_scheme(appearance(), opts))
      end
    end
  end)

  if opts.palette then
    local palette = require("saysdf.palette")
    local cb = w.action_callback
    palette.add({ id = "theme.select", menu = "View", order = 50, label = t("Theme…"), accel = "t", brief = t("Theme: pick a color scheme"), icon = "md_palette", action = cb(M.select) })
    palette.add({ id = "theme.toggle", menu = "View", order = 51, label = t("Light/Dark"), accel = "l", brief = t("Theme: light/dark"), icon = "md_theme_light_dark", action = cb(M.toggle) })
    palette.add({ id = "theme.system", menu = "View", order = 52, label = t("Follow system theme"), accel = "y", brief = t("Theme: follow system"), icon = "md_monitor", action = cb(M.follow_system) })
  end
  return config
end

return M
