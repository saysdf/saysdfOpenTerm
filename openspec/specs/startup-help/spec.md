# startup-help Specification

## Purpose
Show a short startup banner with the version and key hints, and a full help screen on F1. The banner can be dismissed and help can be disabled.

## Requirements

### Requirement: Startup banner
When each new window opens, the terminal SHALL show a banner of 3-5 lines: what this
program is, the main hotkeys (menu F9, palette, git, tab rename), and the line
"F1 — full help". The banner MUST be shown outside the pty: it MUST NOT end up in the shell input,
command history, or scrollback.

#### Scenario: First window
- **WHEN** the user launches saysdfTerm
- **THEN** a banner band is visible above the status bar, and the shell prompt is neither shifted nor corrupted

#### Scenario: History stays clean
- **WHEN** the banner is shown and the user scrolls the shell history with the up arrow
- **THEN** the history contains no banner lines

### Requirement: Banner version line
The first banner line SHALL be `saysdfTerminal <version>`, where the version is `wezterm.version`.
If the version is empty, the line MUST be `saysdfTerminal (version unknown)`. The version line counts
toward the banner's 5-line limit: on overflow the banner MUST drop the last hints.

#### Scenario: Version shown first
- **WHEN** the banner is shown
- **THEN** its first line is `saysdfTerminal <build version>`

#### Scenario: Empty version
- **WHEN** `wezterm.version` is empty
- **THEN** the first line is `saysdfTerminal (version unknown)`

### Requirement: Banner dismissal
The banner SHALL be dismissed by Esc, a click on it, the first input to the pane, or after 20 seconds,
whichever comes first. After dismissal the terminal area MUST get the occupied rows back.

#### Scenario: Typing hides banner
- **WHEN** the banner is shown and the user starts typing a command
- **THEN** the banner disappears, and the typed character goes to the shell

### Requirement: Full help on F1
F1 SHALL open the full help `saysdf help` in a new tab in the built-in
Markdown viewer. The help MUST contain the sections: what this is, window layout, hotkeys, modules and
options, and security (why close confirmation exists, why databases are read-only, why terminal output
does not launch processes, where secrets are stored, why there is no network access by default).

#### Scenario: F1 in shell
- **WHEN** the user presses F1 in a shell
- **THEN** a tab with the help opens, and Esc/q closes it

#### Scenario: F1 in TUI
- **WHEN** `mc` or `claude` is in the foreground
- **THEN** F1 goes to the program and the help does not open

#### Scenario: Help from menu
- **WHEN** the user selects "Help → Full help" in the menu or the palette
- **THEN** the same help opens

### Requirement: Help can be disabled
The config SHALL allow disabling the banner (`help = { banner = false }`) and the whole module
(`help = false`). With the banner disabled, the band MUST NOT take up space.

#### Scenario: Banner off
- **WHEN** the config has `help = { banner = false }`
- **THEN** there is no banner at startup, and F1 still opens the help
