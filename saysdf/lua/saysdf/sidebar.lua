-- saysdfTerminal: tool sidebar (change tool-sidebar, design D1-D6).
--
-- A tool (`saysdf pick` etc.) opens as a top_level split on the side, `width` cells wide,
-- one sidebar per tab. Opening is asynchronous: write_atomic of a small JSON file + pane:split
-- (the GUI thread does not wait for a process). The result arrives as the user var `SAYSDF_SIDEBAR` =
-- `<token>:<kind>:<payload>` and is accepted only if ALL checks pass (D3):
--   1) pane_id is the sidebar of this tab, as recorded by the module;
--   2) `saysdf` is in the pane's foreground (get_foreground_process_name_cached; nil is a refusal
--      that does not spend the token, the TUI retries the send);
--   3) the token matches the one-time token of this open (env SAYSDF_SIDEBAR_TOKEN) and is unused;
--   4) the value is at most 4096 bytes;
--   5) allow-list: pick -- an id from the issued list (empty means cancel, callback with id = nil);
--      action -- `<id>:<arg>`, the id of a saysdf.palette entry with sidebar = true, accept(arg) is truthy;
--      runs entry.sidebar_run(window, pane, arg) or entry.action.
-- Refusals are logged once per reason (log_info).
-- A config reload recreates the module: the tab record (pane, tool, token, origin) is duplicated
-- in wezterm.GLOBAL (`saysdf_sidebar_<tab_id>`) and restored while the pane is alive --
-- toggle finds the open sidebar instead of opening a second one. Callbacks (on_result) do not
-- survive a reload: a pick result after it is rejected, action (saysdf files) keeps working.
--
-- API:
--   open(window, pane, { tool, args, choices?, multi?, on_result(window, pane, kind, payload) }) -> bool
--                                     multi = true -- several actions per pane lifetime (saysdf files)
--   toggle(window, pane, tool?)       -- focus/close/open the last tool
--   selector(spec) -> action          -- InputSelector spec (title, choices {id,label}, fuzzy, action);
--                                        callback: spec.on_choice(window, pane, id, label) or
--                                        spec.action (action_callback -- EmitEvent with the same arguments).
--                                        sidebar = false / no `saysdf` / narrow tab -- InputSelector.
--   is_sidebar(pane) -> bool
--   handle_user_var(window, pane, name, value) -> bool  -- user-var-changed handler (for tests)
-- Options: enabled = true, side = "right", width = 40, key = Ctrl+Shift+Y, palette = true,
--        test_entry = false (palette entry "Sidebar test" -- for checking by eye).

local t = require("saysdf.i18n").domain("sidebar")

local M = {}

M.VAR = "SAYSDF_SIDEBAR"
M.TOKEN_ENV = "SAYSDF_SIDEBAR_TOKEN"
M.MAX_VALUE = 4096
M.MIN_WIDTH = 20
M.BIN = "saysdf"

M.DEFAULTS = {
  enabled = true,
  side = "right",
  width = 40,
  key = { key = "Y", mods = "CTRL|SHIFT" },
  palette = true,
  test_entry = false,
}

local opts = nil
local state = {} -- [tab_id] = { pane_id, tool, token, origin_pane_id, choices = { [id] = label }, on_result, used }
local by_pane = {} -- [pane_id] = tab_id
local last = nil -- the last tool: { tool, spec }
local counter = 0
local logged = {}
local handler_registered = false

local function wezterm()
  return require("wezterm")
end

local function util()
  return require("saysdf.util")
end

function M.merge_opts(o)
  local r = {}
  for k, v in pairs(M.DEFAULTS) do
    r[k] = v
  end
  for k, v in pairs(o or {}) do
    r[k] = v
  end
  return r
end

-- setup() was not called (saysdf.setup({ sidebar = false })) -- disabled: InputSelector everywhere.
local DISABLED = { enabled = false }
function M.options()
  return opts or DISABLED
end

local function reject(reason)
  if not logged[reason] then
    logged[reason] = true
    util().log_info("saysdf.sidebar: result rejected: " .. reason)
  end
  return false
end

-- 32 hex: two hashes of the time, a counter and a table address. Not a secret from the user's own
-- processes, but a program in another pane cannot guess it, and it is one-time (D3).
function M.new_token()
  counter = counter + 1
  local seed = table.concat({ tostring(os.time()), tostring(os.clock()), tostring(counter), tostring({}), tostring(math.random()) }, "|")
  local u = util()
  return (u.hash(seed) .. u.hash(seed .. "#" .. counter)):gsub("-", "")
end

-- Width: `width`, but at most half of the tab; nil -- the tab is too narrow (fallback path).
function M.width_for(tab_cols, o)
  o = o or M.options()
  local w = tonumber(o.width) or 40
  if type(tab_cols) == "number" and tab_cols < 2 * w then
    w = math.floor(tab_cols / 2)
  end
  if w < M.MIN_WIDTH then
    return nil
  end
  return w
end

local function pane_id(p)
  local ok, id = pcall(function()
    return p:pane_id()
  end)
  return ok and id or nil
end

local function tab_of(p)
  local ok, t = pcall(function()
    return p:tab()
  end)
  return ok and t or nil
end

local function tab_id(t)
  local ok, id = pcall(function()
    return t:tab_id()
  end)
  return ok and id or nil
end

local function get_pane(id)
  if id == nil then
    return nil
  end
  local ok, p = pcall(function()
    return wezterm().mux.get_pane(id)
  end)
  return ok and p or nil
end

local function global()
  local ok, g = pcall(function()
    return wezterm().GLOBAL
  end)
  return ok and g or nil
end

local function gkey(tid)
  return "saysdf_sidebar_" .. tostring(tid)
end

-- Tab record in wezterm.GLOBAL (serializable fields only).
local function persist(tid, st)
  local g = global()
  if not g then
    return
  end
  pcall(function()
    g[gkey(tid)] = st and {
      pane_id = st.pane_id,
      tool = st.tool,
      token = st.token,
      origin_pane_id = st.origin_pane_id,
      multi = st.multi,
      used = st.used,
    } or nil
  end)
end

-- Restore the record after a config reload (the pane is still alive).
local function restore(tid)
  if tid == nil or state[tid] then
    return state[tid]
  end
  local g = global()
  local rec = nil
  if g then
    pcall(function()
      local r = g[gkey(tid)]
      if r and r.pane_id then
        rec = {
          pane_id = r.pane_id,
          tool = r.tool,
          token = r.token,
          origin_pane_id = r.origin_pane_id,
          multi = r.multi == true,
          used = r.used == true,
        }
      end
    end)
  end
  if not rec then
    return nil
  end
  if get_pane(rec.pane_id) == nil then
    persist(tid, nil)
    return nil
  end
  -- the pick callback was lost with the previous module -- a pick result is no longer accepted
  rec.stale = true
  state[tid] = rec
  by_pane[rec.pane_id] = tid
  return rec
end

local function forget(tid)
  local st = state[tid]
  if st then
    by_pane[st.pane_id] = nil
    state[tid] = nil
  end
  persist(tid, nil)
end

-- Live tab record (pane closed -- the record is dropped; D5: no tick subscription).
function M.state_for(tid)
  local st = restore(tid)
  if st and get_pane(st.pane_id) == nil then
    forget(tid)
    return nil
  end
  return st
end

function M.is_sidebar(p)
  local id = pane_id(p)
  if id ~= nil and by_pane[id] == nil then
    local t = tab_of(p)
    restore(t and tab_id(t))
  end
  return id ~= nil and by_pane[id] ~= nil and M.state_for(by_pane[id]) ~= nil
end

-- Close the pane: upstream CloseCurrentPane closes the tab's active pane.
local function close_pane(window, sp)
  pcall(function()
    sp:activate()
  end)
  window:perform_action(wezterm().action.CloseCurrentPane({ confirm = false }), sp)
end

local function activate(p)
  if p then
    pcall(function()
      p:activate()
    end)
  end
end

-- Label without escape sequences (InputSelector labels from wezterm.format: CSI SGR, OSC).
function M.plain_label(s)
  s = tostring(s or "")
  s = s:gsub("\27%][^\7\27]*\7", ""):gsub("\27%][^\27]*\27\\", "")
  s = s:gsub("\27%[[%d;:?]*[%a@]", ""):gsub("\27.", "")
  return s
end

local function json_list(choices)
  local list = {}
  for _, c in ipairs(choices) do
    table.insert(list, { id = tostring(c.id), label = M.plain_label(c.label or c.id) })
  end
  return wezterm().json_encode(list)
end

-- Open a tool in the sidebar. spec: { tool, args = { "pick", ... }, choices?, on_result }.
-- Returns false if it cannot be opened (the caller then takes the fallback path).
function M.open(window, pane, spec)
  local o = M.options()
  local bin = util().find_bin_cached(M.BIN)
  if not bin then
    return false
  end
  local tab = tab_of(pane)
  local tid = tab and tab_id(tab)
  if tid == nil then
    return false
  end
  pcall(function()
    tab:set_zoomed(false) -- zoom hides the sidebar (edge case D)
  end)
  local cols = nil
  pcall(function()
    cols = tab:get_size().cols
  end)
  local width = M.width_for(cols, o)
  if not width then
    return false
  end

  -- Called from the sidebar itself -- origin stays the same (edge case D).
  local origin = pane_id(pane)
  local old = M.state_for(tid)
  if old then
    if old.pane_id == origin then
      origin = old.origin_pane_id
    end
    local sp = get_pane(old.pane_id)
    forget(tid)
    if sp then
      close_pane(window, sp)
    end
  end

  local token = M.new_token()
  local args = { bin }
  for _, a in ipairs(spec.args or {}) do
    table.insert(args, a)
  end
  local choices = nil
  if spec.choices then
    local u = util()
    local dir = u.runtime_dir()
    u.ensure_dir(dir)
    local file = u.join(dir, "sidebar-" .. token .. ".json")
    local ok, err = u.write_atomic(file, json_list(spec.choices))
    if not ok then
      u.log_error("saysdf.sidebar: " .. tostring(err))
      return false
    end
    table.insert(args, "--items")
    table.insert(args, file)
    choices = {}
    for _, c in ipairs(spec.choices) do
      choices[tostring(c.id)] = c.label or tostring(c.id)
    end
  end

  local origin_pane = get_pane(origin) or pane
  local new_pane = origin_pane:split({
    args = args,
    domain = "DefaultDomain",
    direction = (o.side == "left") and "Left" or "Right",
    top_level = true,
    size = width,
    set_environment_variables = { [M.TOKEN_ENV] = token },
  })
  local nid = pane_id(new_pane)
  if nid == nil then
    return false
  end
  state[tid] = {
    pane_id = nid,
    tool = spec.tool,
    token = token,
    origin_pane_id = origin,
    choices = choices,
    on_result = spec.on_result,
    used = false,
    -- saysdfTerminal (file-browser D4): spec.multi -- the tool sends several actions per lifetime
    -- (saysdf files): the token is not spent, a repeat of the same value (send retries) is rejected.
    multi = spec.multi == true,
  }
  by_pane[nid] = tid
  persist(tid, state[tid])
  return true
end

-- Repeated call: sidebar focused -- close it and return focus; not focused -- focus it; none -- open.
-- Returns "closed" | "focused" | "opened" | nil.
function M.toggle(window, pane, tool, reopen)
  local tab = tab_of(pane)
  local tid = tab and tab_id(tab)
  local st = tid and M.state_for(tid)
  if st and st.used then
    -- the choice is already delivered, the `saysdf pick` process exits on its own: a nested choice
    -- (Git -> Checkout) opens a new pane instead of focusing the departing one
    forget(tid)
    st = nil
  end
  if st and (tool == nil or st.tool == tool) then
    local sp = get_pane(st.pane_id)
    if st.pane_id == pane_id(pane) then
      local origin = get_pane(st.origin_pane_id)
      forget(tid)
      close_pane(window, sp)
      activate(origin)
      return "closed"
    end
    activate(sp)
    return "focused"
  end
  if reopen then
    return reopen() and "opened" or nil
  end
  return nil
end

-- Selector callback: an on_choice function or an action_callback (EmitEvent) with the InputSelector arguments.
local function call_choice(spec, window, pane, id, label)
  if type(spec.on_choice) == "function" then
    return spec.on_choice(window, pane, id, label)
  end
  local a = spec.action
  local ev = type(a) == "table" and a.EmitEvent or nil
  if ev then
    return wezterm().emit(ev, window, pane, id, label)
  end
end

local function input_selector(spec)
  local w = wezterm()
  local s = {}
  for k, v in pairs(spec) do
    if k ~= "on_choice" and k ~= "tool" then
      s[k] = v
    end
  end
  if s.action == nil and type(spec.on_choice) == "function" then
    s.action = w.action_callback(spec.on_choice)
  end
  return w.action.InputSelector(s)
end

local function state_label(choices, id)
  for _, c in ipairs(choices or {}) do
    if tostring(c.id) == id then
      return c.label
    end
  end
  return nil
end

local function open_selector(window, pane, spec)
  local choices = spec.choices
  if type(choices) == "function" then
    choices = choices(window, pane)
  end
  local args = { "pick", "--title", tostring(spec.title or t("Choose")) }
  if spec.fuzzy then
    table.insert(args, "--fuzzy")
  end
  local tool = spec.tool or spec.title or "pick"
  return M.open(window, pane, {
    tool = tool,
    args = args,
    choices = choices or {},
    on_result = function(win, p, kind, payload)
      if kind == "pick" then
        local id = (payload ~= "") and payload or nil
        local label = id and state_label(choices, id) or nil
        call_choice(spec, win, p, id, label)
      end
    end,
  })
end

-- InputSelector replacement: same spec; disabled -- InputSelector itself.
function M.selector(spec)
  local w = wezterm()
  if not M.options().enabled then
    return input_selector(spec)
  end
  return w.action_callback(function(window, pane)
    local tool = spec.tool or spec.title or "pick"
    local function reopen()
      last = { tool = tool, spec = spec }
      return open_selector(window, pane, spec)
    end
    local r = M.toggle(window, pane, tool, reopen)
    if r == nil then
      -- did not open (no saysdf, narrow tab) -- fallback InputSelector with the same spec
      local s = spec
      if type(spec.choices) == "function" then
        s = {}
        for k, v in pairs(spec) do
          s[k] = v
        end
        s.choices = spec.choices(window, pane)
      end
      window:perform_action(input_selector(s), pane)
    end
  end)
end

-- Ctrl+Shift+Y: the last tool (toggle).
function M.toggle_last(window, pane)
  local r = M.toggle(window, pane, nil, last and function()
    return open_selector(window, pane, last.spec)
  end or nil)
  if r == nil and last == nil then
    require("saysdf.util").notify(window, "saysdfTerm", t("Sidebar: no tool has been opened yet"))
  end
end

local function fg_is_saysdf(p)
  local ok, name = pcall(function()
    return p:get_foreground_process_name_cached()
  end)
  if not ok or name == nil then
    return nil
  end
  return util().process_basename(name) == M.BIN
end

-- user-var-changed handler. true -- the result was accepted and executed.
function M.handle_user_var(window, pane, name, value)
  if name ~= M.VAR then
    return false
  end
  local pid = pane_id(pane)
  if pid and by_pane[pid] == nil then
    local t = tab_of(pane)
    restore(t and tab_id(t)) -- after a config reload
  end
  local tid = pid and by_pane[pid]
  local st = tid and state[tid]
  if not st or st.pane_id ~= pid then
    return reject("not a sidebar pane")
  end
  if type(value) ~= "string" or #value > M.MAX_VALUE then
    return reject("length")
  end
  local token, kind, payload = value:match("^([^:]*):([^:]*):(.*)$")
  if not token then
    return reject("format")
  end
  if st.used or token ~= st.token then
    return reject("token")
  end
  local fg = fg_is_saysdf(pane)
  if fg ~= true then
    return reject(fg == nil and "foreground not known yet" or "foreground is not saysdf")
  end
  local origin = get_pane(st.origin_pane_id)
  if origin == nil then
    local okt, t = pcall(function()
      return wezterm().mux.get_tab(tid)
    end)
    origin = okt and t and t:active_pane() or pane
  end
  if kind == "pick" then
    if st.stale then
      return reject("callback lost (config reload)")
    end
    if payload ~= "" and (st.choices == nil or st.choices[payload] == nil) then
      return reject("id not in the list")
    end
    st.used = true
    persist(tid, st)
    if st.on_result then
      local ok, err = pcall(st.on_result, window, origin, "pick", payload)
      if not ok then
        util().log_error("saysdf.sidebar: " .. tostring(err))
      end
    end
    return true
  elseif kind == "action" then
    local id, arg = payload:match("^([^:]+):?(.*)$")
    local e = id and require("saysdf.palette").by_id(window, origin, id)
    if not e or e.sidebar ~= true or type(e.accept) ~= "function" then
      return reject("action not allowed")
    end
    local okv, valid = pcall(e.accept, arg)
    if not okv or not valid then
      return reject("argument rejected by accept")
    end
    if st.multi then
      if st.last_value == value then
        return reject("repeat")
      end
      st.last_value = value
    else
      st.used = true
      persist(tid, st)
    end
    local ok, err = pcall(function()
      if type(e.sidebar_run) == "function" then
        e.sidebar_run(window, origin, arg)
      else
        window:perform_action(e.action, origin)
      end
    end)
    if not ok then
      util().log_error("saysdf.sidebar: " .. tostring(err))
    end
    return true
  end
  return reject("kind")
end

function M.test_choices()
  return {
    { id = "one", label = t("First item") },
    { id = "two", label = t("Second item") },
    { id = "three", label = t("Third item: a long label to check truncation") },
  }
end

function M.test_selector()
  return M.selector({
    tool = "sidebar.test",
    title = t("Sidebar test"),
    choices = M.test_choices(),
    on_choice = function(window, _, id)
      require("saysdf.util").notify(window, "saysdfTerm", t("Sidebar: chosen {id}", { id = tostring(id) }))
    end,
  })
end

function M.setup(o, config)
  opts = M.merge_opts(o)
  if not opts.enabled then
    return config
  end
  local w = wezterm()
  if not handler_registered then
    handler_registered = true
    w.on("user-var-changed", function(window, pane, name, value)
      if name == M.VAR then -- early exit for foreign variables (D6)
        M.handle_user_var(window, pane, name, value)
      end
    end)
  end
  local cb = w.action_callback(M.toggle_last)
  if opts.key then
    config.keys = config.keys or {}
    table.insert(config.keys, { key = opts.key.key, mods = opts.key.mods, action = cb })
  end
  if opts.palette then
    local palette = require("saysdf.palette")
    palette.add({
      id = "sidebar.toggle", menu = "View", order = 60, label = t("Sidebar"), accel = "b",
      brief = t("Sidebar: last tool"), icon = "md_dock_right",
      key = palette.key_text(opts.key), action = cb,
    })
    if opts.test_entry then
      palette.add({ id = "sidebar.test", menu = "View", order = 61, label = t("Sidebar test"), accel = "x", brief = t("Sidebar test"), icon = "md_dock_right", action = M.test_selector() })
    end
  end
  return config
end

-- For tests.
function M._reset()
  for tid in pairs(state) do
    persist(tid, nil)
  end
  opts, state, by_pane, last, counter, logged, handler_registered = nil, {}, {}, nil, 0, {}, false
end

function M._state()
  return state
end

return M
