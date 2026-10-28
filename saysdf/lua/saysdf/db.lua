-- saysdfTerminal: database viewer `saysdf db` in a split or a tab (change database-viewer, D1, D6, D9).
--
-- saysdf.setup({ db = {
--   keys = true,                                   -- hotkey
--   key = { key = "D", mods = "CTRL|SHIFT" },      -- open `saysdf db` (connection list)
--   target = "split",                              -- "split" or "tab"
--   split = { direction = "Right", size = 0.6 },
--   palette = true,                                -- "Database: open viewer" entry in the palette
--   bin = nil,                                     -- path to saysdf (otherwise next to saysdfTerm / in PATH)
--   connections = nil,                             -- extra connections: { { id=, title=, kind=, ... } }
--                                                  -- written to a temporary TOML and passed via --connections
--                                                  -- (password/secret fields are ignored)
-- } })  -- db = false disables the module
--
-- Writing to the database is enabled only by a button inside `saysdf db` (dangerous mode); this module does not touch it.
--
-- API:
--   M.parse_opts(opts) -> normalized options
--   M.build_args(bin, conn, connections_file) -> { bin, "db", ... }
--       conn: nil | path to a SQLite file | id of a saved connection | { path = } | { id = }
--   M.connections_toml(list) -> TOML text
--   M.open(window, pane, conn)

local M = {}

local t = require("saysdf.i18n").domain("db")

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

local state = { opts = nil }

function M.parse_opts(opts)
  opts = type(opts) == "table" and opts or {}
  local o = {}
  o.keys = opts.keys ~= false
  o.palette = opts.palette ~= false
  o.key = type(opts.key) == "table" and opts.key or { key = "D", mods = "CTRL|SHIFT" }
  o.target = opts.target == "tab" and "tab" or "split"
  local sp = type(opts.split) == "table" and opts.split or {}
  o.split = { direction = sp.direction or "Right", size = tonumber(sp.size) or 0.6 }
  o.bin = type(opts.bin) == "string" and opts.bin ~= "" and opts.bin or nil
  o.connections = type(opts.connections) == "table" and opts.connections or nil
  return o
end

local function looks_like_path(s)
  return s:find("[/\\]") ~= nil or s:lower():match("%.sqlite3?$") ~= nil or s:lower():match("%.db3?$") ~= nil
end

function M.build_args(bin, conn, connections_file)
  local args = { bin, "db" }
  if connections_file then
    table.insert(args, "--connections")
    table.insert(args, connections_file)
  end
  if type(conn) == "table" then
    if type(conn.path) == "string" and conn.path ~= "" then
      table.insert(args, "--")
      table.insert(args, conn.path)
    elseif type(conn.id) == "string" and conn.id ~= "" then
      table.insert(args, "--connection")
      table.insert(args, conn.id)
    end
  elseif type(conn) == "string" and conn ~= "" then
    if looks_like_path(conn) then
      table.insert(args, "--")
      table.insert(args, conn)
    else
      table.insert(args, "--connection")
      table.insert(args, conn)
    end
  end
  return args
end

local FIELDS = { "id", "title", "description", "kind", "host", "port", "database", "user", "path", "tls", "remember_password" }

local function toml_str(s)
  s = tostring(s)
  s = s:gsub("\\", "\\\\"):gsub('"', '\\"'):gsub("%c", function(c)
    return string.format("\\u%04x", c:byte())
  end)
  return '"' .. s .. '"'
end

function M.connections_toml(list)
  local out = {}
  for _, c in ipairs(type(list) == "table" and list or {}) do
    if type(c) == "table" and type(c.id) == "string" and type(c.kind) == "string" then
      table.insert(out, "[[connection]]")
      for _, k in ipairs(FIELDS) do
        local v = c[k]
        if k == "port" and tonumber(v) then
          table.insert(out, k .. " = " .. string.format("%d", tonumber(v)))
        elseif k == "remember_password" and type(v) == "boolean" then
          table.insert(out, k .. " = " .. tostring(v))
        elseif v ~= nil and k ~= "port" and k ~= "remember_password" then
          table.insert(out, k .. " = " .. toml_str(v))
        end
      end
      table.insert(out, "")
    end
  end
  return table.concat(out, "\n")
end

local function toast(window, msg)
  require("saysdf.util").notify(window, "saysdf db", msg)
end

local function connections_file()
  local list = state.opts.connections
  if not list or #list == 0 then
    return nil
  end
  local u = util()
  local dir = u.runtime_dir()
  u.ensure_dir(dir)
  local path = u.join(dir, "db-connections-" .. u.hash(M.connections_toml(list)) .. ".toml")
  local ok = u.write_atomic(path, M.connections_toml(list))
  return ok and path or nil
end

function M.open(window, pane, conn)
  state.opts = state.opts or M.parse_opts(nil)
  local b = state.opts.bin or util().find_bin("saysdf")
  if not b then
    toast(window, t("saysdf not found (next to saysdfTerm or in PATH)"))
    return false
  end
  local args = M.build_args(b, conn, connections_file())
  if state.opts.target == "tab" and window then
    window:perform_action(wezterm().action.SpawnCommandInNewTab({ args = args }), pane)
  else
    local sp = state.opts.split
    pane:split({ args = args, direction = sp.direction, size = sp.size })
  end
  return true
end

function M.setup(opts, config)
  state = { opts = M.parse_opts(opts) }
  local w = wezterm()
  local cb = w.action_callback(function(window, pane)
    M.open(window, pane, nil)
  end)
  if state.opts.keys then
    config.keys = config.keys or {}
    table.insert(config.keys, { key = state.opts.key.key, mods = state.opts.key.mods, action = cb })
  end
  if state.opts.palette then
    local palette = require("saysdf.palette")
    palette.add({
      id = "db.open",
      menu = "Tools",
      order = 30,
      label = t("Database: viewer…"),
      accel = "d",
      key = state.opts.keys and palette.key_text(state.opts.key) or nil,
      hint = { contexts = { "shell" }, label = t("DB"), priority = 60 },
      brief = t("Database: open viewer (saysdf db)"),
      icon = "md_database",
      action = cb,
    })
  end
  return config
end

M._state = function()
  return state
end

return M
