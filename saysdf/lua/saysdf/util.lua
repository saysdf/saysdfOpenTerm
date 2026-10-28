-- saysdfTerminal: shared module helpers (data/runtime paths, atomic writes, hash).
--
-- API:
--   data_dir(env?)   -- saysdfTerm data folder: Windows %LOCALAPPDATA%\saysdfTerm,
--                       macOS ~/Library/Application Support/saysdfTerm,
--                       otherwise $XDG_DATA_HOME/saysdfTerm or ~/.local/share/saysdfTerm;
--                       a non-empty SAYSDFTERM_DATA_DIR overrides all of this (isolated test
--                       runs, saysdf/tools/screenshot.ps1; config::DATA_DIR honors it too)
--   runtime_dir(env?) -- $XDG_RUNTIME_DIR/saysdfTerm or <data>/run
--   join(a, b, ...)   -- joins paths with the OS separator
--   is_windows()
--   write_atomic(path, text) -> ok, err   -- via a temp file and rename
--   read_file(path) -> text|nil
--   find_bin(name, env?) -> path|nil     -- binary: wezterm.executable_dir, then PATH
--                                           (env.executable_dir, env.open are for tests)
--   find_bin_cached(name, env?) -> path|nil -- same, cached per Lua state (rule 5: walking PATH is
--                                           file I/O on the GUI thread); "not found" is cached for
--                                           NEGATIVE_TTL_S so a freshly installed tool is picked up without a restart
--   hash(str) -> "xxxxxxxx-xxxxxxxx"     -- two 32-bit hashes (FNV-1a and djb2), not cryptographic
--   foreground_is_tui(info, list?) -> bool  -- info = { alt_screen, process }; alt screen or the base
--                                           process name (no path/.exe, case-insensitive) in the list
--   pane_foreground(pane) -> info           -- snapshot for foreground_is_tui (API errors give empty);
--                                           name from pane:get_foreground_process_name_cached()
--                                           (no process walk on the GUI thread, lags by <=1 call)
--   ensure_dir(dir)                         -- create a folder; the mkdir process runs only if it is missing,
--                                           once per Lua state
--   notify(window, title, text, opts?)      -- user message: a "notice" segment in the status bar
--                                           (bottom bar of saysdf.hotbar / right status) for NOTIFY_S
--                                           seconds AND a system toast (if Windows notifications are off,
--                                           the toast is invisible; the bar stays). opts = { timeout_ms, pane_id,
--                                           toast = false }; modules never call toast_notification themselves;
--                                           title and text are cleaned with single_line
--   single_line(s)                          -- control and invisible/bidi characters -> one space
--   notify_missing_bin(window, name, title?) -- standard "<name> not found" message via notify
-- env is a table for tests: { triple, home, getenv = function(name) ... }.

local M = {}

M.APP_DIR = "saysdfTerm"

local function wezterm()
  return require("wezterm")
end

local function ctx(env)
  env = env or {}
  local w = (env.triple and env.home) and {} or wezterm()
  return env.triple or w.target_triple or "", env.home or w.home_dir or "", env.getenv or os.getenv
end

function M.is_windows(env)
  local triple = ctx(env)
  return triple:find("windows") ~= nil
end

function M.sep(env)
  return M.is_windows(env) and "\\" or "/"
end

function M.join(...)
  local parts = { ... }
  local sep = package.config:sub(1, 1)
  if type(parts[1]) == "table" then
    sep = M.sep(table.remove(parts, 1))
  end
  local out = table.concat(parts, sep)
  return out
end

function M.data_dir(env)
  local triple, home, getenv = ctx(env)
  local override = getenv("SAYSDFTERM_DATA_DIR")
  if override and override ~= "" then
    return override
  end
  if triple:find("windows") then
    local base = getenv("LOCALAPPDATA") or getenv("APPDATA") or (home .. "\\AppData\\Local")
    return base .. "\\" .. M.APP_DIR
  elseif triple:find("darwin") or triple:find("apple") then
    return home .. "/Library/Application Support/" .. M.APP_DIR
  end
  local xdg = getenv("XDG_DATA_HOME")
  if xdg and xdg ~= "" then
    return xdg .. "/" .. M.APP_DIR
  end
  return home .. "/.local/share/" .. M.APP_DIR
end

function M.runtime_dir(env)
  local triple, _, getenv = ctx(env)
  local xdg = (not triple:find("windows")) and getenv("XDG_RUNTIME_DIR") or nil
  if xdg and xdg ~= "" then
    return xdg .. "/" .. M.APP_DIR
  end
  return M.data_dir(env) .. (triple:find("windows") and "\\run" or "/run")
end

function M.read_file(path)
  local f = io.open(path, "rb")
  if not f then
    return nil
  end
  local s = f:read("a")
  f:close()
  return s
end

local BS = string.char(92)

local function file_exists(path, open)
  local f = (open or io.open)(path, "rb")
  if f then
    f:close()
    return true
  end
  return false
end

-- Looks for an executable next to saysdfTerm, then in PATH. Adds .exe on Windows.
function M.find_bin(name, env)
  env = env or {}
  local _, _, getenv = ctx(env)
  local win = M.is_windows(env)
  local file = (win and not name:lower():match("%.exe$")) and (name .. ".exe") or name
  local sep = M.sep(env)
  local dir = env.executable_dir
  if dir == nil and not env.triple then
    local ok, d = pcall(function()
      return wezterm().executable_dir
    end)
    dir = ok and d or nil
  end
  if type(dir) == "string" and dir ~= "" then
    local p = dir .. sep .. file
    if file_exists(p, env.open) then
      return p
    end
  end
  local path = getenv("PATH") or getenv("Path") or ""
  for entry in path:gmatch("[^" .. (win and ";" or ":") .. "]+") do
    entry = entry:gsub('^"(.*)"$', "%1"):gsub("[/" .. BS .. "]+$", "")
    if entry ~= "" then
      local p = entry .. sep .. file
      if file_exists(p, env.open) then
        return p
      end
    end
  end
  return nil
end

M.NEGATIVE_TTL_S = 30
local bin_cache = {}

function M.find_bin_cached(name, env)
  local now = (env and env.now) or os.time()
  local c = bin_cache[name]
  if c and (c.path or now - c.at < M.NEGATIVE_TTL_S) then
    return c.path or nil
  end
  local p = M.find_bin(name, env)
  bin_cache[name] = { path = p or false, at = now }
  return p
end

function M._forget_bins()
  bin_cache = {}
end

-- Folders that already exist (or were created) in this Lua state. os.execute runs
-- cmd.exe/sh synchronously on the GUI thread: on Windows that freezes the window for seconds (perf-1:
-- 2-6 s on every workspaces autosave). So the process runs only for a missing folder, once.
local known_dirs = {}

local function dir_exists(dir, env)
  local rd = env and env.read_dir
  if rd == nil then
    local okw, w = pcall(wezterm)
    rd = okw and type(w) == "table" and w.read_dir or nil
  end
  return rd ~= nil and (pcall(rd, dir)) or false
end

-- env (tests): { read_dir = fn, run = fn(argv), execute = fn(cmd), windows = bool }
function M.ensure_dir(dir, env)
  if known_dirs[dir] then
    return
  end
  if not dir_exists(dir, env) then
    local win = env and env.windows
    if win == nil then
      win = M.is_windows()
    end
    -- Asynchronously first (wezterm.run_child_process: no console window, the GUI thread does not wait;
    -- cmd /d skips AutoRun). Outside a coroutine (config evaluation) it fails, so fall back to os.execute.
    local run = env and env.run
    if run == nil then
      local okw, w = pcall(wezterm)
      run = okw and type(w) == "table" and w.run_child_process or nil
    end
    local argv = win and { "cmd.exe", "/d", "/c", "mkdir", dir } or { "mkdir", "-p", dir }
    if not (run and pcall(run, argv)) then
      local exec = env and env.execute or os.execute
      if win then
        exec('mkdir "' .. dir .. '" 2>nul')
      else
        exec("mkdir -p '" .. dir:gsub("'", "'\\''") .. "' 2>/dev/null")
      end
    end
  end
  known_dirs[dir] = true
end

function M._forget_dirs()
  known_dirs = {}
end

function M.write_atomic(path, text)
  local tmp = path .. ".tmp"
  local f, err = io.open(tmp, "wb")
  if not f then
    return false, err
  end
  f:write(text)
  f:close()
  local ok, rerr = os.rename(tmp, path)
  if not ok then
    -- Windows: rename does not replace an existing file
    os.remove(path)
    ok, rerr = os.rename(tmp, path)
  end
  return ok and true or false, rerr
end

function M.hash(s)
  local fnv, djb = 2166136261, 5381
  for i = 1, #s do
    local b = s:byte(i)
    fnv = ((fnv ~ b) * 16777619) & 0xffffffff
    djb = ((djb * 33) + b) & 0xffffffff
  end
  return string.format("%08x-%08x", fnv, djb)
end

-- Safe logging: wezterm.log_* may be missing (no-GUI context), then it silently does nothing.
local function safe_log(level, msg)
  local ok, w = pcall(require, "wezterm")
  if ok and type(w) == "table" and type(w["log_" .. level]) == "function" then
    pcall(w["log_" .. level], msg)
  end
end

function M.log_info(msg) safe_log("info", msg) end
function M.log_warn(msg) safe_log("warn", msg) end
function M.log_error(msg) safe_log("error", msg) end

-- saysdfTerminal: who is in the pane foreground (menu-bar D5). Shared by menu, help, hotbar.
M.TUI_PROCESSES = {
  "vim", "nvim", "vi", "nano", "micro", "helix", "hx", "emacs", "mc", "far", "htop", "btop", "top",
  "less", "more", "man", "lazygit", "gitui", "tig", "claude", "saysdf", "tmux", "screen", "zellij",
}

function M.process_basename(p)
  if type(p) ~= "string" or p == "" then
    return nil
  end
  local base = p:match("([^/\\]+)$") or p
  base = base:lower():gsub("%.exe$", "")
  return base
end

function M.foreground_is_tui(info, list)
  info = info or {}
  if info.alt_screen then
    return true
  end
  local base = M.process_basename(info.process)
  if not base then
    return false
  end
  for _, name in ipairs(list or M.TUI_PROCESSES) do
    if tostring(name):lower():gsub("%.exe$", "") == base then
      return true
    end
  end
  return false
end

-- Foreground process name. Upstream get_foreground_process_name() on Windows synchronously
-- walks the process tree (~10 ms on the GUI thread). The fork's get_foreground_process_name_cached()
-- (lua-api-crates/mux/src/saysdf_fg.rs) returns the last known answer at once and refreshes it
-- on a background thread: the answer lags by at most one call (a status tick); only the
-- first request for a pane is synchronous (perf-3). No separate pane-fingerprint cache is needed.
local function fg_name(pane)
  local ok, name = pcall(function()
    return pane:get_foreground_process_name_cached()
  end)
  if ok then
    return name
  end
  -- old binary / test pane without the fork method
  ok, name = pcall(function()
    return pane:get_foreground_process_name()
  end)
  return ok and name or nil
end

function M.pane_foreground(pane)
  local info = {}
  if pane == nil then
    return info
  end
  local ok, alt = pcall(function()
    return pane:is_alt_screen_active()
  end)
  info.alt_screen = ok and alt or false
  info.process = fg_name(pane)
  return info
end

-- ---- notify: status bar message + system toast ----
M.NOTIFY_S = 5
local notices = {} -- [window_id] = { text, until_s }
local notice_registered = false

function M._now()
  return os.time()
end

local function win_id(window)
  local ok, id = pcall(function()
    return window:window_id()
  end)
  return ok and id or "?"
end

-- The window's current message (nil: none or expired).
function M.notice_text(window)
  local n = notices[win_id(window)]
  if n and M._now() < n.until_s then
    return n.text
  end
  notices[win_id(window)] = nil
  return nil
end

local function notice_segment(window)
  local t = M.notice_text(window)
  if not t then
    return nil
  end
  return { { Attribute = { Intensity = "Bold" } }, { Text = "● " .. t }, "ResetAttributes" }
end

-- One line of plain text: control characters and invisible/bidi format characters (RLO,
-- zero-width, ...) become a space. The set is saysdf_i18n::text (native saysdf._i18n), shared
-- with the GUI bars and catalog checks; without the native module only %c is replaced.
function M.single_line(s)
  s = tostring(s or "")
  local ok, native = pcall(require, "saysdf._i18n")
  if ok and type(native) == "table" and type(native.single_line) == "function" then
    return native.single_line(s)
  end
  return (s:gsub("%c+", " "))
end

function M.notify(window, title, text, opts)
  opts = opts or {}
  -- Text often carries untrusted data (file names, command lines, errors of tools): the status
  -- bar and the toast get it as one clean line (security review 2026-10-21-i18n, I5)
  title = M.single_line(title or "saysdfTerm")
  text = M.single_line(text)
  if window == nil then
    M.log_info(title .. ": " .. text)
    return false
  end
  local line = (title ~= "" and (title .. ": ") or "") .. text
  local timeout = tonumber(opts.timeout_ms) or (M.NOTIFY_S * 1000)
  notices[win_id(window)] = { text = line, until_s = M._now() + math.max(1, math.ceil(timeout / 1000)) }
  local okst, status = pcall(require, "saysdf.status")
  if okst and type(status) == "table" then
    if not notice_registered then
      notice_registered = true
      pcall(status.add, "notice", notice_segment, { order = 0 })
    end
    -- show it at once, without waiting for the update-status tick
    pcall(function()
      status.render(window, window:active_pane())
    end)
  end
  if opts.toast == false then
    return true
  end
  local ok, err = pcall(function()
    window:toast_notification(title, text, nil, opts.timeout_ms, opts.pane_id)
  end)
  if not ok then
    M.log_warn("saysdf.util.notify: toast: " .. tostring(err))
  end
  return ok
end

-- Standard toast for a missing helper binary (saysdf, saysdfTerm-cli, ...); shared by modules.
function M.notify_missing_bin(window, name, title)
  local t = require("saysdf.i18n").domain("util")
  return M.notify(window, title or "saysdfTerm", t("{name} not found (next to saysdfTerm or in PATH)", { name = name }))
end

function M._forget_notices()
  notices = {}
  notice_registered = false
end

return M
