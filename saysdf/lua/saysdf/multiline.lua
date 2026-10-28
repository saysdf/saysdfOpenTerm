-- saysdfTerminal: multi-line input (change multiline-input, design D1, D3).
--
-- Shift+Enter and Ctrl+Enter send ESC CR ("\x1b\r", same as Alt+Enter) to the pty.
-- The shell-integration scripts bind Alt+Enter to inserting a newline (PSReadLine AddLine,
-- readline "\C-v\C-j", zle self-insert-unmeta, fish "commandline -i \n"); Claude Code treats
-- "\x1b\r" as a newline without /terminal-setup.
--
-- setup options (require("saysdf").setup({ multiline = {...} })):
--   keys = { "SHIFT", "CTRL" }   -- Enter modifiers that get the binding
--   sequence = "\x1b\r"          -- what to send
-- multiline = false disables the module: no bindings, Shift/Ctrl+Enter are encoded as upstream,
-- and the integration scripts do not add Alt+Enter (they bind it only when
-- SAYSDFTERM_MULTILINE=1, which this module sets, or outside saysdfTerm).

local M = {}

M.DEFAULT_KEYS = { "SHIFT", "CTRL" }
M.DEFAULT_SEQUENCE = "\x1b\r"

function M.setup(opts, config)
  opts = opts or {}
  local act = require("wezterm").action
  local mods = opts.keys or M.DEFAULT_KEYS
  local seq = opts.sequence or M.DEFAULT_SEQUENCE

  local keys = config.keys or {}
  for _, m in ipairs(mods) do
    table.insert(keys, { key = "Enter", mods = m, action = act.SendString(seq) })
  end
  config.keys = keys

  local env = config.set_environment_variables or {}
  env.SAYSDFTERM_MULTILINE = "1"
  config.set_environment_variables = env

  return config
end

return M
