# Changelog

Each release of [saysdfTerm](https://github.com/saysdf/saysdfOpenTerm) has a section `## X.Y.Z`
(optionally followed by ` - YYYY-MM-DD`). `saysdf/tools/release-public.ps1` uses the section as
the release commit message and refuses to release a version without one.

## 0.3.0 - 2026-11-10

**Live theme preview.** In the theme list (sidebar), moving the cursor applies the theme at once in
all windows while the list stays open; Enter keeps it, Esc (or closing the panel) restores the theme
you had before. The InputSelector fallback (no sidebar) works as before. Under the hood `saysdf pick
--hover` reports the resting cursor (debounced) over the sidebar channel; only ids from the issued
list are accepted and hovers are rate-limited.

**Workspaces: names and colors.**
- Tab colors are saved with the workspace and restored with it (old saves load unchanged).
- "Workspaces: save as…" asks for a name (overwrite needs confirmation, default "no") and saves the
  current workspace under it; the workspace takes the new name, so autosave keeps it up to date.
- "Workspaces: rename…" renames the workspace and its saved files; renaming onto another open
  workspace is refused.
- "New workspace…" (first item of the Ctrl+Shift+A list, also in the palette): an open name switches
  to it, a saved one is restored, a new one opens in the current pane's folder.

## 0.2.0 - 2026-11-08

**Review annotations in the code viewer** (`saysdf view`). Mark up a text file or a book chapter
in place with [CriticMarkup](https://github.com/CriticMarkup/CriticMarkup-toolkit) and hand it to an
agent running in the next pane:
- Select with Shift+arrows or the mouse, then Ctrl+R substitute, Ctrl+D delete, Ctrl+N insert,
  Ctrl+K comment; Ctrl+E amends and Ctrl+X removes the annotation under the caret; Alt+Down/Up jump
  between annotations. A multi-line editor at the bottom (Enter saves, Ctrl+J new line, Esc cancels).
- Writes only on an explicit key + Enter, atomically, refusing if the file changed on disk; keeps the
  file's line endings and BOM; removing an annotation restores the original bytes.
- Annotations are highlighted in the code viewer and in the rendered Markdown preview.
- Ctrl+G pastes "review the annotations in <file>" into the agent pane (without Enter), Ctrl+L lists
  all annotations of the project (`saysdf annotations [dir]`), Ctrl+F shows the diff against HEAD.
- Prose files (`.md`, `.txt`) wrap at word boundaries (Ctrl+W toggles); the file name is shown in the
  pane header; hints take up to two rows.
- Opening a file next to an open viewer reuses that viewer split instead of splitting again.

**Keys by physical key everywhere.** TUI tools now recognise keys by their physical position on
Windows too (the virtual-key code), so commands work on any keyboard layout. Viewer commands moved
from single letters to Ctrl/Alt combinations.

**Workspaces restore reliably.**
- The previous session is restored automatically at startup (option `restore`, default `"always"`).
  Autosave no longer overwrites the saved session before it is restored, and a session that shrinks
  (tabs closed before quitting) is written only after 90 seconds.
- Tabs running Claude Code remember their session and reopen it with `claude --resume <id>`; the
  terminal asks once per new `claude` executable before starting it automatically.
- "Workspaces: save current" saves at once and shows a preview of what was saved; commands that look
  like they contain a secret are not saved.
- Autosave work moved off the status-bar path.

## 0.1.1 - 2026-10-28

- Published the requirements for UI localization (`openspec/specs/ui-localization`) and for
  public releases (`openspec/specs/public-release`).
- Clarified that a translation catalog may change a menu accelerator (`[_accel]` table) only to a
  key that is free in that menu; it never displaces another entry's accelerator.

## 0.1.0 - 2026-10-28

First public release of the saysdfTerm fork of WezTerm.

- Norton-style menu bar, hotkey status bar, startup help, close confirmation, pane headers.
- English interface with a Russian translation; choose it with the `language` option (`"auto"`, `"en"`, `"ru"`) and add your own translations as files in `~/.config/saysdfTerm/locales/<lang>/`.
- Shell integration for PowerShell, bash, zsh and fish (prompt navigation, command output,
  notifications).
- Built-in tools: code viewer, Markdown and Mermaid previews, git, diff/merge, SSH connections,
  read-only database viewer, file browser, system monitor, command suggestions, workspaces.
- Security hardening over upstream defaults: no update checks, size limits for escape-sequence
  payloads and images, secrets only in the OS keychain.
- Bootstrap scripts to build and install from source on Windows, Linux and macOS.
