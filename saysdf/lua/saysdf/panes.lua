-- saysdfTerminal: pane headers (change pane-header).
--
-- setup options (require("saysdf").setup({ panes = {...} })); panes = false disables the module:
-- clicks on header buttons are not handled (the GUI draws the header in its default mode).
--   header = "splits"   -- "splits" | "always" | "never"
--     splits -- a header above every pane, only when the tab has more than one pane;
--     always -- above a single pane too;
--     never  -- no headers (upstream behaviour, the pty size does not change).
--   Unknown value -- a warning in the log and "splits".
--
-- The GUI draws the header and reserves the pane size (Rust, saysdf_pane_header.rs + mux/src/tab.rs);
-- Lua is not called per frame. The mode is passed to the GUI through window:saysdf_set_pane_header(mode)
-- from window-config-reloaded (window creation and config reload).
-- A click on a button -> event saysdf-pane-header-click(window, pane, button):
--   "close" -> saysdf.close.close_pane (confirmation per the saysdf.close mode);
--   "zoom"  -> window:saysdf_pane_action(pane, "zoom"): in one call the GUI activates exactly this
--              pane and toggles zoom (maximize / restore), without racing other events;
--   "title" -> activate the pane.
-- For remote/multiplexer domains the header is not drawn (the server computes the size).

local M = {}

M.DEFAULT_HEADER = "splits"
M.HEADERS = { splits = true, always = true, never = true }

local function wezterm()
  return require("wezterm")
end

function M.header_mode(opts)
  local h = (opts or {}).header
  if h == nil then
    return M.DEFAULT_HEADER
  end
  if M.HEADERS[h] then
    return h
  end
  require("saysdf.util").log_warn(
    "saysdf.panes: unknown header '" .. tostring(h) .. "', using '" .. M.DEFAULT_HEADER .. "'"
  )
  return M.DEFAULT_HEADER
end

-- Handles a click on a header button. Returns the name of the action performed (for tests).
function M.on_click(window, pane, button)
  if window == nil or pane == nil then
    return nil
  end
  if button == "close" then
    require("saysdf.close").close_pane(window, pane)
    return "close"
  elseif button == "zoom" then
    window:saysdf_pane_action(pane, "zoom")
    return "zoom"
  elseif button == "title" then
    pcall(function()
      pane:activate()
    end)
    return "title"
  end
  return nil
end

function M.setup(opts, config)
  local mode = M.header_mode(opts)
  M.mode = mode
  local w = wezterm()
  w.on("saysdf-pane-header-click", function(window, pane, button)
    M.on_click(window, pane, button)
    return false
  end)
  -- once per window creation/config reload (not per frame); also sent when returning to "splits"
  w.on("window-config-reloaded", function(window)
    pcall(function()
      window:saysdf_set_pane_header(mode)
    end)
  end)
  return config
end

return M
