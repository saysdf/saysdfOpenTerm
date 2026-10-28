-- saysdfTerminal: notifications about long commands and safe OSC 9/777 (change command-notifications).
--
-- setup options (require("saysdf").setup({ notify = {...} })); notify = false disables the module:
--   min_duration_s = 10     -- command duration threshold, seconds
--   on = "all"              -- "all" - both success and failure; "error" - only non-zero exit code
--   dnd = false             -- initial "do not disturb" state (not kept between launches)
--   app_notifications = true -- false - OSC 9/777 is handled by upstream (notification_handling)
--   title_max = 64, body_max = 256           -- length limits of OSC notifications (characters)
--   rate = { interval_s = 5, burst = 3 }     -- per-pane token bucket for OSC notifications
--   palette = true          -- the "do not disturb" palette entry
--
-- Command data comes from the saysdf-command-finished event (saysdf.shell). OSC 9/777 arrive as the Lua event
-- saysdf-app-notification(window, pane, title, body) from a narrow patch in wezterm-gui/src/frontend.rs;
-- the handler always returns false (upstream's standard display is cancelled), except app_notifications = false.
-- Attention indicator: saysdf.tabs.set_attention (cleared when the tab is activated).
-- Clicking the toast focuses the tab on Windows (5th argument of window:toast_notification is pane_id).
--
--
-- Relation to upstream notification_handling: while the module is enabled (app_notifications ~= false), it filters
-- OSC 9/777 itself - display is suppressed only for the focused pane (like SuppressFromFocusedPane),
-- notification_handling does not apply. With notify = false or app_notifications = false, the upstream
-- notification_handling policy works (clicking such a toast on Windows also focuses the pane).
--
-- OS limits: click -> tab works only on Windows and only while saysdfTerm is running; on macOS
-- and Linux (dbus) the toast is shown, the fallback is the attention indicator on the tab. "Do not disturb" is
-- our own, unrelated to Focus Assist/macOS Focus. No sound.
--
-- API: toggle_dnd() -> bool, set_dnd(on), is_dnd(); pure functions decide/should_notify,
-- format_duration, format_toast, sanitize, take_token - for tests and other modules.

local M = {}

local t = require("saysdf.i18n").domain("notify")

local function wezterm()
  return require("wezterm")
end

M.DEFAULTS = {
  min_duration_s = 10,
  on = "all",
  dnd = false,
  app_notifications = true,
  title_max = 64,
  body_max = 256,
  rate = { interval_s = 5, burst = 3 },
  palette = true,
}

M.EVENT_FINISHED = "saysdf-command-finished"
M.EVENT_APP = "saysdf-app-notification"

-- ---- pure functions ----

local function opt(opts, key)
  if opts ~= nil and opts[key] ~= nil then
    return opts[key]
  end
  return M.DEFAULTS[key]
end

-- Decision for a finished command. info: { exit_code, duration_ms };
-- focus: { window_focused, tab_active, pane_active }; opts: module options + dnd.
-- Returns nil (do nothing) or { toast = bool, attention = true }.
function M.decide(info, focus, opts)
  info = info or {}
  focus = focus or {}
  local ms = tonumber(info.duration_ms)
  if ms == nil or ms < (tonumber(opt(opts, "min_duration_s")) or 10) * 1000 then
    return nil
  end
  if opt(opts, "on") == "error" and tonumber(info.exit_code) == 0 then
    return nil
  end
  if focus.window_focused and focus.tab_active and focus.pane_active then
    return nil
  end
  return { toast = not opt(opts, "dnd"), attention = true }
end

function M.should_notify(info, focus, opts)
  return M.decide(info, focus, opts) ~= nil
end

-- 40000 -> "40s", 65000 -> "1m 05s", 3725000 -> "1h 02m".
function M.format_duration(ms)
  local s = math.floor((tonumber(ms) or 0) / 1000)
  if s < 60 then
    return string.format("%ds", s)
  elseif s < 3600 then
    return string.format("%dm %02ds", s // 60, s % 60)
  end
  return string.format("%dh %02dm", s // 3600, (s % 3600) // 60)
end

-- Toast title and body: "✗ build — 40s, exit 1" / "✓ build — 1m 05s".
function M.format_toast(info, tab_title)
  info = info or {}
  local code = tonumber(info.exit_code) or 0
  local name = (type(tab_title) == "string" and tab_title ~= "") and tab_title or "saysdfTerm"
  local title = (code == 0 and "✓ " or "✗ ") .. name .. " — " .. M.format_duration(info.duration_ms)
  if code ~= 0 then
    title = title .. ", exit " .. tostring(code)
  end
  local body = code == 0 and t("Command finished") or t("Command failed")
  return title, body
end

-- Removes control (C0, DEL, C1) and bidi characters, truncates to max characters.
local BIDI = {
  [0x061C] = true, [0x200E] = true, [0x200F] = true,
  [0x202A] = true, [0x202B] = true, [0x202C] = true, [0x202D] = true, [0x202E] = true,
  [0x2066] = true, [0x2067] = true, [0x2068] = true, [0x2069] = true,
}

local function clean(s, max)
  if type(s) ~= "string" then
    return ""
  end
  local out = {}
  local n = 0
  if utf8.len(s) == nil then
    -- invalid UTF-8: printable ASCII only
    for i = 1, #s do
      local b = s:byte(i)
      if b >= 0x20 and b < 0x7F then
        if n >= max then
          break
        end
        n = n + 1
        out[n] = string.char(b)
      end
    end
  else
    for _, c in utf8.codes(s) do
      local bad = c < 0x20 or c == 0x7F or (c >= 0x80 and c <= 0x9F) or BIDI[c]
      if not bad then
        if n >= max then
          break
        end
        n = n + 1
        out[n] = utf8.char(c)
      end
    end
  end
  return (table.concat(out):gsub("^%s+", ""):gsub("%s+$", ""))
end

-- OSC 9 (body only) and OSC 777 (title + body) -> safe title, body.
function M.sanitize(title, body, opts)
  local ti = clean(title, opt(opts, "title_max"))
  local bo = clean(body, opt(opts, "body_max"))
  if ti == "" then
    ti = clean(bo, opt(opts, "title_max"))
    bo = ti == bo and "" or bo
  end
  return ti, bo
end

-- Token bucket: state[key] = { tokens, t }. Returns true if the message may be shown.
function M.take_token(state, key, now, opts)
  local rate = opt(opts, "rate") or M.DEFAULTS.rate
  local burst = tonumber(rate.burst) or 3
  local interval = tonumber(rate.interval_s) or 5
  local b = state[key]
  if b == nil then
    b = { tokens = burst, t = now }
    state[key] = b
  end
  if interval > 0 then
    b.tokens = math.min(burst, b.tokens + (now - b.t) / interval)
  end
  b.t = now
  if b.tokens >= 1 then
    b.tokens = b.tokens - 1
    return true
  end
  return false
end

-- ---- state ----

local current = { opts = {} }
local buckets = {}
local dropped = {}

function M.is_dnd()
  local g = wezterm().GLOBAL
  if g ~= nil and g.saysdf_notify_dnd ~= nil then
    return g.saysdf_notify_dnd == true
  end
  return current.opts.dnd == true
end

function M.set_dnd(on)
  local g = wezterm().GLOBAL
  if g ~= nil then
    g.saysdf_notify_dnd = on and true or false
  end
  current.opts.dnd = on and true or false
end

function M.toggle_dnd()
  local on = not M.is_dnd()
  M.set_dnd(on)
  require("saysdf.util").log_info("saysdf.notify: do not disturb " .. (on and "on" or "off"))
  return on
end

local function effective_opts()
  local o = {}
  for k, v in pairs(current.opts) do
    o[k] = v
  end
  o.dnd = M.is_dnd()
  return o
end

-- ---- handlers (wezterm environment) ----

local function focus_of(window, pane)
  local f = { window_focused = false, tab_active = false, pane_active = false }
  pcall(function()
    f.window_focused = window:is_focused() and true or false
  end)
  pcall(function()
    local tab = pane:tab()
    local active = window:active_tab()
    f.tab_active = tab ~= nil and active ~= nil and tab:tab_id() == active:tab_id()
  end)
  pcall(function()
    f.pane_active = window:active_pane():pane_id() == pane:pane_id()
  end)
  return f
end

local function set_attention(pane)
  local ok, tabs = pcall(require, "saysdf.tabs")
  if not ok or type(tabs) ~= "table" or type(tabs.set_attention) ~= "function" then
    return
  end
  pcall(function()
    local tab = pane:tab()
    if tab then
      tabs.set_attention(tab, true)
    end
  end)
end

local function toast(window, pane, title, body)
  local pane_id
  pcall(function()
    pane_id = pane:pane_id()
  end)
  return require("saysdf.util").notify(window, title, body, { pane_id = pane_id })
end

local function tab_title(pane)
  local title = ""
  pcall(function()
    title = pane:tab():get_title() or ""
  end)
  if title == "" then
    pcall(function()
      title = pane:get_title() or ""
    end)
  end
  return title
end

function M.on_command_finished(window, pane, info)
  info = info or {}
  if info.duration_ms == nil then
    pcall(function()
      info.duration_ms = tonumber((pane:get_user_vars() or {}).SAYSDF_LAST_DURATION_MS)
    end)
  end
  local o = effective_opts()
  -- Decide without focus first: a short command (almost all) exits without touching the window.
  -- is_focused/active_pane are async GUI calls, ~3 ms each per command (perf-2).
  if M.decide(info, {}, o) == nil then
    return
  end
  local d = M.decide(info, focus_of(window, pane), o)
  if d == nil then
    return
  end
  if d.attention then
    set_attention(pane)
  end
  if d.toast then
    local title, body = M.format_toast(info, tab_title(pane))
    toast(window, pane, title, body)
  end
end

-- Returns false: upstream's standard display is cancelled.
function M.on_app_notification(window, pane, title, body, now)
  local o = effective_opts()
  if o.app_notifications == false then
    return true
  end
  local id = "?"
  pcall(function()
    id = tostring(pane:pane_id())
  end)
  if not M.take_token(buckets, id, now or os.time(), o) then
    dropped[id] = (dropped[id] or 0) + 1
    if dropped[id] == 1 or dropped[id] % 50 == 0 then
      require("saysdf.util").log_info("saysdf.notify: rate limit, dropped " .. dropped[id] .. " notification(s) from pane " .. id)
    end
    return false
  end
  local st, sb = M.sanitize(title, body, o)
  if st == "" then
    return false
  end
  local f = window and focus_of(window, pane) or {}
  if f.window_focused and f.tab_active and f.pane_active then
    return false
  end
  set_attention(pane)
  if not o.dnd and window then
    toast(window, pane, st, sb)
  end
  return false
end

-- action_callback registers a new event on every call (config/src/lua.rs wrap_callback),
-- and palette.entries is called on every update-status tick - create the action once (perf-2).
local dnd_action = nil

function M.palette_entries()
  local w = wezterm()
  dnd_action = dnd_action or w.action_callback(function()
    M.toggle_dnd()
  end)
  return {
    {
      id = "notify.dnd",
      menu = "View",
      accel = "d",
      order = 40,
      brief = M.is_dnd() and t("Notify: turn off do not disturb") or t("Notify: turn on do not disturb"),
      icon = "md_bell_off",
      action = dnd_action,
    },
  }
end

-- ---- setup ----

local registered = false

function M.setup(opts, config)
  opts = opts or {}
  local o = {}
  for k, v in pairs(opts) do
    o[k] = v
  end
  if o.on ~= nil and o.on ~= "all" and o.on ~= "error" then
    require("saysdf.util").log_warn("saysdf.notify: unknown on='" .. tostring(o.on) .. "', using 'all'")
    o.on = "all"
  end
  current.opts = o

  if not registered then
    registered = true
    local w = wezterm()
    w.on(M.EVENT_FINISHED, function(window, pane, info)
      local ok, err = pcall(M.on_command_finished, window, pane, info)
      if not ok then
        require("saysdf.util").log_error("saysdf.notify: " .. tostring(err))
      end
    end)
    w.on(M.EVENT_APP, function(window, pane, title, body)
      local ok, res = pcall(M.on_app_notification, window, pane, title, body)
      if not ok then
        require("saysdf.util").log_error("saysdf.notify: " .. tostring(res))
        return false
      end
      return res
    end)
    if opts.palette ~= false then
      require("saysdf.palette").add(function()
        return M.palette_entries()
      end)
    end
  end
  return config
end

return M
