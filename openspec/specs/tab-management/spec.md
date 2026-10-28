# tab-management Specification

## Purpose
Naming, renaming, and appearance of saysdfTerm tabs: meaningful automatic titles, manual names, color, and status indicators, so that with a dozen tabs it is clear what is happening where.

## Requirements

### Requirement: Rename tab by key binding
The terminal SHALL provide a key binding (default `Ctrl+Shift+E`, configurable or disableable via `setup({ tabs = {...} })`) that prompts for a new tab name, pre-filled with the current name. Confirming a non-empty name MUST set it as the tab name; cancelling (Esc) MUST leave the name unchanged.

#### Scenario: Rename active tab
- **WHEN** the user presses the rename hotkey, types `build`, and presses Enter
- **THEN** the tab shows the title `build`

#### Scenario: Cancel rename
- **WHEN** the name prompt is open and the user presses Esc
- **THEN** the tab name does not change

### Requirement: Rename tab by double click
Double-clicking a tab in the tab bar with the left mouse button SHALL open the same rename prompt for that tab. A single click MUST keep activating the tab as before.

#### Scenario: Double click on inactive tab
- **WHEN** the user double-clicks an inactive tab
- **THEN** the tab becomes active and the name prompt opens for it

### Requirement: Manual name is sticky
A manual tab name SHALL persist for the lifetime of the tab and MUST NOT be overwritten by titles set by programs (OSC 0/2), working directory or process changes.

#### Scenario: Program sets title
- **WHEN** the tab has been renamed to `prod-db` and a program in it sends OSC 2 `vim README.md`
- **THEN** the tab keeps showing `prod-db`

### Requirement: Reset to automatic name
Confirming an empty name in the rename prompt, or a dedicated reset action, SHALL clear the manual name and return the tab to the automatic title.

#### Scenario: Empty name resets
- **WHEN** a tab has a manual name and the user confirms an empty input
- **THEN** the tab shows the automatic title again

### Requirement: Automatic title
Without a manual name the tab title SHALL consist of an icon of the foreground process, the project folder name (basename of the current working directory) and, if known, the git branch. Parts that are unknown MUST be omitted without placeholders. Titles longer than the tab width MUST be truncated with an ellipsis.

#### Scenario: Shell in a git project
- **WHEN** a tab runs `pwsh` in the folder `C:\dev\saysdfTerminal` and the branch `main` is known
- **THEN** the title contains the shell icon, `saysdfTerminal`, and `main`

#### Scenario: Unknown cwd
- **WHEN** the current folder is unknown (for example, a remote ssh session without OSC 7)
- **THEN** the title contains the icon and the program's title, with no empty separators

### Requirement: Process icons
The terminal SHALL map foreground process names to icons for at least: shells (bash, zsh, fish, pwsh, powershell, cmd, nu), editors (vim, nvim, hx, nano), ssh, git/lazygit, claude, python, node, cargo. Unknown processes MUST get a generic icon. Users MUST be able to override and extend the mapping in `setup()`; icons MUST have a plain-text fallback when Nerd Font glyphs are disabled.

#### Scenario: Unknown process
- **WHEN** `mytool.exe` is running in a tab
- **THEN** the generic process icon is shown

#### Scenario: User override
- **WHEN** the user has set an icon for `htop`
- **THEN** a tab running `htop` shows it

### Requirement: Running process indicator
A tab whose panes run a non-shell foreground process SHALL show a "running" indicator; it MUST disappear when only shells remain.

#### Scenario: Build in progress
- **WHEN** `cargo build` is running in a tab
- **THEN** the tab shows the "running" indicator

### Requirement: Failed command indicator
When shell integration (OSC 133) reports a non-zero exit status for the last command in a tab, the tab SHALL show an error indicator until the next command starts or the tab is activated. Without shell integration no error indicator MUST be shown.

#### Scenario: Command fails
- **WHEN** with shell integration, a command in an inactive tab finishes with exit code 1
- **THEN** the tab shows the error indicator

#### Scenario: No shell integration
- **WHEN** the shell does not send OSC 133
- **THEN** the error indicator is never shown

### Requirement: Unseen output indicator
An inactive tab that received output since it was last active SHALL show an "unseen output" indicator, cleared when the tab is activated.

#### Scenario: Output in background tab
- **WHEN** output appears in an inactive tab
- **THEN** the tab shows the unseen output indicator
- **AND** after the tab is activated the indicator disappears

### Requirement: User tab color
The terminal SHALL let the user pick a color for a tab from a fixed palette via a command (palette entry and optional key binding) and reset it; the color MUST persist for the lifetime of the tab and stay readable for active and inactive states.

#### Scenario: Set color
- **WHEN** the user picks the color "red" for a tab
- **THEN** the tab is displayed with a red background/accent, and active and inactive states are distinguishable
