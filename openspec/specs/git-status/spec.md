# git-status Specification

## Purpose
Show the current repository's Git status in the status bar and expose it to other modules through an API. Refresh does not block the UI and never executes code from the repository.

## Requirements

### Requirement: Git status in status bar
The terminal SHALL show in the status bar the state of the git repository containing the active pane's directory: the branch (or a short SHA on a detached HEAD), the number of staged, modified, untracked and conflicted files, and ahead/behind relative to the upstream. Outside a repository or without `git` in PATH the block MUST NOT be shown and MUST NOT cause errors.

#### Scenario: Repository with changes
- **WHEN** the active pane is in a repository on branch `main`, 2 files are modified, 1 is untracked, and the branch is 3 commits ahead of upstream
- **THEN** the status bar shows the branch `main`, the change counters and `↑3`

#### Scenario: Outside a repository
- **WHEN** the active pane's directory is not inside any repository
- **THEN** the git block is not displayed in the status bar

#### Scenario: git not installed
- **WHEN** `git` is not in PATH
- **THEN** the git block is not displayed, a single warning is written to the log, and no repeated notifications are shown

#### Scenario: Pane directory unknown
- **WHEN** the pane has no cwd (no OSC 7 from shell integration)
- **THEN** the git block is not displayed

### Requirement: Non-blocking status refresh
Obtaining the git status MUST NOT block rendering or input: git calls SHALL run asynchronously, the result SHALL be cached by repository root with a limit on the refresh rate, and at most one status request MUST run at a time for a given repository.

#### Scenario: Large repository
- **WHEN** `git status` takes longer than the status bar refresh interval
- **THEN** input and rendering are not slowed down, the status bar shows the last known value, and no new request is started until the previous one finishes

#### Scenario: Changing directory
- **WHEN** the user runs `cd` into a different repository
- **THEN** the status bar shows the new repository's data on the next refresh, without waiting for the old cache to expire

#### Scenario: Command completion
- **WHEN** a command finishes in the pane (the user var `SAYSDF_LAST_EXIT` changes)
- **THEN** the cache for that pane's repository is marked stale and the status is refreshed on the next `update-status`

### Requirement: No repository code execution on status
Status requests MUST NOT run git hooks, external diff/textconv drivers or `core.fsmonitor` from the repository configuration, and MUST NOT take optional index locks.

#### Scenario: Malicious fsmonitor in a repository
- **WHEN** `.git/config` of the repository sets `core.fsmonitor` to an arbitrary command
- **THEN** that command is not executed when the status bar is refreshed

#### Scenario: Concurrent git usage
- **WHEN** the user runs `git rebase` in a pane while the status bar is being refreshed
- **THEN** the status request does not create `index.lock` and does not interfere with the user's command

### Requirement: Git info API for other modules
The `saysdf.git` module SHALL provide other modules with a function that returns the cached repository state of a pane (branch, counters, ahead/behind) without launching any processes, so that `tab-management` can show the branch on a tab.

#### Scenario: Tab title
- **WHEN** `format-tab-title` requests the branch for the tab's pane
- **THEN** the module immediately returns the value from the cache (or `nil` if there is no data yet), without calling git
