# hotkey-statusbar Specification

## Purpose
Display context-sensitive hotkey hints in a bottom status bar. Hints come from the single command registry, are clickable and refresh promptly when the context changes.

## Requirements

### Requirement: Bottom status bar layout
The window SHALL show a status strip at the bottom: on the left, hotkey hints such as "F9 Menu  ^⇧S Git",
on the right, the output of `saysdf.status` (git, metrics). When there is not enough width, the hints MUST be truncated
from right to left by priority, and the right part MUST be preserved.

#### Scenario: Shell in git repo
- **WHEN** the active pane is a shell in a git repository
- **THEN** the left side has a git menu hint and the right side shows the branch and repository status

#### Scenario: Hotbar disabled
- **WHEN** the config has `hotbar = false`
- **THEN** there is no bottom strip, and `saysdf.status` is rendered in the right status of the tab bar

### Requirement: Context selection
The hint context SHALL be selected for the active pane by priority: database dangerous mode,
a `saysdf` utility in the foreground, text selection, SSH session, git repository, shell.
The selection MUST be a pure function of the collected pane state.

#### Scenario: Selection wins over git
- **WHEN** text is selected inside a git repository
- **THEN** selection hints are shown (copy, quick-select, open path)

#### Scenario: Database danger mode
- **WHEN** dangerous mode is enabled in a `saysdf db` pane
- **THEN** the hint for leaving dangerous mode is shown first, highlighted with color

#### Scenario: SSH session
- **WHEN** the foreground process is `ssh`
- **THEN** SSH hints are shown (connection list, new tab to the same host)

### Requirement: Hints come from the command registry
Hints SHALL be taken from the single command registry `saysdf.palette` (the `hint` field), and the hotkey
text from the same entry that the menu uses. Hints of disabled modules MUST NOT
be shown.

#### Scenario: Module disabled
- **WHEN** the `git` module is disabled
- **THEN** there are no git hints in the status bar

### Requirement: Clickable hints
A left click on a hint SHALL perform its action for the active pane in the same way as
pressing the hotkey. A click on the right part MAY perform the status source's action (for example,
opening the git menu), if the source declared one.

#### Scenario: Click git hint
- **WHEN** the user clicks the "Git" hint
- **THEN** the git menu opens, as with Ctrl+Shift+S

### Requirement: Timely refresh
The status bar SHALL be refreshed when the active pane/tab changes and at least once per
`status_update_interval`; a change of selection MUST be reflected no later than the next refresh.

#### Scenario: Switch tab
- **WHEN** the user switches from a `saysdf view` tab to a tab with a shell
- **THEN** the hints change to shell/git immediately after the switch
