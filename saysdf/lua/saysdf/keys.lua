-- saysdfTerminal: layout-independent hotkeys (change layout-independent-keys).
--
--   require('saysdf').setup({ keys = { layout_independent = true } }, config)
--
-- layout_independent (default true): sets config.key_map_preference = "Physical"
-- unless the user set it. Then all bindings (Ctrl+Shift+E, F9, Alt+Shift+arrows,
-- Ctrl+Alt+Space...) match by physical key (US layout position), so with a non-Latin
-- layout the key at the E position still triggers Ctrl+Shift+E. false keeps upstream behavior ("Mapped").
-- The whole module is disabled with `keys = false`.
-- Side effect: your own bindings on punctuation typed with Shift on a non-US layout
-- are matched to the key position on the US layout.
local M = {}

local function default(config, key, value)
  local ok, cur = pcall(function()
    return config[key]
  end)
  if not ok or cur == nil then
    config[key] = value
  end
end

function M.setup(opts, config)
  opts = opts or {}
  config = config or {}
  local on = opts.layout_independent
  if on == nil then
    on = true
  end
  if not on then
    return config
  end
  -- config_builder may throw on read; leave the user's value alone
  default(config, "key_map_preference", "Physical")
  default(config, "enable_kitty_keyboard", true)
  return config
end

return M
