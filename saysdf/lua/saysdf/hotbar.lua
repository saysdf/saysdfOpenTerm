-- saysdfTerminal: bottom bar with contextual hotkey hints (change hotkey-statusbar).
--
-- On the left: hints for the context of the active pane (saysdf.palette registry entries with a
-- hint field); on the right: the existing saysdf.status.compose (git, metrics), no copy of the logic.
-- Data goes to Rust via window:saysdf_set_bar("bottom", spec) only when the hash changes;
-- a click on a hint -> the saysdf-bar-action event -> saysdf.palette.run by id.
-- While the GUI has no bars (no saysdf_set_bar method) the fallback is hints + status in the
-- right status of the tab bar (set_right_status), as before.
--
-- Options of saysdf.setup({ hotbar = false | {...} }):
--   max_hints = 6            -- how many hints to show
--
-- Pure functions (tests via mlua): context(info) -> id, hints(ctx, entries, max) -> list.
-- Contexts by priority: db_danger > tool > selection > ssh > git > shell.

local t, _, N_ = require("saysdf.i18n").domain("hotbar")

local M = {}

M.CONTEXTS = { "db_danger", "tool", "selection", "ssh", "git", "shell" }

local state = { opts = {}, last_hash = {} }

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

-- The SAYSDF_TOOL/SAYSDF_DB_MODE user vars count only if the foreground process is a local
-- saysdf (process name from pane:get_foreground_process_name(), domain "local"): a program over
-- SSH or a background process in the same pty cannot show or clear the DB write mode warning.
-- The hint is informational anyway; `saysdf db` itself controls the mode.
function M.local_saysdf(info)
  info = info or {}
  local dom = info.domain
  return util().process_basename(info.process) == "saysdf" and (dom == nil or dom == "local")
end

-- info = { db_mode, tool, selection (bool), process, domain, git (bool) }
function M.context(info)
  info = info or {}
  local proc = util().process_basename(info.process)
  if M.local_saysdf(info) then
    if info.db_mode == "write" then
      return "db_danger"
    end
    return "tool"
  end
  if proc == "saysdf" then
    return "tool"
  end
  if info.selection then
    return "selection"
  end
  local dom = tostring(info.domain or "")
  if proc == "ssh" or dom:match("^SSH") then
    return "ssh"
  end
  if info.git then
    return "git"
  end
  return "shell"
end

-- Built-in context hints without a registry command (text only; translated in hints()).
M.STATIC = {
  db_danger = { { text = N_("⚠ DB: WRITE ENABLED"), priority = 0 } },
  tool = { { text = N_("q/Esc exit"), priority = 50 } },
}

-- Hints: entries whose hint.contexts contain ctx or with hint.always; sorted by priority.
-- Returns { { text, id } }.
function M.hints(ctx, entries, max)
  max = max or 6
  local list = {}
  for _, s in ipairs(M.STATIC[ctx] or {}) do
    table.insert(list, { text = t(s.text), priority = s.priority, seq = 0 })
  end
  for i, e in ipairs(entries or {}) do
    local h = type(e) == "table" and e.hint
    if type(h) == "table" and e.id then
      local match = h.always == true
      for _, c in ipairs(h.contexts or {}) do
        if c == ctx then
          match = true
        end
      end
      if match then
        local label = h.label or e.brief or e.id
        local text = e.key and (e.key .. " " .. label) or label
        table.insert(list, { text = text, id = e.id, priority = tonumber(h.priority) or 100, seq = i })
      end
    end
  end
  table.sort(list, function(a, b)
    if a.priority ~= b.priority then
      return a.priority < b.priority
    end
    return a.seq < b.seq
  end)
  local out = {}
  for i = 1, math.min(max, #list) do
    table.insert(out, { text = list[i].text, id = list[i].id })
  end
  return out
end

-- The only place that calls the pane/window API.
function M.collect(window, pane)
  local info = {}
  local okv, vars = pcall(function()
    return pane:get_user_vars()
  end)
  if okv and type(vars) == "table" then
    info.db_mode = vars.SAYSDF_DB_MODE
    info.tool = vars.SAYSDF_TOOL
  end
  local fg = util().pane_foreground(pane)
  info.process = fg.process
  info.alt_screen = fg.alt_screen
  local okd, dom = pcall(function()
    return pane:get_domain_name()
  end)
  info.domain = okd and dom or nil
  local oks, sel = pcall(function()
    return window:get_selection_text_for_pane(pane)
  end)
  info.selection = oks and type(sel) == "string" and sel ~= ""
  local okg, git = pcall(require, "saysdf.git")
  if okg and type(git) == "table" and git.root_of then
    local okr, root = pcall(git.root_of, pane, false)
    info.git = okr and root ~= nil and root ~= false
  end
  return info
end

-- format-items -> a string without attributes (for a bar element).
local function plain(items)
  local t = {}
  for _, it in ipairs(items or {}) do
    if type(it) == "table" and it.Text then
      table.insert(t, it.Text)
    end
  end
  return table.concat(t)
end

function M.spec_from(ctx, entries, r)
  local left = M.hints(ctx, entries, state.opts.max_hints or 6)
  local right = {}
  if r ~= "" then
    table.insert(right, { text = r })
  end
  return { left = left, right = right }
end

function M.spec(window, pane, right_items)
  local info = M.collect(window, pane)
  local ctx = M.context(info)
  local entries = require("saysdf.palette").entries(window, pane)
  return M.spec_from(ctx, entries, plain(right_items)), ctx
end

local function window_id(window)
  local ok, id = pcall(function()
    return window:window_id()
  end)
  return ok and id or 0
end

local function spec_hash(spec)
  local parts = {}
  for _, side in ipairs({ "left", "right" }) do
    for _, it in ipairs(spec[side]) do
      table.insert(parts, side .. "|" .. it.text .. "|" .. tostring(it.id))
    end
  end
  return util().hash(table.concat(parts, "\n"))
end

-- Consumer of the right part of saysdf.status: true means the right part went to the bottom bar.
function M.consume(window, pane, right_items)
  if window.saysdf_set_bar == nil then
    -- fallback before the Rust part: hints before the status, on the right of the tab bar
    local spec = M.spec(window, pane, right_items)
    local items = {}
    for _, h in ipairs(spec.left) do
      table.insert(items, { Text = " " .. h.text .. " " })
    end
    if #right_items > 0 then
      table.insert(items, { Text = " │ " })
      for _, it in ipairs(right_items) do
        table.insert(items, it)
      end
    end
    table.insert(items, { Text = " " })
    window:set_right_status(wezterm().format(items))
    return true
  end
  -- Memoization: the bar input is the context, the registry version and the right part text;
  -- if unchanged there are no hints, no hash and no call into Rust (perf-2).
  local ctx = M.context(M.collect(window, pane))
  local palette = require("saysdf.palette")
  local entries = palette.entries(window, pane)
  local r = plain(right_items)
  local wid = window_id(window)
  local ver = palette.version and palette.version() or nil
  local key = ver and (ctx .. "\n" .. tostring(ver) .. "\n" .. r) or nil
  state.last_key = state.last_key or {}
  if key ~= nil and state.last_key[wid] == key then
    return true
  end
  state.last_key[wid] = key
  state.builds = (state.builds or 0) + 1
  local spec = M.spec_from(ctx, entries, r)
  local h = spec_hash(spec)
  if state.last_hash[wid] ~= h then
    if state.last_hash[wid] == nil and window.set_right_status then
      window:set_right_status("") -- the status is now in the bottom bar
    end
    state.last_hash[wid] = h
    window:saysdf_set_bar("bottom", spec)
  end
  return true
end

function M.setup(opts, config)
  opts = opts or {}
  state = { opts = opts, last_hash = {} }
  local status = require("saysdf.status")
  status.set_right_consumer(M.consume)
  require("saysdf.palette").ensure_bar_action()
  return config
end

M._state = function()
  return state
end

return M
