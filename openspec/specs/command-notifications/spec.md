# command-notifications Specification

## Purpose
Notify the user when a long-running command finishes or a program raises a notification, and highlight the tab that needs attention. Notifications are rate limited and can be silenced with do-not-disturb.

## Requirements

### Requirement: Notify on long command completion
When a command that ran longer than the threshold (default 10 s, `setup({ notify = { min_duration_s } })`) finishes in a pane that is not focused (inactive tab or unfocused/minimized window), the terminal SHALL show an OS notification with the tab title, the command duration and the exit code. Data MUST come from the `saysdf-command-finished` event of shell-integration. Commands finishing in the focused pane MUST NOT notify.

#### Scenario: Build finished in background tab
- **WHEN** `cargo build` ran for 40 s in an inactive tab and finished with exit code 1
- **THEN** a notification `✗ build — 40s, exit 1` appears

#### Scenario: Short command
- **WHEN** a command ran for 2 s with a 10 s threshold
- **THEN** there is no notification

### Requirement: Tab attention highlight
A tab whose pane produced a notification (command or program) SHALL be marked with an attention indicator until the tab is activated. The indicator state MUST be shared with `saysdf.tabs` (tab-management indicators) rather than drawn separately.

#### Scenario: Attention cleared on view
- **WHEN** a tab is marked and the user opens it
- **THEN** the indicator disappears

### Requirement: Program notifications are rate limited
Notifications requested by programs via OSC 9 and OSC 777 `notify` SHALL be truncated (title ≤ 64, body ≤ 256 characters, control characters removed) and rate limited per pane (default: at most 1 per 5 s, burst 3); excess notifications MUST be dropped and counted in the log.

#### Scenario: Spam
- **WHEN** a program sends 100 OSC 9 sequences in one second
- **THEN** no more than 3 notifications are shown and the rest are dropped

### Requirement: Do not disturb
The terminal SHALL provide a do-not-disturb toggle (command palette entry and Lua function) that suppresses OS notifications while keeping tab attention indicators.

#### Scenario: DND on
- **WHEN** do-not-disturb is on and a background command finishes
- **THEN** there is no OS notification, but the tab is marked

### Requirement: Click focuses the tab
Where the platform supports notification activation callbacks, clicking a notification SHALL activate the window, tab and pane it came from. Where unsupported, the notification MUST still be shown and the attention indicator is the fallback.

#### Scenario: Click on Windows
- **WHEN** on Windows the user clicks a command-completion notification
- **THEN** the saysdfTerm window is brought to the foreground with that tab active
