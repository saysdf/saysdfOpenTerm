-- saysdfTerminal: shared status bar compositor (right and left).
--
-- `window:set_right_status` overwrites whatever another module set, and upstream calls all
-- `update-status` handlers, but each one rewrites the whole string. saysdf modules add segments
-- here, and a single handler assembles them into one status.
--
-- API:
--   require("saysdf.status").add(name, fn(window, pane) -> string | format-items | nil, opts?)
--       opts.order = N   -- left-to-right order (default 100; equal ones keep insertion order)
--       opts.side = "right" | "left"   -- default "right"
--       a repeated add with the same name replaces the segment.
--   require("saysdf.status").render(window, pane)  -- assemble and set the status (both sides)
--   require("saysdf.status").compose(window, pane, side) -> format-items  -- without setting it
--   require("saysdf.status").separator = " │ "  -- separator (string or format-items)
--   require("saysdf.status").hook(name, fn(window, pane))  -- consumer of the update-status tick
--       (saysdf.menu is the top bar, saysdf.hotbar the bottom one); same single handler
--   require("saysdf.status").set_right_consumer(fn(window, pane, items) -> bool | nil)
--       if the consumer returns true, set_right_status is not called (hotbar puts the right part
--       into the bottom bar); false keeps the old behavior (fallback while the GUI has no bars)
--
--   require("saysdf.status").defer(fn())  -- postpone work (process polling etc.) to the end of the tick:
--       render first sets the status and bars, then runs the deferred work (in the same coroutine,
--       so it may wait on wezterm.run_child_process). Outside render it runs immediately.
--
-- A segment error is logged and skipped. An empty result (nil, "", {}) hides the segment.
-- The `update-status` handler is registered lazily on the first add(), once.
--
-- Your own user handler: call render from it
--   wezterm.on("update-status", function(window, pane)
--     require("saysdf.status").render(window, pane)
--   end)
-- (a user handler is called by upstream alongside the module one; render is idempotent,
-- but the last set_right_status wins).

local M = {}

M.separator = " │ "

local segments = {}
local seq = 0
local registered = false
local hooks = {}
local right_consumer = nil
-- Work deferred per render coroutine (different windows render in different coroutines and may
-- interleave at an await); the "main" key is a call outside a coroutine.
local deferred = {}
local function co_key()
  return coroutine.running() or "main"
end

local function wezterm()
  return require("wezterm")
end

local function log_error(msg)
  require("saysdf.util").log_error(msg)
end

local function to_items(v)
  if v == nil or v == "" then
    return nil
  end
  if type(v) == "string" then
    return { { Text = v } }
  end
  if type(v) == "table" then
    if next(v) == nil then
      return nil
    end
    if v.Text ~= nil or v.Foreground ~= nil or v.Background ~= nil or v.Attribute ~= nil
      or v.ResetAttributes ~= nil then
      return { v }
    end
    return v
  end
  return nil
end

function M.add(name, fn, opts)
  if type(name) ~= "string" or type(fn) ~= "function" then
    error("saysdf.status.add(name, fn, opts): a name and a function are required")
  end
  opts = opts or {}
  local side = opts.side == "left" and "left" or "right"
  local entry
  for _, s in ipairs(segments) do
    if s.name == name then
      entry = s
    end
  end
  if not entry then
    seq = seq + 1
    entry = { name = name, seq = seq }
    table.insert(segments, entry)
  end
  entry.fn = fn
  entry.side = side
  entry.order = tonumber(opts.order) or 100
  table.sort(segments, function(a, b)
    if a.order ~= b.order then
      return a.order < b.order
    end
    return a.seq < b.seq
  end)
  M.ensure_handler()
end

function M.ensure_handler()
  if not registered then
    registered = true
    wezterm().on("update-status", function(window, pane)
      M.render(window, pane)
    end)
  end
end

function M.hook(name, fn)
  for _, h in ipairs(hooks) do
    if h.name == name then
      h.fn = fn
      M.ensure_handler()
      return
    end
  end
  table.insert(hooks, { name = name, fn = fn })
  M.ensure_handler()
end

function M.set_right_consumer(fn)
  right_consumer = fn
  M.ensure_handler()
end

function M.compose(window, pane, side)
  side = side or "right"
  local out = {}
  local sep = to_items(M.separator) or {}
  for _, s in ipairs(segments) do
    if s.side == side then
      local ok, res = pcall(s.fn, window, pane)
      if ok then
        local items = to_items(res)
        if items then
          if #out > 0 then
            for _, it in ipairs(sep) do
              table.insert(out, it)
            end
          end
          for _, it in ipairs(items) do
            table.insert(out, it)
          end
        end
      else
        log_error("saysdf.status: segment '" .. s.name .. "': " .. tostring(res))
      end
    end
  end
  return out
end

local function has_side(side)
  for _, s in ipairs(segments) do
    if s.side == side then
      return true
    end
  end
  return false
end

local function apply(window, pane, side, setter)
  if not has_side(side) then
    return
  end
  local items = M.compose(window, pane, side)
  local text = ""
  if #items > 0 then
    table.insert(items, 1, { Text = " " })
    table.insert(items, { Text = " " })
    text = wezterm().format(items)
  end
  window[setter](window, text)
end

function M.defer(fn)
  local list = deferred[co_key()]
  if list then
    table.insert(list, fn)
    return
  end
  local ok, err = pcall(fn)
  if not ok then
    log_error("saysdf.status: deferred: " .. tostring(err))
  end
end

function M.render(window, pane)
  local k = co_key()
  local outer = deferred[k]
  local list = {}
  deferred[k] = list
  local ok, err = pcall(M.render_now, window, pane)
  deferred[k] = outer
  for _, fn in ipairs(list) do
    local okf, errf = pcall(fn)
    if not okf then
      log_error("saysdf.status: deferred: " .. tostring(errf))
    end
  end
  if not ok then
    error(err, 0)
  end
end

function M.render_now(window, pane)
  local consumed = false
  if right_consumer then
    local ok, res = pcall(right_consumer, window, pane, M.compose(window, pane, "right"))
    if ok then
      consumed = res == true
    else
      log_error("saysdf.status: right consumer: " .. tostring(res))
    end
  end
  if not consumed then
    apply(window, pane, "right", "set_right_status")
  end
  apply(window, pane, "left", "set_left_status")
  for _, h in ipairs(hooks) do
    local ok, err = pcall(h.fn, window, pane)
    if not ok then
      log_error("saysdf.status: hook '" .. h.name .. "': " .. tostring(err))
    end
  end
end

-- For tests.
function M._reset()
  hooks = {}
  right_consumer = nil
  deferred = {}
  segments = {}
  seq = 0
  registered = false
end

return M
