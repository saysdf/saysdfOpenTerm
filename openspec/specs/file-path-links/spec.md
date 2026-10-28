# file-path-links Specification

## Purpose
Recognize file paths in terminal output and open them by click in a split pane suited to the file type. Paths are resolved and checked for existence first, and executables are never run.

## Requirements

### Requirement: Path recognition in terminal output
The terminal SHALL highlight file paths on hover: Unix (absolute, `./`, `../`, `~/`, `src/a.rs`) and Windows (`C:\a`, `C:/a`), with an optional suffix `:line`, `:line:col`, `(line)` or `(line,col)`. A bare name without `/` or `\` SHALL be recognized only together with `:line`. Trailing punctuation MUST NOT be part of the path. Recognition MUST live in a single module `saysdf.paths`, shared by click and quick-select.

#### Scenario: rustc output
- **WHEN** the output contains the line `  --> src/main.rs:10:5`
- **THEN** `src/main.rs:10:5` is highlighted on hover, and parsing yields the path `src/main.rs`, line 10, column 5

#### Scenario: MSVC output
- **WHEN** the output contains `C:\proj\a.cpp(12,5): error C2065`
- **THEN** the path `C:\proj\a.cpp`, line 12, column 5 is recognized

#### Scenario: pytest output with a colon after the line number
- **WHEN** the output contains `tests/test_x.py:12: AssertionError`
- **THEN** the path `tests/test_x.py` and line 12 are recognized, without the trailing `:`

#### Scenario: Bare name without a line
- **WHEN** the output contains the word `README.md` without `/` and without `:line`
- **THEN** it is not highlighted as a path

#### Scenario: A URL is not a path
- **WHEN** the output contains `https://example.com/a/b.rs`
- **THEN** it is handled by the standard URL rule, not as a file path

### Requirement: Relative path resolution
A relative path SHALL be resolved against the working directory of the pane where the click happened: first from OSC 7 (`pane:get_current_working_dir()`), otherwise from the foreground process's directory, if available. A path with `~` SHALL be resolved against the home directory. If the pane's directory is unknown, a relative path MUST NOT be resolved against the terminal process's directory; the user gets a notification.

#### Scenario: Directory from OSC 7
- **WHEN** the shell reported the directory `C:\proj` via OSC 7 and the user clicks `src/a.rs:3`
- **THEN** `C:\proj\src\a.rs` is opened at line 3

#### Scenario: Directory unknown
- **WHEN** there is neither OSC 7 nor process information and the user clicks a relative path
- **THEN** nothing is opened and a notification about the unknown pane directory is shown

### Requirement: Existence check before opening
Before opening, the path SHALL be checked for existence. A nonexistent path MUST NOT be opened and MUST NOT cause a process to be launched; the user gets a short notification (which can be turned off by an option). Highlighting on hover is pattern-based and does not guarantee that the file exists.

#### Scenario: File does not exist
- **WHEN** the user clicks `src/missing.rs:1` and the file does not exist
- **THEN** nothing is opened and a "file not found" notification is shown

### Requirement: Open by file type in a split
Ctrl+click on a path SHALL open the file in a new split according to its type: Markdown — `saysdf md`, Mermaid (`.mmd`, `.mermaid`) — `saysdf mermaid`, images — `saysdfTerm-cli imgcat --hold`, anything else — `saysdf view` at the given line and column, a directory — a shell in that directory. Processes MUST be launched with an argument list, without a shell. If a utility is missing, the user SHALL get a notification saying what is missing.

#### Scenario: Code at a line
- **WHEN** the user Ctrl+clicks `src/a.rs:42:7` and the file exists
- **THEN** a split with `saysdf view` on the file `src/a.rs`, line 42, column 7 opens on the right

#### Scenario: Markdown
- **WHEN** the user Ctrl+clicks `docs/readme.md`
- **THEN** a split with `saysdf md` for `docs/readme.md` opens

#### Scenario: Utility missing
- **WHEN** the `saysdf` binary is neither next to the terminal nor in `PATH`
- **THEN** no split is opened and a notification says that `saysdf` was not found

### Requirement: Executables are never run
Clicking a path MUST NOT run the file itself, whatever its extension. Executable and script files (`.exe`, `.com`, `.bat`, `.cmd`, `.ps1`, `.sh`, `.msi`, `.lnk`, `.app` and any others) SHALL be opened only as text in `saysdf view` or in an external editor. The standard OS file-association opening (ShellExecute, `xdg-open`, `open`) MUST NOT be used for paths.

#### Scenario: Click on a script
- **WHEN** the user Ctrl+clicks `deploy.ps1:3`
- **THEN** `deploy.ps1` is opened as text in `saysdf view` and the script is not executed

### Requirement: External editor action
The module SHALL provide a separate "open in external editor" action (Ctrl+Shift+click by default) with a command from the options, given as an argument list with `{path}`, `{line}`, `{col}` substitution. If no editor is configured, the action SHALL open the file like a regular click and report that no editor is set.

#### Scenario: Editor configured
- **WHEN** the options contain `editor = { "code", "-g", "{path}:{line}:{col}" }` and the user Ctrl+Shift+clicks `src/a.rs:4:2`
- **THEN** `code -g <absolute path>:4:2` is launched

### Requirement: Module setup
The module `saysdf.paths` SHALL be enabled through `require("saysdf").setup({ paths = {...} | false })` and, when enabled, add its own rules to `config.hyperlink_rules` without removing existing ones, as well as an `open-uri` event handler for its own links and for `file://`. Other links (http, mailto, etc.) MUST be handled in the standard way.

#### Scenario: Module disabled
- **WHEN** `setup({ paths = false })`
- **THEN** `hyperlink_rules` and the `open-uri` handlers are the same as without the module
