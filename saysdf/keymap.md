# saysdfTerm hotkey registry

A single list of the keyboard and mouse bindings of all fork modules. Any change that adds
a binding first checks this file and records the binding here — otherwise there will be a conflict.
Upstream WezTerm bindings (Ctrl+Shift+P — palette, Ctrl+Shift+Space — QuickSelect,
Ctrl+Shift+L — debug overlay, etc.) must be taken into account too: `saysdfTerm-cli show-keys`.

Keys are named by their physical position on a US layout: Ctrl+Shift+E is the E key (the У key on a Russian layout) on any layout.

| Binding | Action | Change / module |
|---|---|---|
| Enter | send | — (default behavior) |
| Shift+Enter, Ctrl+Enter | newline (`\x1b\r`) | multiline-input / `saysdf.multiline` |
| Alt+Enter (in the shell: ESC CR from the pty) | newline in PowerShell/bash/zsh/fish; pressing Alt+Enter in the GUI is upstream `ToggleFullScreen`, unchanged | multiline-input / `shell-integration` scripts |
| Alt+Shift+Up / Down | previous / next prompt (Ctrl+Shift+arrows are upstream, pane navigation) | shell-integration / `saysdf.shell` |
| Ctrl+Shift+O | copy the output of the last command | shell-integration / `saysdf.shell` |
| triple click | select a zone (command output) | shell-integration / `saysdf.shell` |
| Ctrl+Shift+W, tab close button | close the tab (with confirmation) | close-confirmation / `saysdf.close` |
| Alt+F4, window close button | close the window (with confirmation) | close-confirmation |
| Ctrl+Shift+E | rename the tab (Ctrl+Shift+R is upstream, reload config) | tab-management / `saysdf.tabs` |
| double click on a tab | rename the tab | tab-management |
| Ctrl+click on a path | open in the built-in viewer | clickable-file-paths / `saysdf.paths` |
| Ctrl+Shift+click on a path | open in an external editor | clickable-file-paths |
| Ctrl+Shift+G | open a file in the built-in viewer (prompts for a path) | code-viewer / `saysdf.viewer` |
| Ctrl+Shift+H | list of SSH connections | ssh-connections / `saysdf.ssh` |
| Ctrl+Shift+I | load metrics in the status bar (on/off) | system-monitor / `saysdf.monitor` |
| Ctrl+Shift+J | load and process monitor in a split | system-monitor |
| Ctrl+Shift+A | list of workspaces | project-workspaces / `saysdf.workspaces` |
| Ctrl+Shift+Space | quick-select: copy | quick-select (upstream) |
| Ctrl+Alt+Space | quick-select: open | quick-select / `saysdf.quickselect` |
| Ctrl+Shift+S | Git menu (commit, push, pull, checkout, submodules, diff) | git-integration / `saysdf.git` |
| Ctrl+Shift+D | `saysdf db` database viewer in a split (list of connections; Ctrl+Shift+D is free upstream — checked against `wezterm-gui/src/commands.rs`) | database-viewer / `saysdf.db` |
| Ctrl+Shift+B | `saysdf files` file browser in the side panel (toggle; not taken upstream — checked against `wezterm-gui/src/commands.rs`) | file-browser / `saysdf.files` |
| Ctrl+Shift+Y | show/hide the side panel (last tool; not taken upstream) | tool-sidebar / `saysdf.sidebar` |
| Ctrl+Shift+Z | zoom/unzoom the pane — upstream `TogglePaneZoomState`, unchanged | pane-header (reused) |
| click on `×` / `□` / `▣` in a pane header | close the pane (with confirmation per `saysdf.close`) / maximize / restore | pane-header / `saysdf.panes` (event `saysdf-pane-header-click`) |
| F9 | open the menu from the keyboard (arrows, Enter, Esc); if a TUI is in the foreground (alt-screen or a process from the list: vim, mc, htop, claude, saysdf…), F9 goes to the program | menu-bar / `saysdf.menu` |
| Ctrl+Shift+F9 | always open the menu, even over a TUI program (not taken upstream — `wezterm-gui/src/commands.rs`) | menu-bar / `saysdf.menu` |
| F1 | full help `saysdf help`; over a TUI, F1 goes to the program | startup-help / `saysdf.help` |
| click on an item of the top menu bar / on a status-bar hint | open the menu / run the hint's action | menu-bar, hotkey-statusbar |
| Esc, click on the banner | close the startup banner (works only while the banner is shown) | startup-help |

## Keyboard layout

Hotkeys do not depend on the keyboard layout (change layout-independent-keys): `saysdf.setup({ keys = { layout_independent = true } })`
(true by default) sets `config.key_map_preference = "Physical"` unless the user set it themselves —
bindings are matched by the physical key (the US-layout position), so on a Russian layout
Ctrl+Shift+У works as Ctrl+Shift+E. `layout_independent = false` or `keys = false` gives upstream behavior (matching by character).
Side effect: your own bindings on punctuation characters are matched by the US-layout position.

Confirmations ("Press Y to confirm, N or Esc to cancel") wait for the physical Y key — on a Russian layout that is `н`.
Items of the F9 menu are invoked by their `[X]` accelerator — also by physical key.

Fallback in TUI utilities: single-letter commands and answers by physical key work only if the terminal
reports the physical key: the base-layout key of the kitty keyboard protocol in saysdfTerm on Linux/macOS,
the virtual key code of the console input record on Windows. In other terminals, over ssh from a host
without saysdfTerm, with `enable_kitty_keyboard = false` (Linux/macOS) and with win32-input-mode off (Windows),
commands accept only the typed US letter: on a Russian layout, switch the layout.

`saysdf.keys` enables `config.enable_kitty_keyboard = true` unless the user set it themselves.
Programs that request kitty keyboard themselves (neovim, helix, kakoune) receive keys in its
encoding (Tab ≠ Ctrl+I, Esc ≠ Ctrl+[, release events); bash, pwsh, `mc`, `less` get the previous one.
If a program recognizes combinations incorrectly, set `config.enable_kitty_keyboard = false` (the fork's utilities
then work via the fallback).

The remaining F-keys (F2–F8, F10–F12) are not intercepted by the fork — they belong to programs.

The theme menu (theme-switcher / `saysdf.theme`: "View → Theme…", "Light/Dark", "Follow system theme") has
no hotkey; use the menu and the command palette.

Inside the `saysdf` TUI utilities (view, md, mermaid, diff, db, mon), their own keys act
only within their pane and are not part of this registry.

### Viewer keys (text annotations; `saysdf view` / `md`)

TUI-internal keys of the viewer panes, not GUI bindings (no new global hotkey). Also listed in the
F9 menu / palette entry "Help → Viewer keys". Every command is a Ctrl combination matched by the
physical key on any layout (text-annotations D13). The last two rows (`s`, `]]`/`[[`) are
the current markdown keys: they move to Ctrl+S and `]`/`[` in step D-1 (the Lua help text already
shows the target keys).

| Key | Action |
|---|---|
| Shift+arrows, Shift+Home/End, Shift+PgUp/PgDn, mouse drag | select from the caret (`saysdf view`) |
| Ctrl+Space | start / clear selection mode at the caret; then arrows move the caret, Ctrl+Left/Right by word, Ctrl+Home/End to file start/end |
| Ctrl+R / Ctrl+D / Ctrl+N / Ctrl+K | on a selection: substitute / delete (Enter confirms) / insert after / comment |
| Ctrl+E / Ctrl+X | amend / remove the annotation under the caret |
| Alt+Down / Alt+Up | next / previous annotation |
| Ctrl+W | `saysdf view`: soft wrap on/off (on by default for `.md`, `.markdown`, `.txt`) |
| Ctrl+F | `saysdf view`: diff of the file against `HEAD` (`saysdf diff`); back to the viewer on close |
| Ctrl+L | `saysdf view`: annotation list of the project folder (`saysdf annotations`); Enter opens the chosen mark here, Esc returns |
| Ctrl+G | `saysdf view`: hand the file to the agent pane (a phrase with the path is pasted, no Enter); only in a viewer opened by saysdfTerm |
| Esc | clear the selection / cancel the editor (never quits from a sub-mode) |
| `r`, `/`, `n`, `q`, arrows | pager: reload, search, next match, quit, scroll (physical keys) |
| `s` (until D-1: Ctrl+S) | markdown: source / rendered view |
| `]]` / `[[` (until D-1: `]` / `[`) | markdown: next / previous heading. **Breaking:** these were single `]` / `[` before; `diff`, `merge`, `db` keep the single keys |
| Enter / Ctrl+J, Alt+Enter / Esc | input editor: save / new line / cancel (Ctrl+J is the reliable newline: Alt+Enter is upstream fullscreen in the GUI) |

Palette / menu entries (`saysdf.annotations`): "Annotations in project…", "Hand annotations to the
agent" (Tools), "Viewer keys" (Help).
