-- saysdfTerminal: project workspaces (change project-workspaces).
--
-- setup options (require("saysdf").setup({ workspaces = {...} })); workspaces = false disables:
--   key = { key = "A", mods = "CTRL|SHIFT" }  -- workspace list; false -- no hotkey
--   palette = true                            -- palette entries (via saysdf.palette)
--   dir = nil                                 -- storage folder; default <data>/workspaces
--   autosave = true, autosave_interval = 30   -- autosave (deferred update-status, debounce, only on change)
--   shrink_delay = 90, grace = 5              -- a smaller session is written only after 90 s; first
--                                             -- autosave 5 s after startup restore (change workspace-restore)
--   restore = "always"                        -- at startup: "always" (default) | "ask" | "never" (gui-startup)
--   claude_interval = autosave_interval       -- re-check a running Claude Code pane at most this often
-- The palette entry "Workspaces: save current" saves at once (even a smaller session, bypassing
-- shrink_delay), masks secrets (a pane whose command looks like it has one keeps only its folder),
-- shows "Saved: N tabs, M panes, K Claude sessions" and opens <name>.preview.md in the viewer.
-- Panes running Claude Code store pane.claude = { session, exe } (detected in the deferred
-- autosave pass); workspace-trust.json keeps programs[<exe>] = { kind = "claude-resume" }.
--   claude_resume = true                      -- restore runs exactly { exe, "--resume", uuid } (D7):
-- strict uuid, session file exists, exe absolute + named claude + exists + allowed; a new exe is
-- asked about once ("No" is remembered as claude-resume-denied; the palette "allow/forbid
-- commands" of the workspace resets it). Any failed check -> a shell in the saved cwd + a log line.
-- A workspace's allow_commands flag (palette "allow/forbid commands") lives in its file;
-- without the flag commands are neither saved nor passed to spawn.
--
-- User data (outside the repo): <data>/workspaces/<name>.json, <data>/workspace-trust.json
-- (<data> is saysdf.util.data_dir()). Restoring does NOT run commands: only shells in the
-- saved folders. Commands from the project's `.saysdf/workspace.json` run only after
-- confirmation; the approval is bound to the exact command list (and its hash) -- if the list
-- changes, ask again.
--
-- `.saysdf/workspace.json` format:
--   { "name": "proj", "tabs": [ { "title": "srv", "cwd": "server", "command": ["cargo", "run"] } ] }
--   cwd is relative to the project root (absolute paths and ".." are rejected); command is a string or an array.
--
-- Pure functions (tested): layout_from_panes, plan_splits, to_data, from_data,
-- normalize_cwd, parse_project, commands_hash, is_trusted, trust, startup_action, session_weight,
-- commit_decision, valid_claude, is_program_trusted, trust_program.

local util = require("saysdf.util")
local t, _, _ = require("saysdf.i18n").domain("workspaces")

local M = {}
M.VERSION = 1
M._state = { opts = {} }

local function wezterm()
  return require("wezterm")
end

-- Choosing happens in the sidebar (`saysdf.sidebar.selector`), with InputSelector as the fallback.
local function sidebar()
  return require("saysdf.sidebar")
end

-- ---- layout: pane geometry <-> split tree ----
-- pane: { left, top, width, height, cwd?, args? }
-- tree: { pane = {cwd, args} } | { split = "h"|"v", ratio = 0..1, a = node, b = node }
--   "h" is a cut by a vertical line (a on the left, b on the right), "v" by a horizontal one (a on top).

local function try_cut(panes, lo, size)
  local min, max = math.huge, -math.huge
  for _, p in ipairs(panes) do
    min = math.min(min, p[lo])
    max = math.max(max, p[lo] + p[size])
  end
  local cuts = {}
  for _, p in ipairs(panes) do
    local x = p[lo] + p[size]
    if x < max then
      table.insert(cuts, x)
    end
  end
  table.sort(cuts)
  for _, x in ipairs(cuts) do
    local a, b, ok = {}, {}, true
    for _, p in ipairs(panes) do
      if p[lo] + p[size] <= x then
        table.insert(a, p)
      elseif p[lo] >= x then
        table.insert(b, p)
      else
        ok = false
        break
      end
    end
    if ok and #a > 0 and #b > 0 then
      return a, b, (x - min) / (max - min)
    end
  end
end

local function leaf(p)
  return { pane = { cwd = p.cwd, args = p.args, claude = p.claude } }
end

local function round3(x)
  return math.floor(x * 1000 + 0.5) / 1000
end

function M.layout_from_panes(panes)
  if #panes == 0 then
    return nil
  end
  if #panes == 1 then
    return leaf(panes[1])
  end
  local kind = "h"
  local a, b, r = try_cut(panes, "left", "width")
  if not a then
    kind = "v"
    a, b, r = try_cut(panes, "top", "height")
  end
  if not a then
    -- not a guillotine layout: stack the panes top to bottom
    a, b, r, kind = { panes[1] }, { table.unpack(panes, 2) }, 1 / #panes, "v"
  end
  return { split = kind, ratio = round3(r), a = M.layout_from_panes(a), b = M.layout_from_panes(b) }
end

local function first_leaf(n)
  while not n.pane do
    n = n.a
  end
  return n.pane
end

-- Plan: { root = {cwd, args}, steps = { {from = i, new = j, direction = "Right"|"Bottom",
-- size = share of the new pane, cwd, args} } }; pane 1 is the original one.
function M.plan_splits(layout)
  local steps, count = {}, 1
  local function walk(n, idx)
    if n.pane then
      return
    end
    count = count + 1
    local new = count
    local p = first_leaf(n.b)
    table.insert(steps, {
      from = idx,
      new = new,
      direction = n.split == "h" and "Right" or "Bottom",
      size = round3(1 - n.ratio),
      cwd = p.cwd,
      args = p.args,
      claude = p.claude,
    })
    walk(n.a, idx)
    walk(n.b, new)
  end
  walk(layout, 1)
  return { root = first_leaf(layout), steps = steps }
end

-- ---- serialization ----

local function valid_node(n, depth)
  if type(n) ~= "table" or depth > 64 then
    return false
  end
  if n.pane ~= nil then
    return type(n.pane) == "table" and (n.pane.cwd == nil or type(n.pane.cwd) == "string")
  end
  return (n.split == "h" or n.split == "v")
    and type(n.ratio) == "number" and n.ratio > 0 and n.ratio < 1
    and valid_node(n.a, depth + 1) and valid_node(n.b, depth + 1)
end

local function absolute(p)
  return type(p) == "string" and (p:find("^/") or p:find("^%a:[/\\]") or p:find("^\\\\")) ~= nil
end

-- A stored Claude Code session (D6): { session = <strict uuid>, exe = <absolute path named claude> }.
-- -> clean copy | nil, reason
function M.valid_claude(c)
  if type(c) ~= "table" then
    return nil, "not an object"
  end
  if not require("saysdf.claude").is_uuid(c.session) then
    return nil, "bad session id"
  end
  if not absolute(c.exe) or util.process_basename(c.exe) ~= "claude" then
    return nil, "exe is not an absolute path to claude"
  end
  return { session = c.session, exe = c.exe }
end

-- Copy of the tree; commands (args) are kept only with keep (the workspace's allow_commands flag).
-- The claude field is kept regardless of keep (it is not an arbitrary command), but only if valid;
-- on_drop(reason) is called for an invalid one.
local function strip(n, keep, on_drop)
  if n.pane then
    local args = nil
    if keep and type(n.pane.args) == "table" then
      args = {}
      for _, a in ipairs(n.pane.args) do
        table.insert(args, tostring(a))
      end
    end
    local claude = nil
    if n.pane.claude ~= nil then
      local err
      claude, err = M.valid_claude(n.pane.claude)
      if not claude and on_drop then
        on_drop(err)
      end
    end
    return { pane = { cwd = n.pane.cwd, args = args, claude = claude } }
  end
  return { split = n.split, ratio = n.ratio, a = strip(n.a, keep, on_drop), b = strip(n.b, keep, on_drop) }
end

-- saysdf.tabs is loaded lazily (tab colors live there); nil when unavailable.
local function tabs_api()
  local ok, m = pcall(require, "saysdf.tabs")
  return ok and m or nil
end

-- ws: { name, allow_commands?, tabs = { {title?, color?, layout} } } -> table for JSON
function M.to_data(ws)
  local keep = ws.allow_commands == true
  local tabs = {}
  for _, t in ipairs(ws.tabs or {}) do
    table.insert(tabs, { title = t.title, color = t.color, layout = strip(t.layout, keep) })
  end
  return { version = M.VERSION, name = ws.name, allow_commands = keep or nil, tabs = tabs }
end

-- -> ws | nil, err
function M.from_data(t)
  if type(t) ~= "table" then
    return nil, "not an object"
  end
  if type(t.version) ~= "number" then
    return nil, "no version"
  end
  if t.version > M.VERSION then
    return nil, "future version " .. t.version
  end
  if type(t.name) ~= "string" or t.name == "" then
    return nil, "no name"
  end
  if type(t.tabs) ~= "table" then
    return nil, "no tabs"
  end
  local keep = t.allow_commands == true
  local tabs = {}
  local function on_drop(reason)
    util.log_warn("saysdf.workspaces: " .. t.name .. ": claude field dropped: " .. reason)
  end
  for i, tab in ipairs(t.tabs) do
    if type(tab) ~= "table" or not valid_node(tab.layout, 0) then
      return nil, "bad layout in tab " .. i
    end
    local color = nil
    if tab.color ~= nil then
      color = tabs_api() and tabs_api().resolve_color(tab.color) or nil
      if not color then
        util.log_warn("saysdf.workspaces: " .. t.name .. ": tab color dropped: " .. tostring(tab.color))
      end
    end
    table.insert(tabs, {
      title = type(tab.title) == "string" and tab.title or nil,
      color = color,
      layout = strip(tab.layout, keep, on_drop),
    })
  end
  return { name = t.name, allow_commands = keep, tabs = tabs }
end

-- ---- masking and preview (change workspace-restore, D8) ----

local SECRET_NAMES = { "token", "secret", "passw", "pwd", "key", "auth", "bearer", "cookie", "credential" }

local function secret_name(name)
  local n = tostring(name):lower()
  for _, w in ipairs(SECRET_NAMES) do
    if n:find(w, 1, true) then
      return true
    end
  end
  return false
end

local function looks_like_path(a)
  return a:find("^/") or a:find("^%.%.?[/\\]") or a:find("^~") or a:find("^%a:[/\\]") or a:find("\\", 1, true)
end

local function looks_like_uuid(a)
  return a:lower():find("^%x%x%x%x%x%x%x%x%-%x%x%x%x%-%x%x%x%x%-%x%x%x%x%-%x%x%x%x%x%x%x%x%x%x%x%x$") ~= nil
end

-- argv -> masked argv, had_secret. Secret values become "***"; the program (argv[1]) and the value
-- after `--resume` are never touched.
function M.mask_args(args)
  local out, had = {}, false
  local mask_next = false
  for i, a in ipairs(args or {}) do
    a = tostring(a)
    local r = a
    local was_mask_next = mask_next
    mask_next = false
    if i == 1 or (args[i - 1] == "--resume") then
      r = a
    elseif was_mask_next and not a:find("^%-") then
      r = "***"
    elseif a:find("^%-") then
      local name, val = a:match("^%-+([^=]*)=(.*)$")
      if name then
        if secret_name(name) and val ~= "" then
          r = a:match("^(%-+[^=]*=)") .. "***"
        end
      elseif secret_name(a:gsub("^%-+", "")) then
        mask_next = true
      end
    else
      local name, val = a:match("^([%w_%.]+)=(.*)$")
      local scheme, user, rest = a:match("^(%a[%w+.-]*://)([^:/@]*):[^@/]*@(.*)$")
      if name and secret_name(name) and val ~= "" then
        r = name .. "=***"
      elseif scheme then
        r = scheme .. user .. ":***@" .. rest
      elseif #a >= 32 and not a:find("[^%w+/=_%-]") and not looks_like_path(a) and not looks_like_uuid(a) then
        r = "***"
      end
    end
    if r ~= a then
      had = true
    end
    out[i] = r
  end
  return out, had
end

local function home_short(path, home)
  if type(path) ~= "string" or path == "" then
    return "?"
  end
  if home and home ~= "" then
    local h = home:gsub("\\", "/"):gsub("/$", "")
    local p = path:gsub("\\", "/")
    if p == h then
      return "~"
    elseif p:sub(1, #h + 1) == h .. "/" then
      return "~" .. p:sub(#h + 1)
    end
  end
  return path
end

local function leaves(n, acc)
  if n.pane then
    table.insert(acc, n.pane)
  else
    leaves(n.a, acc)
    leaves(n.b, acc)
  end
  return acc
end

-- Leaves of a split tree with their place: { pane, label = "left"|"top right"|..., share = 0..1 }.
-- A pane spanning a whole axis has no word for it; a single pane has an empty label.
local function placed(n, x0, x1, y0, y1, acc)
  if n.pane then
    local function word(a, b, lo, mid, hi)
      local at_lo, at_hi = a < 0.02, b > 0.98
      if at_lo and at_hi then
        return nil
      end
      return at_lo and lo or (at_hi and hi or mid)
    end
    local v = word(y0, y1, t("top"), t("middle"), t("bottom"))
    local h = word(x0, x1, t("left"), t("center"), t("right"))
    local label = (v and h) and (v .. " " .. h) or (v or h or "")
    table.insert(acc, { pane = n.pane, label = label, share = (x1 - x0) * (y1 - y0) })
    return acc
  end
  if n.split == "h" then
    local xm = x0 + (x1 - x0) * n.ratio
    placed(n.a, x0, xm, y0, y1, acc)
    placed(n.b, xm, x1, y0, y1, acc)
  else
    local ym = y0 + (y1 - y0) * n.ratio
    placed(n.a, x0, x1, y0, ym, acc)
    placed(n.b, x0, x1, ym, y1, acc)
  end
  return acc
end

-- Human-readable Markdown preview of a workspace (what a manual save wrote).
-- ws: { name, allow_commands?, tabs = { {title?, layout} } }; panes may carry claude = { session }.
-- opts: { home?, file?, time?, restore?, positions? }; positions adds "(left, 50%)" to panes of a split tab;
-- a pane with omitted = true had a command that looked like a secret (it was left out of the file).
function M.preview_md(name, ws, opts)
  opts = opts or {}
  local L = {}
  local function add(s)
    table.insert(L, s)
  end
  local head = t("Workspace \"{name}\"", { name = name })
  if opts.time then
    head = head .. " " .. t("saved {time}", { time = opts.time })
  end
  add("# " .. head)
  add("")
  if opts.file then
    add(t("File: `{path}`", { path = home_short(opts.file, opts.home) }))
  end
  if ws.allow_commands then
    add(t("Commands: allowed"))
  else
    add(t("Commands: not allowed (only shells and Claude Code sessions are restored)"))
  end
  if opts.restore then
    add(t("Restore at startup: {mode}", { mode = tostring(opts.restore) }))
  end
  for ti, tab in ipairs(ws.tabs or {}) do
    add("")
    if tab.title and tab.title ~= "" then
      add("## " .. t("Tab {n} - \"{title}\"", { n = ti, title = tab.title }))
    else
      add("## " .. t("Tab {n}", { n = ti }))
    end
    for pi, it in ipairs(placed(tab.layout, 0, 1, 0, 1, {})) do
      local p = it.pane
      local where = ""
      if opts.positions and #placed(tab.layout, 0, 1, 0, 1, {}) > 1 then
        local pct = math.floor(it.share * 100 + 0.5) .. "%"
        where = " (" .. (it.label ~= "" and (it.label .. ", ") or "") .. pct .. ")"
      end
      add("- " .. t("Pane {n}", { n = pi }) .. where .. " - `" .. home_short(p.cwd, opts.home) .. "`")
      if type(p.claude) == "table" and type(p.claude.session) == "string" and p.claude.session ~= "" then
        add("  - " .. t("Claude Code: resume session `{id}`", { id = p.claude.session:sub(1, 8) .. "..." }))
      end
      if p.omitted then
        add("  - " .. t("command not saved: looks like it contains a secret"))
      elseif p.args and #p.args > 0 then
        local masked, had = M.mask_args(p.args)
        if had then
          add("  - " .. t("command not saved: looks like it contains a secret"))
        else
          add("  - " .. t("command: `{cmd}`", { cmd = table.concat(masked, " ") }))
        end
      end
    end
  end
  add("")
  add("## " .. t("Not saved"))
  add("- " .. t("Terminal history (scrollback), window size and position, environment variables."))
  return table.concat(L, "\n") .. "\n"
end

-- ---- autosave and startup ----

-- Autosave decision: at most once per debounce seconds and only when the serialization changed.
-- st: { last_check?, last_text? } (mutated). -> true if text should be written.
function M.autosave_decide(st, now, text, debounce)
  debounce = debounce or 30
  if st.last_check and now - st.last_check < debounce then
    return false
  end
  st.last_check = now
  if text == st.last_text then
    return false
  end
  st.last_text = text
  return true
end

-- Action at GUI startup (change workspace-restore, D2/D9).
-- mode = "always" (default, nil) | "ask" | "never" ("none" is an alias).
-- wss -- the previous session already read into memory: a list of workspaces ({ tabs = {{layout}} }),
-- or a boolean "a last-session exists" (legacy callers).
-- "always" restores any non-empty session, even one tab: its cwd counts. "ask" asks only when
-- session_worth_restoring (with a boolean there is nothing to judge, so it asks).
-- -> "restore" | "ask" | "none"
function M.startup_action(mode, wss)
  mode = mode or "always"
  local has
  if type(wss) == "table" then
    has = M.session_weight(wss).panes > 0
  else
    has = wss == true
  end
  if not has or mode == "never" or mode == "none" then
    return "none"
  end
  if mode == "always" then
    return "restore"
  end
  if type(wss) == "table" and not M.session_worth_restoring(wss) then
    return "none"
  end
  return "ask"
end

-- Weight of a session (D4): { panes = leaf panes, claude = panes with a stored Claude session },
-- summed over all workspaces. wss: list of workspaces ({ tabs = {{layout}} }).
function M.session_weight(wss)
  local w = { panes = 0, claude = 0 }
  local function walk(n, depth)
    if type(n) ~= "table" or depth > 64 then
      return
    end
    if n.pane ~= nil then
      w.panes = w.panes + 1
      local c = type(n.pane) == "table" and n.pane.claude
      if type(c) == "table" and type(c.session) == "string" and c.session ~= "" then
        w.claude = w.claude + 1
      end
      return
    end
    walk(n.a, depth + 1)
    walk(n.b, depth + 1)
  end
  for _, ws in ipairs(type(wss) == "table" and wss or {}) do
    for _, tab in ipairs(type(ws) == "table" and type(ws.tabs) == "table" and ws.tabs or {}) do
      walk(type(tab) == "table" and tab.layout, 0)
    end
  end
  return w
end

-- Shrink guard (D4): may a snapshot of weight new_w replace the saved one of weight saved_w?
-- st: { shrink_since? } (mutated); saved_w: nil when nothing is saved yet;
-- opts: number (shrink delay, s) or { delay = 90, manual = false }.
-- - an empty mux (no panes) is never written ("skip"), not even by a manual save;
-- - a manual save always writes and resets the guard;
-- - growth or the same weight (in both components) -> "write" at once, the guard is reset;
-- - a shrink is written only after it persisted for delay seconds: "wait" until then.
-- The 30 s debounce and "only on change" stay in autosave_decide.
-- -> "write" | "wait" | "skip"
function M.commit_decision(st, now, new_w, saved_w, opts)
  if type(opts) ~= "table" then
    opts = { delay = opts }
  end
  local delay = tonumber(opts.delay) or 90
  local np, nc = (new_w and new_w.panes) or 0, (new_w and new_w.claude) or 0
  if np <= 0 then
    return "skip"
  end
  if opts.manual or saved_w == nil then
    st.shrink_since = nil
    return "write"
  end
  if np >= (saved_w.panes or 0) and nc >= (saved_w.claude or 0) then
    st.shrink_since = nil
    return "write"
  end
  if st.shrink_since == nil then
    st.shrink_since = now
    return "wait"
  end
  if now - st.shrink_since >= delay then
    st.shrink_since = nil
    return "write"
  end
  return "wait"
end

-- Whether to ask about restoring: the previous session is non-trivial -- more than one tab
-- (across all workspaces together) or a tab with a split. One tab with one pane is a
-- silent normal start. wss is the list of loaded workspaces ({ tabs = {{layout}} }).
function M.session_worth_restoring(wss)
  local tabs = 0
  for _, ws in ipairs(wss or {}) do
    for _, t in ipairs(ws.tabs or {}) do
      tabs = tabs + 1
      if tabs > 1 or (type(t.layout) == "table" and t.layout.pane == nil) then
        return true
      end
    end
  end
  return false
end

-- last-session.json data: { version, workspaces = {names} } -> list of names | nil
function M.session_names(t)
  if type(t) ~= "table" or type(t.workspaces) ~= "table" or (tonumber(t.version) or 0) > M.VERSION then
    return nil
  end
  local out = {}
  for _, n in ipairs(t.workspaces) do
    if type(n) == "string" and n ~= "" then
      table.insert(out, n)
    end
  end
  return #out > 0 and out or nil
end

function M.encode(ws)
  return wezterm().json_encode(M.to_data(ws))
end

function M.decode(text)
  local ok, t = pcall(wezterm().json_parse, text)
  if not ok then
    return nil, "bad json"
  end
  return M.from_data(t)
end

-- ---- project file and trust ----

-- cwd from `.saysdf/workspace.json` inside the root: absolute paths and ".." are forbidden.
-- -> absolute path | nil, err
function M.normalize_cwd(root, rel, windows)
  rel = rel or "."
  if rel:match("^[/\\]") or rel:match("^%a:") or rel:match("^~") then
    return nil, "absolute path"
  end
  local parts = {}
  for seg in rel:gmatch("[^/\\]+") do
    if seg == ".." then
      return nil, "outside project"
    elseif seg ~= "." then
      table.insert(parts, seg)
    end
  end
  local sep = windows and "\\" or "/"
  root = root:gsub("[/\\]+$", "")
  if #parts == 0 then
    return root
  end
  return root .. sep .. table.concat(parts, sep)
end

function M.trust_key(root, windows)
  local k = root:gsub("[/\\]+$", "")
  if windows then
    k = k:gsub("/", "\\"):lower()
  end
  return k
end

-- Project commands { {cwd, args = {...}} } -> canonical strings
local function canon(commands)
  local out = {}
  for _, c in ipairs(commands or {}) do
    local args = type(c.args) == "table" and c.args or { tostring(c.args or "") }
    table.insert(out, (c.cwd or ".") .. "\0" .. table.concat(args, "\0"))
  end
  return out
end

function M.commands_hash(commands)
  return util.hash(table.concat(canon(commands), "\n"))
end

-- store: { [trust_key] = { hash, commands = {canonical strings} } }
function M.is_trusted(store, root, commands, windows)
  local e = store and store[M.trust_key(root, windows)]
  if type(e) ~= "table" or e.hash ~= M.commands_hash(commands) then
    return false
  end
  local c = canon(commands)
  if type(e.commands) ~= "table" or #e.commands ~= #c then
    return false
  end
  for i = 1, #c do
    if e.commands[i] ~= c[i] then
      return false
    end
  end
  return true
end

function M.trust(store, root, commands, windows)
  store[M.trust_key(root, windows)] = { hash = M.commands_hash(commands), commands = canon(commands) }
  return store
end

-- Programs the user ran in this terminal (D7): store.programs[trust_key(exe)] = { kind = "claude-resume" }.
-- Project entries are keyed by absolute paths, so the "programs" key cannot collide with them.
function M.is_program_trusted(store, exe, kind, windows)
  local p = type(store) == "table" and store.programs
  if type(p) ~= "table" or not absolute(exe) then
    return false
  end
  local e = p[M.trust_key(exe, windows)]
  return type(e) == "table" and e.kind == kind
end

function M.trust_program(store, exe, kind, windows)
  if not absolute(exe) then
    return store
  end
  if type(store.programs) ~= "table" then
    store.programs = {}
  end
  store.programs[M.trust_key(exe, windows)] = { kind = kind }
  return store
end

-- Project files from repositories cannot request a Claude resume (D7): only title, cwd and
-- command are read from a tab; any other field (claude included) is ignored.
-- -> { name, tabs = {{title, cwd(abs), args?}}, commands = {{cwd(rel), args}} } | nil, err
function M.parse_project(root, t, windows)
  if type(t) ~= "table" or type(t.tabs) ~= "table" then
    return nil, "no tabs"
  end
  local res = {
    name = type(t.name) == "string" and t.name or root:match("([^/\\]+)[/\\]*$"),
    tabs = {},
    commands = {},
  }
  for i, tab in ipairs(t.tabs) do
    local cwd, err = M.normalize_cwd(root, tab.cwd, windows)
    if not cwd then
      return nil, "tab " .. i .. ": " .. err
    end
    local args = tab.command
    if type(args) == "string" then
      args = { args }
    end
    table.insert(res.tabs, { title = tab.title, cwd = cwd, args = args })
    if args then
      table.insert(res.commands, { cwd = tab.cwd or ".", args = args })
    end
  end
  return res
end

-- ---- GUI work (checked by hand) ----

function M.dir()
  return M._state.opts.dir or util.join(util.data_dir(), "workspaces")
end

local function safe_name(name)
  return (name:gsub("[^%w%-_. ]", "_"))
end

local function cwd_of(pane)
  local u = pane:get_current_working_dir()
  if u == nil then
    return nil
  end
  local ok, p = pcall(function()
    return u.file_path
  end)
  if ok and p then
    if util.is_windows() then
      p = p:gsub("^/(%a:)", "%1")
    end
    return p
  end
  return tostring(u)
end

local SHELLS = { bash = 1, sh = 1, zsh = 1, fish = 1, nu = 1, cmd = 1, pwsh = 1, powershell = 1, wsl = 1, conhost = 1 }

local function is_shell(path)
  local exe = tostring(path or ""):match("([^/\\]+)$") or ""
  exe = exe:lower():gsub("%.exe$", "")
  return SHELLS[exe] ~= nil
end

-- argv cache per pane_id, keyed by the cached foreground name (D5): { [pane_id] = { name, argv } }
local args_cache = {}

-- argv of the pane's process if it is not a shell (for workspaces with allow_commands).
-- get_foreground_process_info (expensive) is called only when the cached foreground name is not
-- a shell and differs from the last call for this pane.
local function args_of(pane)
  local name = util.pane_foreground(pane).process
  if name == nil or name == "" or is_shell(name) then
    return nil
  end
  local okid, id = pcall(function()
    return pane:pane_id()
  end)
  local c = okid and args_cache[id]
  if c and c.name == name then
    return c.argv
  end
  local ok, info = pcall(function()
    return pane:get_foreground_process_info()
  end)
  local argv
  if ok and type(info) == "table" and type(info.argv) == "table" and #info.argv > 0 and not is_shell(info.argv[1]) then
    argv = info.argv
  end
  if okid then
    args_cache[id] = { name = name, argv = argv }
  end
  return argv
end

-- The "allow commands" flag is stored in the workspace's saved file (user data); it is cached
-- in memory so the autosave tick does not read the file on every pass.
local allow_cache = {}

function M.commands_allowed(name)
  if allow_cache[name] == nil then
    local ws = M.load(name)
    allow_cache[name] = ws ~= nil and ws.allow_commands == true
  end
  return allow_cache[name]
end

-- ---- Claude Code session detection (D6) ----
-- Runs only in the deferred autosave pass and on manual save, never in the synchronous part of
-- update-status. Per pane: { checked_at, start_time, argv_id, exe, bound = {session, start_time},
-- result = {session, exe}|nil }. The expensive process info and the directory listing are read
-- again only when the pane's claude process changed (its cached foreground name left "claude")
-- or the entry is older than the check interval.
local claude_cache = {}

local function is_local(pane)
  local ok, d = pcall(function()
    return pane:get_domain_name()
  end)
  return not ok or d == nil or d == "local"
end

local function claude_interval()
  local o = M._state.opts or {}
  return o.claude_interval or o.autosave_interval or 30
end

-- Test hook / override: the folder with Claude Code projects.
function M._claude_projects_dir()
  return require("saysdf.claude").projects_dir()
end

-- Linux boot time (btime of /proc/stat), read once: process start times there are clock ticks
-- since boot. Small kernel file, read only in the deferred pass (rule 5).
local boot_time_cache
function M._boot_time()
  if boot_time_cache == nil then
    boot_time_cache = false
    local text = util.read_file("/proc/stat")
    local bt = text and tonumber(text:match("%f[%w]btime%s+(%d+)"))
    if bt then
      boot_time_cache = bt
    end
  end
  return boot_time_cache or nil
end

-- Test hook: the process start time from get_foreground_process_info() in epoch seconds
-- (the raw value is in OS-specific units, see saysdf.claude.start_unix).
function M._process_start_unix(raw)
  local triple = wezterm().target_triple or ""
  return require("saysdf.claude").start_unix(raw, triple, triple:find("linux") and M._boot_time() or nil)
end

local function dir_entries(dir)
  local f = wezterm().saysdf_dir_entries
  if type(f) ~= "function" then
    return {} -- old binary: no Claude sessions found (D1)
  end
  local ok, list = pcall(f, dir, { suffix = ".jsonl" })
  return (ok and type(list) == "table") and list or {}
end

-- items: { { pane, id, cwd } }; all = items cover every pane of the mux (closed panes are pruned).
-- -> { [pane_id] = { session, exe } }
function M.detect_claude(items, now, all)
  local claude = require("saysdf.claude")
  now = now or M._clock()
  local interval = claude_interval()
  local groups, order, live = {}, {}, {}
  for _, it in ipairs(items) do
    local name = util.pane_foreground(it.pane).process
    if util.process_basename(name) == "claude" and it.cwd and is_local(it.pane) then
      live[it.id] = true
      local c = claude_cache[it.id]
      if c == nil then
        c = {}
        claude_cache[it.id] = c
      end
      c.stale = c.checked_at == nil or now - c.checked_at >= interval or c.cwd ~= it.cwd
      if c.stale then
        c.checked_at, c.cwd = now, it.cwd
        local ok, info = pcall(function()
          return it.pane:get_foreground_process_info()
        end)
        info = (ok and type(info) == "table") and info or {}
        c.start_time = M._process_start_unix(info.start_time)
        c.argv_id = claude.argv_session(info.argv)
        local exe = info.executable
        c.exe = (absolute(exe) and util.process_basename(exe) == "claude") and exe or nil
      end
      if not groups[it.cwd] then
        groups[it.cwd] = {}
        table.insert(order, it.cwd)
      end
      table.insert(groups[it.cwd], it.id)
    else
      claude_cache[it.id] = nil -- no claude in the foreground: a new one is detected afresh
    end
  end
  if all then
    for id in pairs(claude_cache) do
      if not live[id] then
        claude_cache[id] = nil -- closed pane
      end
    end
  end
  local out = {}
  for _, cwd in ipairs(order) do
    local ids = groups[cwd]
    local any_stale = false
    for _, id in ipairs(ids) do
      any_stale = any_stale or claude_cache[id].stale
    end
    if any_stale then
      local mangled = claude.mangle(cwd)
      local files = mangled and dir_entries(util.join(M._claude_projects_dir(), mangled)) or {}
      local panes = {}
      for _, id in ipairs(ids) do
        local c = claude_cache[id]
        table.insert(panes, { pane_id = id, start_time = c.start_time, argv_id = c.argv_id, bound = c.bound })
      end
      local reasons = {}
      local found = claude.match_sessions(panes, files, reasons)
      for _, id in ipairs(ids) do
        local c = claude_cache[id]
        local sid = found[id]
        c.bound = sid and { session = sid, start_time = c.start_time } or nil
        c.result = (sid and c.exe) and { session = sid, exe = c.exe } or nil
        local why = (not mangled and "no project folder name")
          or reasons[id]
          or (sid and not c.exe and "executable not recognised")
        -- once per pane per claude process (the cache entry is dropped when claude leaves the pane)
        if why and not c.logged then
          c.logged = true
          util.log_info(
            "saysdf.workspaces: claude session not found (" .. why .. ") in " .. tostring(mangled)
          )
        end
      end
    end
    for _, id in ipairs(ids) do
      out[id] = claude_cache[id].result
    end
  end
  return out
end

-- All panes of the mux (every workspace): { { pane, id, cwd } }.
local function mux_panes(only_ws)
  local items = {}
  for _, win in ipairs(wezterm().mux.all_windows()) do
    if only_ws == nil or win:get_workspace() == only_ws then
      for _, tab in ipairs(win:tabs()) do
        for _, info in ipairs(tab:panes_with_info()) do
          local okid, id = pcall(function()
            return info.pane:pane_id()
          end)
          if okid then
            table.insert(items, { pane = info.pane, id = id, cwd = cwd_of(info.pane) })
          end
        end
      end
    end
  end
  return items
end

-- Test hook: forget the argv / allow-commands / Claude caches.
function M._forget_caches()
  args_cache, allow_cache, claude_cache = {}, {}, {}
end

-- Workspace snapshot: windows of this workspace -> tabs -> layout; commands only with the flag.
-- claude_map: { [pane_id] = {session, exe} } from detect_claude; nil -> detected for this workspace.
function M.snapshot(name, allow_commands, claude_map)
  if allow_commands == nil then
    allow_commands = M.commands_allowed(name)
  end
  claude_map = claude_map or M.detect_claude(mux_panes(name))
  local ws = { name = name, allow_commands = allow_commands, tabs = {} }
  for _, win in ipairs(wezterm().mux.all_windows()) do
    if win:get_workspace() == name then
      for _, tab in ipairs(win:tabs()) do
        local panes = {}
        for _, info in ipairs(tab:panes_with_info()) do
          local okid, id = pcall(function()
            return info.pane:pane_id()
          end)
          table.insert(panes, {
            left = info.left, top = info.top, width = info.width, height = info.height,
            cwd = cwd_of(info.pane),
            args = allow_commands and args_of(info.pane) or nil,
            claude = okid and claude_map[id] or nil,
          })
        end
        local title = tab:get_title()
        local okc, color = pcall(function()
          return tabs_api() and tabs_api().get_color(tab) or nil
        end)
        table.insert(ws.tabs, {
          title = title ~= "" and title or nil,
          color = okc and color or nil,
          layout = M.layout_from_panes(panes),
        })
      end
    end
  end
  return ws
end

function M.save(name, allow_commands, text)
  util.ensure_dir(M.dir())
  if allow_commands ~= nil then
    allow_cache[name] = allow_commands == true
  end
  text = text or M.encode(M.snapshot(name, allow_commands))
  local ok, err = util.write_atomic(util.join(M.dir(), safe_name(name) .. ".json"), text)
  if not ok then
    util.log_error("saysdf.workspaces: save failed: " .. tostring(err))
  end
  return ok
end

-- Manual save (task 3.4): snapshot with Claude detection, secrets masked (a pane whose command looks
-- like it has a secret keeps only its folder), written at once (no shrink guard), then a Markdown
-- preview of what was saved goes to <name>.preview.md and opens in the viewer (notice if unavailable).
-- -> { tabs, panes, claude, preview } | nil when nothing was written.
function M.save_manual(window, pane, name)
  name = name or window:active_workspace()
  local snap = M.snapshot(name, nil, M.detect_claude(mux_panes(nil), M._clock(), true))
  local counts = { tabs = #snap.tabs, panes = 0, claude = 0 }
  for _, tab in ipairs(snap.tabs) do
    for _, p in ipairs(leaves(tab.layout, {})) do
      counts.panes = counts.panes + 1
      if p.claude and p.claude.session then
        counts.claude = counts.claude + 1
      end
      if p.args then
        local masked, had = M.mask_args(p.args)
        if had then
          p.args, p.omitted = nil, true
        else
          p.args = masked
        end
      end
    end
  end
  if counts.tabs == 0 then
    util.notify(window, t("Workspaces"), t("Nothing to save: the workspace has no tabs"))
    return nil
  end
  local file = util.join(M.dir(), safe_name(name) .. ".json")
  if not M.save(name, nil, M.encode(snap)) then
    return nil
  end
  util.notify(window, t("Workspaces"), t("Saved: {tabs} tabs, {panes} panes, {claude} Claude sessions", counts))
  local preview = util.join(M.dir(), safe_name(name) .. ".preview.md")
  local md = M.preview_md(name, snap, {
    home = wezterm().home_dir, file = file, time = os.date("%Y-%m-%d %H:%M"),
    restore = (M._state.opts or {}).restore or "always", positions = true,
  })
  if util.write_atomic(preview, md) then
    local okv, viewer = pcall(require, "saysdf.viewer")
    local opened = false
    if okv and type(viewer) == "table" and type(viewer.open) == "function" then
      local okc, res = pcall(viewer.open, window, pane, preview)
      opened = okc and res ~= false
    end
    if not opened then
      util.notify(window, t("Workspaces"), t("Preview: {path}", { path = preview }))
    end
  end
  counts.preview = preview
  return counts
end

-- ---- Naming: rename / save as (change workspace-naming, D2/D3) ----

-- Pure decision for renaming `old` to `new`. open: names of open workspaces; exists(name) -> bool: a
-- saved file for that name exists. -> { action = "cancel"|"same"|"refuse"|"confirm"|"go", name = trimmed }
function M.rename_decision(old, new, open, exists)
  new = type(new) == "string" and new:match("^%s*(.-)%s*$") or ""
  if new == "" then
    return { action = "cancel" }
  end
  if new == old then
    return { action = "same", name = new }
  end
  for _, n in ipairs(open or {}) do
    if n == new then
      return { action = "refuse", name = new }
    end
  end
  if exists and exists(new) then
    return { action = "confirm", name = new }
  end
  return { action = "go", name = new }
end

-- Move <old>.json and <old>.preview.md to the new names (write_atomic + remove). -> ok, err
local function move_saved(old, new)
  local a, b = safe_name(old), safe_name(new)
  if a == b then
    return true -- both names map to the same file
  end
  local moved = false
  for _, ext in ipairs({ ".json", ".preview.md" }) do
    local from, to = util.join(M.dir(), a .. ext), util.join(M.dir(), b .. ext)
    local text = util.read_file(from)
    if text then
      local ok, err = util.write_atomic(to, text)
      if not ok then
        return false, err
      end
      os.remove(from)
      moved = true
    elseif moved then
      os.remove(to) -- no stale preview of the replaced workspace
    end
  end
  return true
end

-- Ask a name with PromptInputLine; cb(name) gets the raw line (nil on Esc).
function M.prompt_name(window, pane, description, initial, cb)
  local w = wezterm()
  window:perform_action(
    w.action.PromptInputLine({
      description = description,
      initial_value = initial or "",
      action = w.action_callback(function(_, _, line)
        cb(line)
      end),
    }),
    pane
  )
end

local SESSION_FILE = "last-session.json"

-- Write last-session.json with the given workspace names.
local function write_last_session(names)
  util.write_atomic(
    util.join(M.dir(), SESSION_FILE),
    wezterm().json_encode({ version = M.VERSION, workspaces = names })
  )
end

-- Rename the open workspace `old` to `new` and move its saved files. opts.done(ok, name) is called when
-- finished (not on cancel/refusal -> ok=false). Overwrite of a saved `new` is confirmed (default: no).
function M.rename_workspace(window, pane, old, new, opts)
  local w = wezterm()
  local done = (opts and opts.done) or function() end
  local function exists(n)
    return util.read_file(util.join(M.dir(), safe_name(n) .. ".json")) ~= nil
  end
  local d = M.rename_decision(old, new, w.mux.get_workspace_names(), exists)
  if d.action == "cancel" then
    return done(false)
  elseif d.action == "same" then
    return done(true, d.name)
  elseif d.action == "refuse" then
    util.notify(window, t("Workspaces"), t("A workspace named {name} is already open", { name = d.name }))
    return done(false)
  end
  local function go()
    local ok = pcall(w.mux.rename_workspace, old, d.name)
    if not ok then
      util.notify(window, t("Workspaces"), t("Could not rename the workspace"))
      return done(false)
    end
    local moved, err = move_saved(old, d.name)
    if not moved then
      util.log_error("saysdf.workspaces: move failed: " .. tostring(err))
      util.notify(window, t("Workspaces"), t("The saved files were not moved: {error}", { error = tostring(err) }))
    end
    allow_cache[d.name], allow_cache[old] = allow_cache[old], nil
    if M._state.autosave then
      M._state.autosave.last_text = nil -- the names changed: compare afresh on the next pass
    end
    -- a crash before the next autosave must not leave the old name in last-session.json
    write_last_session(w.mux.get_workspace_names())
    return done(true, d.name)
  end
  if d.action == "go" then
    return go()
  end
  window:perform_action(
    w.action.InputSelector({
      title = t("Replace the saved workspace {name}?", { name = d.name }),
      choices = {
        { id = "no", label = t("No, keep it") },
        { id = "yes", label = t("Yes, replace") },
      },
      action = w.action_callback(function(_, _, id)
        if id == "yes" then
          go()
        else
          done(false)
        end
      end),
    }),
    pane
  )
end

-- "Workspaces: rename…"
function M.rename_prompt(window, pane)
  local old = window:active_workspace()
  M.prompt_name(window, pane, t("Workspace name (Esc to cancel)"), old, function(line)
    if line ~= nil then
      M.rename_workspace(window, pane, old, line)
    end
  end)
end

-- "Workspaces: save as…": the active workspace takes the new name, then is saved under it.
function M.save_as_prompt(window, pane)
  local old = window:active_workspace()
  M.prompt_name(window, pane, t("Save workspace as (Esc to cancel)"), old, function(line)
    M.rename_workspace(window, pane, old, line, {
      done = function(ok, name)
        if ok then
          M.save_manual(window, pane, name)
        end
      end,
    })
  end)
end

function M.load(name)
  local text = util.read_file(util.join(M.dir(), safe_name(name) .. ".json"))
  if not text then
    return nil, "not found"
  end
  return M.decode(text)
end

function M.saved_names()
  local out = {}
  local ok, files = pcall(wezterm().read_dir, M.dir())
  if ok then
    for _, f in ipairs(files) do
      local n = f:match("([^/\\]+)%.json$")
      if n then
        table.insert(out, n)
      end
    end
  end
  table.sort(out)
  return out
end

-- ---- Claude Code resume (D7) ----

M.RESUME_KIND = "claude-resume"
M.DENIED_KIND = "claude-resume-denied"

-- Existence of the executable; restore is a one-shot startup action, not a hot path (rule 5).
-- Tests replace it.
function M._exists(path)
  local f = io.open(path, "rb")
  if f then
    f:close()
    return true
  end
  return false
end

-- Context of one restore: the trust store (read once), OS flag, per-cwd session listings.
function M.resume_context()
  return { store = M.trust_store(), windows = util.is_windows(), sessions = {} }
end

local function session_exists(ctx, cwd, uuid)
  local claude = require("saysdf.claude")
  local set = ctx.sessions[cwd]
  if not set then
    set = {}
    local mangled = claude.mangle(cwd)
    if mangled then
      for _, e in ipairs(dir_entries(util.join(M._claude_projects_dir(), mangled))) do
        local id = claude.file_session(type(e) == "table" and e.path or nil)
        if id then
          set[id] = true
        end
      end
    end
    ctx.sessions[cwd] = set
  end
  return set[uuid] == true
end

-- Decide the argv of a pane with a stored Claude session. The argv is built here and only here,
-- always exactly { exe, "--resume", uuid }; nothing else from the saved file reaches it.
-- -> argv | nil, reason ("pending" = all checks pass but the exe was never answered for).
-- The workspace's allow_commands flag does not matter for resume.
function M.resume_args(c, cwd, ctx)
  if (M._state.opts or {}).claude_resume == false then
    return nil, "claude_resume is off"
  end
  local ok, reason = M.valid_claude(c)
  if not ok then
    return nil, reason
  end
  if type(cwd) ~= "string" or not session_exists(ctx, cwd, ok.session) then
    return nil, "session file not found"
  end
  if not M._exists(ok.exe) then
    return nil, "exe not found"
  end
  if M.is_program_trusted(ctx.store, ok.exe, M.RESUME_KIND, ctx.windows) then
    return { ok.exe, "--resume", ok.session }
  end
  if M.is_program_trusted(ctx.store, ok.exe, M.DENIED_KIND, ctx.windows) then
    return nil, "exe not allowed by the user"
  end
  return nil, "pending"
end

-- Executables of ws that pass every check but trust: { {key, exe} }, unique by trust key.
function M.pending_programs(ws, ctx, acc, seen)
  acc, seen = acc or {}, seen or {}
  for _, tab in ipairs(ws.tabs or {}) do
    for _, p in ipairs(leaves(tab.layout, {})) do
      if p.claude ~= nil then
        local _, reason = M.resume_args(p.claude, p.cwd, ctx)
        local key = reason == "pending" and M.trust_key(p.claude.exe, ctx.windows)
        if key and not seen[key] then
          seen[key] = true
          table.insert(acc, { key = key, exe = p.claude.exe })
        end
      end
    end
  end
  return acc
end

-- args of a restored pane: a Claude pane gets the resume argv or a shell (never its saved args);
-- any other pane gets its saved args only with allow_commands.
local function pane_args(p, allow_commands, ctx, ws_name)
  if p.claude ~= nil then
    local argv, reason = M.resume_args(p.claude, p.cwd, ctx)
    if not argv then
      -- the reason and the workspace only: no saved values, no file contents
      util.log_warn("saysdf.workspaces: " .. tostring(ws_name) .. ": claude resume skipped, shell opened: "
        .. (reason == "pending" and "exe not allowed yet" or tostring(reason)))
    end
    return argv
  end
  return allow_commands and p.args or nil
end

-- Restore; commands only with allow_commands (approved project commands) or the saved
-- workspace's ws.allow_commands flag. ctx: resume context (nil -> read now).
-- -> the first window and pane
function M.restore(ws, allow_commands, ctx)
  allow_commands = allow_commands == true or ws.allow_commands == true
  ctx = ctx or M.resume_context()
  local mux = wezterm().mux
  local window, first
  for i, t in ipairs(ws.tabs) do
    local plan = M.plan_splits(t.layout)
    local root_args = pane_args(plan.root, allow_commands, ctx, ws.name)
    local tab, pane
    if i == 1 then
      tab, pane, window = mux.spawn_window({ workspace = ws.name, cwd = plan.root.cwd, args = root_args })
      first = pane
    else
      tab, pane = window:spawn_tab({ cwd = plan.root.cwd, args = root_args })
    end
    if t.title then
      tab:set_title(t.title)
    end
    if t.color and tabs_api() then
      tabs_api().set_color(tab, t.color)
    end
    local made = { pane }
    for _, s in ipairs(plan.steps) do
      made[s.new] = made[s.from]:split({
        direction = s.direction,
        size = s.size,
        cwd = s.cwd,
        args = pane_args(s, allow_commands, ctx, ws.name),
      })
    end
  end
  mux.set_active_workspace(ws.name)
  return window, first
end

-- Remove the "no" answers for the Claude executables saved in workspace name, so the next
-- restore asks again (palette "allow/forbid commands" of that workspace).
function M.forget_denials(name)
  local ws = M.load(name)
  if not ws then
    return 0
  end
  local store, win_os, n = M.trust_store(), util.is_windows(), 0
  for _, tab in ipairs(ws.tabs) do
    for _, p in ipairs(leaves(tab.layout, {})) do
      local c = p.claude
      if c and c.exe and type(store.programs) == "table" then
        local key = M.trust_key(c.exe, win_os)
        local e = store.programs[key]
        if type(e) == "table" and e.kind == M.DENIED_KIND then
          store.programs[key] = nil
          n = n + 1
        end
      end
    end
  end
  if n > 0 then
    M.save_trust_store(store)
  end
  return n
end

local TRUST_FILE = "workspace-trust.json"

function M.trust_store()
  local text = util.read_file(util.join(util.data_dir(), TRUST_FILE))
  if not text then
    return {}
  end
  local ok, t = pcall(wezterm().json_parse, text)
  return (ok and type(t) == "table") and t or {}
end

function M.save_trust_store(store)
  util.ensure_dir(util.data_dir())
  return util.write_atomic(util.join(util.data_dir(), TRUST_FILE), wezterm().json_encode(store))
end

-- Open the project's `.saysdf/workspace.json` in root; commands only after confirmation.
function M.open_project(window, pane, root)
  local w = wezterm()
  local win_os = util.is_windows()
  local text = util.read_file(util.join(root, ".saysdf", "workspace.json"))
  if not text then
    return util.log_warn("saysdf.workspaces: no .saysdf/workspace.json in " .. root)
  end
  local okj, parsed = pcall(w.json_parse, text)
  local proj, err = M.parse_project(root, okj and parsed or nil, win_os)
  if not proj then
    return util.log_error("saysdf.workspaces: " .. tostring(err))
  end
  local ws = { name = proj.name, tabs = {} }
  for _, tab in ipairs(proj.tabs) do
    table.insert(ws.tabs, { title = tab.title, layout = { pane = { cwd = tab.cwd, args = tab.args } } })
  end
  local store = M.trust_store()
  if #proj.commands == 0 or M.is_trusted(store, root, proj.commands, win_os) then
    return M.restore(ws, true)
  end
  local lines = {}
  for _, c in ipairs(proj.commands) do
    table.insert(lines, c.cwd .. ": " .. table.concat(c.args, " "))
  end
  window:perform_action(w.action.InputSelector({
    title = t("Project commands: {commands}", { commands = table.concat(lines, "; ") }),
    choices = {
      { id = "run", label = t("Run (remember for this command list)") },
      { id = "shells", label = t("Shells only") },
      { id = "cancel", label = t("Cancel") },
    },
    action = w.action_callback(function(_, _, id)
      if id == "run" then
        M.trust(store, root, proj.commands, win_os)
        M.save_trust_store(store)
        M.restore(ws, true)
      elseif id == "shells" then
        M.restore(ws, false)
      end
    end),
  }), pane)
end

-- "New workspace…": switch to an open one, restore a saved one, else spawn a fresh one in the pane's cwd.
function M.new_workspace(window, pane, name)
  local w = wezterm()
  name = type(name) == "string" and name:match("^%s*(.-)%s*$") or ""
  if name == "" then
    return
  end
  for _, n in ipairs(w.mux.get_workspace_names()) do
    if n == name then
      return window:perform_action(w.action.SwitchToWorkspace({ name = name }), pane)
    end
  end
  local ws = M.load(name)
  if ws then
    return M.restore(ws, false)
  end
  local spawn = {}
  local cwd = pane and cwd_of(pane)
  if cwd then
    spawn.cwd = cwd
  end
  window:perform_action(w.action.SwitchToWorkspace({ name = name, spawn = spawn }), pane)
end

function M.new_workspace_prompt(window, pane)
  M.prompt_name(window, pane, t("New workspace name (Esc to cancel)"), "", function(line)
    M.new_workspace(window, pane, line)
  end)
end

-- Workspace list: open + saved, fuzzy.
function M.choose(window, pane)
  local w = wezterm()
  local seen, choices = {}, { { id = "new:", label = t("New workspace…") } }
  for _, n in ipairs(w.mux.get_workspace_names()) do
    seen[n] = true
    table.insert(choices, { id = "open:" .. n, label = t("{name}  (open)", { name = n }) })
  end
  for _, n in ipairs(M.saved_names()) do
    if not seen[n] then
      table.insert(choices, { id = "saved:" .. n, label = t("{name}  (saved)", { name = n }) })
    end
  end
  window:perform_action(sidebar().selector({
    tool = "workspaces",
    title = t("Workspaces"),
    fuzzy = true,
    choices = choices,
    on_choice = function(win, p, id)
      if not id then
        return
      end
      local kind, name = id:match("^(%a+):(.*)$")
      if kind == "new" then
        M.new_workspace_prompt(win, p)
      elseif kind == "open" then
        win:perform_action(w.action.SwitchToWorkspace({ name = name }), p)
      else
        local ws, err = M.load(name)
        if ws then
          M.restore(ws, false)
        else
          util.log_error("saysdf.workspaces: " .. tostring(err))
        end
      end
    end,
  }), pane)
end

-- Clock (seconds); tests replace it.
function M._clock()
  return os.time()
end

M.GRACE_S = 5 -- after the gate opens: restored shells report their cwd (OSC 7) first (D2)
M.STARTUP_TIMEOUT_S = 120 -- the gate is stuck in "startup" (gui-startup never ran): open it (D3)

-- Autosave gate (D2): "startup" -> "restoring" -> "open"; autosave writes only when "open"
-- and the grace period after opening has passed.
function M.open_gate(now)
  local st = M._state
  st.gate = "open"
  st.open_at = now or M._clock()
end

local function gate_allows(st, now)
  if st.gate == "startup" then
    -- gui-startup did not fire (attach, mux server): open after a timeout, counted from the
    -- first tick, so a stuck gate cannot disable autosave for good.
    st.startup_seen = st.startup_seen or now
    if now - st.startup_seen >= M.STARTUP_TIMEOUT_S then
      util.log_warn("saysdf.workspaces: no gui-startup, autosave enabled")
      M.open_gate(now)
    end
    return false
  end
  if st.gate ~= nil and st.gate ~= "open" then
    return false
  end
  return now - (st.open_at or 0) >= (st.opts.grace or M.GRACE_S)
end

-- One autosave pass (deferred from update-status): all open workspaces + last-session.
-- Writes only through the gate (D2) and the shrink guard (D4); an empty mux is never written.
function M.autosave_tick(now)
  local w = wezterm()
  local st = M._state
  st.autosave = st.autosave or {}
  now = now or M._clock()
  if not gate_allows(st, now) then
    return false
  end
  local interval = st.opts.autosave_interval or 30
  local last = st.autosave.last_check
  if last and now - last < interval then
    return false -- no snapshot more often than the debounce
  end
  local names = w.mux.get_workspace_names()
  local parts, wss = {}, {}
  -- one detection pass over all workspaces: panes in the same cwd are matched together
  local claude_map = M.detect_claude(mux_panes(nil), now, true)
  for _, n in ipairs(names) do
    local snap = M.snapshot(n, nil, claude_map)
    table.insert(wss, snap)
    parts[n] = M.encode(snap)
    table.insert(parts, parts[n])
  end
  local all = table.concat(names, "\n") .. "\n" .. table.concat(parts, "\n")
  if not M.autosave_decide(st.autosave, now, all, interval) then
    return false
  end
  if st.autosave.saved_w == nil and not st.autosave.saved_w_known then
    -- after a config reload: the weight of what is on disk (read once, without quarantine)
    st.autosave.saved_w_known = true
    local prev = M.read_previous({ quarantine = false })
    if prev and #prev.wss > 0 then
      st.autosave.saved_w = M.session_weight(prev.wss)
    end
  end
  local new_w = M.session_weight(wss)
  local verdict = M.commit_decision(st.autosave, now, new_w, st.autosave.saved_w, st.opts.shrink_delay or 90)
  if verdict ~= "write" then
    st.autosave.last_text = nil -- not written: compare again on the next pass
    return false
  end
  for _, n in ipairs(names) do
    M.save(n, nil, parts[n])
  end
  write_last_session(names)
  st.autosave.saved_w = new_w
  st.autosave.saved_w_known = true
  return true
end

function M.last_session()
  local text = util.read_file(util.join(M.dir(), SESSION_FILE))
  if not text then
    return nil
  end
  local ok, t = pcall(wezterm().json_parse, text)
  return ok and M.session_names(t) or nil
end

-- Read the previous session (last-session.json + every listed <name>.json) into memory (D2).
-- A corrupt workspace file is skipped with a log line and, unless opts.quarantine == false,
-- renamed to <name>.json.bad (so the next autosave does not lose the evidence).
-- -> { names = {..}, wss = {..}, bad = {..} } | nil (no previous session)
function M.read_previous(opts)
  opts = opts or {}
  local names = M.last_session()
  if not names then
    return nil
  end
  local prev = { names = {}, wss = {}, bad = {} }
  for _, n in ipairs(names) do
    local path = util.join(M.dir(), safe_name(n) .. ".json")
    local text = util.read_file(path)
    if text then
      local ws, err = M.decode(text)
      if ws then
        table.insert(prev.names, n)
        table.insert(prev.wss, ws)
        allow_cache[n] = ws.allow_commands == true
      else
        table.insert(prev.bad, n)
        util.log_warn("saysdf.workspaces: skip corrupt " .. n .. ": " .. tostring(err))
        if opts.quarantine ~= false then
          os.remove(path .. ".bad")
          local ok, rerr = os.rename(path, path .. ".bad")
          if not ok then
            util.log_error("saysdf.workspaces: cannot rename " .. n .. " to .bad: " .. tostring(rerr))
          end
        end
      end
    else
      util.log_warn("saysdf.workspaces: skip " .. n .. ": not found")
    end
  end
  return prev
end

-- Restore the previous session's workspaces from the in-memory copy (never re-reads files that
-- an autosave may have overwritten). Commands only for workspaces with the flag.
-- wss: list of workspaces (from read_previous); strings are names, loaded for legacy callers.
-- Claude Code resume asks once per new executable (D7): workspaces with a pane whose exe was never
-- answered wait until every such exe got one prompt; the others restore at once. "Allow" stores
-- programs[exe] = claude-resume, "No" stores claude-resume-denied (no nagging; reset by the palette
-- "allow/forbid commands" of that workspace), cancel stores nothing (shells now, ask next time).
-- done() runs when everything is restored (the startup gate opens there).
function M.restore_session(wss, done)
  done = done or function() end
  local w = wezterm()
  local list = {}
  for _, ws in ipairs(wss or {}) do
    local err
    if type(ws) == "string" then
      local n = ws
      ws, err = M.load(n)
      err = n .. ": " .. tostring(err)
    end
    if type(ws) == "table" then
      table.insert(list, ws)
    else
      util.log_warn("saysdf.workspaces: skip " .. tostring(err))
    end
  end
  local ok_ctx, ctx = pcall(M.resume_context)
  if not ok_ctx then
    ctx = { store = {}, windows = util.is_windows(), sessions = {} }
  end
  local pending, seen, now, held = {}, {}, {}, {}
  for _, ws in ipairs(list) do
    local okp, mine = pcall(M.pending_programs, ws, ctx)
    mine = okp and mine or {}
    for _, e in ipairs(mine) do
      if not seen[e.key] then
        seen[e.key] = true
        table.insert(pending, e)
      end
    end
    table.insert(#mine > 0 and held or now, ws)
  end
  local first_window, first_pane
  local function run(ws)
    local ok, win, pane = pcall(M.restore, ws, false, ctx)
    if not ok then
      util.log_error("saysdf.workspaces: restore " .. tostring(ws.name) .. ": " .. tostring(win))
    elseif not first_window then
      first_window, first_pane = win, pane
    end
  end
  for _, ws in ipairs(now) do
    run(ws)
  end
  local function finish()
    for _, ws in ipairs(held) do
      run(ws)
    end
    done()
  end
  if #pending == 0 then
    return finish()
  end
  if not first_window then
    -- every workspace waits: the question needs a window
    local _, p, win = w.mux.spawn_window({})
    first_window, first_pane = win, p
  end
  local okg, gui = pcall(function()
    return first_window:gui_window()
  end)
  if not okg or not gui then
    util.log_warn("saysdf.workspaces: no window to ask about Claude Code resume: shells opened")
    return finish()
  end
  local changed, i = false, 0
  local function ask_next()
    i = i + 1
    local e = pending[i]
    if not e then
      if changed then
        M.save_trust_store(ctx.store)
      end
      return finish()
    end
    gui:perform_action(w.action.InputSelector({
      title = t("Allow saysdfTerm to start {exe} --resume automatically?", { exe = e.exe }),
      choices = {
        { id = "allow", label = t("Allow (remember this program)") },
        { id = "deny", label = t("No, open shells (remember)") },
      },
      action = w.action_callback(function(_, _, id)
        if id == "allow" or id == "deny" then
          M.trust_program(ctx.store, e.exe, id == "allow" and M.RESUME_KIND or M.DENIED_KIND, ctx.windows)
          changed = true
        end
        ask_next()
      end),
    }), first_pane)
  end
  ask_next()
end

-- gui-startup was given an explicit command or folder (`saysdfTerm start -- prog`, "open here").
local function explicit(cmd)
  if type(cmd) ~= "table" and type(cmd) ~= "userdata" then
    return false
  end
  local ok, has = pcall(function()
    return cmd.args ~= nil or cmd.cwd ~= nil
  end)
  return ok and has
end

-- gui-startup (D2): read the previous session first, keep the gate closed until the decision
-- is made and the restore has finished, then open it (autosave waits a further grace period).
function M.on_startup(cmd)
  local w = wezterm()
  local st = M._state
  st.gate = "restoring"
  local prev = M.read_previous()
  st.previous = prev
  local wss = prev and prev.wss or {}
  st.autosave = st.autosave or {}
  if #wss > 0 then
    -- the shrink guard compares against the session on disk, not against "nothing saved"
    st.autosave.saved_w = M.session_weight(wss)
    st.autosave.saved_w_known = true
  end
  local act = M.startup_action(st.opts.restore or "always", wss)
  if act == "restore" then
    -- done may run later: after the Claude Code resume question (D7); the gate stays closed until then
    M.restore_session(wss, function()
      local okw, wins = pcall(w.mux.all_windows)
      if not explicit(cmd) and okw and type(wins) == "table" and #wins == 0 then
        w.mux.spawn_window(cmd or {}) -- every restore failed: still start with a window
      elseif explicit(cmd) then
        -- the requested window on top of the restored session
        local _, _, window = w.mux.spawn_window(cmd)
        pcall(function()
          w.mux.set_active_workspace(window:get_workspace())
        end)
      end
      M.open_gate()
    end)
    return
  end
  local _, pane, window = w.mux.spawn_window(cmd or {})
  if act ~= "ask" then
    return M.open_gate()
  end
  local gui = window and window:gui_window()
  if not gui then
    return M.open_gate()
  end
  local names = {}
  for _, ws in ipairs(wss) do
    table.insert(names, ws.name)
  end
  gui:perform_action(w.action.InputSelector({
    title = t("Restore previous session ({names})?", { names = table.concat(names, ", ") }),
    choices = { { id = "yes", label = t("Restore (without running commands)") }, { id = "no", label = t("No") } },
    action = w.action_callback(function(_, _, id)
      if id == "yes" then
        return M.restore_session(wss, M.open_gate)
      end
      M.open_gate() -- no or cancel (id == nil)
    end),
  }), pane)
end

function M.toggle_commands(name)
  local allowed = not M.commands_allowed(name)
  -- also re-enables the Claude Code resume question for the executables of this workspace (D7);
  -- read from the saved file before the save below replaces it
  M.forget_denials(name)
  M.save(name, allowed)
  util.log_info("saysdf.workspaces: commands for " .. name .. ": " .. tostring(allowed))
  return allowed
end

function M.setup(opts, config)
  opts = opts or {}
  M._state.opts = opts
  local w = wezterm()
  -- Gate (D2/D3): closed until gui-startup has decided and restored, unless windows already
  -- exist (config reload) or restoring is off.
  local okw, wins = pcall(function()
    return w.mux.all_windows()
  end)
  local live = okw and type(wins) == "table" and #wins > 0
  local restore = opts.restore or "always"
  if live or restore == "never" or restore == "none" then
    M.open_gate()
  else
    M._state.gate = "startup"
  end
  if opts.autosave ~= false then
    -- an update-status tick consumer: the tick runs deferred, after the status and bars are set (D5)
    local status = require("saysdf.status")
    status.hook("workspaces.autosave", function()
      status.defer(function()
        local ok, err = pcall(M.autosave_tick)
        if not ok then
          util.log_error("saysdf.workspaces: autosave: " .. tostring(err))
        end
      end)
    end)
  end
  if restore ~= "never" and restore ~= "none" then
    w.on("gui-startup", M.on_startup)
  end
  -- attach to a mux server: no restore (the server owns the panes); autosave continues (D3)
  w.on("gui-attached", function()
    if M._state.gate == "startup" then
      M.open_gate()
    end
  end)
  if opts.key ~= false and config then
    local k = opts.key or { key = "A", mods = "CTRL|SHIFT" }
    config.keys = config.keys or {}
    table.insert(config.keys, { key = k.key, mods = k.mods, action = w.action_callback(M.choose) })
  end
  if opts.palette ~= false then
    local palette = require("saysdf.palette")
    local kt = opts.key ~= false and palette.key_text(opts.key or { key = "A", mods = "CTRL|SHIFT" }) or nil
    palette.add({ id = "workspaces.choose", menu = "File", order = 10, label = t("Workspaces…"), accel = "w", key = kt,
      hint = { contexts = { "shell", "git" }, label = t("Projects"), priority = 70 },
      brief = t("Workspaces: list"), icon = "md_view_dashboard", action = w.action_callback(M.choose) })
    palette.add({
      id = "workspaces.save", menu = "File", order = 11, accel = "s",
      brief = t("Workspaces: save current"),
      icon = "md_content_save",
      action = w.action_callback(function(win, pane)
        M.save_manual(win, pane)
      end),
    })
    palette.add({
      id = "workspaces.commands", menu = "File", order = 13, accel = "c",
      brief = t("Workspaces: allow/forbid commands of the current one"),
      icon = "md_play_circle_outline",
      action = w.action_callback(function(win)
        M.toggle_commands(win:active_workspace())
      end),
    })
    palette.add({
      id = "workspaces.open_project", menu = "File", order = 12, accel = "o",
      brief = t("Workspaces: open the project of the current folder (.saysdf/workspace.json)"),
      icon = "md_folder_open",
      action = w.action_callback(function(win, pane)
        local cwd = cwd_of(pane)
        if cwd then
          M.open_project(win, pane, cwd)
        end
      end),
    })
    palette.add({
      id = "workspaces.new", menu = "File", order = 10.5, accel = "k",
      brief = t("Workspaces: new…"),
      icon = "md_plus_box",
      action = w.action_callback(M.new_workspace_prompt),
    })
    palette.add({
      id = "workspaces.save_as", menu = "File", order = 11.1, accel = "v",
      brief = t("Workspaces: save as…"),
      icon = "md_content_save_edit",
      action = w.action_callback(M.save_as_prompt),
    })
    palette.add({
      id = "workspaces.rename", menu = "File", order = 11.2, accel = "m",
      brief = t("Workspaces: rename…"),
      icon = "md_rename_box",
      action = w.action_callback(M.rename_prompt),
    })
  end
  return config
end

return M
