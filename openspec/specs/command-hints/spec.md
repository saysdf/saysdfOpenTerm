# command-hints Specification

## Purpose
Offer command-line completion and inline history suggestions in the shell, driven by bundled CLI specs. Completion generators are read-only and hints can be disabled per program.

## Requirements

### Requirement: Completion from bundled CLI specs
The terminal SHALL ship completion specs for popular CLIs (at least git, cargo, docker, kubectl, npm) as files in the repository, embedded into `saysdf`. `saysdf hints complete --shell <shell> -- <words...>` SHALL return candidates (subcommands, flags with descriptions, paths, git branches) for the given command line. Specs MUST NOT be downloaded from the network.

#### Scenario: Subcommand flags with descriptions
- **WHEN** `saysdf hints complete --shell pwsh -- git commit --am` is invoked
- **THEN** the output contains `--amend` with its description from the spec

#### Scenario: Unknown command
- **WHEN** there is no spec for the command
- **THEN** the output is empty, the exit code is 0, and the shell falls back to its own completion

### Requirement: Shell integration of completions
The `saysdf.hints` module SHALL provide completer scripts that register `saysdf hints complete` as the native completer for spec-covered commands in PowerShell (`Register-ArgumentCompleter -Native`, list shown via PSReadLine `MenuComplete`), bash (`complete -C`), zsh (`compdef` function) and fish (`complete -c ... -a`). Existing user completions for a command MUST take precedence unless the user opts in to override.

#### Scenario: PowerShell menu
- **WHEN** in pwsh with the scripts loaded the user types `cargo b` and presses Tab
- **THEN** a menu with `bench` and `build` and their descriptions is shown

### Requirement: Inline history suggestion
The module SHALL enable a grey inline suggestion from history, accepted with Right Arrow, using the shell's native mechanism: PSReadLine `PredictionSource History` (PowerShell 7.2+ / PSReadLine 2.1+), fish built-in autosuggestions, zsh-autosuggestions when installed. When the mechanism is unavailable (e.g. bash, old PSReadLine) the module MUST report that once and continue without it.

#### Scenario: Missing zsh-autosuggestions
- **WHEN** zsh-autosuggestions is not installed in zsh
- **THEN** flag hints work, and `saysdf hints doctor` reports that the inline suggestion tail is unavailable

### Requirement: Read-only generators
Dynamic candidates (paths, git branches) MUST be read-only: listing directories and running `git for-each-ref` with a timeout of at most 500 ms in the current directory. Completion MUST NOT execute the command being typed or any program from the spec other than the allowlisted generators.

#### Scenario: Slow repository
- **WHEN** `git for-each-ref` does not finish within 500 ms
- **THEN** branches are not shown, and the other hints are returned without delay

### Requirement: Disable for programs
Completer scripts SHALL be installed only into interactive shells started by saysdfTerm and SHALL be disableable per command via `setup({ hints = { disabled = {...} } })`. Full-screen programs (vim, Claude Code) MUST NOT be affected because hints run inside the shell line editor only.

#### Scenario: Disabled command
- **WHEN** `kubectl` is listed in `disabled`
- **THEN** only the shell's built-in completion is used for `kubectl`
