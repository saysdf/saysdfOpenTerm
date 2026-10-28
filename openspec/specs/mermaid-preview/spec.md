# mermaid-preview Specification

## Purpose
Render Mermaid flowcharts and sequence diagrams as character graphics in the terminal, with an ASCII fallback. Unsupported diagram types and errors are reported without crashing, and output can be scrolled and zoomed.

## Requirements

### Requirement: Render Mermaid as character graphics
The terminal SHALL render `.mmd`/`.mermaid` files and `mermaid` blocks in Markdown as character graphics (Unicode box-drawing) directly in the pane text, without a browser, Node, or images, on Windows, Linux, and macOS. Rendering MUST NOT access the network or execute code from the diagram.

#### Scenario: Open a file
- **WHEN** the user opens `diagram.mmd` containing `flowchart TD`
- **THEN** the diagram is drawn in the split with boxes and arrows

### Requirement: ASCII fallback
The renderer SHALL support an ASCII mode (only `+ - | > < v ^` and printable ASCII), enabled by a setting or a flag.

#### Scenario: ASCII mode
- **WHEN** ASCII mode is enabled
- **THEN** the diagram is drawn using ASCII characters only

### Requirement: Flowchart support
The renderer SHALL support flowchart/graph with the directions TD/TB/BT/LR/RL, nodes with the shapes `[]`, `()`, `{}`, `(())`, `>]` (a shape may be simplified to a box with distinct corners), node labels, the edges `-->`, `---`, `-.->`, `==>` with labels, chains `A --> B --> C`, `&`, subgraph (a box around the group), and `%%` comments. Cycles MUST be drawn without hanging.

#### Scenario: Simple graph with a cycle
- **WHEN** the diagram is `flowchart LR; A[Start] --> B{Ok?}; B -- yes --> C; B -- no --> A`
- **THEN** nodes A, B, C run left to right, the edge is labeled `yes`, and the back edge to A is drawn

#### Scenario: Unsupported construct
- **WHEN** a flowchart contains `click`, `style`, or `classDef`
- **THEN** they are ignored and the rest of the diagram is drawn

### Requirement: Sequence diagram support
The renderer SHALL support sequenceDiagram: `participant`/`actor` with aliases, the messages `->>`, `-->>`, `->`, `-->`, `-x`, labels, `Note left of/right of/over`, `autonumber`, and `loop`/`alt`/`else`/`opt` blocks drawn as boxes.

#### Scenario: Two participants
- **WHEN** `sequenceDiagram; Alice->>Bob: Hi; Bob-->>Alice: Hello`
- **THEN** two columns with lifelines, an arrow Alice→Bob labeled `Hi`, and a dashed arrow back

### Requirement: Unsupported types and errors
Unsupported diagram types (gantt, pie, mindmap, class, state, etc.) and diagrams with a parse error SHALL be shown as highlighted source with a note giving the reason (type not supported, or the error line).

#### Scenario: Gantt
- **WHEN** a `gantt` block is opened
- **THEN** the highlighted source is shown with the note "gantt type is not supported yet"

#### Scenario: Syntax error
- **WHEN** a flowchart contains a line that cannot be parsed
- **THEN** the source and the number of the line with the error are shown

### Requirement: Scroll without wrapping
A diagram MUST NOT be wrapped or cropped: if it is wider or taller than the pane, it scrolls horizontally and vertically (keys, wheel, Shift+wheel).

#### Scenario: Wide diagram
- **WHEN** the diagram is wider than the pane
- **THEN** the right part is reachable by horizontal scrolling, and lines are not wrapped

### Requirement: Zoom
The renderer SHALL support zoom as a change of layout density: at least three levels — compact (minimal padding, labels shortened with `…`), normal, and spacious (large padding, full labels). Zoom is changed with the hotkeys `+`/`-`/`0` and the wheel with a modifier; when zoom changes, the same node (or the nearest one) MUST remain at the center of the view.

#### Scenario: Zoom out
- **WHEN** the user presses `-`
- **THEN** the diagram becomes narrower and shorter, and long labels are shortened with `…`

#### Scenario: Reset
- **WHEN** the user presses `0`
- **THEN** the normal level is restored

### Requirement: Theme colors
The renderer SHALL use the terminal palette colors (ANSI colors) rather than fixed RGB, so that the diagram is readable in both light and dark themes.

#### Scenario: Light theme
- **WHEN** the terminal has a light color scheme
- **THEN** the diagram's boxes and text contrast with the background
