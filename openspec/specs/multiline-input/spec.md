# multiline-input Specification

## Purpose
Defines the behavior of Enter, Shift+Enter, and Ctrl+Enter during input in saysdfTerm: Enter submits the input, while Shift+Enter and Ctrl+Enter insert a line break in shells and in TUI programs such as Claude Code.

## Requirements

### Requirement: Enter submits input
In the default config, Enter without modifiers SHALL be passed to the program as a plain Enter (CR), unchanged from upstream.

#### Scenario: Run a command
- **WHEN** the user types `echo hi` in PowerShell and presses Enter
- **THEN** the command is executed

### Requirement: Unambiguous sequence for Shift+Enter and Ctrl+Enter
In the default config, Shift+Enter and Ctrl+Enter SHALL send the program the same sequence, ESC CR (`\x1b\r`, "Alt+Enter"), which is distinguishable from Enter, on Windows, Linux, and macOS.

#### Scenario: Bytes at the program input (Unix)
- **WHEN** `cat -v` is running in bash and the user presses Shift+Enter
- **THEN** the program receives `^[` and CR, not a lone CR

#### Scenario: Key event on Windows
- **WHEN** `[Console]::ReadKey($true)` is running in PowerShell and the user presses Ctrl+Enter
- **THEN** the program receives Enter with the Alt modifier

### Requirement: Line break in shells
With shell integration connected, Shift+Enter and Ctrl+Enter SHALL insert a line break into the command being edited without executing it, in PowerShell (PSReadLine), bash (readline), zsh (zle), and fish; a subsequent Enter MUST execute the whole multi-line command.

#### Scenario: Multi-line command in PowerShell
- **WHEN** the user types `echo 1`, presses Shift+Enter, types `echo 2`, and presses Enter
- **THEN** both lines are executed and `1` and `2` are printed

#### Scenario: Multi-line command in bash
- **WHEN** the user types `echo 1`, presses Ctrl+Enter, types `echo 2`, and presses Enter
- **THEN** `1` and `2` are printed

### Requirement: Line break in Claude Code
Shift+Enter and Ctrl+Enter SHALL insert a line break in the Claude Code input field without sending the message, with no additional Claude Code setup (`/terminal-setup` is not required).

#### Scenario: Two-line message
- **WHEN** in Claude Code the user types text, presses Shift+Enter, and continues typing
- **THEN** the input field contains two lines, and the message is not sent until Enter is pressed

### Requirement: Disabling and alternative mode
The behavior SHALL be disabled via `require("saysdf").setup({ multiline_input = false })`; Shift+Enter and Ctrl+Enter are then encoded as in upstream, and the integration scripts MUST NOT add multi-line input bindings.

#### Scenario: Disable
- **WHEN** the config sets `multiline_input = false`
- **THEN** there are no Shift+Enter and Ctrl+Enter bindings in the terminal key list, and no Alt+Enter handler is added in PowerShell

### Requirement: Shell bindings do not overwrite user bindings
The integration scripts SHALL add only an Alt+Enter (ESC CR) binding for inserting a line break and MUST NOT change it if the user has already assigned their own action to Alt+Enter.

#### Scenario: User binding preserved
- **WHEN** in the PowerShell profile the user has assigned their own function to Alt+Enter
- **THEN** after the integration is connected, Alt+Enter runs the user's function
