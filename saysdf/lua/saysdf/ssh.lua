-- saysdfTerminal: SSH connections (change ssh-connections).
--
-- The module is enabled by default; disable it with require("saysdf").setup({ ssh = false }).
-- Host enumeration is lazy: enumerate_ssh_hosts is called when the list opens, not on setup.
-- Options:
--   enabled = true            -- false: the module is off (same as ssh = false)
--   backend = "ssh"           -- "ssh": the system `ssh <alias>` in a new tab;
--                                "domain": SpawnTab{ DomainName = "SSH:<alias>" } (needs
--                                config.ssh_domains = wezterm.default_ssh_domains())
--   ssh_bin = nil             -- path to ssh; otherwise PATH (saysdf.util.find_bin)
--   split = false             -- true: open in a split instead of a tab
--   keys = true               -- Ctrl+Shift+H: pick a connection (saysdf/keymap.md)
--   key = { key = "H", mods = "CTRL|SHIFT" }
--   palette = true            -- "SSH: ..." command palette entries (via saysdf.palette)
--   hosts = {}                -- metadata in the config: { alias = { title, description, group, color } },
--                                takes priority over the JSON file
--   meta_file = nil           -- metadata JSON; default <config_dir>/saysdf/ssh-connections.json
--   desc_width = 60           -- description truncation (columns)
--   bin = nil                 -- path to saysdf (for "+ Add connection…")
--   hold_on_error = true      -- ssh failed: keep the tab open (the upstream exit_behavior defaults
--                                to "Close", which would hide the ssh error together with the tab)
--   config_files = nil        -- extra ssh config files for enumerate_ssh_hosts (isolated checks;
--                                upstream reads the default files anyway)
--
-- API:
--   list(force?) -> { {alias, hostname, user, port, title, description, group, color}, ... }
--       enumerate_ssh_hosts() + JSON + opts.hosts; cached in wezterm.GLOBAL, invalidate() resets it
--   choices(list, opts?) -> InputSelector choices (label = wezterm.format, id = alias | ADD_ID)
--   row_text(entry, title_width, desc_width) -> plain string (for tests/search)
--   build_action(entry, opts, ssh_bin, windows?) -> { kind = "tab"|"split"|"domain", args?, domain? }
--   ssh_args(ssh_bin, alias, hold, windows) -> argv (keeps the tab open on error)
--   resolve_choice(id, list?) -> { kind = "add" } | { kind = "host", entry } | nil (cancelled)
--   open(window, pane, alias), pick(window, pane), add(window, pane)
--
-- No secrets are stored: authentication is ssh-agent/keys/prompts in the system ssh; known_hosts is ssh's job.

local t = require("saysdf.i18n").domain("ssh")

local M = {}

M.ADD_ID = "__saysdf_ssh_add__"
M.GLOBAL_KEY = "saysdf_ssh_hosts"

local state = { opts = {} }

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

-- The picker lives in the sidebar (`saysdf.sidebar.selector`); InputSelector is the fallback.
local function sidebar()
  return require("saysdf.sidebar")
end

local function log_warn(msg)
  util().log_warn("saysdf.ssh: " .. msg)
end

-- Control characters -> space, collapse whitespace, trim the edges.
function M.sanitize(s)
  if s == nil then
    return ""
  end
  s = tostring(s):gsub("%c", " "):gsub("%s+", " ")
  return (s:gsub("^ ", ""):gsub(" $", ""))
end

local function width(s)
  local w = wezterm()
  if type(w.column_width) == "function" then
    return w.column_width(s)
  end
  return utf8.len(s) or #s
end

-- Truncate by columns with "…".
function M.truncate(s, max)
  if width(s) <= max then
    return s
  end
  local out, used = {}, 0
  for _, cp in utf8.codes(s) do
    local ch = utf8.char(cp)
    local cw = width(ch)
    if used + cw > max - 1 then
      break
    end
    table.insert(out, ch)
    used = used + cw
  end
  return table.concat(out) .. "…"
end

function M.meta_path()
  local p = state.opts.meta_file
  if type(p) == "string" and p ~= "" then
    return p
  end
  local dir = wezterm().config_dir or ""
  return util().join(dir, "saysdf", "ssh-connections.json")
end

local function read_meta()
  local text = util().read_file(M.meta_path())
  if not text or text:match("^%s*$") then
    return {}
  end
  local ok, v = pcall(wezterm().json_parse, text)
  if ok and type(v) == "table" then
    return v
  end
  log_warn("cannot parse " .. M.meta_path())
  return {}
end

local function enumerate()
  local w = wezterm()
  if type(w.enumerate_ssh_hosts) ~= "function" then
    return {}
  end
  local files = type(state.opts.config_files) == "table" and state.opts.config_files or {}
  local ok, hosts = pcall(w.enumerate_ssh_hosts, table.unpack(files))
  if ok and type(hosts) == "table" then
    return hosts
  end
  log_warn("enumerate_ssh_hosts: " .. tostring(hosts))
  return {}
end

local function str(v)
  return type(v) == "string" and v or (v ~= nil and tostring(v) or "")
end

-- Build the list without the cache.
function M.collect()
  local hosts = enumerate()
  local meta = read_meta()
  local extra = type(state.opts.hosts) == "table" and state.opts.hosts or {}
  local list = {}
  for alias, cfg in pairs(hosts) do
    cfg = type(cfg) == "table" and cfg or {}
    local m = {}
    for _, src in ipairs({ meta[alias] or false, extra[alias] or false }) do
      if type(src) == "table" then
        for k, v in pairs(src) do
          m[k] = v
        end
      end
    end
    local title = M.sanitize(m.title)
    table.insert(list, {
      alias = alias,
      hostname = str(cfg.hostname),
      user = str(cfg.user),
      port = str(cfg.port),
      title = title ~= "" and title or alias,
      description = M.sanitize(m.description),
      group = M.sanitize(m.group),
      color = M.sanitize(m.color),
    })
  end
  table.sort(list, function(a, b)
    -- groups alphabetically, ungrouped last; within a group by title
    if a.group ~= b.group then
      if a.group == "" or b.group == "" then
        return b.group == ""
      end
      return a.group:lower() < b.group:lower()
    end
    if a.title:lower() ~= b.title:lower() then
      return a.title:lower() < b.title:lower()
    end
    return a.alias < b.alias
  end)
  return list
end

function M.list(force)
  local G = wezterm().GLOBAL
  if not force and G and type(G[M.GLOBAL_KEY]) == "table" then
    local out = {}
    for _, e in ipairs(G[M.GLOBAL_KEY]) do
      local t = {}
      for k, v in pairs(e) do
        t[k] = v
      end
      table.insert(out, t)
    end
    return out
  end
  local list = M.collect()
  if G then
    G[M.GLOBAL_KEY] = list
  end
  return list
end

function M.invalidate()
  local G = wezterm().GLOBAL
  if G then
    G[M.GLOBAL_KEY] = nil
  end
end

local function pad(s, w)
  local n = w - width(s)
  return n > 0 and (s .. string.rep(" ", n)) or s
end

local function head(e)
  local h = e.title
  if e.alias ~= e.title then
    h = h .. " (" .. e.alias .. ")"
  end
  if e.group ~= "" then
    h = "[" .. e.group .. "] " .. h
  end
  return h
end

-- Plain row: "[group] Title (alias)   description".
function M.row_text(e, title_width, desc_width)
  local h = head(e)
  if e.description == "" then
    return h
  end
  return pad(h, title_width) .. "  " .. M.truncate(e.description, desc_width or 60)
end

local function resolve_color(c)
  if c == "" then
    return nil
  end
  local ok, tabs = pcall(require, "saysdf.tabs")
  if ok and type(tabs.resolve_color) == "function" then
    return tabs.resolve_color(c)
  end
  return c:match("^#%x+$") and c or nil
end

function M.choices(list, opts)
  opts = opts or state.opts or {}
  local w = wezterm()
  local dw = tonumber(opts.desc_width) or 60
  local tw = 0
  for _, e in ipairs(list) do
    tw = math.max(tw, width(head(e)))
  end
  tw = math.min(tw, 48)
  local out = {}
  for _, e in ipairs(list) do
    local items = {}
    local color = resolve_color(e.color)
    if color then
      table.insert(items, { Foreground = { Color = color } })
    end
    table.insert(items, { Text = "● " })
    table.insert(items, "ResetAttributes")
    if e.group ~= "" then
      table.insert(items, { Attribute = { Intensity = "Half" } })
      table.insert(items, { Text = "[" .. e.group .. "] " })
      table.insert(items, "ResetAttributes")
    end
    table.insert(items, { Attribute = { Intensity = "Bold" } })
    table.insert(items, { Text = e.title })
    table.insert(items, "ResetAttributes")
    if e.alias ~= e.title then
      table.insert(items, { Attribute = { Intensity = "Half" } })
      table.insert(items, { Text = " (" .. e.alias .. ")" })
      table.insert(items, "ResetAttributes")
    end
    if e.description ~= "" then
      local gap = math.max(tw - width(head(e)), 0) + 2
      table.insert(items, { Attribute = { Intensity = "Half" } })
      table.insert(items, { Text = string.rep(" ", gap) .. M.truncate(e.description, dw) })
      table.insert(items, "ResetAttributes")
    end
    table.insert(out, { id = e.alias, label = w.format(items) })
  end
  local add = #list == 0 and t("+ Add connection… (no hosts in ~/.ssh/config)") or t("+ Add connection…")
  table.insert(out, { id = M.ADD_ID, label = add })
  return out
end

-- Alias and path safe for a cmd.exe command line without quotes (otherwise no wrapper).
local function cmd_safe(s)
  return type(s) == "string" and s ~= "" and not s:find('[%s"&|<>^%%()!]') and true or false
end

-- argv for ssh. hold: on a non-zero exit wait for a key so the error stays visible
-- (otherwise exit_behavior = "Close" closes the tab at once, so "nothing happens").
-- Unix: the alias is a positional sh argument, never interpolated. Windows: cmd /c only for
-- a safe path and alias.
function M.ssh_args(ssh_bin, alias, hold, windows)
  if not hold then
    return { ssh_bin, alias }
  end
  if windows then
    if cmd_safe(ssh_bin) and cmd_safe(alias) and not alias:find("^%-") then
      return { "cmd.exe", "/d", "/c", ssh_bin, alias, "||", "pause" }
    end
    return { ssh_bin, alias }
  end
  return {
    "/bin/sh",
    "-c",
    '"$0" "$@" || { c=$?; printf "\n' .. t("[ssh: exit code %s — press Enter to close the tab]") .. '\n" "$c"; read -r _; exit "$c"; }',
    ssh_bin,
    alias,
  }
end

function M.build_action(e, opts, ssh_bin, windows)
  opts = opts or {}
  if opts.backend == "domain" then
    return { kind = "domain", domain = "SSH:" .. e.alias }
  end
  if windows == nil then
    windows = util().is_windows()
  end
  return { kind = opts.split and "split" or "tab", args = M.ssh_args(ssh_bin, e.alias, opts.hold_on_error ~= false, windows) }
end

-- Map an InputSelector choice to an entry: id = alias or ADD_ID; nil = cancelled.
function M.resolve_choice(id, list)
  if id == nil or id == "" then
    return nil
  end
  if id == M.ADD_ID then
    return { kind = "add" }
  end
  for _, e in ipairs(list or M.list()) do
    if e.alias == id then
      return { kind = "host", entry = e }
    end
  end
  return { kind = "host", entry = { alias = id, title = id, description = "", group = "", color = "" } }
end

local function notify(window, msg)
  log_warn(msg)
  util().notify(window, "saysdf", msg)
end

function M.open(window, pane, alias)
  local w = wezterm()
  local e = M.resolve_choice(alias).entry
  local ssh_bin
  if state.opts.backend ~= "domain" then
    ssh_bin = state.opts.ssh_bin
    if type(ssh_bin) ~= "string" or ssh_bin == "" then
      ssh_bin = util().find_bin_cached("ssh")
    end
    if not ssh_bin then
      notify(window, t("ssh not found in PATH (Windows: OpenSSH Client component); or backend = 'domain'"))
      return false
    end
  end
  local act = M.build_action(e, state.opts, ssh_bin)
  util().log_info("saysdf.ssh: opening " .. e.alias .. " (" .. act.kind .. ")")
  if act.kind == "split" then
    pane:split({ args = act.args, direction = "Right" })
    return true
  elseif act.kind == "domain" then
    window:perform_action(w.action.SpawnTab({ DomainName = act.domain }), pane)
  else
    window:perform_action(w.action.SpawnCommandInNewTab({ args = act.args }), pane)
  end
  local ok, tab = pcall(function()
    return window:active_tab()
  end)
  if ok and tab then
    pcall(function()
      tab:set_title(e.title)
    end)
    if e.color ~= "" then
      local okt, tabs = pcall(require, "saysdf.tabs")
      if okt then
        pcall(tabs.set_color, tab, e.color)
      end
    end
  end
  return true
end

local function json_array(list)
  local parts = {}
  for _, s in ipairs(list) do
    -- aliases from the ssh config: escape quotes, backslashes and control characters
    local q = s:gsub('[%c"\\]', function(c)
      return string.format("\\u%04x", c:byte())
    end)
    table.insert(parts, '"' .. q .. '"')
  end
  return "[" .. table.concat(parts, ",") .. "]"
end

function M.add_args(bin, existing_file, meta_file, ssh_dir)
  local args = { bin, "ssh", "add", "--existing-file", existing_file, "--meta-file", meta_file }
  if ssh_dir then
    table.insert(args, "--ssh-dir")
    table.insert(args, ssh_dir)
  end
  return args
end

function M.add(window, pane)
  local w = wezterm()
  local bin = state.opts.bin
  if type(bin) ~= "string" or bin == "" then
    bin = util().find_bin_cached("saysdf")
  end
  if not bin then
    log_warn("saysdf not found")
    util().notify_missing_bin(window, "saysdf")
    return false
  end
  local aliases = {}
  for _, e in ipairs(M.list(true)) do
    table.insert(aliases, e.alias)
  end
  local dir = util().runtime_dir()
  pcall(util().ensure_dir, dir)
  local tmp = util().join(dir, "ssh-existing.json")
  local ok, err = util().write_atomic(tmp, json_array(aliases))
  if not ok then
    notify(window, t("cannot write {path}: {err}", { path = tmp, err = tostring(err) }))
    return false
  end
  local ssh_dir = util().join(w.home_dir or "", ".ssh")
  pane:split({ args = M.add_args(bin, tmp, M.meta_path(), ssh_dir), direction = "Bottom", size = 0.6 })
  -- the cache is stale: the next pick re-reads the ssh config
  M.invalidate()
  return true
end

function M.pick(window, pane)
  window:perform_action(
    sidebar().selector({
      tool = "ssh",
      title = t("SSH connections"),
      choices = M.choices(M.list()),
      fuzzy = true,
      fuzzy_description = "SSH: ",
      on_choice = function(win, p, id)
        local r = M.resolve_choice(id)
        if not r then
          return
        elseif r.kind == "add" then
          M.add(win, p)
        else
          M.open(win, p, r.entry.alias)
        end
      end,
    }),
    pane
  )
end

-- Entries with action_callback are built once per setup: action_callback registers a new event
-- on every call, while palette.entries runs on every update-status tick (perf-2).
local palette_cache = nil

function M.palette_entries()
  if palette_cache == nil then
    palette_cache = M._build_palette_entries()
  end
  return palette_cache
end

function M._build_palette_entries()
  local w = wezterm()
  local o = state and state.opts or {}
  local keytext = o.keys ~= false
      and require("saysdf.palette").key_text(type(o.key) == "table" and o.key or { key = "H", mods = "CTRL|SHIFT" })
    or nil
  return {
    {
      id = "ssh.connect",
      menu = "File",
      order = 20,
      label = t("SSH: connect…"),
      accel = "h",
      key = keytext,
      hint = { contexts = { "shell", "git", "ssh" }, label = "SSH", priority = 40 },
      brief = t("SSH: connect…"),
      icon = "md_server_network",
      action = w.action_callback(function(window, pane)
        M.pick(window, pane)
      end),
    },
    {
      id = "ssh.add",
      menu = "File",
      accel = "a",
      order = 21,
      brief = t("SSH: add connection…"),
      icon = "md_server_plus",
      action = w.action_callback(function(window, pane)
        M.add(window, pane)
      end),
    },
  }
end

local palette_added = false

function M.setup(opts, config)
  opts = opts or {}
  state = { opts = opts }
  palette_cache = nil
  if opts.enabled == false then
    return config
  end
  M.invalidate()
  local w = wezterm()
  if opts.keys ~= false then
    local b = type(opts.key) == "table" and opts.key or { key = "H", mods = "CTRL|SHIFT" }
    local keys = config.keys or {}
    table.insert(keys, {
      key = b.key,
      mods = b.mods,
      action = w.action_callback(function(window, pane)
        M.pick(window, pane)
      end),
    })
    config.keys = keys
  end
  if opts.palette ~= false and not palette_added then
    palette_added = true
    require("saysdf.palette").add(function()
      if state.opts.enabled ~= false and state.opts.palette ~= false then
        return M.palette_entries()
      end
    end)
  end
  return config
end

M._state = function()
  return state
end

return M
