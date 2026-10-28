-- saysdfTerminal: UI localization (change i18n). A thin wrapper over the native module
-- saysdf._i18n (crate saysdf-i18n): Lua and the Rust tools share one implementation of catalogs,
-- language detection, plural rules and placeholders.
--
-- In a module (the domain is the module name; its Russian catalog is saysdf/locales/ru/<name>.toml):
--   local t, tn, N_ = require("saysdf.i18n").domain("quickselect")
--   t("Copied: {text}", { text = s })     -- translation with {name} placeholders substituted
--   tn("{n} file", "{n} files", count)     -- plural form for count; {n} = count
--   N_("Tools")                            -- marks a key for the catalog check, returns it unchanged
-- Rules: keys are double-quoted string literals (the completeness test reads them from the
-- source); call t() when the text is built (in setup() or a handler), never at module load,
-- because the language is chosen by saysdf.setup({ language = ... }) and changes on reload.
--
-- Not a feature module: it has no setup(); saysdf.setup calls M.select() and M.apply_global()
-- before the modules.
-- Tests: each test has its own Lua state; a test that checks text calls M.select("en") (or
-- "ru") in that state, never relies on the system language, and needs no lock.

local M = {}

local native = require("saysdf._i18n")

M.AUTO = "auto"
M.ENV = native.env

local function log_warn(msg)
  local ok, util = pcall(require, "saysdf.util")
  if ok and type(util) == "table" and type(util.log_warn) == "function" then
    return util.log_warn(msg)
  end
  local okw, w = pcall(require, "wezterm")
  if okw and type(w) == "table" and type(w.log_warn) == "function" then
    pcall(w.log_warn, msg)
  end
end

-- Chooses the language ("auto", "en", "ru", ...) and loads the catalogs: embedded ones and the
-- user's <config_dir>/locales/<lang>/*.toml. Broken user files are skipped; all problems go to
-- the log as one warning. Returns the resolved language tag. Affects only this Lua state (each
-- config evaluation and each test has its own language); see M.apply_global.
function M.select(language, config_dir)
  if type(language) ~= "string" then
    language = M.AUTO
  end
  if type(config_dir) ~= "string" then
    config_dir = nil
  end
  local lang, warnings, rejected = native.init(language, config_dir)
  if type(warnings) == "table" and #warnings > 0 then
    log_warn("saysdf.i18n: " .. table.concat(warnings, "; "))
  end
  M.rejected_user_files = tonumber(rejected) or 0
  return lang
end

-- Broken user catalog files skipped by the last M.select of this Lua state (saysdf.setup
-- tells the user once per config load; the details are in the log).
M.rejected_user_files = 0

-- Makes the language of this Lua state the one of the Rust code in the process (status/menu
-- bars, diff rendering in the GUI). Only saysdf.setup calls it, for the config being loaded;
-- tests select a language with M.select and never change the process one.
function M.apply_global()
  return native.apply_global()
end

function M.language()
  return native.language()
end

-- Languages with a catalog (English, embedded, user ones under <config_dir>/locales).
function M.languages(config_dir)
  return native.languages(config_dir)
end

-- Accelerator of a menu entry set by the current language's catalog (table _accel), or nil.
function M.accel(id)
  return native.accel(tostring(id))
end

-- Canonical form of a menu accelerator ("q" -> "Q", "ctrl+q" -> "Ctrl+Q"), or nil if the menu
-- bar would reject it. Two accelerators are the same key when their canonical forms are equal.
function M.accel_key(spec)
  if type(spec) ~= "string" then
    return nil
  end
  return native.accel_key(spec)
end

-- Translation functions of a domain: t(key, vars), tn(one, other, n, vars), N_(key).
function M.domain(name)
  local function t(key, vars)
    return native.t(name, key, vars)
  end
  local function tn(one, other, n, vars)
    return native.tn(name, one, other, tonumber(n) or 0, vars)
  end
  local function N_(key)
    return key
  end
  return t, tn, N_
end

return M
