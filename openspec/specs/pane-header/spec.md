# pane-header Specification

## Purpose
Show a header row above each pane with its title and context, plus buttons to close (with confirmation), zoom and restore the pane. The header can be turned off in the config.

## Requirements

### Requirement: Pane header row
The terminal SHALL draw a one-row header above each pane of a tab when `panes.header` is `"always"`, or when it is `"splits"` (default) and the tab has more than one pane or a zoomed pane. The pane's terminal area MUST be reduced by exactly one row and the pty MUST receive the reduced size. With `"never"` the layout MUST be identical to upstream.

#### Scenario: Two splits
- **WHEN** a tab is split into two panes and `header = "splits"`
- **THEN** each pane has a header row above it, and `stty size` in each pane shows one row fewer than its rectangle

#### Scenario: Single pane
- **WHEN** a tab has one pane and `header = "splits"`
- **THEN** there is no header, and the pty size is the same as upstream

#### Scenario: Too small pane
- **WHEN** a pane has fewer than two rows
- **THEN** no header is drawn and no row is reserved

### Requirement: Header content
The header SHALL show the pane title (or the cwd basename when the title is empty), truncated to the width by cells, and the buttons close `×` and zoom `□` (or unzoom `▣` when zoomed) at the right edge. Text MUST pass `sanitize_text` (no control/bidi characters). The active pane's header MUST be visually distinguishable from inactive ones. Rendering MUST NOT call Lua per frame.

#### Scenario: Hostile title
- **WHEN** a program in the pane sets a title containing control and bidi characters
- **THEN** they are stripped, and the width does not exceed the pane width

#### Scenario: Narrow pane
- **WHEN** the width is insufficient for the text and the buttons
- **THEN** the text is truncated first and the buttons remain; if there is not enough room even for the buttons, only `×` is shown

### Requirement: Header buttons
A left click on `×` SHALL close that pane following `saysdf.close` rules (confirmation unless the mode/skip list allows closing silently). A click on `□`/`▣` SHALL toggle zoom of that pane (activating it first). A click on the header text SHALL activate the pane. Clicks MUST be resolved against the layout of the frame that was drawn; a stale hit MUST be ignored.

#### Scenario: Close with confirmation
- **WHEN** the user clicks `×` above a pane with a running process and the mode is `always`
- **THEN** the upstream pane-close confirmation is shown, and the other panes are not affected

#### Scenario: Zoom and back
- **WHEN** the user clicks `□` above an inactive pane
- **THEN** it becomes active and is zoomed to fill the tab; the header shows `▣`; a click on `▣` restores the layout

#### Scenario: Layout changed between frames
- **WHEN** a pane was closed after drawing but before the click was handled
- **THEN** the click is ignored without an error

### Requirement: Header option
The module SHALL accept `saysdf.setup({ panes = { header = "splits" | "always" | "never" } })`; an unknown value MUST log a warning and fall back to `"splits"`. `panes = false` MUST disable the module (upstream layout).

#### Scenario: Disabled
- **WHEN** `panes = false`
- **THEN** there are no headers, and pane sizes are the same as upstream
