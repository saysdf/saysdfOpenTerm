-- saysdfTerminal: switchable profiler of Lua handlers (perf-2).
--
-- Enable with the environment variable SAYSDFTERM_PROFILE=1 when starting saysdfTerm. Then
-- saysdf.setup() calls install() before setting up the modules, and every M.FLUSH_S seconds
-- the log (level info, `saysdf.profile:`) gets one line per label:
--   saysdf.profile: <label> n=<calls> avg_ms=<mean> max_ms=<max> total_ms=<sum> wait_ms=<wait>
-- ms=<list> is the time of each call in the window (for p95 over a run; perf-3).
-- avg/max/total is the time the handler held the GUI thread (wall minus the wait in
-- wezterm.run_child_process within the same coroutine); wait_ms is how long the coroutine waited
-- for child processes (the GUI thread is free meanwhile, but the handler's result is late).
-- Labels: ev:<event>@<module> for every wezterm.on handler registered after install
-- (saysdf modules and the user config after setup); seg:<name>/hook:<name> for segments and hooks of
-- saysdf.status; fn:<module>.<function> for hot module functions (M.FUNCS).
-- Without the variable the module does nothing and costs nothing.
--
-- API: enabled(getenv?), install(w?), wrap(label, fn), record(label, sync_ms, wait_ms),
-- flush(force?), now_ms(), _stats(), _reset() (tests).

local M = {}

M.FLUSH_S = 10

-- Hot functions wrapped at install (a field of the module table).
M.FUNCS = {
  { "saysdf.util", "pane_foreground" },
  { "saysdf.git", "root_of" },
  { "saysdf.git", "refresh" },
  { "saysdf.hotbar", "collect" },
  { "saysdf.hotbar", "spec" },
  { "saysdf.palette", "entries" },
  { "saysdf.status", "compose" },
  { "saysdf.tabs", "format" },
  { "saysdf.notify", "on_command_finished" },
  { "saysdf.shell", "on_user_var" },
  { "saysdf.workspaces", "autosave_tick" },
}

local stats = {}
local waited = setmetatable({}, { __mode = "k" }) -- coroutine -> ms spent waiting for child processes
local installed = false
local last_flush = nil
local log = nil

local function wezterm()
  return require("wezterm")
end

function M.enabled(getenv)
  local v = (getenv or os.getenv)("SAYSDFTERM_PROFILE")
  return v ~= nil and v ~= "" and v ~= "0"
end

local clock = nil
function M.now_ms()
  if clock == nil then
    local ok, w = pcall(wezterm)
    if ok and type(w) == "table" and type(w.time) == "table" and type(w.time.now) == "function" then
      clock = function()
        return tonumber(w.time.now():format("%s%.6f")) * 1000
      end
    else
      clock = function()
        return os.clock() * 1000
      end
    end
  end
  return clock()
end

local function wait_of()
  local co = coroutine.running()
  return co and waited[co] or 0, co
end

function M.record(label, sync_ms, wait_ms)
  local s = stats[label]
  if not s then
    s = { n = 0, total = 0, max = 0, wait = 0, ms = {} }
    stats[label] = s
  end
  s.n = s.n + 1
  s.ms[s.n] = string.format("%.2f", sync_ms)
  s.total = s.total + sync_ms
  s.wait = s.wait + (wait_ms or 0)
  if sync_ms > s.max then
    s.max = sync_ms
  end
  M.flush(false)
end

function M.flush(force)
  local now = os.time()
  if last_flush == nil then
    last_flush = now
  end
  if not force and now - last_flush < M.FLUSH_S then
    return
  end
  last_flush = now
  local names = {}
  for k in pairs(stats) do
    table.insert(names, k)
  end
  table.sort(names)
  for _, k in ipairs(names) do
    local s = stats[k]
    local line = string.format(
      "saysdf.profile: %s n=%d avg_ms=%.3f max_ms=%.3f total_ms=%.1f wait_ms=%.1f ms=%s",
      k, s.n, s.total / s.n, s.max, s.total, s.wait, table.concat(s.ms, ","))
    if log then
      log(line)
    else
      pcall(function()
        wezterm().log_info(line)
      end)
    end
  end
  stats = {}
end

function M.wrap(label, fn)
  return function(...)
    local w0, _ = wait_of()
    local t0 = M.now_ms()
    local r = table.pack(pcall(fn, ...))
    local wall = M.now_ms() - t0
    local w1 = wait_of()
    local wait = math.max(0, w1 - w0)
    M.record(label, math.max(0, wall - wait), wait)
    if not r[1] then
      error(r[2], 0)
    end
    return table.unpack(r, 2, r.n)
  end
end

-- The module whose setup is running now (wezterm's Lua has no debug library, so the handler
-- label comes from here); outside module setups it is "config".
local current_module = nil

local function wrap_setups()
  local ok, list = pcall(require, "saysdf._bundled")
  if not ok or type(list) ~= "table" then
    return
  end
  for _, full in ipairs(list) do
    local okm, mod = pcall(require, full)
    if okm and type(mod) == "table" and type(mod.setup) == "function" then
      local orig = mod.setup
      local short = full:gsub("^saysdf%.", "")
      mod.setup = function(...)
        local prev = current_module
        current_module = short
        local r = table.pack(pcall(orig, ...))
        current_module = prev
        if not r[1] then
          error(r[2], 0)
        end
        return table.unpack(r, 2, r.n)
      end
    end
  end
end

-- w is the wezterm table (tests pass their own); opts.log is an output function instead of log_info.
function M.install(w, opts)
  if installed then
    return false
  end
  installed = true
  w = w or wezterm()
  opts = opts or {}
  log = opts.log
  local orig_on = w.on
  w.on = function(name, fn)
    if type(fn) ~= "function" then
      return orig_on(name, fn)
    end
    return orig_on(name, M.wrap("ev:" .. tostring(name) .. "@" .. (current_module or "config"), fn))
  end
  local orig_run = w.run_child_process
  if type(orig_run) == "function" then
    w.run_child_process = function(...)
      local t0 = M.now_ms()
      local r = table.pack(pcall(orig_run, ...))
      local dt = M.now_ms() - t0
      local co = coroutine.running()
      if co then
        waited[co] = (waited[co] or 0) + dt
      end
      if not r[1] then
        error(r[2], 0)
      end
      return table.unpack(r, 2, r.n)
    end
  end
  wrap_setups()
  for _, f in ipairs(M.FUNCS) do
    local ok, mod = pcall(require, f[1])
    if ok and type(mod) == "table" and type(mod[f[2]]) == "function" then
      mod[f[2]] = M.wrap("fn:" .. f[1]:gsub("^saysdf%.", "") .. "." .. f[2], mod[f[2]])
    end
  end
  local ok, status = pcall(require, "saysdf.status")
  if ok and type(status) == "table" then
    local add, hook, consumer = status.add, status.hook, status.set_right_consumer
    status.add = function(name, fn, o)
      return add(name, type(fn) == "function" and M.wrap("seg:" .. tostring(name), fn) or fn, o)
    end
    status.hook = function(name, fn)
      return hook(name, type(fn) == "function" and M.wrap("hook:" .. tostring(name), fn) or fn)
    end
    status.set_right_consumer = function(fn)
      return consumer(type(fn) == "function" and M.wrap("consumer:right", fn) or fn)
    end
  end
  if not opts.quiet then
    local line = "saysdf.profile: on (SAYSDFTERM_PROFILE), flush every " .. M.FLUSH_S .. " s"
    if log then
      log(line)
    else
      pcall(function()
        wezterm().log_info(line)
      end)
    end
  end
  return true
end

function M._stats()
  return stats
end

function M._reset()
  stats = {}
  installed = false
  last_flush = nil
  log = nil
  clock = nil
end

return M
