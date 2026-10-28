# code-viewer Specification

## Purpose
Show a text or source file in a split pane with syntax highlighting, line numbers, navigation and search. The viewer is read-only: it neutralizes control characters, handles large and binary files safely, and hands editing off to an external editor.

## Requirements

### Requirement: Open file in split viewer
On a user action (a click on a `file:line[:col]` path, a hotkey, a Lua call) the terminal SHALL open a file in the built-in viewer in a new split pane next to the current one, positioning the view at the given line and column. The viewer MUST work on Windows, Linux and macOS and MUST NOT open a file without a user action.

#### Scenario: Opening a path with a line number
- **WHEN** the user opens `src/main.rs:42:7` via the viewer module
- **THEN** a split with the file appears to the right of the current pane, line 42 is visible and highlighted, and the cursor is at column 7

#### Scenario: Line beyond the end of the file
- **WHEN** the given line number is greater than the number of lines in the file
- **THEN** the viewer shows the last line and a message in the status line, without an error

#### Scenario: File not found
- **WHEN** the path does not exist or is not readable
- **THEN** the pane shows a clear error message with the path and closes on `q`

#### Scenario: Viewer utility not found
- **WHEN** the viewer binary is missing next to the terminal and in `PATH`
- **THEN** the module shows a notification saying what is missing and no split is created

### Requirement: Syntax highlighting and line numbers
The viewer SHALL highlight syntax by file extension or name, show line numbers, and MUST show the file without highlighting if the language is not recognized.

#### Scenario: Known language
- **WHEN** a `.rs` file is open
- **THEN** keywords, strings and comments are colored, with line numbers on the left

#### Scenario: Unknown language
- **WHEN** a file with an unknown extension is open
- **THEN** the text is shown without highlighting, with line numbers

### Requirement: Navigation and search
The viewer SHALL support vertical and horizontal scrolling (keyboard and mouse wheel), jumping to a line, and substring search with navigation to the next and previous match. Long lines MUST NOT be wrapped.

#### Scenario: Search
- **WHEN** the user presses `/`, types `fn main` and presses Enter
- **THEN** the view jumps to the first match after the current position, matches are highlighted, and `n`/`N` move to the next/previous match

#### Scenario: Horizontal scrolling
- **WHEN** a line is wider than the pane and the user presses the right arrow or Shift+wheel
- **THEN** the view scrolls to the right and the line numbers stay in place

### Requirement: Escaping control characters
The viewer MUST NOT output control characters and escape sequences from the file contents to the terminal: C0 (except tab), DEL, C1 and ESC SHALL be displayed as visible placeholders.

#### Scenario: File with an escape sequence
- **WHEN** the file contains the bytes `ESC ] 52 ; ...` or `ESC [ 2 J`
- **THEN** they are shown as visible characters (for example `␛]52;...`), and the clipboard and the terminal screen are not changed

#### Scenario: Tab
- **WHEN** a line contains a tab
- **THEN** it is expanded to spaces up to the nearest position that is a multiple of the tab width

### Requirement: Auto-reload on file change
The viewer SHALL re-read the file when it changes on disk, preserving the current position and selection if the line still exists.

#### Scenario: File modified
- **WHEN** the open file is overwritten by another program
- **THEN** the view is updated within a second and the scroll position is preserved

#### Scenario: File deleted
- **WHEN** the open file is deleted
- **THEN** the last contents stay on screen and the status reports "file deleted"; when the file reappears, it is re-read

### Requirement: Large and binary files
The viewer MUST NOT load a file into memory in full if it is larger than a threshold (16 MiB by default); a binary file SHALL be shown in a hex view.

#### Scenario: Binary file
- **WHEN** a file with null bytes or invalid UTF-8 in the initial block is open
- **THEN** a hex view (offset, bytes, ASCII) is shown and no highlighting is applied

#### Scenario: Large text file
- **WHEN** a text file larger than the threshold is open
- **THEN** it is shown without highlighting and read on demand, the status reports the limitation, and the interface responds without noticeable delay

### Requirement: Open in external editor
On a dedicated hotkey the viewer SHALL open the current file at the current line in an external editor whose command is set in the module settings; if no command is set, a message MUST be shown rather than launching anything by default.

#### Scenario: Editor configured
- **WHEN** the editor `{ 'code', '-g', '{path}:{line}' }` is set in the settings and the user presses `e`
- **THEN** the editor is launched with the current file and line

#### Scenario: Editor not configured
- **WHEN** no editor is set and the user presses `e`
- **THEN** the status explains how to set an editor; no process is launched

### Requirement: Close viewer
The viewer SHALL close with a single key (`q`, or Esc outside input mode), closing its pane and returning focus to the adjacent one.

#### Scenario: Closing
- **WHEN** the user presses `q`
- **THEN** the viewer pane closes and focus moves to the adjacent pane
