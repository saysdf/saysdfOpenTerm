# shell-integration Specification

## Purpose
Gives the terminal and the fork's Lua modules reliable information about the shell's prompt, input, output, exit code, command duration, and current folder — a single source for tabs, git, notifications, and navigation.

## Requirements

### Requirement: Supported shells
saysdfTerm SHALL ship integration scripts for Windows PowerShell 5.1, PowerShell 7+ (pwsh), bash (4+), zsh, and fish on Windows, Linux, and macOS (where the shell is available). No integration is shipped for cmd.exe, and the terminal MUST work with it without errors.

#### Scenario: A script exists for each shell
- **WHEN** a built saysdfTerm is installed
- **THEN** integration scripts for PowerShell, bash, zsh, and fish are located next to it

#### Scenario: cmd.exe without integration
- **WHEN** the user launches cmd.exe in a tab
- **THEN** the shell works as usual, with no extra output or errors

### Requirement: OSC 133 semantic zone markup
A connected integration SHALL mark up each command cycle with OSC 133 sequences: prompt start (A), input start (B), output start (C), and command end (D) with the exit code.

#### Scenario: Zones after a command
- **WHEN** the command `Get-Date` is run in PowerShell with the integration
- **THEN** the terminal knows the Prompt, Input, and Output zones of that command, and the exit code is 0

#### Scenario: Nonzero exit code
- **WHEN** the command `false` is run in bash
- **THEN** the command end is marked with exit code 1

### Requirement: Current folder via OSC 7
The integration SHALL report the shell's current folder with an OSC 7 sequence every time the prompt is shown, as a `file://` URL from which the terminal obtains a correct path on the current OS, including Windows paths with a drive letter and spaces.

#### Scenario: Folder change in PowerShell
- **WHEN** the user runs `cd 'C:\Program Files'`
- **THEN** the pane's current folder is `C:\Program Files`

#### Scenario: New tab inherits the folder
- **WHEN** after `cd /tmp` in zsh the user opens a new tab
- **THEN** the new tab starts in `/tmp`

### Requirement: Exit code and duration for other modules
The terminal SHALL make the exit code and duration of each pane's last command available to the fork's Lua modules through a documented API of the integration module and a command-completion event.

#### Scenario: Completion event
- **WHEN** the command `sleep 2` finishes in a pane with exit code 0
- **THEN** the modules receive an event with exit code 0 and a duration of at least 2000 ms

### Requirement: Automatic connection
When the terminal itself launches a supported shell (the default shell or one from the built-in launch list), the integration SHALL connect without editing the user's profiles and rc files; the user's profiles and rc files MUST be loaded as usual.

#### Scenario: User profile preserved
- **WHEN** the user's PowerShell profile defines a custom `prompt` and aliases
- **THEN** their prompt and aliases work in a new tab, and OSC 133 markup is present

#### Scenario: User files unchanged
- **WHEN** the terminal has launched a shell with the integration
- **THEN** no file in the user's home folder is created or modified

### Requirement: Disabling the integration
It SHALL be possible to disable the integration entirely via `require("saysdf").setup({ shell = false })` and in parts (zones, folder, user variables) via environment variables. A disabled integration MUST NOT emit any control sequence.

#### Scenario: Full disable
- **WHEN** the config sets `require("saysdf").setup({ shell = false })`
- **THEN** new tabs launch the shell without the integration scripts

#### Scenario: Disable OSC 7 only
- **WHEN** the folder-disabling variable is set before the shell starts
- **THEN** OSC 133 is emitted and OSC 7 is not

### Requirement: Manual connection
The scripts SHALL support manual connection (dot-source/source from a profile or on a remote machine) and MUST be idempotent: connecting again does not duplicate the markup.

#### Scenario: Repeated source
- **WHEN** the integration script is connected twice in the same session
- **THEN** each zone is marked exactly once

### Requirement: No interference with shell behavior
The scripts SHALL change only the markup output and, for multi-line input, the key bindings described in `multiline-input`. The exit code (`$?`, `$LASTEXITCODE`), history, and the output of the user's prompt MUST be preserved.

#### Scenario: Exit code is not lost
- **WHEN** an external program is run in PowerShell with exit code 3 and the prompt is then shown
- **THEN** `$LASTEXITCODE` equals 3

### Requirement: Jumping between prompts
The default config SHALL have key bindings for jumping to the previous and next prompt in the pane's history.

#### Scenario: Jump to the previous prompt
- **WHEN** after several commands the user presses the "previous prompt" binding
- **THEN** the view scrolls so that the previous command's prompt is visible at the top

### Requirement: Copying command output
The user SHALL be able to copy the output of the last command with a single key binding and to select the output of any command with the mouse; copying MUST happen only on a user action.

#### Scenario: Copy the last command's output
- **WHEN** after `git status` the user presses the "copy output" binding
- **THEN** the clipboard contains exactly the output of `git status`, without the prompt and the typed command

#### Scenario: Select output with the mouse
- **WHEN** the user triple-clicks on a command's output
- **THEN** the entire output zone of that command is selected
