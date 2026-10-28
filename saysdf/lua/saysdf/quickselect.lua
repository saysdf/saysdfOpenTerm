-- saysdfTerminal: quick select with paths, hashes, IPs and an "open" mode (change quick-select).
--
-- Usage: saysdf.setup({ quickselect = { ... } }); saysdf.setup({ quickselect = false }) disables it.
-- Options (all optional):
--   patterns = { '...' }                       -- your own patterns; they come FIRST
--   open_key = { key = 'Space', mods = 'CTRL|ALT' }  -- "open" mode; false = no binding
-- Ctrl+Shift+Space stays the stock one (copy); patterns from config.quick_select_patterns
-- apply there too. Order: existing and user patterns, paths (saysdf.paths), IPv4[:port], hashes.
-- "open": http/https -> browser; path -> saysdf.paths.open (like a click); anything else -> toast.
-- Security: only http/https go to the browser; file://, ssh:// and others never do.

local M = {}

local t = require("saysdf.i18n").domain("quickselect")

local function wezterm()
  return require("wezterm")
end

local IPV4 = [[\b\d{1,3}(?:\.\d{1,3}){3}(?::\d{1,5})?\b]]
local HASH = [[\b[0-9a-f]{7,40}\b]]
local URL = [=[https?://[^\s"'<>()\[\]]*[^\s"'<>()\[\].,;:!?]]=]

-- Patterns for the regular quick select: paths, IPv4[:port], hashes.
function M.patterns()
  local p = {}
  for _, x in ipairs(require("saysdf.paths").quick_select_patterns()) do
    table.insert(p, x)
  end
  table.insert(p, IPV4)
  table.insert(p, HASH)
  return p
end

-- Patterns of the "open" mode: URLs and paths.
function M.open_patterns()
  local p = { URL }
  for _, x in ipairs(require("saysdf.paths").quick_select_patterns()) do
    table.insert(p, x)
  end
  return p
end

-- Pure function: what to do with the selected text -> "url" | "path" | "copy".
function M.decide(text)
  if type(text) ~= "string" or text == "" then
    return "copy"
  end
  if text:match("^https?://") then
    return "url"
  end
  if text:match("^file://") or text:sub(1, #require("saysdf.paths").SCHEME) == require("saysdf.paths").SCHEME then
    return "path"
  end
  if text:match("^%a[%w+.%-]*://") then
    return "copy" -- ssh://, ftp:// and other schemes
  end
  if text:match("^%d+%.%d+%.%d+%.%d+:?%d*$") or text:match("^%x+$") then
    return "copy"
  end
  if text:match("[/\\]") or text:match("^[%w_.+%-]+%.%w+:%d+") or text:match("^[%w_.+%-]+%.%w+%(%d+") then
    return "path"
  end
  return "copy"
end

-- deps = { open_with(url), open_path(target), toast(msg) }; returns the kind of action.
function M.open_selected(text, deps)
  local kind = M.decide(text)
  if kind == "url" then
    deps.open_with(text)
  elseif kind == "path" then
    deps.open_path(text)
  else
    deps.toast(t("Copied, cannot be opened: {text}", { text = tostring(text) }))
  end
  return kind
end

function M.setup(opts, config)
  opts = opts or {}
  local w = wezterm()
  config = config or w.config_builder()

  local list = config.quick_select_patterns or {}
  for _, p in ipairs(opts.patterns or {}) do
    table.insert(list, p)
  end
  for _, p in ipairs(M.patterns()) do
    table.insert(list, p)
  end
  config.quick_select_patterns = list

  if opts.open_key ~= false and w.action then
    local key = opts.open_key or { key = "Space", mods = "CTRL|ALT" }
    local open_patterns = {}
    for _, p in ipairs(opts.patterns or {}) do
      table.insert(open_patterns, p)
    end
    for _, p in ipairs(M.open_patterns()) do
      table.insert(open_patterns, p)
    end
    local keys = config.keys or {}
    local open_action = w.action.QuickSelectArgs({
        label = "open",
        patterns = open_patterns,
        skip_action_on_paste = true,
        action = w.action_callback(function(window, pane)
          local text = window:get_selection_text_for_pane(pane)
          local paths = require("saysdf.paths")
          M.open_selected(text, {
            open_with = function(u)
              w.open_with(u)
            end,
            open_path = function(t)
              paths.open(window, pane, t)
            end,
            toast = function(m)
              require("saysdf.util").notify(window, "saysdf", m)
            end,
          })
        end),
      })
    table.insert(keys, { key = key.key, mods = key.mods, action = open_action })
    config.keys = keys
    -- saysdfTerminal: registry entries (menu-bar 1.5).
    local palette = require("saysdf.palette")
    palette.add({ id = "quickselect.open", menu = "Tools", order = 60, label = t("Quick select: open"), accel = "q",
      key = palette.key_text(key), brief = t("Quick select: open a path or URL"), action = open_action,
      hint = { contexts = { "selection" }, label = t("Open"), priority = 20 } })
    palette.add({ id = "quickselect.copy", menu = "Tools", order = 61, label = t("Quick select: copy"), accel = "c",
      key = "Ctrl+Shift+Space", palette = false, brief = t("Quick select: copy"), action = w.action.QuickSelect })
  end
  return config
end

return M
