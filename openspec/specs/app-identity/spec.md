# app-identity Specification

## Purpose
Defines the name under which saysdfTerm appears to the user and the OS (binaries, window, shortcuts, bundles) and where it stores its config, data, cache and sockets, so that it can live next to the official WezTerm without sharing anything with it.

## Requirements

### Requirement: Executable names
The shipped binaries SHALL be named `saysdfTerm` (GUI), `saysdfTerm-cli` (CLI with the subcommands `cli`, `imgcat`, `start`, etc.) and `saysdfTerm-mux-server` (mux server), with the `.exe` suffix on Windows. Binaries named `wezterm*` MUST NOT be shipped.

#### Scenario: Contents of the distribution
- **WHEN** a user installs a saysdfTerm build on any OS
- **THEN** the installation directory contains `saysdfTerm`, `saysdfTerm-cli` and `saysdfTerm-mux-server` and does not contain the files `wezterm`, `wezterm-gui`, `wezterm-mux-server`

#### Scenario: CLI starts the GUI
- **WHEN** the user runs `saysdfTerm-cli start`
- **THEN** the `saysdfTerm` GUI binary from the same directory is launched

#### Scenario: GUI starts the mux server
- **WHEN** the GUI connects to a unix domain without an explicit `serve_command` and the server is not running
- **THEN** `saysdfTerm-mux-server --daemonize` is launched from the GUI binary's directory

### Requirement: Config file location
saysdfTerm SHALL look for the config in this order: the path from `--config-file`; the `SAYSDFTERM_CONFIG_FILE` environment variable; on Windows, `saysdfTerm.lua` next to the binary; `~/.saysdfTerm.lua`; `saysdfTerm.lua` in each config directory (`$XDG_CONFIG_HOME/saysdfTerm`, otherwise `~/.config/saysdfTerm`, then `$XDG_CONFIG_DIRS/*/saysdfTerm`). The first file found is used.

#### Scenario: Config in the default directory
- **WHEN** only `~/.config/saysdfTerm/saysdfTerm.lua` exists (Windows: `%USERPROFILE%\.config\saysdfTerm\saysdfTerm.lua`)
- **THEN** that file is loaded

#### Scenario: Environment variable takes precedence
- **WHEN** `SAYSDFTERM_CONFIG_FILE=<path>` is set and `~/.config/saysdfTerm/saysdfTerm.lua` exists
- **THEN** the file at `<path>` is loaded, and if it does not exist, a configuration error is reported

### Requirement: Isolation from WezTerm configuration
saysdfTerm MUST NOT read the WezTerm configuration: the files `wezterm.lua` and `~/.wezterm.lua`, the `~/.config/wezterm` directory and the `WEZTERM_CONFIG_FILE` variable.

#### Scenario: Only a WezTerm config exists
- **WHEN** `~/.config/wezterm/wezterm.lua` exists and there is no saysdfTerm config
- **THEN** saysdfTerm starts with the default config and does not execute the WezTerm file

#### Scenario: Launched from inside WezTerm
- **WHEN** saysdfTerm is launched from a WezTerm shell in which `WEZTERM_CONFIG_FILE` is set
- **THEN** that variable is ignored when looking for the config

### Requirement: Separate data, cache and runtime directories
Data, cache and runtime files (mux sockets, daemon pid and log files, PKI) SHALL be stored in a `saysdfTerm` subdirectory of the corresponding OS data, cache and runtime directories, separate from the WezTerm directories.

#### Scenario: Running alongside WezTerm
- **WHEN** WezTerm and saysdfTerm are running at the same time under the same user
- **THEN** they have different mux and GUI sockets, and `saysdfTerm-cli cli list` shows only saysdfTerm panes

### Requirement: Lua search path for user modules
The directories searched for user Lua modules SHALL include the saysdfTerm config directories and `~/.saysdfTerm`, and not the WezTerm directories; the Lua API `require("wezterm")` SHALL remain available unchanged.

#### Scenario: User module
- **WHEN** the config runs `require("mymod")` and `~/.config/saysdfTerm/mymod.lua` exists
- **THEN** the module is loaded from that file

#### Scenario: API compatibility
- **WHEN** the config runs `local wezterm = require("wezterm")`
- **THEN** the module is available with the same set of functions as before

### Requirement: Application identity in the OS
The window, shortcuts and bundles SHALL present themselves as saysdfTerm: window class / app id `com.saysdf.saysdfTerm` (X11, Wayland, Windows AppUserModelID), default title `saysdfTerm`, a `.desktop` file on Linux, and a `saysdfTerm.app` bundle with `CFBundleName` = `saysdfTerm` on macOS.

#### Scenario: Default window title
- **WHEN** the running program has not set a pane title
- **THEN** the window title contains `saysdfTerm`, not `wezterm`

#### Scenario: Grouping in the taskbar
- **WHEN** WezTerm and saysdfTerm windows are open
- **THEN** the OS groups them as two different applications
