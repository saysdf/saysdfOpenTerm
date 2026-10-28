# diff-viewer Specification

## Purpose
Display file and Git diffs in a split pane with side-by-side and unified modes, syntax and intra-line highlighting, and hunk navigation. Hunks can be staged or unstaged, and the viewer integrates with git difftool.

## Requirements

### Requirement: Diff view modes
The `saysdf diff` utility SHALL show differences between two files, two directories, or a git repository's working tree and index/HEAD, in side-by-side and inline modes switchable with a key. Unchanged regions SHALL be collapsed and expandable, and long lines SHALL scroll horizontally.

#### Scenario: Two files
- **WHEN** the user runs `saysdf diff a.rs b.rs`
- **THEN** a side-by-side view opens with added, removed and changed lines highlighted

#### Scenario: Switching mode
- **WHEN** the user presses the mode-switch key
- **THEN** the view switches between side-by-side and inline, preserving the position

#### Scenario: No changes
- **WHEN** the files are identical
- **THEN** the utility shows the message "no differences" and closes on `q`

#### Scenario: Binary file
- **WHEN** one of the files is binary
- **THEN** instead of a line-by-line diff, "binary files differ" is shown along with the file sizes

### Requirement: Syntax and intra-line highlighting
The diff view SHALL highlight syntax by file type using the shared highlighting engine and SHALL emphasize the changed fragments within changed lines. Control characters in the content MUST be displayed as visible characters rather than interpreted by the terminal.

#### Scenario: One word changed
- **WHEN** one word in a line is changed
- **THEN** the line is marked as changed, and the word itself is emphasized with a brighter background

#### Scenario: Escape sequence in a file
- **WHEN** a file being compared contains `ESC]52;...`
- **THEN** the sequence is displayed as visible characters and does not change the clipboard

### Requirement: File and hunk navigation
The utility SHALL show a list of changed files (for directories and git mode) and SHALL let the user move to the next/previous hunk and file with keys.

#### Scenario: Hunk navigation
- **WHEN** the user presses the "next hunk" key
- **THEN** the view scrolls to the start of the next hunk, and on the last hunk it moves to the next file

### Requirement: Stage and unstage hunks
In git mode the utility SHALL let the user add a hunk or selected lines to the index and remove them from the index. Index changes MUST be made via `git apply --cached` with a patch and MUST be verified with `--check` before being applied; on error the index MUST remain unchanged.

#### Scenario: Staging a hunk
- **WHEN** in the "working tree ↔ index" view the user presses stage on a hunk
- **THEN** the hunk goes into the index, the view is refreshed, and the hunk disappears from the unstaged list

#### Scenario: Unstaging a hunk
- **WHEN** in the "index ↔ HEAD" view the user presses unstage on a hunk
- **THEN** the hunk is removed from the index via a reverse patch, and the working tree is not changed

#### Scenario: Patch does not apply
- **WHEN** the file has changed since the view was built and the patch fails `git apply --check --cached`
- **THEN** an error message is shown, the index is not changed, and the view is reloaded

### Requirement: Git difftool integration
The utility SHALL work as `git difftool --tool=saysdf`. Configuring git (`difftool.saysdf.cmd`) SHALL be done only on an explicit user command, showing the keys to be written and asking for confirmation.

#### Scenario: Configuring difftool
- **WHEN** the user runs `saysdf diff --install-git-tool`
- **THEN** the utility shows the keys and the scope (`--global`), and writes them only after confirmation
