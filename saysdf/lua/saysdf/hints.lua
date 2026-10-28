-- saysdfTerminal: command hints in the shell (change command-hints, design D1, D4).
--
-- The hints themselves work inside the shell's line editor: the shell-integration scripts register
-- `saysdf hints complete` as the completer for commands with built-in specs (pwsh
-- Register-ArgumentCompleter -Native + MenuComplete on Tab, bash complete -C, zsh compdef,
-- fish complete -a) and enable the grey history tail through the stock mechanism (PSReadLine
-- PredictionSource, fish autosuggestions, zsh-autosuggestions if installed). This module only
-- passes the settings to the shell through environment variables; it installs no third-party modules.
--
-- setup options (require("saysdf").setup({ hints = {...} })):
--   disabled = { "kubectl" }  -- commands without our hints (true = all) -> SAYSDF_HINTS_DISABLED
--   override = false          -- true: register on top of existing completers too
--   inline = true             -- false: do not touch the PSReadLine PredictionSource
--   menu = true               -- false: do not bind MenuComplete to Tab (pwsh)
--   commands = nil            -- command list instead of the built-in one (git cargo docker kubectl npm)
--   bin = nil                 -- path to saysdf (default: next to saysdfTerm, then PATH)
-- hints = false disables the module: SAYSDFTERM_HINTS is not set, and the integration scripts inside
-- saysdfTerm (SAYSDFTERM_SHELL_INTEGRATION=1) do not enable hints.
-- Check in the shell: `saysdf hints doctor`.

local M = {}

M.DEFAULT_COMMANDS = { "git", "cargo", "docker", "kubectl", "npm" }

local function list(v)
  if type(v) == "table" then
    local out = {}
    for _, x in ipairs(v) do
      if type(x) == "string" and x ~= "" then
        table.insert(out, x)
      end
    end
    return table.concat(out, ",")
  elseif type(v) == "string" then
    return v
  end
  return ""
end

-- Environment variables for the shell from the options (no side effects).
function M.env(opts, bin)
  opts = opts or {}
  local env = { SAYSDFTERM_HINTS = "1" }
  if opts.disabled == true then
    env.SAYSDF_HINTS_DISABLED = "*"
  else
    local d = list(opts.disabled)
    if d ~= "" then
      env.SAYSDF_HINTS_DISABLED = d
    end
  end
  if opts.override == true then
    env.SAYSDF_HINTS_OVERRIDE = "1"
  end
  if opts.inline == false then
    env.SAYSDFTERM_HINTS_INLINE = "0"
  end
  if opts.menu == false then
    env.SAYSDFTERM_HINTS_MENU = "0"
  end
  local c = list(opts.commands)
  if c ~= "" then
    env.SAYSDFTERM_HINTS_COMMANDS = c
  end
  bin = opts.bin or bin
  if type(bin) == "string" and bin ~= "" then
    env.SAYSDFTERM_HINTS_BIN = bin
  end
  return env
end

function M.setup(opts, config)
  opts = opts or {}
  local bin = opts.bin
  if bin == nil then
    local ok, util = pcall(require, "saysdf.util")
    if ok and type(util) == "table" and type(util.find_bin) == "function" then
      local okb, p = pcall(util.find_bin, "saysdf")
      bin = okb and p or nil
    end
  end
  local env = config.set_environment_variables or {}
  for k, v in pairs(M.env(opts, bin)) do
    env[k] = v
  end
  config.set_environment_variables = env
  return config
end

return M
