# tool-sidebar Specification

## Purpose
Host tool panels in a sidebar pane that can replace the tool selector and report results back to the terminal. Results arrive only through a trusted channel and are checked against an allow-list.

## Requirements

### Requirement: Sidebar pane
The module `saysdf.sidebar` SHALL open a tool as a top-level split on the configured side (`right` by default) with a fixed width in cells (`width`, default 40, clamped to at most half of the tab), running the `saysdf` binary directly (not via a shell). A tab MUST have at most one sidebar. Opening MUST NOT block the GUI thread (no waiting for the process).

#### Scenario: Open
- **WHEN** the user invokes a tool (for example, the git menu) and there is no sidebar
- **THEN** a split 40 cells wide with `saysdf pick` appears on the right, with focus in it

#### Scenario: Toggle
- **WHEN** the same tool is invoked again while its pane is focused
- **THEN** the pane closes and focus returns to the previous pane
- **WHEN** the pane exists but is not focused
- **THEN** it receives focus

#### Scenario: Replace
- **WHEN** one tool is open and another is invoked
- **THEN** the old sidebar closes and a new one opens (one per tab)

#### Scenario: Narrow tab
- **WHEN** the tab width is less than `2 * width`
- **THEN** the width is reduced to half of the tab; if the tab is already 20 cells wide, the fallback `InputSelector` is used

### Requirement: Selector replacement
`saysdf.sidebar.selector(spec)` SHALL accept the same table as `InputSelector` (`title`, `choices` with `id`/`label`, `fuzzy`, `action` callback) and return an action. When the sidebar is enabled it MUST show the choices in `saysdf pick`; otherwise, or if the `saysdf` binary is not found, it MUST fall back to `InputSelector` with the same spec.

#### Scenario: Fallback
- **WHEN** `sidebar = { enabled = false }` or the `saysdf` binary is not found
- **THEN** the upstream `InputSelector` opens with the same list and callback

#### Scenario: Choices transport
- **WHEN** `saysdf pick` is opened
- **THEN** the list is passed via a file in `runtime_dir()` (`write_atomic`), not on the command line; labels are plain text without escape sequences

#### Scenario: Cancel
- **WHEN** the user presses Esc in `saysdf pick`
- **THEN** the callback is called with `id = nil` (as in InputSelector) and the pane closes

### Requirement: Trusted result channel
The TUI SHALL return a result via user var `SAYSDF_SIDEBAR` = `<token>:<kind>:<payload>`. The GUI MUST accept it only if: the pane id equals the sidebar pane recorded for the tab; `get_foreground_process_name_cached()` of that pane is the `saysdf` binary; the token matches the unused one-time token of this open (passed via env); the value is at most 4096 bytes. Anything else MUST be ignored. The handler MUST cost at most 1 ms.

#### Scenario: Forged user var from another pane
- **WHEN** a program in an ordinary pane prints OSC 1337 SetUserVar `SAYSDF_SIDEBAR`
- **THEN** nothing is executed

#### Scenario: Wrong token
- **WHEN** the value came from the sidebar pane but the token does not match or has already been used
- **THEN** nothing is executed

### Requirement: Result allow-list
For `pick` the payload MUST be an id from the choices list given for this token; for `action` it MUST be an id of a `saysdf.palette` entry with `sidebar = true`, and its argument MUST pass the entry's `accept(arg)` validator. Anything else MUST be ignored and logged once at debug level.

#### Scenario: Unknown action
- **WHEN** the action id is not in the registry or the entry lacks `sidebar = true`
- **THEN** nothing is executed

#### Scenario: Valid pick
- **WHEN** the user has selected an item in `saysdf pick`
- **THEN** the original selector's callback is called with that id, in the window and pane from which the tool was opened

### Requirement: Sidebar hotkey and options
The module SHALL bind Ctrl+Shift+Y to toggle the sidebar with the last used tool (or the file browser when none was used) and register the palette entry "Sidebar". Options: `sidebar = { enabled = true, side = "right", width = 40 }`; `sidebar = false` MUST restore the previous behaviour everywhere.

#### Scenario: Hotkey
- **WHEN** the user presses Ctrl+Shift+Y in a tab without a sidebar
- **THEN** the last tool opens; pressing it again inside the sidebar closes it
