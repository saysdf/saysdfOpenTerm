-- saysdfTerminal: clickable file:line:col paths (change clickable-file-paths).
--
-- Usage: saysdf.setup({ paths = { ... } }) or require('saysdf.paths').setup(opts, config).
-- Options (all optional):
--   editor  = { 'code', '-g', '{path}:{line}:{col}' }  -- external editor for Ctrl+Shift+click;
--             arguments as a list, no shell; {path}/{line}/{col} are substituted per element
--   split   = { direction = 'Right', size = 0.5 }      -- for opening a folder in a split
--   notify  = true                                    -- error toasts (otherwise log only)
--   mouse   = true                                    -- Ctrl/Ctrl+Shift+click (D7)
-- API for other modules: M.hyperlink_rules(), M.quick_select_patterns() (quick-select),
-- M.parse(text), M.resolve_with(cwd, proc_cwd, home, path), M.classify(path), M.open(...).
--
-- Security (D6): highlighting uses its own scheme saysdf-path:, so the OS association (ShellExecute,
-- xdg-open) never opens the path; files of any type, including executables, are opened only in
-- the viewer (saysdf.viewer) or the user's external editor — they are never run.

local M = {}

local t = require("saysdf.i18n").domain("paths")

local function wezterm()
  return require("wezterm")
end

M.SCHEME = "saysdf-path:"

-- Line/column suffix: :l, :l:c, (l), (l,c). Non-capturing groups only.
local SUFFIX = [[(?::\d+(?::\d+)?|\(\d+(?:,\s*\d+)?\))]]
-- Character before the path (instead of lookbehind, which Rust regex lacks). The path itself is group 1.
local PRE = [[(?:^|[\s"'(\[<{=,>])]]

-- D3. Patterns: path body (without prefix and suffix) and whether the suffix is required.
local BODIES = {
  -- Windows: C:\a\b.rs, C:/a/b
  { body = [[[A-Za-z]:[\\/](?:[^\s:*?"<>|()\[\]',;]*\w)?]], suffix_required = false },
  -- UNC: \\server\share\a.txt
  { body = [[\\\\[\w.$\-]+\\(?:[^\s:*?"<>|()\[\]',;]*\w)?]], suffix_required = false },
  -- With a separator: src/a.rs, /usr/x, ./a, ~/a; the last component must contain a letter
  -- (rejects 1.2/3.4 and dates like 2026/10/17)
  { body = [[(?:~|\.{1,2})?/?(?:[\w.@+\-]+/)+[\w.@+\-]*[^\W\d](?:[\w.@+\-]*\w)?]], suffix_required = false },
  -- A bare name with an extension only with a line: main.rs:10
  { body = [[[\w+\-][\w.+\-]*\.[A-Za-z0-9]{1,10}]], suffix_required = true },
}

local function regex_for(b)
  return PRE .. "(" .. b.body .. SUFFIX .. (b.suffix_required and "" or "?") .. ")"
end

-- Rules for config.hyperlink_rules: group 1 is highlighted, the link is saysdf-path:<text>.
function M.hyperlink_rules()
  local rules = {}
  for _, b in ipairs(BODIES) do
    table.insert(rules, { regex = regex_for(b), format = M.SCHEME .. "$1", highlight = 1 })
  end
  return rules
end

-- Patterns for config.quick_select_patterns (quick-select takes the first capturing group).
function M.quick_select_patterns()
  local pats = {}
  for _, b in ipairs(BODIES) do
    table.insert(pats, regex_for(b))
  end
  return pats
end

local function percent_decode(s)
  return (s:gsub("%%(%x%x)", function(h)
    return string.char(tonumber(h, 16))
  end))
end

-- `C:/x` from `/C:/x`
local function strip_drive_slash(p)
  if p:match("^/[A-Za-z]:[/\\]") or p:match("^/[A-Za-z]:$") then
    return p:sub(2)
  end
  return p
end

-- file://host/path -> path (host is ignored; UNC on Windows is not supported)
local function file_url_path(u)
  local rest = u:gsub("^file://", "")
  local path = rest:match("^[^/]*(/.*)$") or rest
  return strip_drive_slash(percent_decode(path))
end

-- M.parse(text) -> { path, line, col } (line/col are numbers or nil)
function M.parse(text)
  if type(text) ~= "string" or text == "" then
    return nil
  end
  local s = text
  if s:sub(1, #M.SCHEME) == M.SCHEME then
    s = s:sub(#M.SCHEME + 1)
  end
  if s:match("^file://") then
    s = file_url_path(s)
  end
  local line, col
  local l, c = s:match("%((%d+),?%s*(%d*)%)$")
  if l then
    s = s:gsub("%(%d+,?%s*%d*%)$", "")
  else
    l, c = s:match(":(%d+):(%d+)$")
    if l then
      s = s:gsub(":%d+:%d+$", "")
    else
      l = s:match(":(%d+)$")
      if l then
        s = s:gsub(":%d+$", "")
      end
    end
  end
  line = tonumber(l)
  col = tonumber(c ~= "" and c or nil)
  if s == "" then
    return nil
  end
  return { path = s, line = line, col = col }
end

function M.is_absolute(p)
  return p:match("^/") ~= nil or p:match("^[A-Za-z]:[/\\]") ~= nil or p:match("^\\\\") ~= nil
end

local function join(dir, p)
  local sep = dir:match("\\") and not dir:match("/") and "\\" or "/"
  if dir:sub(-1) == "/" or dir:sub(-1) == "\\" then
    return dir .. p
  end
  return dir .. sep .. p
end

-- cwd from OSC 7: a file://... string, or already a path
local function cwd_to_path(cwd)
  if type(cwd) ~= "string" or cwd == "" then
    return nil
  end
  if cwd:match("^file://") then
    return file_url_path(cwd)
  end
  return strip_drive_slash(cwd)
end

-- Pure resolution function (D4). Returns a path, or nil and a reason.
function M.resolve_with(cwd_url, proc_cwd, home, path)
  if not path or path == "" then
    return nil, "empty path"
  end
  path = strip_drive_slash(path)
  if path == "~" or path:match("^~[/\\]") then
    if not home then
      return nil, "unknown home"
    end
    return home .. path:sub(2)
  end
  if M.is_absolute(path) then
    return path
  end
  local base = cwd_to_path(cwd_url) or cwd_to_path(proc_cwd)
  if not base then
    return nil, "unknown cwd"
  end
  local rel = path:gsub("^%./", ""):gsub("^%.\\", "")
  return join(base, rel)
end

function M.resolve(pane, path)
  local cwd
  local ok, url = pcall(function()
    return pane:get_current_working_dir()
  end)
  if ok and url then
    if type(url) == "string" then
      cwd = url
    else
      local ok2, fp = pcall(function()
        return url.file_path
      end)
      cwd = ok2 and fp or tostring(url)
    end
  end
  local proc
  local okp, info = pcall(function()
    return pane:get_foreground_process_info()
  end)
  if okp and info then
    proc = info.cwd
  end
  return M.resolve_with(cwd, proc, wezterm().home_dir, path)
end

local KINDS = {
  md = "markdown", markdown = "markdown",
  mmd = "mermaid", mermaid = "mermaid",
  png = "image", jpg = "image", jpeg = "image", gif = "image", bmp = "image", webp = "image",
}

-- "markdown" | "mermaid" | "image" | "dir" | "text"; unknown and executable files are "text".
function M.classify(path, is_dir)
  if is_dir then
    return "dir"
  end
  local ext = path:match("%.([%w]+)$")
  return (ext and KINDS[ext:lower()]) or "text"
end

-- true, is_dir | false
function M.exists(path)
  local okd = pcall(function()
    return wezterm().read_dir(path)
  end)
  if okd then
    return true, true
  end
  local f = io.open(path, "r")
  if f then
    f:close()
    return true, false
  end
  return false
end

-- Substitute {path}/{line}/{col} per element (no shell).
function M.editor_args(editor, path, line, col)
  local args = {}
  for _, a in ipairs(editor) do
    local v = a:gsub("{path}", function() return path end)
      :gsub("{line}", tostring(line or 1))
      :gsub("{col}", tostring(col or 1))
    table.insert(args, v)
  end
  return args
end

local state = { opts = { notify = true, split = { direction = "Right", size = 0.5 } }, editor_next = false }

local function notify(window, msg)
  require("saysdf.util").log_warn("saysdf.paths: " .. msg)
  if state.opts.notify ~= false and window then
    require("saysdf.util").notify(window, "saysdf", msg)
  end
end

-- M.open(window, pane, target, how): how = "editor" | nil (viewer). Returns true/false.
function M.open(window, pane, target, how)
  local tgt = M.parse(target)
  if not tgt then
    return false
  end
  local full, why = M.resolve(pane, tgt.path)
  if not full then
    notify(window, why == "unknown cwd" and t("unknown pane folder: {path}", { path = tgt.path }) or why)
    return false
  end
  local ok, is_dir = M.exists(full)
  if not ok then
    -- diff prefix a/ b/: strip it if no file exists with the prefix
    local stripped = tgt.path:match("^[ab]/(.+)$")
    if stripped then
      local f2 = M.resolve(pane, stripped)
      if f2 then
        ok, is_dir = M.exists(f2)
        if ok then
          full = f2
        end
      end
    end
  end
  if not ok then
    notify(window, t("file not found: {path}", { path = full }))
    return false
  end
  local kind = M.classify(full, is_dir)

  if how == "editor" and kind ~= "dir" then
    local editor = state.opts.editor
    if type(editor) ~= "table" or #editor == 0 then
      notify(window, t("external editor is not set (paths.editor)"))
      return false
    end
    wezterm().background_child_process(M.editor_args(editor, full, tgt.line, tgt.col))
    return true
  end

  if kind == "dir" then
    local sp = state.opts.split or {}
    pane:split({ direction = sp.direction or "Right", size = sp.size or 0.5, cwd = full })
    return true
  end

  local has_viewer, viewer = pcall(require, "saysdf.viewer")
  if not has_viewer or type(viewer) ~= "table" or type(viewer.open) ~= "function" then
    notify(window, t("viewer is not installed (saysdf.viewer)"))
    return false
  end
  viewer.open(window, pane, full, tgt.line, tgt.col)
  return true
end

-- open-uri handler: only saysdf-path: and file://, everything else is stock (returns nil).
function M.on_open_uri(window, pane, uri)
  if type(uri) ~= "string" then
    return nil
  end
  if uri:sub(1, #M.SCHEME) ~= M.SCHEME and not uri:match("^file://") then
    return nil
  end
  local how = state.editor_next and "editor" or nil
  state.editor_next = false
  M.open(window, pane, uri, how)
  return false
end

function M.setup(opts, config)
  opts = opts or {}
  local w = wezterm()
  config = config or w.config_builder()
  for k, v in pairs(opts) do
    state.opts[k] = v
  end

  local rules = config.hyperlink_rules or w.default_hyperlink_rules()
  for _, r in ipairs(M.hyperlink_rules()) do
    table.insert(rules, r)
  end
  config.hyperlink_rules = rules

  w.on("open-uri", M.on_open_uri)

  if opts.mouse ~= false and w.action then
    local act = w.action
    local mb = config.mouse_bindings or {}
    table.insert(mb, { event = { Up = { streak = 1, button = "Left" } }, mods = "CTRL", action = act.OpenLinkAtMouseCursor })
    table.insert(mb, { event = { Down = { streak = 1, button = "Left" } }, mods = "CTRL", action = act.Nop })
    table.insert(mb, {
      event = { Up = { streak = 1, button = "Left" } },
      mods = "CTRL|SHIFT",
      action = w.action_callback(function(window, pane)
        state.editor_next = true
        window:perform_action(act.OpenLinkAtMouseCursor, pane)
      end),
    })
    table.insert(mb, { event = { Down = { streak = 1, button = "Left" } }, mods = "CTRL|SHIFT", action = act.Nop })
    config.mouse_bindings = mb
  end
  return config
end

-- for tests
M._state = state

return M
