-- saysdfTerminal: git in the status bar and the git operations menu (change git-integration).
--
-- Options of saysdf.setup({ git = { ... } }):
--   bin = nil            -- path to saysdf (default: next to saysdfTerm or in PATH)
--   ttl_ms = 3000        -- status cache TTL (at least 1000); with a slow git x3, up to 30 s
--   untracked = true     -- false: do not count untracked files (`-uno`, big repos)
--   icons = false        -- true: Nerd Font branch glyph
--   keys = true, key = { key = "S", mods = "CTRL|SHIFT" }  -- git menu
--   palette = true       -- "Git: ..." entries in the palette (saysdf.palette)
--   split = { direction = "Bottom", size = 0.35 }          -- split for git operations
--
-- API:
--   info(pane) -> { branch, ahead, behind, dirty } | nil  -- cache only, no processes (for saysdf.tabs)
--   find_root(dir, exists?) -> root | nil                  -- walk up to .git (a directory or a gitdir: file)
--   should_refresh(entry, now_ms, ttl_ms) -> bool          -- cache policy (pure function)
--   next_ttl(base_ms, duration_ms) -> ms                   -- adaptive TTL
--   format_status(data, opts) -> string
--   push_plan(data, remote, unpushed) -> args, text        -- push arguments and the confirmation text
--   menu(window, pane), status_text(window)
--   saysdf_argv(bin, ...) -> argv                          -- `saysdf --lang <language> ...` for run_child_process
--
-- The block is a segment of the shared `saysdf.status` (order 10); polling is `saysdf git-status --json <root>`
-- (run_child_process) at the end of the segment; the block is drawn from the cache before the request. Local
-- domain only. Cache reset: user var SAYSDF_LAST_EXIT (end of a shell command), a pane cwd change, the
-- module's git operations.

local M = {}

local t, _, N_ = require("saysdf.i18n").domain("git")

local MIN_TTL = 1000
local MAX_TTL = 30000

local state = { opts = {}, cache = {}, roots = {}, pane_cwd = {}, window_text = {} }

local function wezterm()
  return require("wezterm")
end

-- Selection goes to the sidebar (`saysdf.sidebar.selector`); the fallback is InputSelector.
local function sidebar()
  return require("saysdf.sidebar")
end

local function now_ms()
  local ok, v = pcall(function()
    return tonumber(wezterm().time.now():format("%s%.3f")) * 1000
  end)
  if ok and v then
    return math.floor(v)
  end
  return os.time() * 1000
end
M._now = now_ms

-- ---------- pure functions ----------

local function default_exists(path)
  local f = io.open(path, "rb")
  if f then
    f:close()
    return true
  end
  -- directory: on Windows io.open cannot open a directory, so try a file inside
  local h = io.open(path .. "/HEAD", "rb")
  if h then
    h:close()
    return true
  end
  return false
end

local function parent(dir)
  local p = dir:gsub("[/\\]+$", "")
  local up = p:match("^(.*)[/\\][^/\\]+$")
  if up == nil then
    return nil
  end
  if up == "" then
    return "/"
  end
  if up:match("^%a:$") then
    return up .. "\\"
  end
  return up
end

-- known(dir) -> root | false | nil -- an already known answer for a directory (root_of cache): the walk
-- stops at it without touching the filesystem above. visited(dir) -- every checked directory (for the cache).
function M.find_root(dir, exists, known, visited)
  exists = exists or default_exists
  local cur = dir
  for _ = 1, 64 do
    if cur == nil or cur == "" then
      return nil
    end
    local base = cur:gsub("[/\\]+$", "")
    if known and cur ~= dir then
      local k = known(base)
      if k ~= nil then
        return k or nil
      end
    end
    if visited then
      visited(base)
    end
    if exists(base .. "/.git") then
      return base == "" and "/" or base
    end
    cur = parent(cur)
  end
  return nil
end

function M.should_refresh(entry, now, ttl)
  if entry == nil then
    return true
  end
  if entry.inflight then
    return false
  end
  if entry.stale or entry.fetched_at == nil then
    return true
  end
  return now - entry.fetched_at >= (entry.ttl or ttl or 3000)
end

function M.next_ttl(base, duration)
  base = math.max(MIN_TTL, base or 3000)
  if duration and duration > 1000 then
    return math.min(MAX_TTL, base * 3)
  end
  return base
end

function M.format_status(d, opts)
  if type(d) ~= "table" or d.root == nil then
    return nil
  end
  opts = opts or {}
  local name = d.branch
  if d.detached or not name then
    name = d.oid and ("@" .. d.oid:sub(1, 7)) or "(detached)"
  end
  local parts = { (opts.icons and "\u{e725} " or "") .. name }
  if d.label then
    table.insert(parts, d.label)
  end
  local function add(n, prefix)
    if type(n) == "number" and n > 0 then
      table.insert(parts, prefix .. n)
    end
  end
  add(d.staged, "+")
  add(d.modified, "~")
  add(d.untracked, "?")
  add(d.conflicts, "!")
  add(d.ahead, "↑")
  add(d.behind, "↓")
  add(d.submodules_dirty, "sub:")
  return table.concat(parts, " ")
end

-- Push arguments and the confirmation text. data -- status, remote -- the remote for a branch without
-- upstream, unpushed -- list of { path, sha } submodules. args = nil when there is nothing to push (detached).
function M.push_plan(d, remote, unpushed)
  if type(d) ~= "table" or d.detached or not d.branch then
    return nil, t("HEAD is detached: push cancelled")
  end
  local args, target
  if d.upstream then
    args = { "push" }
    target = d.upstream
  else
    remote = remote or "origin"
    args = { "push", "-u", remote, d.branch }
    target = t("{remote}/{branch} (new upstream)", { remote = remote, branch = d.branch })
  end
  local lines = { t("Push {branch} → {target}?", { branch = d.branch, target = target }) }
  if type(d.ahead) == "number" then
    table.insert(lines, t("Commits to push: {n}", { n = tostring(d.ahead) }))
  end
  if type(d.behind) == "number" and d.behind > 0 then
    table.insert(lines, t("Warning: branch is behind by {n} (push may be rejected)", { n = tostring(d.behind) }))
  end
  for _, u in ipairs(unpushed or {}) do
    table.insert(
      lines,
      t("Warning: submodule {path} — commit {sha} is not pushed", { path = u.path, sha = tostring(u.sha):sub(1, 7) })
    )
  end
  return args, table.concat(lines, "\n")
end

-- ---------- cache ----------

local function cwd_of(pane)
  local ok, cwd = pcall(function()
    return pane:get_current_working_dir()
  end)
  if not ok or cwd == nil then
    return nil
  end
  local okt, tabs = pcall(require, "saysdf.tabs")
  local path
  if okt and type(tabs.cwd_path) == "function" then
    path = tabs.cwd_path(cwd)
  elseif type(cwd) == "string" then
    path = cwd
  end
  if type(path) ~= "string" then
    return nil
  end
  -- file:///C:/x -> /C:/x -> C:/x
  return (path:gsub("^/(%a:)", "%1"))
end

local function is_local(pane)
  local ok, d = pcall(function()
    return pane:get_domain_name()
  end)
  return not ok or d == "local"
end

-- Repository root for the pane cwd. The per-directory cache has no TTL: the answer changes only from
-- shell commands (git init, rm -r .git, clone), so an entry is marked stale by SAYSDF_LAST_EXIT
-- (invalidate) and rechecked with one filesystem call (is the root still there?) or a walk up if
-- there was no repository. The walk stops at directories with a known answer (perf-2).
-- refresh_root = false -- cache only, no filesystem. exists -- for tests.
function M.root_of(pane, refresh_root, exists)
  if pane == nil or not is_local(pane) then
    return nil
  end
  local cwd = cwd_of(pane)
  if not cwd then
    return nil
  end
  local r = state.roots[cwd]
  if r and (not refresh_root or not r.stale) then
    return r.root or nil
  end
  if not refresh_root then
    return nil
  end
  exists = exists or default_exists
  if r and r.root and exists(r.root .. "/.git") then
    r.stale = false
    return r.root
  end
  if r and r.root then
    -- repository removed: forget every directory that pointed at this root
    for d, k in pairs(state.roots) do
      if k.root == r.root then
        state.roots[d] = nil
      end
    end
  end
  -- recheck after a command: a full walk up (git init may have happened above cwd)
  local use_known = r == nil
  local visited = {}
  local root = M.find_root(cwd, exists, use_known and function(dir)
    local k = state.roots[dir]
    if k and not k.stale then
      return k.root
    end
    return nil
  end, function(dir)
    table.insert(visited, dir)
  end)
  -- Directories between cwd and the root (or all of them if there is no repository) get the same answer.
  for _, d in ipairs(visited) do
    state.roots[d] = { root = root or false }
  end
  state.roots[cwd] = { root = root or false }
  state.root_walks = (state.root_walks or 0) + 1
  return root
end

function M.info(pane)
  local root = M.root_of(pane, false)
  local e = root and state.cache[root]
  local d = e and e.data
  if type(d) ~= "table" or d.root == nil then
    return nil
  end
  local branch = d.branch
  if d.detached or not branch then
    branch = d.oid and ("@" .. d.oid:sub(1, 7)) or nil
  end
  return { branch = branch, ahead = d.ahead, behind = d.behind, dirty = d.dirty }
end

function M.find_bin()
  if state.bin then
    return state.bin
  end
  local b = state.opts.bin
  if type(b) ~= "string" or b == "" then
    b = require("saysdf.util").find_bin("saysdf")
  end
  state.bin = b
  return b
end

-- argv of a `saysdf` call with the language of this config: run_child_process does not pass
-- set_environment_variables (SAYSDF_LANG), so the language goes as the global `--lang` option.
function M.saysdf_argv(bin, ...)
  local argv = { bin, "--lang", require("saysdf.i18n").language() }
  for i = 1, select("#", ...) do
    argv[3 + i] = select(i, ...)
  end
  return argv
end

local function rebase_label(root)
  local function ex(p)
    local f = io.open(root .. "/.git/" .. p, "rb")
    if f then
      f:close()
      return true
    end
    return false
  end
  if ex("rebase-merge/head-name") or ex("rebase-apply/head-name") then
    return "REBASE"
  elseif ex("MERGE_HEAD") then
    return "MERGE"
  end
  return nil
end

-- Asynchronous poll (call from the coroutine of an event handler).
function M.refresh(root)
  local e = state.cache[root] or {}
  state.cache[root] = e
  local bin = M.find_bin()
  if not bin then
    if not state.warned then
      state.warned = true
      require("saysdf.util").log_warn("saysdf.git: saysdf not found, the git block is disabled")
    end
    return
  end
  e.inflight = true
  e.stale = false
  local args = M.saysdf_argv(bin, "git-status", "--json")
  if state.opts.untracked == false then
    table.insert(args, "--no-untracked")
  end
  table.insert(args, root)
  local t0 = now_ms()
  local ok, success, out = pcall(wezterm().run_child_process, args)
  local dur = now_ms() - t0
  e.inflight = false
  e.fetched_at = now_ms()
  e.ttl = M.next_ttl(state.opts.ttl_ms, dur)
  if ok and success then
    local okj, data = pcall(wezterm().json_parse, out)
    if okj and type(data) == "table" then
      if data.root then
        data.label = rebase_label(root)
      end
      e.data = data
      return
    end
  end
  e.data = nil
end

function M.invalidate(pane)
  local root = pane and M.root_of(pane, false)
  if root and state.cache[root] then
    state.cache[root].stale = true
  end
  -- the command may have created/removed a repository: recheck the root on the next tick
  local cwd = pane and cwd_of(pane)
  local r = cwd and state.roots[cwd]
  if r then
    r.stale = true
  end
end

local function status_segment(window, pane)
  local root = M.root_of(pane, true)
  -- pane cwd changed -> reset
  local id = pane:pane_id()
  local cwd = cwd_of(pane)
  if state.pane_cwd[id] ~= cwd then
    state.pane_cwd[id] = cwd
    if root and state.cache[root] then
      state.cache[root].stale = true
    end
  end
  local e = root and state.cache[root]
  local text = e and M.format_status(e.data, state.opts) or ""
  state.window_text[window:window_id()] = text
  if root and M.should_refresh(e, now_ms(), state.opts.ttl_ms) then
    -- polling happens after the status and bars are set (status.defer): rendering does not wait for the git process
    e = e or {}
    state.cache[root] = e
    e.inflight = true
    local status = require("saysdf.status")
    local run = function()
      e.inflight = false
      M.refresh(root)
    end
    if status.defer then
      status.defer(run)
    else
      run()
    end
  end
  return text
end

function M.status_text(window)
  return state.window_text[window:window_id()] or ""
end

-- ---------- operations ----------

local function toast(window, msg)
  require("saysdf.util").log_info("saysdf.git: " .. msg)
  require("saysdf.util").notify(window, "git", msg)
end

local function git(root, args)
  local cmd = { "git", "-C", root }
  for _, a in ipairs(args) do
    table.insert(cmd, a)
  end
  local ok, success, out, err = pcall(wezterm().run_child_process, cmd)
  if not ok then
    return false, "", tostring(success)
  end
  return success, out or "", err or ""
end

function M.run_in_split(window, pane, root, args)
  local bin = M.find_bin()
  if not bin then
    require("saysdf.util").notify_missing_bin(window, "saysdf", "git")
    return
  end
  local cmd = { bin, "git-run", "--cwd", root, "--" }
  for _, a in ipairs(args) do
    table.insert(cmd, a)
  end
  local sp = state.opts.split or {}
  pane:split({ args = cmd, cwd = root, direction = sp.direction or "Bottom", size = sp.size or 0.35 })
  if state.cache[root] then
    state.cache[root].stale = true
  end
end

local function confirm(window, pane, text, yes, on_yes)
  local w = wezterm()
  window:perform_action(
    w.action.InputSelector({
      title = text,
      choices = { { id = "yes", label = yes }, { id = "no", label = t("Cancel") } },
      action = w.action_callback(function(win, p, id)
        if id == "yes" then
          on_yes(win, p)
        end
      end),
    }),
    pane
  )
end

local function prompt_message(window, pane, title, on_text)
  local w = wezterm()
  window:perform_action(
    w.action.PromptInputLine({
      description = title,
      action = w.action_callback(function(win, p, text)
        if text and text:match("%S") then
          on_text(win, p, text)
        end
      end),
    }),
    pane
  )
end

local function fresh_status(root)
  local bin = M.find_bin()
  if not bin then
    return nil
  end
  local ok, success, out = pcall(wezterm().run_child_process, M.saysdf_argv(bin, "git-status", "--json", root))
  if ok and success then
    local okj, d = pcall(wezterm().json_parse, out)
    if okj then
      return d
    end
  end
  return nil
end

local actions = {}

function actions.commit(window, pane, root)
  local staged = git(root, { "diff", "--cached", "--quiet" })
  if staged then
    toast(window, t("No staged changes (git add or Commit all)"))
    return
  end
  prompt_message(window, pane, t("Commit message"), function(win, p, msg)
    M.run_in_split(win, p, root, { "commit", "-m", msg })
  end)
end

function actions.commit_all(window, pane, root)
  confirm(window, pane, t("Commit all (-a): commit ALL changes to tracked files?"), t("Yes, commit -a"), function(win, p)
    prompt_message(win, p, t("Commit message (commit -a)"), function(w2, p2, msg)
      M.run_in_split(w2, p2, root, { "commit", "-a", "-m", msg })
    end)
  end)
end

function actions.push(window, pane, root)
  local d = fresh_status(root)
  if not d or not d.root then
    toast(window, t("Could not get the repository status"))
    return
  end
  local remote
  if not d.upstream and d.branch then
    local ok, out = git(root, { "config", "--get", "branch." .. d.branch .. ".pushRemote" })
    if not ok then
      ok, out = git(root, { "config", "--get", "remote.pushDefault" })
    end
    remote = ok and out:match("%S+") or "origin"
  end
  local unpushed = {}
  local bin = M.find_bin()
  local okp, success, out = pcall(
    wezterm().run_child_process,
    M.saysdf_argv(bin, "git-status", "--submodules-push-check", root)
  )
  if okp and success then
    local okj, j = pcall(wezterm().json_parse, out)
    if okj and type(j) == "table" and type(j.unpushed) == "table" then
      unpushed = j.unpushed
    end
  end
  local args, text = M.push_plan(d, remote, unpushed)
  if not args then
    toast(window, text)
    return
  end
  confirm(window, pane, text, t("Push"), function(win, p)
    M.run_in_split(win, p, root, args)
  end)
end

function actions.pull(window, pane, root)
  M.run_in_split(window, pane, root, { "pull" })
end

function actions.checkout(window, pane, root)
  local ok, out = git(root, { "for-each-ref", "--format=%(refname:short)", "refs/heads", "refs/remotes" })
  if not ok then
    toast(window, t("Could not get the branch list"))
    return
  end
  local locals, choices = {}, {}
  for ref in out:gmatch("[^\r\n]+") do
    if not ref:match("/HEAD$") and ref ~= "origin" then
      table.insert(choices, { id = ref, label = ref })
      if not ref:find("/") then
        locals[ref] = true
      end
    end
  end
  window:perform_action(
    sidebar().selector({
      tool = "git.checkout",
      title = t("Checkout"),
      fuzzy = true,
      choices = choices,
      on_choice = function(win, p, id)
        if not id then
          return
        end
        local args
        local remote, name = id:match("^([^/]+)/(.+)$")
        if remote and not locals[id] and not locals[name] then
          args = { "switch", "--track", id }
        else
          args = { "switch", locals[id] and id or name }
        end
        local d = state.cache[root] and state.cache[root].data
        if type(d) == "table" and d.dirty then
          confirm(win, p, t("Working tree is not clean. Switch to {branch}?", { branch = id }), t("Switch"), function(w2, p2)
            M.run_in_split(w2, p2, root, args)
          end)
        else
          M.run_in_split(win, p, root, args)
        end
      end,
    }),
    pane
  )
end

function actions.submodules(window, pane, root)
  local ok, out = git(root, { "submodule", "status", "--recursive" })
  local names = { [" "] = N_("ok"), ["+"] = N_("moved"), ["-"] = N_("not initialized"), ["U"] = N_("conflict") }
  local choices = { { id = "\0update", label = t("Update submodules: submodule update --init --recursive") } }
  if ok then
    for line in out:gmatch("[^\r\n]+") do
      local st, rest = line:match("^(.)%x+ (.+)$")
      local path = rest and (rest:gsub(" %([^()]*%)$", ""))
      if st and path and path ~= "" then
        table.insert(choices, { id = path, label = path .. "  [" .. (names[st] and t(names[st]) or st) .. "]" })
      end
    end
  end
  window:perform_action(
    sidebar().selector({
      tool = "git.submodules",
      title = t("Submodules (select to open in a new pane)"),
      choices = choices,
      on_choice = function(win, p, id)
        if id == "\0update" then
          M.run_in_split(win, p, root, { "submodule", "update", "--init", "--recursive" })
        elseif id then
          p:split({ cwd = root .. "/" .. id, direction = "Right" })
        end
      end,
    }),
    pane
  )
end

local function split_bin(window, pane, root, args, direction)
  local bin = M.find_bin()
  if not bin then
    require("saysdf.util").notify_missing_bin(window, "saysdf", "git")
    return
  end
  local cmd = { bin }
  for _, a in ipairs(args) do
    table.insert(cmd, a)
  end
  local sp = state.opts.split or {}
  pane:split({ args = cmd, cwd = root, direction = direction or "Right", size = sp.diff_size or 0.6 })
  if state.cache[root] then
    state.cache[root].stale = true
  end
end

-- diff-merge-tool 4.1: `saysdf diff` (working tree <-> index) in a split
function actions.diff(window, pane, root)
  split_bin(window, pane, root, { "diff", "--cwd", root })
end

function actions.diff_staged(window, pane, root)
  split_bin(window, pane, root, { "diff", "--cwd", root, "--staged" })
end

-- conflicts: git mergetool with the saysdf tool (set up by `saysdf merge --install-git-tool`)
function actions.conflicts(window, pane, root)
  M.run_in_split(window, pane, root, { "mergetool", "--tool=saysdf" })
end

function M.ui_missing()
  return t("lazygit is not installed; install: winget install JesseDuffield.lazygit (or scoop install lazygit)")
end

-- lazygit, else gitui; nil if neither exists (the lookup is cached: util.find_bin_cached)
function M.ui_bin()
  local util = require("saysdf.util")
  return util.find_bin_cached("lazygit") or util.find_bin_cached("gitui")
end

function actions.ui(window, pane, root)
  local bin = M.ui_bin()
  if not bin then
    toast(window, M.ui_missing())
    return
  end
  pane:split({ args = { bin }, cwd = root, direction = "Right", size = 0.6 })
end

M.ACCELS = {
  diff = "d", diff_staged = "s", conflicts = "r", commit = "c", commit_all = "a",
  push = "p", pull = "l", checkout = "b", submodules = "m", ui = "u",
}

-- Labels are catalog keys (N_); t(item.label) is applied where they are shown.
M.ITEMS = {
  { id = "diff", label = N_("Diff") },
  { id = "diff_staged", label = N_("Diff (staged)") },
  { id = "conflicts", label = N_("Resolve conflicts (mergetool)") },
  { id = "commit", label = N_("Commit (staged)") },
  { id = "commit_all", label = N_("Commit all (-a)…") },
  { id = "push", label = N_("Push… (with confirmation)") },
  { id = "pull", label = N_("Pull") },
  { id = "checkout", label = N_("Checkout branch") },
  { id = "submodules", label = N_("Submodules") },
  { id = "ui", label = N_("Open lazygit/gitui") },
}

function M.act(window, pane, id)
  local root = M.root_of(pane, true)
  if not root then
    toast(window, t("Pane is not in a git repository (or the domain is not local)"))
    return
  end
  local f = actions[id]
  if f then
    f(window, pane, root)
  end
end

function M.menu(window, pane)
  local choices = {}
  for _, item in ipairs(M.ITEMS) do
    table.insert(choices, { id = item.id, label = t(item.label) })
  end
  window:perform_action(
    sidebar().selector({
      tool = "git",
      title = t("Git"),
      choices = choices,
      on_choice = function(win, p, id)
        if id then
          M.act(win, p, id)
        end
      end,
    }),
    pane
  )
end

function M.setup(opts, config)
  opts = opts or {}
  state = { opts = opts, cache = {}, roots = {}, pane_cwd = {}, window_text = {} }
  local w = wezterm()
  require("saysdf.status").add("git", status_segment, { order = 10 })
  w.on("user-var-changed", function(_, pane, name)
    if name == "SAYSDF_LAST_EXIT" then
      M.invalidate(pane)
    end
  end)
  if opts.keys ~= false then
    local b = type(opts.key) == "table" and opts.key or { key = "S", mods = "CTRL|SHIFT" }
    config.keys = config.keys or {}
    table.insert(config.keys, {
      key = b.key,
      mods = b.mods,
      action = w.action_callback(function(window, pane)
        M.menu(window, pane)
      end),
    })
  end
  if opts.palette ~= false then
    local palette = require("saysdf.palette")
    local keytext = opts.keys ~= false
        and palette.key_text(type(opts.key) == "table" and opts.key or { key = "S", mods = "CTRL|SHIFT" })
      or nil
    -- saysdfTerminal: registry entry "Git menu" (menu-bar D4, hotkey-statusbar D3).
    palette.add({
      id = "git.menu",
      brief = t("Git: menu…"),
      label = t("Git menu…"),
      accel = "g",
      menu = "Git",
      order = 1,
      key = keytext,
      hint = { contexts = { "git" }, label = t("Git"), priority = 10 },
      icon = "dev_git",
      action = w.action_callback(function(window, pane)
        M.menu(window, pane)
      end),
    })
    for i, item in ipairs(M.ITEMS) do
      palette.add({
        id = "git." .. item.id,
        brief = t("Git: {action}", { action = t(item.label) }),
        label = t(item.label),
        accel = M.ACCELS[item.id],
        menu = "Git",
        order = 10 + i,
        hint = (item.id == "commit") and { contexts = { "git" }, label = t("Commit"), priority = 20 }
          or (item.id == "diff") and { contexts = { "git" }, label = t("Diff"), priority = 30 }
          or nil,
        icon = "dev_git",
        action = w.action_callback(function(window, pane)
          M.act(window, pane, item.id)
        end),
      })
    end
  end
  return config
end

M._state = function()
  return state
end

return M
