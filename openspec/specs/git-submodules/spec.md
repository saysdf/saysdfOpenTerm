# git-submodules Specification

## Purpose
List the status of Git submodules and let the user update them and navigate into them. Warn about submodule commits that have not been pushed.

## Requirements

### Requirement: Submodule status list
The module SHALL show a list of the repository's submodules with the status of each: path, current commit, difference from the commit recorded in the superproject, presence of local changes, and whether it is uninitialized. The status bar SHALL show an indicator if at least one submodule is not in order.

#### Scenario: Submodule has moved
- **WHEN** a submodule's commit differs from the one recorded in the superproject
- **THEN** the submodule is marked as modified in the list, and the submodule indicator is visible in the status bar

#### Scenario: Uninitialized submodule
- **WHEN** a submodule is declared in `.gitmodules` but not initialized
- **THEN** it is marked as uninitialized in the list

### Requirement: Submodule update
The module SHALL run `git submodule update --init --recursive` only on an explicit user action, in an operation pane with visible output.

#### Scenario: Updating submodules
- **WHEN** the user selects "Update submodules"
- **THEN** `git submodule update --init --recursive` runs in a split, followed by a notification and a status refresh on completion

### Requirement: Navigate into submodule
The module SHALL open a new pane in the directory of the selected submodule.

#### Scenario: Going into a submodule
- **WHEN** the user selects a submodule in the list and the "open" action
- **THEN** a new pane opens with its cwd in the submodule's directory

### Requirement: Unpushed submodule commit warning
Before a push of the superproject, the module SHALL check that the submodule commits referenced by the superproject's HEAD are contained in the submodules' remote-tracking branches, and MUST show a warning in the push confirmation if they are not. The check MUST NOT access the network.

#### Scenario: Submodule commit not pushed
- **WHEN** the superproject references a submodule commit that is not in any of the submodule's remote-tracking branches
- **THEN** the push confirmation contains a warning with the submodule path and SHA; the push runs only after explicit confirmation
