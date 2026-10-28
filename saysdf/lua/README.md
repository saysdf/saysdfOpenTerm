# Built-in Lua modules of saysdfTerm

Everything in `saysdf/lua/` is embedded into the binary by the `saysdf/lua-bundle` crate (change
`bundled-lua-modules`). The user needs no files on disk.

- `saysdf/lua/saysdf/init.lua` - the root module, `require("saysdf")`.
- `saysdf/lua/saysdf/<name>.lua` - the module `require("saysdf.<name>")`.
- `saysdf/lua/default_config.lua` - the built-in config, run when the user has no
  `saysdfTerm.lua` (and `-n` / `--skip-config` is not set). If it fails, the error goes to the log and
  startup continues with the default config and no modules.

## Adding a module

Create the file `saysdf/lua/saysdf/<name>.lua` that returns a table with a function
`setup(opts, config)`:

```lua
local M = {}

function M.setup(opts, config)
  -- opts - the table from saysdf.setup({ <name> = {...} }) or {}
  -- config - the user's config_builder, modify it in place
end

return M
```

Nothing else needs editing: `build.rs` collects the module list, and `saysdf.setup` calls each
module's `setup` itself (in alphabetical order). Subfolders are not supported.

## Keyboard layout

`require("saysdf.keys")` (option `setup({ keys = { layout_independent = true } })`, true by default) sets
`config.key_map_preference = "Physical"` unless the user set it; `false` keeps upstream behavior.

## Disabling or configuring

```lua
local wezterm = require("wezterm")
local config = wezterm.config_builder()
require("saysdf").setup({
  git = false,              -- module disabled and not loaded
  tabs = { max = 10 },      -- module options
}, config)
return config
```

An unknown key in `opts` produces a log warning and loading continues. A config that never calls
`saysdf.setup` enables no modules.

## Overriding

A user file `<config dir>/saysdf/<name>.lua` (or any path from `package.path`)
takes priority over the built-in module. To start from the built-in source:

```
saysdfTerm-cli show-module --list
saysdfTerm-cli show-module tabs > ~/.config/saysdfTerm/saysdf/tabs.lua
```

Names: `tabs` or `saysdf.tabs`; `saysdf` is the root module, `default-config` is the built-in config.

## Modules

- `shell` - shell integration: starts the shell with the scripts, prompt navigation bindings,
  `last_command(pane)`, the `saysdf-command-finished` event; see `saysdf/shell-integration/README.md`.
- `multiline` - multi-line input: Shift+Enter and Ctrl+Enter send ESC CR (`\x1b\r`, like
  Alt+Enter), the shell integration scripts bind Alt+Enter to inserting a line break; Claude Code understands
  this without `/terminal-setup`. Options `keys = { "SHIFT", "CTRL" }`, `sequence = "\x1b\r"`;
  `multiline = false` restores upstream behavior; see the "Multi-line input" section in
  `saysdf/shell-integration/README.md`.
- `panes` - pane headers (change pane-header): a row above each pane with its title and
  buttons `□`/`▣` (zoom, `TogglePaneZoomState`) and `×` (`saysdf.close.close_pane` - confirmation depends on
  the `close` mode). The GUI draws it and reserves the row (the pane's pty is one row shorter); Lua only
  handles the `saysdf-pane-header-click(window, pane, button)` event. Option
  `header = "splits" | "always" | "never"` (default `splits` - when the tab has 2+ panes);
  there is no header for multiplexer tabs (`wezterm connect`).
- `files` - the `saysdf files` file browser (change file-browser): Ctrl+Shift+B toggles it in
  the side panel (if the side panel is off, a split on the right with no GUI actions), menu "File -> File
  browser". Browser actions go through the side panel channel: `files.open` (built-in viewer,
  files are never executed), `files.cd_pane` (folder as the cwd of a new pane), `files.copy_path`; a path
  is accepted only if it exists and lies under the tab's browser root. The root follows the cwd of the
  active pane: in `status.defer` on cwd change - `write_atomic` to `runtime_dir()/files-*.cwd`,
  and the browser watches the file. Options `key`, `palette`, `sync`.
- `sidebar` - tool side panel (change tool-sidebar): a split on the right 40 cells wide,
  one per tab, Ctrl+Shift+Y opens the last tool; `sidebar.selector(spec)` replaces
  `InputSelector` with the same spec (without `saysdf`, or with `sidebar = { enabled = false }`, it is plain
  `InputSelector`); the result is the user var `SAYSDF_SIDEBAR` with checks of the pane, process, token,
  length and an allow-list. Options `side`, `width`, `key`, `palette`, `test_entry`.
