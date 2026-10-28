-- saysdfTerminal: load metrics in the status bar and a monitor in a split (change system-monitor, D1, D5).
--
-- saysdf.setup({ monitor = {
--   interval = 2,          -- seconds between collector samples (at least 1)
--   enabled = false,       -- enable the segment right at GUI startup
--   keys = true,           -- hotkeys
--   key = { key = "I", mods = "CTRL|SHIFT" },        -- metrics in the status bar on/off
--   split_key = { key = "J", mods = "CTRL|SHIFT" },  -- `saysdf mon` in a split
--   split = { direction = "Right", size = 0.45 },
--   palette = true,        -- "Monitor: ..." entries in the palette
--   bin = nil,             -- path to saysdf (by default next to saysdfTerm or in PATH)
-- } })
--
-- How it works: the hotkey starts a background `saysdf mon --status-file <runtime>/mon-<pid>.json
-- --interval N --parent-pid <GUI pid>` that writes JSON atomically. The status bar segment
-- (saysdf.status) only reads the file and never starts processes per frame. Disabling deletes the
-- file; the collector exits within one interval (or when the GUI is closed). A file older than
-- 3 intervals or a broken one gives `mon: n/a`.
--
-- API (pure functions for tests):
--   M.parse_opts(opts) -> normalized options
--   M.format_status(sample, now, interval) -> string; sample is the parsed JSON or nil
--   M.fmt_pct(v), M.fmt_bytes(v), M.fmt_mem(used, total)

local M = {}

local t = require("saysdf.i18n").domain("monitor")

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

local state = { opts = nil, enabled = false, path = nil, last_reason = nil }

function M.parse_opts(opts)
  opts = type(opts) == "table" and opts or {}
  local o = {}
  local i = tonumber(opts.interval) or 2
  if i ~= i or i < 1 then
    i = 1
  end
  o.interval = i
  o.enabled = opts.enabled == true
  o.keys = opts.keys ~= false
  o.palette = opts.palette ~= false
  o.key = type(opts.key) == "table" and opts.key or { key = "I", mods = "CTRL|SHIFT" }
  o.split_key = type(opts.split_key) == "table" and opts.split_key or { key = "J", mods = "CTRL|SHIFT" }
  local sp = type(opts.split) == "table" and opts.split or {}
  o.split = { direction = sp.direction or "Right", size = tonumber(sp.size) or 0.45 }
  o.bin = type(opts.bin) == "string" and opts.bin ~= "" and opts.bin or nil
  return o
end

function M.fmt_pct(v)
  v = tonumber(v) or 0
  if v < 0 then
    v = 0
  end
  return string.format("%.0f%%", v)
end

function M.fmt_bytes(v)
  v = tonumber(v) or 0
  if v < 1024 then
    return string.format("%dB", math.floor(v))
  end
  local units = { "K", "M", "G", "T", "P" }
  local x, i = v / 1024, 1
  while x >= 1024 and i < #units do
    x = x / 1024
    i = i + 1
  end
  if x < 10 then
    return string.format("%.1f%s", x, units[i])
  end
  return string.format("%.0f%s", x, units[i])
end

function M.fmt_mem(used, total)
  local g = 1024 * 1024 * 1024
  local t = (tonumber(total) or 0) / g
  local ts
  if t >= 10 or math.abs(t - math.floor(t + 0.5)) < 0.05 then
    ts = string.format("%.0f", t)
  else
    ts = string.format("%.1f", t)
  end
  return string.format("%.1f/%sG", (tonumber(used) or 0) / g, ts)
end

-- Segment text: `CPU 12% RAM 7.1/16G ↓1.2M ↑40K D 3.0M/s` (+ ` GPU n%`), or `mon: n/a`.
-- The second value is the reason for n/a (for the log).
function M.format_status(s, now, interval)
  interval = tonumber(interval) or 2
  if type(s) ~= "table" or type(s.ts) ~= "number" or type(s.cpu) ~= "number" then
    return "mon: n/a", "no data or broken JSON"
  end
  if now and now - s.ts > 3 * interval then
    return "mon: n/a", "data is stale (" .. tostring(now - s.ts) .. " s)"
  end
  local out = string.format(
    "CPU %s RAM %s ↓%s ↑%s D %s/s",
    M.fmt_pct(s.cpu),
    M.fmt_mem(s.mem_used, s.mem_total),
    M.fmt_bytes(s.net_rx),
    M.fmt_bytes(s.net_tx),
    M.fmt_bytes((tonumber(s.disk_read) or 0) + (tonumber(s.disk_write) or 0))
  )
  if type(s.gpu) == "number" then
    out = out .. " GPU " .. M.fmt_pct(s.gpu)
  end
  return out, nil
end

local function gui_pid()
  local ok, pid = pcall(function()
    return wezterm().procinfo.pid()
  end)
  return ok and pid or nil
end

local function bin()
  return state.opts.bin or util().find_bin("saysdf")
end

local function toast(window, msg)
  require("saysdf.util").notify(window, "saysdf mon", msg)
end

local function status_path()
  local u = util()
  return u.join(u.runtime_dir(), "mon-" .. tostring(gui_pid() or "x") .. ".json")
end

-- Reads the file and formats it (called from update-status; starts nothing).
function M.segment()
  if not state.enabled or not state.path then
    return nil
  end
  local text = util().read_file(state.path)
  local sample
  if text then
    local ok, v = pcall(wezterm().json_parse, text)
    if ok then
      sample = v
    end
  end
  local s, reason = M.format_status(sample, os.time(), state.opts.interval)
  if reason ~= state.last_reason then
    state.last_reason = reason
    if reason then
      util().log_warn("saysdf.monitor: " .. reason .. " (" .. tostring(state.path) .. ")")
    end
  end
  return s
end

function M.enable(window)
  local b = bin()
  if not b then
    toast(window, t("saysdf not found (next to saysdfTerm or in PATH)"))
    return false
  end
  local u = util()
  local path = status_path()
  u.ensure_dir(u.runtime_dir())
  local args = { b, "mon", "--status-file", path, "--interval", tostring(state.opts.interval) }
  local pid = gui_pid()
  if pid then
    table.insert(args, "--parent-pid")
    table.insert(args, tostring(pid))
  end
  local ok, err = pcall(wezterm().background_child_process, args)
  if not ok then
    u.log_error("saysdf.monitor: failed to start the collector: " .. tostring(err))
    toast(window, t("Failed to start the metrics collector"))
    return false
  end
  state.path = path
  state.enabled = true
  state.last_reason = nil
  return true
end

function M.disable()
  state.enabled = false
  if state.path then
    os.remove(state.path)
  end
end

function M.toggle(window, pane)
  if state.enabled then
    M.disable()
  else
    M.enable(window)
  end
  pcall(function()
    require("saysdf.status").render(window, pane)
  end)
end

function M.open_split(window, pane)
  local b = bin()
  if not b then
    toast(window, t("saysdf not found (next to saysdfTerm or in PATH)"))
    return
  end
  local args = { b, "mon" }
  local ok, info = pcall(function()
    return pane:get_foreground_process_info()
  end)
  if ok and type(info) == "table" and info.pid then
    table.insert(args, "--highlight-pid")
    table.insert(args, tostring(info.pid))
  end
  local sp = state.opts.split
  pane:split({ args = args, direction = sp.direction, size = sp.size })
end

function M.setup(opts, config)
  state = { opts = M.parse_opts(opts), enabled = false, path = nil, last_reason = nil }
  local w = wezterm()
  require("saysdf.status").add("monitor", function()
    return M.segment()
  end, { order = 50 })
  if state.opts.enabled then
    w.on("gui-startup", function()
      M.enable(nil)
    end)
  end
  if state.opts.keys then
    config.keys = config.keys or {}
    table.insert(config.keys, {
      key = state.opts.key.key,
      mods = state.opts.key.mods,
      action = w.action_callback(M.toggle),
    })
    table.insert(config.keys, {
      key = state.opts.split_key.key,
      mods = state.opts.split_key.mods,
      action = w.action_callback(M.open_split),
    })
  end
  if state.opts.palette then
    local palette = require("saysdf.palette")
    local kt = state.opts.keys and palette.key_text(state.opts.key) or nil
    local skt = state.opts.keys and palette.key_text(state.opts.split_key) or nil
    palette.add({ id = "monitor.toggle", menu = "View", accel = "m", order = 30, key = kt, brief = t("Monitor: status bar metrics (on/off)"), icon = "md_chart_line", action = w.action_callback(M.toggle) })
    palette.add({ id = "monitor.split", menu = "Tools", order = 40, key = skt, label = t("Load monitor"), accel = "m", brief = t("Monitor: processes and charts in a split"), icon = "md_chart_line", action = w.action_callback(M.open_split) })
  end
  return config
end

M._state = function()
  return state
end

return M
