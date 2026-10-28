-- saysdfTerminal: shell integration (change shell-integration, design D2, D3, D7).
--
-- Starts the default shell with the integration script loaded from
-- <scripts dir>/ (saysdfterm.ps1, bash/saysdfterm.bashrc, zsh/, saysdfterm.fish),
-- accepts the user vars SAYSDF_LAST_EXIT / SAYSDF_LAST_DURATION_MS, stores the last
-- command of every pane and emits the "saysdf-command-finished" event.
--
-- setup options (require("saysdf").setup({ shell = {...} })); shell = false disables the whole module:
--   default_prog = true   -- set config.default_prog unless the user set their own
--   shell = nil           -- default shell name/path instead of auto-detection ("pwsh", "bash", ...)
--   launch_menu = true    -- add launch_menu items for the shells found
--   keys = true           -- bindings (false = no keys and no mouse)
--   prev_prompt = { key = "UpArrow", mods = "ALT|SHIFT" }   -- false = no such binding
--   next_prompt = { key = "DownArrow", mods = "ALT|SHIFT" }
--   copy_output = { key = "O", mods = "CTRL|SHIFT" }
--   triple_click = true   -- triple click selects a semantic zone
--   dir = nil             -- scripts dir (else SAYSDFTERM_SHELL_INTEGRATION_DIR, then next to the exe)
--
-- API: command(shell, dir?) -> argv | nil; last_command(pane) -> { exit_code, duration_ms,
-- finished_at, pane_id } | nil; scripts_dir() -> path | nil.

local M = {}

local t = require("saysdf.i18n").domain("shell")

local function wezterm()
  return require("wezterm")
end

-- Override points for tests.
M._getenv = os.getenv
M._exists = function(path)
  local f = io.open(path, "rb")
  if f then
    f:close()
    return true
  end
  return false
end

local function os_kind()
  local triple = wezterm().target_triple or ""
  if triple:find("windows") then
    return "windows"
  elseif triple:find("darwin") or triple:find("apple") then
    return "macos"
  end
  return "linux"
end

local function sep()
  return os_kind() == "windows" and "\\" or "/"
end

local function join(...)
  local s = sep()
  local parts = { ... }
  local out = parts[1]:gsub("[/\\]+$", "")
  for i = 2, #parts do
    out = out .. s .. parts[i]
  end
  return out
end

local function basename(path)
  local name = path:match("([^/\\]+)$") or path
  return (name:lower():gsub("%.exe$", ""))
end

-- Scripts dir marker: a file that is always present.
local MARKER = "saysdfterm.ps1"

-- D2: the first existing scripts dir or nil.
function M.scripts_dir(override)
  local candidates = {}
  if override then
    table.insert(candidates, override)
  end
  local env = M._getenv("SAYSDFTERM_SHELL_INTEGRATION_DIR")
  if env and env ~= "" then
    table.insert(candidates, env)
  end
  local exe_dir = wezterm().executable_dir
  if exe_dir then
    table.insert(candidates, join(exe_dir, "shell-integration"))
    local kind = os_kind()
    if kind == "macos" then
      -- <App>/Contents/MacOS -> <App>/Contents/Resources
      table.insert(candidates, join(exe_dir, "..", "Resources", "shell-integration"))
    elseif kind == "linux" then
      table.insert(candidates, join(exe_dir, "..", "share", "saysdfterm", "shell-integration"))
    end
  end
  for _, dir in ipairs(candidates) do
    if M._exists(join(dir, MARKER)) then
      return dir
    end
  end
  return nil
end

-- PowerShell: single-quoted string, ' is doubled.
function M.ps_quote(s)
  return "'" .. (s:gsub("'", "''")) .. "'"
end

-- fish: single-quoted string, \ and ' are escaped.
function M.fish_quote(s)
  return "'" .. s:gsub("\\", "\\\\"):gsub("'", "\\'") .. "'"
end

-- Shell kind by name/path, or nil for an unknown one.
function M.shell_kind(shell)
  local name = basename(shell)
  if name == "pwsh" or name == "powershell" then
    return "powershell"
  elseif name == "bash" or name == "zsh" or name == "fish" then
    return name
  end
  return nil
end

-- D3: argv to start the shell with integration; nil if the shell is unknown or there is no scripts dir.
function M.command(shell, dir)
  local kind = M.shell_kind(shell)
  dir = dir or M.scripts_dir()
  if not kind or not dir then
    return nil
  end
  if kind == "powershell" then
    local loader = ". ([scriptblock]::Create([IO.File]::ReadAllText("
      .. M.ps_quote(join(dir, "saysdfterm.ps1"))
      .. ")))"
    return { shell, "-NoLogo", "-NoExit", "-Command", loader }
  elseif kind == "bash" then
    return { shell, "--rcfile", join(dir, "bash", "saysdfterm.bashrc"), "-i" }
  elseif kind == "zsh" then
    -- ZDOTDIR trick: our .zshenv restores the original ZDOTDIR and loads the user files
    return {
      "/usr/bin/env",
      "SAYSDFTERM_ORIG_ZDOTDIR=" .. (M._getenv("ZDOTDIR") or ""),
      "ZDOTDIR=" .. join(dir, "zsh"),
      shell,
      "-l",
    }
  elseif kind == "fish" then
    return { shell, "-l", "--init-command", "source " .. M.fish_quote(join(dir, "saysdfterm.fish")) }
  end
  return nil
end

local function find_in_path(name)
  local path = M._getenv("PATH") or ""
  local delim = os_kind() == "windows" and ";" or ":"
  for d in path:gmatch("[^" .. delim .. "]+") do
    local candidate = join(d, name)
    if M._exists(candidate) then
      return candidate
    end
  end
  return nil
end

-- Default shell: Windows -- pwsh from PATH, else powershell; Unix -- $SHELL.
function M.default_shell()
  if os_kind() == "windows" then
    if find_in_path("pwsh.exe") then
      return "pwsh.exe"
    end
    return "powershell.exe"
  end
  local sh = M._getenv("SHELL")
  if sh and sh ~= "" then
    return sh
  end
  return nil
end

-- ---- last command and event ----

local last = {}

function M.last_command(pane)
  if pane == nil then
    return nil
  end
  local id = type(pane) == "number" and pane or pane:pane_id()
  return last[id]
end

-- user-var-changed handler (D7). Returns info if a command completion was recorded.
function M.on_user_var(window, pane, name, value)
  if name == "SAYSDF_LAST_EXIT" then
    local code = tonumber(value)
    if code == nil then
      return nil
    end
    local vars = pane:get_user_vars() or {}
    local info = {
      pane_id = pane:pane_id(),
      exit_code = code,
      -- the scripts send the duration right after the code; by the time of the event it is usually there
      duration_ms = tonumber(vars.SAYSDF_LAST_DURATION_MS),
      finished_at = os.time(),
    }
    last[info.pane_id] = info
    wezterm().emit("saysdf-command-finished", window, pane, info)
    return info
  elseif name == "SAYSDF_LAST_DURATION_MS" then
    local info = last[pane:pane_id()]
    local ms = tonumber(value)
    if info and ms then
      info.duration_ms = ms
    end
  end
  return nil
end

-- Ctrl+Shift+O: text of the last Output zone.
function M.copy_last_output(window, pane)
  local zones = pane:get_semantic_zones("Output") or {}
  local zone = zones[#zones]
  local text = zone and pane:get_text_from_semantic_zone(zone) or ""
  text = text:gsub("%s+$", "")
  if text == "" then
    require("saysdf.util").notify(window, "saysdfTerm", t("No output from the last command"))
    return
  end
  window:copy_to_clipboard(text, "Clipboard")
end

-- ---- setup ----

local registered = false

local function append(config, field, items)
  local list = config[field] or {}
  for _, item in ipairs(items) do
    table.insert(list, item)
  end
  config[field] = list
end

local function binding(opt, default)
  if opt == false then
    return nil
  end
  if type(opt) == "table" then
    return opt
  end
  return default
end

function M.setup(opts, config)
  opts = opts or {}
  local w = wezterm()
  local dir = M.scripts_dir(opts.dir)

  if not dir then
    require("saysdf.util").log_warn("saysdf.shell: shell-integration scripts not found; shells start without integration")
  else
    local env = config.set_environment_variables or {}
    env.SAYSDFTERM_SHELL_INTEGRATION = "1"
    if os_kind() == "macos" and env.SAYSDFTERM_BASH_LOGIN == nil then
      env.SAYSDFTERM_BASH_LOGIN = "1"
    end
    config.set_environment_variables = env

    if opts.default_prog ~= false and config.default_prog == nil then
      local sh = opts.shell or M.default_shell()
      local argv = sh and M.command(sh, dir)
      if argv then
        config.default_prog = argv
      end
    end

    if opts.launch_menu ~= false then
      local names = os_kind() == "windows" and { "pwsh.exe", "powershell.exe" }
        or { "bash", "zsh", "fish" }
      local items = {}
      for _, n in ipairs(names) do
        local found = find_in_path(n)
        if found then
          local argv = M.command(os_kind() == "windows" and n or found, dir)
          if argv then
            table.insert(items, { label = basename(n) .. " (shell integration)", args = argv })
          end
        end
      end
      if #items > 0 then
        append(config, "launch_menu", items)
      end
    end
  end

  if not registered then
    registered = true
    w.on("user-var-changed", function(window, pane, name, value)
      M.on_user_var(window, pane, name, value)
    end)
  end

  if opts.keys ~= false then
    local act = w.action
    local keys = {}
    local prev = binding(opts.prev_prompt, { key = "UpArrow", mods = "ALT|SHIFT" })
    if prev then
      table.insert(keys, { key = prev.key, mods = prev.mods, action = act.ScrollToPrompt(-1) })
    end
    local nxt = binding(opts.next_prompt, { key = "DownArrow", mods = "ALT|SHIFT" })
    if nxt then
      table.insert(keys, { key = nxt.key, mods = nxt.mods, action = act.ScrollToPrompt(1) })
    end
    local copy = binding(opts.copy_output, { key = "O", mods = "CTRL|SHIFT" })
    if copy then
      local cb = w.action_callback(function(window, pane)
        M.copy_last_output(window, pane)
      end)
      table.insert(keys, { key = copy.key, mods = copy.mods, action = cb })
      -- like upstream for Ctrl+Shift+letter: "O" is normalized to CTRL+O, so add "o" too
      if type(copy.key) == "string" and copy.key:match("^%u$") then
        table.insert(keys, { key = copy.key:lower(), mods = copy.mods, action = cb })
      end
    end
    if #keys > 0 then
      append(config, "keys", keys)
    end
    -- saysdfTerminal: registry entries (menu-bar 1.5).
    local palette = require("saysdf.palette")
    if copy then
      palette.add({ id = "shell.copy_output", menu = "Tools", order = 70, label = t("Copy command output"), accel = "y",
        key = palette.key_text(copy), brief = t("Shell: copy the output of the last command"),
        hint = { contexts = { "shell" }, label = t("Output"), priority = 80 },
        action = w.action_callback(function(window, pane) M.copy_last_output(window, pane) end) })
    end
    if prev then
      palette.add({ id = "shell.prev_prompt", menu = "View", order = 50, label = t("Previous prompt"), accel = "[",
        key = palette.key_text(prev), brief = t("Shell: go to the previous prompt"), action = act.ScrollToPrompt(-1) })
    end
    if nxt then
      palette.add({ id = "shell.next_prompt", menu = "View", order = 51, label = t("Next prompt"), accel = "]",
        key = palette.key_text(nxt), brief = t("Shell: go to the next prompt"), action = act.ScrollToPrompt(1) })
    end
    if opts.triple_click ~= false then
      append(config, "mouse_bindings", {
        {
          event = { Down = { streak = 3, button = "Left" } },
          mods = "NONE",
          -- fullscreen programs (vim, less) have no zones -- the upstream line selection stays
          alt_screen = false,
          action = act.SelectTextAtMouseCursor("SemanticZone"),
        },
      })
    end
  end

  return config
end

return M
