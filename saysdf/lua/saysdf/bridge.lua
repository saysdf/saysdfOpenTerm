-- saysdfTerminal: trusted user-var channel from a fork TUI back to Lua (change text-annotations,
-- design D7; extracted from saysdf.sidebar, change tool-sidebar D3).
--
-- A fork TUI (`saysdf pick`, `saysdf files`, `saysdf view`) sends the user var `<VAR>` =
-- `<token>:<kind>:<payload>`. Terminal output is untrusted, so a value is accepted only if ALL
-- checks pass, in this order:
--   1) the pane is owned by the caller (`lookup(pane)` returns its record: sidebar pane, viewer...);
--   2) the value is a string of at most `max` bytes (default 4096);
--   3) the value has the `<token>:<kind>:<payload>` format;
--   4) the token matches the record's token and the record is not spent (`used`);
--   5) the expected binary (default `saysdf`) is the pane's foreground process
--      (get_foreground_process_name_cached; nil is a refusal that does not spend the token,
--      the TUI retries the send);
--   6) allow-list: `kind` has a handler; the handler validates the payload itself
--      and spends the token with `spend` / `consume` before running anything.
-- Refusals are logged once per reason and channel (log_info).
--
-- API:
--   MAX_VALUE, BIN
--   new_token() -> string                 -- 32 hex, one-time token for a TUI's env
--   parse(value, max?) -> token, kind, payload | nil, reason
--   foreground_is(pane, bin?) -> true | false | nil (not known yet)
--   check(pane, rec, value, o?) -> token, kind, payload | nil, reason   -- checks 2-5
--                                         o = { max?, bin? }
--   spend(rec)                            -- one-time record: rec.used = true
--   consume(rec, value) -> true | nil, "repeat"
--                                         -- rec.multi: several values per lifetime, an exact
--                                            repeat of the last value (send retries) is refused;
--                                            otherwise the record is spent
--   rejecter(prefix) -> { reject(reason) -> false, reset() }
--   channel(spec) -> { handle(window, pane, name, value) -> bool, reject(reason), reset() }
--     spec = { var, name (log prefix), lookup(pane) -> rec | nil, not_owned = reason?,
--              max?, bin?, kinds = { [kind] = fn(window, pane, rec, payload, value) -> true | nil, reason } }
--   Handlers return true when the value was accepted and executed, or nil/false plus a reason;
--   a handler validates the payload first, then spends the token (spend/consume), then runs.
--   An error inside a handler is logged (log_error) and counts as a refusal.

local M = {}

M.MAX_VALUE = 4096
M.BIN = "saysdf"

local counter = 0

local function util()
  return require("saysdf.util")
end

-- 32 hex: two hashes of the time, a counter and a table address. Not a secret from the user's own
-- processes, but a program in another pane cannot guess it, and it is one-time.
function M.new_token()
  counter = counter + 1
  local seed = table.concat({ tostring(os.time()), tostring(os.clock()), tostring(counter), tostring({}), tostring(math.random()) }, "|")
  local u = util()
  return (u.hash(seed) .. u.hash(seed .. "#" .. counter)):gsub("-", "")
end

function M.parse(value, max)
  if type(value) ~= "string" or #value > (max or M.MAX_VALUE) then
    return nil, "length"
  end
  local token, kind, payload = value:match("^([^:]*):([^:]*):(.*)$")
  if not token then
    return nil, "format"
  end
  return token, kind, payload
end

function M.foreground_is(pane, bin)
  local ok, name = pcall(function()
    return pane:get_foreground_process_name_cached()
  end)
  if not ok or name == nil then
    return nil
  end
  return util().process_basename(name) == (bin or M.BIN)
end

function M.check(pane, rec, value, o)
  o = o or {}
  local token, kind, payload = M.parse(value, o.max)
  if not token then
    return nil, kind
  end
  if rec.used or rec.token == nil or token ~= rec.token then
    return nil, "token"
  end
  local bin = o.bin or M.BIN
  local fg = M.foreground_is(pane, bin)
  if fg ~= true then
    return nil, fg == nil and "foreground not known yet" or ("foreground is not " .. bin)
  end
  return token, kind, payload
end

function M.spend(rec)
  rec.used = true
end

function M.consume(rec, value)
  if rec.multi then
    if rec.last_value == value then
      return nil, "repeat"
    end
    rec.last_value = value
  else
    rec.used = true
  end
  return true
end

function M.rejecter(prefix)
  local logged = {}
  local r = {}
  function r.reject(reason)
    reason = tostring(reason)
    if not logged[reason] then
      logged[reason] = true
      util().log_info(prefix .. ": result rejected: " .. reason)
    end
    return false
  end
  function r.reset()
    logged = {}
  end
  return r
end

function M.channel(spec)
  assert(type(spec.var) == "string", "saysdf.bridge.channel: var")
  assert(type(spec.lookup) == "function", "saysdf.bridge.channel: lookup")
  local rj = M.rejecter(spec.name or "saysdf.bridge")
  local ch = { var = spec.var, reject = rj.reject, reset = rj.reset }
  function ch.handle(window, pane, name, value)
    if name ~= spec.var then
      return false
    end
    local rec = spec.lookup(pane)
    if rec == nil then
      return rj.reject(spec.not_owned or "not an owned pane")
    end
    local token, kind, payload = M.check(pane, rec, value, { max = spec.max, bin = spec.bin })
    if not token then
      return rj.reject(kind) -- kind holds the reason here
    end
    local h = spec.kinds and spec.kinds[kind]
    if type(h) ~= "function" then
      return rj.reject("kind")
    end
    local ok, res, reason = pcall(h, window, pane, rec, payload, value)
    if not ok then
      util().log_error((spec.name or "saysdf.bridge") .. ": " .. tostring(res))
      return false
    end
    if res ~= true then
      return rj.reject(reason or "rejected")
    end
    return true
  end
  return ch
end

-- For tests.
function M._reset()
  counter = 0
end

return M
