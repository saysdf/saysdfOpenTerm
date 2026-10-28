# close-confirmation Specification

## Purpose
Rules for when and how saysdfTerm asks for confirmation before closing a window, tab, pane or the whole application, so that an accidental close does not kill running processes.

## Requirements

### Requirement: Confirmation mode setting
The terminal SHALL provide a single setting `close_confirmation` in `require("saysdf").setup({ close = {...} })` with values `"always"`, `"only-if-running-process"` and `"never"`. The default MUST be `"always"` (user decision: always ask). The mode MUST apply equally to windows, tabs, panes and quitting the application.

#### Scenario: Default mode
- **WHEN** the user has not set `close_confirmation`
- **THEN** the `"always"` mode is in effect

#### Scenario: Unknown value
- **WHEN** the user sets an unknown value (for example, `"sometimes"`)
- **THEN** a warning is written to the log and the default mode is in effect

### Requirement: Mode always
In mode `"always"` the terminal SHALL ask for confirmation before closing any window, tab or pane, even if only an idle shell runs in it and even if the window has a single tab.

#### Scenario: Window with one idle shell
- **WHEN** the mode is `"always"` and the window has one tab with an idle shell
- **AND** the user clicks the window's close button or presses Alt+F4
- **THEN** a confirmation dialog is shown and the window is not closed until the user answers "yes"

#### Scenario: Pane with idle shell
- **WHEN** the mode is `"always"` and the user closes a pane with an idle shell
- **THEN** a confirmation dialog is shown

### Requirement: Mode only-if-running-process
In mode `"only-if-running-process"` the terminal SHALL ask for confirmation only if at least one affected pane runs a process other than the shells listed in `skip_close_confirmation_for_processes_named`, and SHALL close without a dialog otherwise.

#### Scenario: Running build in one of several tabs
- **WHEN** `cargo build` is running in one of the window's three tabs
- **AND** the user closes the window
- **THEN** a confirmation dialog is shown

#### Scenario: Only idle shells
- **WHEN** all panes of the tab being closed contain only idle shells
- **THEN** the tab is closed immediately, without a dialog

### Requirement: Mode never
In mode `"never"` the terminal SHALL close windows, tabs, panes and the application without any confirmation dialog.

#### Scenario: Window with running ssh
- **WHEN** the mode is `"never"` and `ssh` is running in a tab
- **AND** the user closes the window
- **THEN** the window is closed immediately

### Requirement: Every close path is covered
The confirmation rules SHALL apply to every user-initiated close path: the window close button, Alt+F4 and the OS window menu; the close button on a tab; the default key bindings and the command palette entries for closing a tab or a pane; the `QuitApplication` action. Key bindings explicitly configured by the user with `confirm = false` MUST remain unconfirmed.

#### Scenario: Tab close button
- **WHEN** the mode is `"always"` and the user clicks the close button on an inactive tab
- **THEN** the tab becomes active and a confirmation dialog is shown in it

#### Scenario: User binding without confirm
- **WHEN** the user has bound a key to `CloseCurrentPane { confirm = false }`
- **THEN** that key closes the pane without a dialog in any mode

### Requirement: Dialog shows what is being closed
The confirmation dialog SHALL state the kind of object being closed (window, tab, pane, application) and SHALL list the names of non-shell foreground processes that will be terminated; for a window or the application it SHALL also show the number of tabs and panes.

#### Scenario: Closing a window
- **WHEN** a window with 3 tabs and 4 panes is being closed, with `claude` running in one pane and `ssh` in another
- **THEN** the dialog contains "window", "3 tabs, 4 panes" and the names `claude` and `ssh`

#### Scenario: Closing a pane with idle shell in mode always
- **WHEN** the mode is `"always"` and a pane containing only `pwsh` is being closed
- **THEN** the dialog contains "pane" and does not contain a list of processes (or states that no processes are running)

### Requirement: Declining keeps everything intact
Declining or dismissing the dialog (No, Esc, closing the overlay) SHALL leave the window, tabs, panes and their processes unchanged.

#### Scenario: Esc in dialog
- **WHEN** the close-window dialog is open and the user presses Esc
- **THEN** the window, all tabs and processes remain
