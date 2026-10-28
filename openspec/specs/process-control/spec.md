# process-control Specification

## Purpose
List running processes and let the user act on them from a context menu. Dangerous actions require explicit confirmation.

## Requirements

### Requirement: Process list
The monitor SHALL list processes with PID, name, CPU %, memory, user and command line, support sorting by any column, incremental search (`/`) by name or command line, and a tree view (`t`). Processes of the pane the monitor was opened from MUST be highlighted.

#### Scenario: Search and sort
- **WHEN** the user presses `/`, types `cargo`, and sorts by memory
- **THEN** only processes with `cargo` in the name or command line are visible, in descending order of memory

#### Scenario: Tree view
- **WHEN** the user presses `t`
- **THEN** processes are shown as a parent → descendants tree

### Requirement: Process context menu
Right click on a process or the menu key (`m`) SHALL open a context menu with: pause, resume, stop, kill, change priority, show descendants, copy command line, open executable folder. Pause/resume MUST use `NtSuspendProcess`/`NtResumeProcess` on Windows and `SIGSTOP`/`SIGCONT` on Unix; stop MUST use `SIGTERM` on Unix and `WM_CLOSE` to the process windows on Windows (a process without windows is offered kill instead).

#### Scenario: Pause and resume
- **WHEN** the user selects "Pause" for their own process, then "Resume"
- **THEN** the process is suspended (status `paused`) and then continues running

#### Scenario: Copy command line
- **WHEN** the user selects "Copy command line"
- **THEN** the full command line is placed on the clipboard (OSC 52)

### Requirement: Confirmation for dangerous actions
Stop and kill of a process owned by another user, a system/service process, the terminal itself or its ancestors SHALL require explicit confirmation naming the process and PID. Actions MUST run with the user's own rights only, without privilege elevation; a permission error MUST be shown, not hidden.

#### Scenario: Kill foreign process
- **WHEN** the user selects kill for a process owned by another user
- **THEN** the prompt `Terminate <name> (PID n) owned by <user>? [y/N]` is shown; without `y` nothing happens

#### Scenario: Access denied
- **WHEN** the OS denies access
- **THEN** the monitor shows the error `permission denied`, and the process is unchanged
