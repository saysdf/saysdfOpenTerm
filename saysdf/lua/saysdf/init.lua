-- saysdfTerminal: root module of the bundled Lua modules (change bundled-lua-modules, design D4).
--
-- Contract of a feature module saysdf/lua/saysdf/<name>.lua: it returns a table with
-- setup(opts, config), where opts is the table from saysdf.setup({ <name> = {...} }) or {},
-- config is the shared config_builder. Adding a module = adding a file: the module list comes
-- from the list embedded in the binary (package.preload["saysdf._bundled"]), no edits here.
-- A module without setup (a library such as saysdf.util) is skipped.
--
-- Top-level options that are not modules:
--   language = "auto" | "en" | "ru" | ...   -- UI language (change i18n); "auto" = system language


-- util may be missing from reduced module sets: log safely without it.
local function log_warn(msg)
  local ok, util = pcall(require, "saysdf.util")
  if ok and type(util) == "table" then
    return util.log_warn(msg)
  end
  local okw, w = pcall(require, "wezterm")
  if okw and type(w) == "table" and type(w.log_warn) == "function" then
    pcall(w.log_warn, msg)
  end
end

local M = {}

-- Option keys of saysdf.setup that are not module names.
M.OPTIONS = { language = true }

local function wezterm()
  return require("wezterm")
end

-- Short names of the bundled modules ("tabs" for "saysdf.tabs"), without the root one.
function M.modules()
  local ok, bundled = pcall(require, "saysdf._bundled")
  local result = {}
  if not ok or type(bundled) ~= "table" then
    return result
  end
  for _, full in ipairs(bundled) do
    local short = full:match("^saysdf%.(.+)$")
    if short then
      table.insert(result, short)
    end
  end
  return result
end

-- Broken user catalog files (skipped, details in the log): one notice per config load. The
-- config is evaluated before any window exists, so the notice waits for the first status
-- update of a window (a status hook); each config load has its own Lua state and flag.
function M.notify_rejected_catalogs(n)
  n = tonumber(n) or 0
  if n <= 0 then
    return false
  end
  local ok, status = pcall(require, "saysdf.status")
  if not ok or type(status) ~= "table" or type(status.hook) ~= "function" then
    return false
  end
  local shown = false
  status.hook("i18n-rejected-catalogs", function(window, _)
    if shown or window == nil then
      return
    end
    shown = true
    local _, tn = require("saysdf.i18n").domain("saysdf")
    require("saysdf.util").notify(
      window,
      "saysdfTerm",
      tn(
        "{n} translation file in the config folder has errors and was ignored; see the log",
        "{n} translation files in the config folder have errors and were ignored; see the log",
        n
      )
    )
  end)
  return true
end

-- Chooses the UI language and passes it to the tools the terminal starts (SAYSDF_LANG in
-- config.set_environment_variables, unless the user set it). Returns the language or nil.
function M.select_language(language, config)
  local ok, i18n = pcall(require, "saysdf.i18n")
  if not ok or type(i18n) ~= "table" then
    return nil
  end
  local dir
  pcall(function()
    dir = wezterm().config_dir
  end)
  local lang = i18n.select(language, dir)
  -- Rust code of the GUI (bars, renderers) follows the loaded config: the only apply_global call
  i18n.apply_global()
  M.notify_rejected_catalogs(i18n.rejected_user_files)
  local env = config.set_environment_variables or {}
  if env[i18n.ENV] == nil then
    env[i18n.ENV] = lang
  end
  config.set_environment_variables = env
  return lang
end

function M.setup(opts, config)
  opts = opts or {}
  config = config or wezterm().config_builder()

  -- SAYSDFTERM_PROFILE=1: handler timings to the log (saysdf.profile)
  local okp, profile = pcall(require, "saysdf.profile")
  if okp and type(profile) == "table" and profile.enabled() then
    profile.install()
  end

  -- The language goes first: module setup() builds translated labels.
  M.select_language(opts.language, config)

  local known = {}
  for key in pairs(M.OPTIONS) do
    known[key] = true
  end
  for _, name in ipairs(M.modules()) do
    known[name] = true
    local o = opts[name]
    if o ~= false then
      local mod = require("saysdf." .. name)
      if type(mod) == "table" and type(mod.setup) == "function" then
        mod.setup(type(o) == "table" and o or {}, config)
      elseif o ~= nil then
        log_warn("saysdf.setup: module '" .. name .. "' has no setup(), options ignored")
      end
    end
  end

  for key in pairs(opts) do
    if not known[key] then
      log_warn("saysdf.setup: unknown module '" .. tostring(key) .. "'")
    end
  end

  return config
end

return M
