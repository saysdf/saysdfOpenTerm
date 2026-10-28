# merge-tool Specification

## Purpose
Resolve Git merge conflicts in a three-way view inside a split pane. The user picks resolutions per conflict, saves the result and marks the file resolved, and the tool integrates with git mergetool.

## Requirements

### Requirement: Three-way merge view
The `saysdf merge` utility SHALL show a conflicting file in three panes (ours, base, theirs) plus a result pane, aligning the conflict blocks horizontally, with syntax highlighting by the shared engine.

#### Scenario: Launch as a mergetool
- **WHEN** git invokes `saysdf merge $BASE $LOCAL $REMOTE $MERGED`
- **THEN** the ours/base/theirs panes and the result open, with the cursor on the first conflict

#### Scenario: File with conflict markers
- **WHEN** the user runs `saysdf merge <file>` on a file with the markers `<<<<<<<`, `|||||||`, `=======`, `>>>>>>>`
- **THEN** the utility reconstructs the sides from the markers (base only if there is a diff3/zdiff3 section) and shows the same view

### Requirement: Conflict resolution actions
For each conflict the user SHALL be able to accept ours, accept theirs, accept both sides in either order, or edit the result manually; the utility SHALL show the number of unresolved conflicts.

#### Scenario: Accept theirs
- **WHEN** on a conflict the user presses "accept theirs"
- **THEN** the block in the result is replaced with the theirs version, and the unresolved counter decreases

#### Scenario: Manual edit
- **WHEN** the user opens a result block for editing
- **THEN** the block opens in an external editor (`$EDITOR` or an option) and is considered resolved after saving

### Requirement: Save and mark resolved
The utility SHALL write the result to `$MERGED` only on an explicit save and MUST warn when saving with unresolved conflicts. The exit code SHALL tell git whether the file is resolved (0 means resolved, anything else is nonzero).

#### Scenario: All conflicts resolved
- **WHEN** the user saves the result with no unresolved conflicts and exits
- **THEN** the file is written, the utility exits with code 0, and git marks the file as resolved

#### Scenario: Exit without saving
- **WHEN** the user exits without saving
- **THEN** `$MERGED` is not modified and the exit code is nonzero

#### Scenario: Save with unresolved conflicts
- **WHEN** the user saves while conflicts remain
- **THEN** the utility asks for confirmation and writes the unresolved blocks with conflict markers

### Requirement: Git mergetool integration
The utility SHALL work as `git mergetool --tool=saysdf`; the git configuration (`mergetool.saysdf.cmd`, `mergetool.saysdf.trustExitCode=true`) SHALL be written only on an explicit command with confirmation.

#### Scenario: Mergetool setup
- **WHEN** the user runs `saysdf merge --install-git-tool` and confirms
- **THEN** the mergetool keys are written to the global git config; nothing is written without confirmation
