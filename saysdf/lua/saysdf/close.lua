-- saysdfTerminal: close confirmation (change close-confirmation, design D1).
--
-- setup options (require("saysdf").setup({ close = {...} })); close = false disables the module
-- entirely (upstream behavior: ask only if a process outside the skip list is running):
--   close_confirmation = "always"   -- "always" | "only-if-running-process" | "never"
--     always                  -- ask before closing any window, tab or pane, even with an
--                                empty shell and a single tab;
--     only-if-running-process -- ask only if what is being closed has a process not in
--                                skip_close_confirmation_for_processes_named;
--     never                   -- close without asking (window_close_confirmation = "NeverPrompt").
--   An unknown value logs a warning and falls back to the default mode.
--
-- How it works: a handler for the mux-is-process-stateful event (always -> true, never -> false,
-- only-if-running-process -> nil, i.e. the upstream check against the skip list) and
-- config.window_close_confirmation (never -> "NeverPrompt", otherwise "AlwaysPrompt"; a value the
-- user set before setup() is overwritten by the mode). If the user set their own
-- skip_close_confirmation_for_processes_named, the Windows shells (cmd.exe, pwsh.exe,
-- powershell.exe) are appended without duplicates; the upstream default already has them.
-- wsl.exe is not added: a WSL tab always counts as "has a process".
--
-- Limitations:
--   * keys/actions with confirm = false (e.g. CloseCurrentPane { confirm = false })
--     close without asking in any mode;
--   * if the pane's process list could not be determined (no rights, race at startup), upstream
--     treats the pane as closable without asking - always mode will not apply to it;
--   * remote multiplexer panes (ClientPane): the window closes without asking, the hook is not called;
--   * mux-is-process-stateful: the first handler that returns non-nil wins. A handler registered
--     before setup() overrides the mode; one registered after setup() only works for
--     only-if-running-process (in the other modes this module has already returned true/false).

local M = {}

local t = require("saysdf.i18n").domain("close")

M.DEFAULT_MODE = "always"
M.MODES = {
  ["always"] = true,
  ["only-if-running-process"] = true,
  ["never"] = true,
}
M.WINDOWS_SHELLS = { "cmd.exe", "pwsh.exe", "powershell.exe" }

local function wezterm()
  return require("wezterm")
end

-- Mode from the options; an unknown value logs a warning and falls back to the default.
function M.mode(opts)
  local m = (opts or {}).close_confirmation
  if m == nil then
    return M.DEFAULT_MODE
  end
  if M.MODES[m] then
    return m
  end
  require("saysdf.util").log_warn(
    "saysdf.close: unknown close_confirmation '" .. tostring(m) .. "', using '" .. M.DEFAULT_MODE .. "'"
  )
  return M.DEFAULT_MODE
end

-- Answer for mux-is-process-stateful: true = ask, false = do not ask, nil = upstream decides.
function M.stateful(mode)
  if mode == "always" then
    return true
  elseif mode == "never" then
    return false
  end
  return nil
end

-- The user's list plus the Windows shells without duplicates; nil (upstream default) is left alone.
function M.skip_list(list)
  if type(list) ~= "table" then
    return list
  end
  local out, seen = {}, {}
  for _, name in ipairs(list) do
    if not seen[name] then
      seen[name] = true
      table.insert(out, name)
    end
  end
  for _, name in ipairs(M.WINDOWS_SHELLS) do
    if not seen[name] then
      seen[name] = true
      table.insert(out, name)
    end
  end
  return out
end

-- Mode chosen by the last setup() (for close_pane); the default before setup.
M.current_mode = M.DEFAULT_MODE

-- Whether closing a pane needs confirmation (change pane-header D5): never -> no; always -> yes;
-- only-if-running-process -> upstream decides by the skip list (confirm = true, upstream will not
-- ask for a shell from the list).
function M.confirm_for(mode)
  return mode ~= "never"
end

-- Close a specific pane by the module's rules: the GUI activates exactly this pane and in the same
-- call runs upstream's CloseCurrentPane { confirm = ... } (the confirmation overlay sits above it).
-- It used to be pane:activate() + perform_action(CloseCurrentPane): between them the GUI could
-- handle another event (a pane change) and close the wrong one (review L2).
function M.close_pane(window, pane)
  if window == nil or pane == nil then
    return false
  end
  window:saysdf_pane_action(pane, "close", M.confirm_for(M.current_mode))
  return true
end

function M.setup(opts, config)
  local mode = M.mode(opts)
  M.current_mode = mode
  local answer = M.stateful(mode)

  wezterm().on("mux-is-process-stateful", function(_proc)
    return answer
  end)

  config.window_close_confirmation = (mode == "never") and "NeverPrompt" or "AlwaysPrompt"
  config.skip_close_confirmation_for_processes_named =
    M.skip_list(config.skip_close_confirmation_for_processes_named)

  -- saysdfTerminal: menu entry "Tabs -> Close" (upstream's palette already has it).
  local w = wezterm()
  if w.action and opts ~= nil and opts.palette ~= false then
    require("saysdf.palette").add({
      id = "close.tab",
      menu = "Tabs",
      order = 90,
      label = t("Close tab"),
      accel = "q",
      key = "Ctrl+Shift+W",
      palette = false,
      brief = t("Close tab"),
      action = w.action.CloseCurrentTab({ confirm = true }),
    })
  end

  return config
end

return M
