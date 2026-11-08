# saysdfTerm — help

## What it is

saysdfTerm is a terminal based on WezTerm with a set of built-in modules: a git menu, SSH connections,
a database viewer, a load monitor, a file and Markdown viewer, workspaces, tab management and command
hints in the shell. Everything works in ordinary terminal panes, without separate windows.

## Window layout

- **Top bar — the menu** (File, Tabs, Git, Tools, View, Help). A click on a title or **F9** opens
  the menu; arrows, Enter, Esc. The underlined letter picks an entry quickly.
- **Tab bar** — as in WezTerm; double-click a tab to rename it.
- **Bottom bar — hints** for the current context (shell, git repository, selection, SSH,
  a `saysdf` tool, a database with write access) and the status on the right (git, load). Hints
  are clickable.

## How to use it

- **Ctrl+Shift+P** — command palette: the same commands as in the menu, with search.
- **F9** — menu; if a full-screen program is in the foreground (vim, mc, htop, claude…),
  F9 goes to it. **Ctrl+Shift+F9** always opens the menu.
- **Ctrl+Shift+B** — file browser in the side panel: arrows, Enter — open a file in the
  viewer / expand a folder, Backspace — up, `/` — filter, `.` — hidden files, `o` — folder as the cwd
  of a new pane, `y` — copy the path, `n`/`N` — create a file/folder, `r` — rename,
  `d` — delete (permanently, with confirmation; a non-empty folder — after typing its name),
  `p` — pin the root (otherwise it follows the folder of the active pane).
- **F1** — this help (over a full-screen program F1 goes to it).
- The startup banner closes with Esc, a click or the first input; to turn it off:
  `help = { banner = false }` in `saysdf.setup({...})`.

## Viewer keys (`saysdf view`)

Annotations are CriticMarkup written into the file itself (`{++add++}`, `{--cut--}`, `{~~old~>new~~}`,
`{==mark==}{>>note<<}`); nothing else in the file is touched.

Every command is a Ctrl combination and works by the physical key on any keyboard layout.

- **Shift+arrows**, **Shift+Home/End**, **Shift+PgUp/PgDn** or a mouse drag — select text from the caret;
  **Ctrl+Space** starts or clears selection mode at the caret. In selection mode the arrows (also
  **Ctrl+Left/Right** by word, **Ctrl+Home/End** to the file start/end) move the caret, with Shift they
  extend the selection. **Esc** clears the selection (it never quits from there).
- On a selection: **Ctrl+R** replace, **Ctrl+D** delete (confirm with Enter), **Ctrl+N** insert,
  **Ctrl+K** comment; the text is typed in the input line (Enter writes, Esc cancels).
- **Alt+Down** / **Alt+Up** — next / previous annotation (the caret lands on it). The status line shows
  how many annotations the file has.
- On an annotation: **Ctrl+E** — change its text, **Ctrl+X** — remove it (confirm with Enter); the covered
  text stays.
- **Ctrl+L** — list of the annotations of the project folder (`saysdf annotations`); Enter on a row opens that
  file at that line, Esc returns to the same place.
- **Ctrl+G** — hand the file to the agent pane: a phrase with its path is pasted there without Enter (only in
  a viewer opened by saysdfTerm).
- **Ctrl+F** — diff of the file against `HEAD` (`saysdf diff`); after closing it you are back in the viewer.
- **Ctrl+W** — soft wrap on or off (on by default for `.md`, `.markdown`, `.txt`).
- Plain keys stay the pager's: arrows and PgUp/PgDn scroll, `/` search, `n` next match, `r` reload, `q` quit.

## Why it works this way

- One command model: the palette, the menu and the hints show the same things — nothing gets lost.
- The bars and the banner are drawn outside the terminal: they do not end up in the shell history
  or the scrollback.
- The fork does not touch F-keys other than F1/F9 — they belong to programs.

## Configuration

The config is Lua: `return require("saysdf").setup({ <module> = false | { ... } })`.
`false` turns a module off entirely (with its menu entries). Source and options of any module:
`saysdfTerm-cli show-module <name>`.

## Security

- Program output is untrusted data: it cannot start processes, open files or links, or write
  to the clipboard on its own. Links and paths open only on your action.
- `saysdf db` opens databases read-only; writing is enabled explicitly, by typing the database
  name, and is highlighted in the status bar.
- SSH and database passwords are kept in the system secret store, not in the config.
- Project commands (workspaces) run only after you approve the exact list of commands.
