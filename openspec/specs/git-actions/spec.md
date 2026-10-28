# git-actions Specification

## Purpose
Run common Git operations (commit, checkout, push) from hotkeys and the command palette without leaving the terminal. Push is always explicit and operation output stays visible.

## Requirements

### Requirement: Git actions from hotkey and command palette
The terminal SHALL provide the git operations commit, push, pull and checkout through a menu opened by a hotkey and through the command palette. Operations SHALL run in the repository of the active pane's directory.

#### Scenario: Git menu
- **WHEN** the user presses the git menu hotkey inside a repository
- **THEN** a list of operations opens (commit, push, pull, checkout, submodules, diff, external git UI)

#### Scenario: Command palette
- **WHEN** the user opens the command palette
- **THEN** it contains the entries `Git: Commit`, `Git: Push`, `Git: Pull`, `Git: Checkout branch`

#### Scenario: Outside a repository
- **WHEN** an operation is invoked outside a git repository
- **THEN** a "not a git repository" notification is shown and git is not launched

### Requirement: Commit with message input
Commit SHALL prompt for the commit message in an input field and MUST NOT create a commit with an empty message or when the input is cancelled. The message MUST be passed to git without involving a shell.

#### Scenario: Successful commit
- **WHEN** there are staged changes and the user enters the message `fix: typo`
- **THEN** `git commit -m "fix: typo"` is run and the result is shown to the user

#### Scenario: Cancellation
- **WHEN** the user closes the input field or enters an empty string
- **THEN** no commit is created

#### Scenario: Nothing to commit
- **WHEN** there are no staged changes
- **THEN** the user gets a notification suggesting to open the diff for staging; the commit is not performed

### Requirement: Explicit push only
Push MUST be performed only by an explicit user action (a menu or palette entry) with a confirmation showing the remote and branch. No other operation MUST trigger a push implicitly, and the module MUST NOT run fetch, pull or push automatically.

#### Scenario: Push confirmation
- **WHEN** the user selects Push
- **THEN** a confirmation with `<remote>/<branch>` is shown; the push runs only after confirmation

#### Scenario: No upstream
- **WHEN** the branch has no upstream
- **THEN** the confirmation explicitly offers `push -u origin <branch>`

### Requirement: Operation output visibility
Operations that access the network or run hooks (commit, push, pull) SHALL run in a separate split pane with visible output, credential prompts and hook output; a brief summary SHALL be shown as a notification on completion.

#### Scenario: Push with a password prompt
- **WHEN** git asks for credentials during a push
- **THEN** the prompt is visible in the operation pane and the user can type a response

#### Scenario: Pull error
- **WHEN** a pull fails (conflict, no network)
- **THEN** the operation pane stays open with the output, and a notification reports the error

### Requirement: Branch checkout
Checkout SHALL show a list of local and remote branches with fuzzy search and switch to the selected branch; if the working tree is dirty, a warning MUST be shown before switching.

#### Scenario: Switching branch
- **WHEN** the user selects the branch `feature/x` from the list
- **THEN** `git switch feature/x` is run and the status bar is updated

#### Scenario: Remote branch
- **WHEN** the remote branch `origin/y` is selected and there is no local `y`
- **THEN** `git switch --track origin/y` is run

### Requirement: External git UI
The module SHALL open lazygit or gitui in a split if one of them is found in PATH, and MUST report what is missing if neither is available.

#### Scenario: External UI not installed
- **WHEN** the user selects "external git UI" and neither lazygit nor gitui is found
- **THEN** a notification naming the utilities is shown and no split is opened
