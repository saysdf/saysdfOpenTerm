# markdown-preview Specification

## Purpose
Render a Markdown file as formatted text in a split pane with navigation, search and reload. Control characters are escaped, and images, links and Mermaid blocks are handled without executing content.

## Requirements

### Requirement: Open Markdown preview in split
The terminal SHALL open a `.md`/`.markdown` file in rendered form in a split next to the current pane on a path click, a hotkey or a Lua call. The preview MUST work without Node, a browser or external programs on Windows, Linux and macOS.

#### Scenario: Opening by click
- **WHEN** the user opens the path `README.md`
- **THEN** a split with the rendered document appears next to the current pane

#### Scenario: Opening at a line
- **WHEN** `docs/x.md:120` is opened
- **THEN** the preview is scrolled to the block that corresponds to line 120 of the source

### Requirement: Rendered elements
The preview SHALL render: headings (levels distinguishable), paragraphs wrapped to the pane width, bold, italic, strikethrough and inline code, bulleted, numbered and nested lists, task lists, blockquotes, horizontal rules, GFM tables with alignment, code blocks with language highlighting, links. Unsupported elements (raw HTML, footnotes) MUST be shown as text rather than dropped.

#### Scenario: Table
- **WHEN** the document contains a GFM table with column alignment
- **THEN** it is drawn with a border and the columns are aligned as specified

#### Scenario: Code block
- **WHEN** the document contains a code block with the language `rust`
- **THEN** the code is highlighted as Rust and is not wrapped; a wide block scrolls horizontally

#### Scenario: Raw HTML
- **WHEN** the document contains `<details>` or `<script>`
- **THEN** the HTML is shown as text and nothing is executed

### Requirement: Mermaid blocks
Code blocks with the language `mermaid` SHALL be rendered as diagrams according to the rules of the `mermaid-preview` capability; on a parse error the block's source MUST be shown with an error note.

#### Scenario: Flowchart in a document
- **WHEN** the document contains a `mermaid` block with `flowchart LR`
- **THEN** a character-based diagram is drawn in its place

### Requirement: Images
Local images SHALL be shown via the terminal's graphics protocol if available, otherwise as a caption with the alt text and path. Remote images (http/https) MUST NOT be loaded unless the user explicitly enables it in the settings.

#### Scenario: Remote image
- **WHEN** the document contains `![logo](https://example.com/a.png)` and loading is not enabled
- **THEN** a caption with the address is shown and no network request is made

#### Scenario: Local image
- **WHEN** the document contains `![](img/a.png)` and the file exists
- **THEN** the image is shown in the preview, or a caption if the protocol is unavailable

### Requirement: Links
Links SHALL be displayed highlighted with the address visible; following a link MUST happen only on an explicit user action. A relative link to a `.md` file SHALL open in the same preview, and an `#...` anchor SHALL scroll to the heading.

#### Scenario: Link to another document
- **WHEN** the user selects the link `docs/guide.md` and presses Enter
- **THEN** the preview opens `docs/guide.md`; Backspace returns to the previous document

### Requirement: Escaping control characters
The preview MUST NOT output control characters and escape sequences from the document to the terminal; they SHALL be displayed as visible placeholders.

#### Scenario: Escape in text
- **WHEN** the document contains an ESC byte
- **THEN** it is shown as `␛` and the terminal state does not change

### Requirement: Navigation, search and reload
The preview SHALL support scrolling, search with navigation between matches, jumping between headings, and closing with a single key; when the file changes on disk the preview SHALL be redrawn, preserving the position.

#### Scenario: Auto-refresh
- **WHEN** the file is saved in an editor
- **THEN** the preview updates within a second and the position is preserved

#### Scenario: Resize
- **WHEN** the pane width changes
- **THEN** paragraphs are re-laid out to the new width
