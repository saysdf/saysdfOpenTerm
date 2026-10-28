# quick-select Specification

## Purpose
Select text patterns in terminal output by keyboard and copy or open them. It adds extended patterns and works over any output.

## Requirements

### Requirement: Extended quick-select patterns
The `saysdf.quickselect` module SHALL add to `config.quick_select_patterns` the path patterns from `saysdf.paths` (with `:line`, `:line:col`, `(line,col)`, and Windows paths) and an `IPv4:port` pattern, while keeping the upstream built-in patterns (URLs, hashes, IPs, UUIDs, numbers). Path patterns MUST be taken from `saysdf.paths` rather than duplicated. User patterns from the options SHALL be added to them.

#### Scenario: Whole path with line number
- **WHEN** `src/app.rs:42:7` is on screen and the user starts quick-select
- **THEN** the label is placed on the whole `src/app.rs:42:7`, not separately on `src/app.rs` and `42`

#### Scenario: Commit hash
- **WHEN** the screen shows `git log --oneline` output containing `a1b2c3d`
- **THEN** the hash gets a label, and typing the label copies it to the clipboard

### Requirement: Copy mode
The standard quick-select hotkey (Ctrl+Shift+Space) SHALL remain: typing a label copies the value to the clipboard, and typing the label in uppercase copies it and pastes it into the pane.

#### Scenario: Copy
- **WHEN** the user presses Ctrl+Shift+Space and types the label of a path
- **THEN** the path is copied to the clipboard and nothing is opened

### Requirement: Open mode
A separate hotkey (Ctrl+Alt+Space by default, configurable) SHALL start quick-select with the "open" action label: typing a label opens the value — a path via `saysdf.paths` (the same rules as for a click: existence check, opened in a split according to its type, executables are not launched), an `http`/`https` URL in the browser. Values of other kinds MUST NOT be opened; they are copied and the user gets a notification.

#### Scenario: Open a path
- **WHEN** the user presses Ctrl+Alt+Space and types the label for `src/app.rs:42`
- **THEN** the file opens in a `saysdf view` split at line 42

#### Scenario: Open a URL
- **WHEN** the user types the label for `https://example.com`
- **THEN** the URL opens in the default browser

#### Scenario: Value that cannot be opened
- **WHEN** the user types the label of an IP address in "open" mode
- **THEN** the value is copied, nothing is launched, and a notification is shown

### Requirement: Works over any output
Quick-select SHALL work on the visible text of the pane regardless of the program, including a TUI on the alternate screen, and MUST NOT send anything to the program other than an explicit paste (label in uppercase).

#### Scenario: Over a TUI
- **WHEN** `htop` or `lazygit` is open in the pane and the user presses Ctrl+Alt+Space
- **THEN** labels are placed on the visible values, and the program receives no keystrokes until a selection is made

### Requirement: Module setup
The module SHALL be enabled via `require("saysdf").setup({ quickselect = {...} | false })` with the options `open_key` and `patterns` (additional). With `false`, the quick-select config remains the standard one.

#### Scenario: Module disabled
- **WHEN** `setup({ quickselect = false })`
- **THEN** `quick_select_patterns` and the key bindings are not changed
