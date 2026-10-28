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
sends the base-layout key (kitty keyboard protocol in saysdfTerm on Linux/macOS). In other terminals,
over ssh from a host without saysdfTerm, on Windows, and with `enable_kitty_keyboard = false`, commands accept only
the typed US letter: on a Russian layout, switch the layout.

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
