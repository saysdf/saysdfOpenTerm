-- saysdfTerminal: Claude Code session helpers (change workspace-restore, design D6).
-- Pure functions only: no I/O, no process calls, nothing runs at require time.
--
-- API:
--   projects_dir(env?) -> path   -- $CLAUDE_CONFIG_DIR/projects if set (non-empty), else
--                                   <home>/.claude/projects; separator by target OS.
--                                   env = { triple, home, getenv } for tests (like saysdf.util)
--   mangle(cwd) -> name|nil      -- project folder name: every code point outside [A-Za-z0-9]
--                                   becomes one "-"; trailing separators are dropped (a drive or
--                                   filesystem root is kept). Invalid UTF-8 or empty -> nil
--   is_uuid(s) -> bool           -- strict lowercase 8-4-4-4-12 hex, exactly 36 characters
--   argv_session(argv) -> uuid|nil -- the id after --resume / -r / --session-id, or in
--                                   --resume=<id> / --session-id=<id>; first valid one wins
--   file_session(path) -> uuid|nil -- the id of a "<uuid>.jsonl" path (basename stem)
--   start_unix(raw, triple, boot_time?) -> seconds|nil -- LocalProcessInfo.start_time is in OS
--                                   units: Windows FILETIME (100 ns since 1601), Linux clock ticks
--                                   since boot (USER_HZ = 100; boot_time = btime of /proc/stat),
--                                   macOS seconds since the epoch. Unknown OS / no boot time -> nil
--   match_sessions(panes, files, reasons?) -> { [pane_id] = uuid }; reasons (optional table)
--                                   is filled with { [pane_id] = reason } for panes without an id
--       panes = { { pane_id, start_time, argv_id?, bound? } } from one cwd,
--               bound = { session = uuid, start_time = t } (the last stored binding);
--       files = entries of wezterm.saysdf_dir_entries(dir, { suffix = ".jsonl" }):
--               { path, modified, created? }; names that are not UUIDs are ignored.
--       Order: sticky binding -> argv id -> claim by start time -> no id.
--       A wrong guess is worse than no resume: panes whose claude processes started within
--       AMBIGUOUS_S of each other get no id from the claim step, and a continued session (file
--       created before the start, appended after it) binds only if exactly one such file exists.
--       reason (for panes without an id): "no start time", "no session files",
--       "no file modified after start", "ambiguous".

local M = {}

-- Window for "this file was created by this process" and for ambiguity between two processes.
M.SLACK_S = 2
M.AMBIGUOUS_S = 2

local function home_and_env(env)
  env = env or {}
  local getenv = env.getenv or os.getenv
  local triple, home = env.triple, env.home
  if triple == nil or home == nil then
    local ok, w = pcall(require, "wezterm")
    w = ok and w or {}
    triple = triple or w.target_triple or ""
    home = home or w.home_dir or ""
  end
  return triple, home, getenv
end

function M.projects_dir(env)
  local triple, home, getenv = home_and_env(env)
  local sep = triple:find("windows") and "\\" or "/"
  local base = getenv("CLAUDE_CONFIG_DIR")
  if base == nil or base == "" then
    base = home:gsub("[/\\]+$", "") .. sep .. ".claude"
  else
    base = base:gsub("[/\\]+$", "")
  end
  return base .. sep .. "projects"
end

-- Drops trailing "/" and "\" unless the path is a root ("/", "C:\", "C:/").
local function strip_trailing(p)
  while #p > 1 and p:find("[/\\]$") and not p:match("^%a:[/\\]$") do
    p = p:sub(1, -2)
  end
  return p
end

function M.mangle(cwd)
  if type(cwd) ~= "string" or cwd == "" or utf8.len(cwd) == nil then
    return nil
  end
  cwd = strip_trailing(cwd)
  local out = cwd:gsub(utf8.charpattern, function(ch)
    if ch:match("^[A-Za-z0-9]$") then
      return ch
    end
    return "-"
  end)
  return out
end

-- Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01.
local FILETIME_EPOCH_S = 11644473600
local USER_HZ = 100

function M.start_unix(raw, triple, boot_time)
  raw = tonumber(raw)
  if raw == nil or type(triple) ~= "string" then
    return nil
  end
  if triple:find("windows") then
    return raw / 1e7 - FILETIME_EPOCH_S
  elseif triple:find("apple") or triple:find("darwin") then
    return raw
  elseif triple:find("linux") then
    boot_time = tonumber(boot_time)
    return boot_time and (boot_time + raw / USER_HZ) or nil
  end
  return nil
end

local UUID = "^%x%x%x%x%x%x%x%x%-%x%x%x%x%-%x%x%x%x%-%x%x%x%x%-%x%x%x%x%x%x%x%x%x%x%x%x$"

function M.is_uuid(s)
  return type(s) == "string" and #s == 36 and s:match(UUID) ~= nil and not s:find("[A-F]")
end

local FLAGS = { ["--resume"] = true, ["-r"] = true, ["--session-id"] = true }

function M.argv_session(argv)
  if type(argv) ~= "table" then
    return nil
  end
  for i, a in ipairs(argv) do
    if type(a) == "string" then
      local v
      if FLAGS[a] then
        v = argv[i + 1]
      else
        v = a:match("^%-%-resume=(.*)$") or a:match("^%-%-session%-id=(.*)$")
      end
      if M.is_uuid(v) then
        return v
      end
    end
  end
  return nil
end

function M.file_session(path)
  if type(path) ~= "string" then
    return nil
  end
  local stem = path:match("([^/\\]+)%.jsonl$")
  return M.is_uuid(stem) and stem or nil
end

function M.match_sessions(panes, files, reasons)
  local result = {}
  local by_id = {} -- uuid -> { id, modified, created }
  local list = {}
  for _, f in ipairs(files or {}) do
    local id = M.file_session(f.path)
    if id and type(f.modified) == "number" and not by_id[id] then
      local e = { id = id, modified = f.modified, created = f.created }
      by_id[id] = e
      list[#list + 1] = e
    end
  end
  local claimed = {}
  local rest = {}
  for _, p in ipairs(panes or {}) do
    local b = p.bound
    if type(b) == "table" and by_id[b.session] and b.start_time ~= nil and b.start_time == p.start_time then
      -- 1. sticky binding: same claude process, file still there
      result[p.pane_id] = b.session
      claimed[b.session] = true
    elseif M.is_uuid(p.argv_id) and by_id[p.argv_id] then
      -- 2. explicit id on the command line
      result[p.pane_id] = p.argv_id
      claimed[p.argv_id] = true
    elseif type(p.start_time) == "number" then
      rest[#rest + 1] = p
    end
  end

  table.sort(rest, function(a, b)
    if a.start_time ~= b.start_time then
      return a.start_time < b.start_time
    end
    return tostring(a.pane_id) < tostring(b.pane_id)
  end)
  -- Two unresolved processes started too close together: their files cannot be told apart.
  local ambiguous = {}
  for i = 2, #rest do
    if rest[i].start_time - rest[i - 1].start_time <= M.AMBIGUOUS_S then
      ambiguous[i] = true
      ambiguous[i - 1] = true
    end
  end

  -- 3. claim by start time; ambiguous panes still claim (so others cannot take their files),
  -- but their result is dropped.
  reasons = reasons or {}
  for _, p in ipairs(panes or {}) do
    if result[p.pane_id] == nil and type(p.start_time) ~= "number" then
      reasons[p.pane_id] = "no start time"
    end
  end
  for i, p in ipairs(rest) do
    local best, why
    for _, e in ipairs(list) do
      local t = e.created or e.modified
      if not claimed[e.id] and t >= p.start_time - M.SLACK_S and (best == nil or t < (best.created or best.modified)) then
        best = e
      end
    end
    if best == nil then
      -- a continued session that keeps writing into an older file: only if it is the only one
      local n = 0
      for _, e in ipairs(list) do
        if not claimed[e.id] and e.modified >= p.start_time then
          n = n + 1
          best = e
        end
      end
      if n > 1 then
        best, why = nil, "ambiguous"
      end
    end
    if best then
      claimed[best.id] = true
      if not ambiguous[i] then
        result[p.pane_id] = best.id
      else
        why = "ambiguous"
      end
    end
    if result[p.pane_id] == nil then
      reasons[p.pane_id] = why or (#list == 0 and "no session files" or "no file modified after start")
    end
  end
  return result
end

return M
