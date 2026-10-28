# file-browser Specification

## Purpose
Provide a file tree for the project with Git status marks and common file actions. Actions are validated and deletion is safe, and the tree follows the working directory of the active pane.

## Requirements

### Requirement: File tree
`saysdf files [dir]` SHALL show the directory as a tree (directories first, case-insensitive name order), expand/collapse with Right/Left or Enter on a directory, filter by name with `/`, toggle hidden entries with `.`. Directory reading MUST be lazy (only expanded directories) and capped at 10 000 entries per directory with a visible "truncated" mark. Symlinks MUST be shown but not followed when expanding to avoid cycles.

#### Scenario: Large directory
- **WHEN** a directory has more than 10 000 entries
- **THEN** the first 10 000 are shown along with a "…truncated" line, and the interface does not freeze

#### Scenario: Symlink loop
- **WHEN** a link points to a parent directory
- **THEN** it is shown as a link and is not expanded recursively

### Requirement: Git status marks
When the root is inside a git repository, the browser SHALL mark files and ancestor directories with their status (modified, added, untracked, deleted, conflict) computed in a background thread by the same code as `saysdf git-status`. The UI MUST stay responsive while status is computed; without git the marks MUST simply be absent.

#### Scenario: No git
- **WHEN** git is not installed or the directory is not in a repository
- **THEN** the tree is shown without marks and without an error

### Requirement: File actions
The browser SHALL provide: open file (Enter) via the `files.open` sidebar action handled by `saysdf.viewer` in a split next to the origin pane; open directory as cwd of a new pane (`o`, action `files.cd_pane`); copy path (`y`, action `files.copy_path`); create file/directory (`n`/`N`), rename (`r`) via `form.rs`. The browser MUST NOT execute files or open them with an external program.

#### Scenario: Open file
- **WHEN** the user presses Enter on `src/main.rs`
- **THEN** the built-in viewer opens that file next to the origin pane

#### Scenario: Executable file
- **WHEN** the user presses Enter on a `.exe`/`.sh`/`.bat` file
- **THEN** the file is opened in the viewer as text/hex but is not executed

### Requirement: Action validation
Actions sent to the GUI MUST use the trusted channel of `tool-sidebar`. The GUI validators MUST check that the path exists and lies under the browser root it was opened for.

#### Scenario: Path outside root
- **WHEN** a `files.open` action with a path outside the root arrives on the channel
- **THEN** the GUI rejects the action

### Requirement: Safe delete
Delete (`d`) SHALL always ask for confirmation showing the path. A non-empty directory MUST be deleted only after the user types its name exactly; symlinks MUST be removed as links without touching the target. Rename and create MUST NOT overwrite an existing entry.

#### Scenario: Non-empty directory
- **WHEN** the user deletes a non-empty directory `build`
- **THEN** typing `build` is required; any other input cancels the deletion

#### Scenario: Rename onto existing
- **WHEN** the new name already exists
- **THEN** an error is shown and nothing is overwritten

### Requirement: Follow active pane cwd
While the root is not pinned (`p` toggles), the browser SHALL change its root to the cwd of the tab's active non-sidebar pane (reported by OSC 7) within one second of the change. The GUI side MUST pass the cwd through a file in `runtime_dir()` (written only when the cwd changes), and the browser MUST watch it with the shared `watch.rs`; keystrokes MUST NOT be injected into the browser pane.

#### Scenario: cd in shell
- **WHEN** the user runs `cd ../other` in the active pane
- **THEN** the browser root changes to `other` and the expanded directories are reset

#### Scenario: Pinned
- **WHEN** the root is pinned
- **THEN** a cwd change does not change the root

### Requirement: Hotkey and entry points
The module `saysdf.files` SHALL bind Ctrl+Shift+B to toggle the file browser in the sidebar and add the palette/menu entry "Files". Without the sidebar (disabled) it MUST open the browser in a regular split on the right.

#### Scenario: Toggle
- **WHEN** the user presses Ctrl+Shift+B twice (the second time inside the browser)
- **THEN** the browser opens and then closes
