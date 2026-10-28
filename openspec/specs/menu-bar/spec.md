# menu-bar Specification

## Purpose
Provide a top menu bar, a terminal area and a status bar as a three-band window layout, driven by a single command registry. Menus open by mouse or F9, function keys stay available to TUI programs, and only trusted content may define menu items.

## Requirements

### Requirement: Three-band window layout
The window SHALL show, from top to bottom: the menu band, the tab band (upstream), the terminal area,
and the status bar. The fork's bands MUST occupy one cell row each, and the terminal area MUST
shrink by exactly their height so that no terminal row is covered.

#### Scenario: Menu enabled
- **WHEN** the `menu` module is enabled
- **THEN** a row "File  Tabs  Git  Tools  View  Help" is visible above the tabs, and the pty row count is reduced by one

#### Scenario: Menu disabled
- **WHEN** the config has `menu = false`
- **THEN** the menu band is not drawn and takes no space

#### Scenario: Tab bar at bottom
- **WHEN** `tab_bar_at_bottom = true`
- **THEN** the menu band remains the top row of the window, and the tabs are drawn above the status bar

### Requirement: Single command registry
Menu items SHALL be built from the same registry as the command palette (`saysdf.palette`).
Each fork module MUST register its commands once; an entry with a `menu` field
appears both in the palette and in the corresponding menu.

#### Scenario: Module command in both places
- **WHEN** the `git` module registers the entry `{ id = "git.commit", brief = "Commit", menu = "Git" }`
- **THEN** "Commit" appears in the Ctrl+Shift+P palette and in the "Git" menu

#### Scenario: Disabled module
- **WHEN** the `db` module is disabled
- **THEN** its items appear neither in the palette nor in the menu

### Requirement: Open menu by mouse
A left click on a menu title SHALL open a dropdown list of its items below the
title; a click on an item MUST run the action and close the menu; a click outside the menu or Esc
MUST close the menu without running any action.

#### Scenario: Click Git
- **WHEN** the user clicks "Git" in the menu band
- **THEN** a list of git commands with their hotkeys on the right opens below the title

#### Scenario: Click outside
- **WHEN** the menu is open and the user clicks in the terminal area
- **THEN** the menu closes, no action is run, and the click is not forwarded to the program

### Requirement: Keyboard menu with F9
F9 SHALL open the first menu from the keyboard: the left/right arrows switch between menus,
up/down move between items, Enter runs the item, and Esc closes the menu. An accelerator letter (underlined) MAY
select an item.

#### Scenario: Navigate and run
- **WHEN** the user presses F9 in a shell, then Right until "Git", then Down and Enter
- **THEN** the selected git command is run

### Requirement: Function keys belong to TUI programs
If a TUI program is in the foreground of the pane, F9 and F1 MUST be passed to the program
unchanged. The foreground is considered a TUI if the alt-screen is active OR the foreground
process name is in a configurable list (`vim`, `nvim`, `mc`, `htop`, `btop`, `lazygit`,
`claude`, `saysdf` by default). The fork does not intercept any other F-keys.

#### Scenario: mc in pane
- **WHEN** `mc` is running in the pane and the user presses F9
- **THEN** mc receives F9, and the terminal menu does not open

#### Scenario: Claude Code without alt-screen
- **WHEN** the `claude` process is in the foreground (without alt-screen)
- **THEN** F1 and F9 go to the program

#### Scenario: Mouse still works over TUI
- **WHEN** `htop` is in the foreground and the user clicks "Git" in the menu band
- **THEN** the menu opens (a click on the band does not belong to the program)

### Requirement: Menu content is trusted only
The text and items of the menu band MUST be built only from the config and Lua modules; terminal
output (escape sequences, OSC, user vars) MUST NOT add items or actions.

#### Scenario: Malicious output
- **WHEN** a program prints an escape sequence that looks like menu markup
- **THEN** the menu band does not change
