# Changelog

Each release of [saysdfTerm](https://github.com/saysdf/saysdfOpenTerm) has a section `## X.Y.Z`
(optionally followed by ` - YYYY-MM-DD`). `saysdf/tools/release-public.ps1` uses the section as
the release commit message and refuses to release a version without one.

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
