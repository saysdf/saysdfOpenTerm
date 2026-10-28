# layout-independent-keys Specification

## Purpose
Make hotkeys work under any keyboard layout by matching key bindings on the physical key (its position on the US layout) instead of the typed character. A config flag turns this on by default and can restore the upstream behavior.

## Requirements

### Requirement: Layout independent flag
The terminal SHALL provide `require("saysdf").setup({ keys = { layout_independent = <bool> } }, config)`; the default MUST be true. When true and the user has not set `config.key_map_preference`, the module SHALL set it to `"Physical"`. When false (or `keys = false`), the module MUST NOT change the config.

#### Scenario: Default
- **WHEN** the user calls `setup({}, config)` without `key_map_preference`
- **THEN** `config.key_map_preference == "Physical"`

#### Scenario: User value wins
- **WHEN** the user has set `config.key_map_preference = "Mapped"`
- **THEN** the value is not overwritten

#### Scenario: Flag off
- **WHEN** `keys = { layout_independent = false }`
- **THEN** `key_map_preference` is not set (upstream behavior)

#### Scenario: Russian layout
- **WHEN** the flag is on and the Russian keyboard layout is active
- **THEN** Ctrl+Shift+E (physical key E), F9, Alt+Shift+arrows, Ctrl+Alt+Space and Shift/Ctrl+Enter work as they do with the US layout
