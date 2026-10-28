# bundled-modules Specification

## Purpose
Defines how the fork's Lua modules are shipped inside the saysdfTerm binary, loaded via `require`, enabled, disabled and configured through `saysdf.setup`, overridden by user files, and how to view their source.

## Requirements

### Requirement: Bundled modules are available via require
Every module in the repository's `saysdf/lua/saysdf/<module>.lua` SHALL be embedded in the binary and available in the Lua config as `require("saysdf.<module>")` without any files on disk; `require("saysdf")` SHALL return the root module (`saysdf/lua/saysdf/init.lua`).

#### Scenario: Loading a bundled module
- **WHEN** the config runs `require("saysdf.tabs")` and there is no module file on disk
- **THEN** the bundled `tabs` module is returned

#### Scenario: Unknown module
- **WHEN** the config runs `require("saysdf.nope")`
- **THEN** the usual Lua "module not found" error is raised, and the list of searched locations mentions the bundled modules

### Requirement: User file overrides bundled module
The file `<config dir>/saysdf/<module>.lua` (or any other `package.path` location that yields `saysdf/<module>.lua`) SHALL take precedence over the bundled module of the same name.

#### Scenario: Override
- **WHEN** `~/.config/saysdfTerm/saysdf/tabs.lua` exists and the config runs `require("saysdf.tabs")`
- **THEN** the user's file is loaded and the bundled module is not executed

### Requirement: Setup enables and configures modules
`require("saysdf").setup(opts, config)` SHALL call the `setup` of every bundled module except the disabled ones (`opts.<module> = false`), passing it `opts.<module>` (a table or `{}`) and `config`; the function SHALL return `config`. If `config` is not passed, a new one is created via `wezterm.config_builder()`.

#### Scenario: Disabling a module
- **WHEN** the config calls `saysdf.setup({ git = false }, config)`
- **THEN** the `git` module is not loaded and does not modify `config`; the other modules are set up

#### Scenario: Module options
- **WHEN** the config calls `saysdf.setup({ tabs = { max = 10 } }, config)`
- **THEN** the `tabs` module receives `{ max = 10 }`

#### Scenario: Unknown key
- **WHEN** `opts` contains a key that matches no module
- **THEN** a warning with the key name is written to the log and config loading continues

### Requirement: Built-in default config without user config
If no user config is found, saysdfTerm SHALL execute a built-in default config equivalent to `return require("saysdf").setup({})`, that is, with all modules. If the built-in config fails with an error, saysdfTerm SHALL write the error to the log and start with the default config without any modules.

#### Scenario: First launch
- **WHEN** no config file exists in any search location
- **THEN** the terminal starts with the bundled modules enabled

#### Scenario: Launch with config skipped
- **WHEN** the terminal is started with the flag that skips the config (`-n` / `--skip-config`)
- **THEN** the built-in default config is not executed either

### Requirement: Show bundled module source
The command `saysdfTerm-cli show-module <name>` SHALL print the source of a bundled module to stdout (`saysdf.<name>` or `<name>`; `saysdf` is the root module, `default-config` is the built-in config), and `saysdfTerm-cli show-module --list` SHALL print the list of bundled modules. For an unknown name the command SHALL exit with a non-zero code and print the list of available names.

#### Scenario: Viewing the source
- **WHEN** the user runs `saysdfTerm-cli show-module tabs > tabs.lua`
- **THEN** the file contains the source of the bundled `tabs` module, suitable for use as an override

#### Scenario: Unknown name
- **WHEN** the user runs `saysdfTerm-cli show-module nope`
- **THEN** the command prints the list of available modules to stderr and exits with code 1
