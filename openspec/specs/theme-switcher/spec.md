# theme-switcher Specification

## Purpose
Let the user switch the color theme from the menu using preset schemes. The choice is persisted, the bars follow the active scheme, and the data directory can be overridden.

## Requirements

### Requirement: Theme menu items
The `saysdf.theme` module SHALL register in `saysdf.palette` (the "View" menu) the items "Theme…",
"Light/Dark", and "Follow system theme". "Theme…" SHALL show the selection from
`saysdf.theme_presets` (group, label, contrast; a note when `ansi_min < 2`), followed by all
the other built-in schemes, with fuzzy search, and apply the chosen scheme to all windows via
`set_config_overrides`. The default `light`/`dark` pair SHALL have a text contrast of at least 7:1.

#### Scenario: Choose a scheme from the list
- **WHEN** the user selects "View → Theme…", types "drac", and presses Enter on "Dracula"
- **THEN** the "Dracula" scheme is applied in all windows without a restart

#### Scenario: Light/Dark
- **WHEN** the current scheme equals the `dark` option and the "Light/Dark" item is selected
- **THEN** the scheme from the `light` option is applied; from any other scheme, the `dark` scheme is applied

#### Scenario: Follow system
- **WHEN** the "Follow system theme" item is selected and the OS appearance is dark
- **THEN** the `dark` scheme is applied, and when the OS appearance changes to light, the `light` scheme is applied

### Requirement: Preset schemes registration
Non-built-in schemes from `saysdf.theme_presets` MUST be registered in `config.color_schemes` during
`setup()`, without overwriting the user's schemes of the same name.

#### Scenario: Scheme from the selection
- **WHEN** the user selects "Campbell (Windows Terminal)", which is not among the built-in schemes
- **THEN** the scheme is applied; a user scheme of the same name in `config.color_schemes` is not replaced

### Requirement: Persisted choice
The choice MUST be saved to `<data>/theme.json` with an atomic write and SHALL be applied at startup
in `setup()` on top of `config.color_scheme`. With `theme = { respect_config = true }` and `config.color_scheme`
set in the config, the saved choice MUST NOT change the scheme at startup.
A corrupted file SHALL be ignored.

#### Scenario: Choice survives restart
- **WHEN** the user chose "Dracula" and the config has `config.color_scheme = 'Builtin Solarized Light'`
- **THEN** "Dracula" is in effect after a restart

#### Scenario: Config takes priority
- **WHEN** `respect_config = true` is set and the config has `config.color_scheme`
- **THEN** the scheme from the config is in effect at startup

### Requirement: Bars follow the scheme
If `colors.tab_bar` is set neither in the config nor in the scheme, the menu, status bar, banner, and
tab bands SHALL take their background and text from the scheme (background: the scheme background mixed with some of the text color), the text MUST have a
contrast against the band background of at least 7:1, and the bands SHALL be redrawn when the scheme changes via overrides.

#### Scenario: Light scheme
- **WHEN** "Builtin Solarized Light" is in effect
- **THEN** the bands are light, with dark, readable text on them

### Requirement: Data dir override
A non-empty `SAYSDFTERM_DATA_DIR` variable SHALL replace the data folder in both `saysdf.util.data_dir()`
and `config::DATA_DIR`; `saysdf/tools/screenshot.ps1` SHALL by default launch the terminal
with a temporary data folder and delete it after the screenshot.

#### Scenario: Test screenshot
- **WHEN** `screenshot.ps1` is run without `-RealData`
- **THEN** no files in the real saysdfTerm data folder are created or modified
