# project-workspaces Specification

## Purpose
Save and restore a project's pane and tab layout and switch between named workspaces. Restoring recreates the layout only and never runs commands automatically.

## Requirements

### Requirement: Save workspace layout
The terminal SHALL save the layout of a workspace (windows, tabs, split tree with sizes, cwd per pane, tab titles, optional command per pane) as JSON in the saysdfTerm data folder (`<data>/workspaces/<name>.json`) on explicit save and automatically (debounced, at most once per 30 s) while the workspace is open.

#### Scenario: Save current layout
- **WHEN** the user runs "Save workspace" with the name `api`
- **THEN** `workspaces/api.json` is created with the tabs, splits, and cwd of each pane

### Requirement: Switch workspaces
The terminal SHALL provide a key binding (default `Ctrl+Shift+A`) opening a fuzzy list of open and saved workspaces. Selecting an open one MUST switch to it (`SwitchToWorkspace`); selecting a saved but closed one MUST recreate it via `wezterm.mux.spawn_window{ workspace = ... }`.

#### Scenario: Open saved workspace
- **WHEN** the user selects the saved, closed workspace `api`
- **THEN** its tabs and splits are created in the saved folders, and it becomes active

### Requirement: Restore after restart without running commands
On GUI startup the terminal SHALL offer to restore the workspaces open at last exit (`restore = "ask"|"always"|"never"`, default `ask`). Restored panes MUST open shells in the saved cwd; saved commands MUST NOT run unless the user allowed commands for that workspace.

#### Scenario: Restore with commands not allowed
- **WHEN** after a restart the user confirms restoring a workspace with the command `npm run dev`
- **THEN** the pane opens a shell in the right folder, and the command is not run

### Requirement: Workspace file in project repository
A workspace MAY be defined in `.saysdf/workspace.json` in a project folder. Its cwd paths MUST resolve inside the project folder. Commands from such a file MUST run only after a confirmation that lists every command; the approval MUST be stored outside the repository, bound to the project path and the exact command list, and MUST be requested again when the commands change.

#### Scenario: Cloned repository
- **WHEN** the user opens a workspace from a freshly cloned repo that contains commands
- **THEN** the list of commands and a confirmation prompt are shown; until the user agrees, only shells are started

#### Scenario: Commands changed
- **WHEN** a command in the file has changed after approval
- **THEN** confirmation is requested again

#### Scenario: Path escapes project
- **WHEN** `cwd` in the file points to `../../`
- **THEN** the pane opens in the project root, and a warning is written to the log
