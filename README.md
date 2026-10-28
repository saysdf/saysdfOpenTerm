# saysdfTerminal

A personal fork of [WezTerm](https://github.com/wezterm/wezterm): a GPU-accelerated,
cross-platform terminal (Windows, Linux, macOS) with developer tools built in —
git, diff/merge, a code viewer, Markdown and Mermaid previews, SSH and database
browsers, a file browser and a system monitor.

The app is called **saysdfTerm**. Everything WezTerm can do still works; the fork's
features are shipped inside the binary as Lua modules plus a small companion CLI
(`saysdf`), so there is nothing to install separately.

> **Status:** a personal project, built for the author's own daily use. It works,
> but expect rough edges. The UI (menus, hints, help) is currently in **Russian**.
> Issues and pull requests are welcome, but there are no release builds and no
> support guarantees.

## Features

**Terminal basics**
- Norton-style menu bar (F9), tab bar and a status bar that shows hotkeys relevant
  to what you are doing right now
- Startup banner and built-in help (F1)
- Confirmation before closing a window, tab or pane
- Pane headers with close / zoom / restore buttons
- Tab renaming, icons and activity indicators
- Ctrl+Enter and Shift+Enter insert a newline instead of running the command
- Hotkeys work on any keyboard layout (matched by physical key; can be turned off)
- Theme menu with high-contrast themes, including the stock Windows, macOS and
  Ubuntu terminal palettes; the bars follow the theme
- Shell integration (OSC 133 / OSC 7) for PowerShell, bash, zsh and fish: jump
  between prompts, copy the last command's output, command-finished notifications

**Viewing and navigation**
- Built-in code viewer with syntax highlighting
- Clickable `file:line` paths in command output
- Quick-select for paths, hashes and URLs
- Markdown preview
- Mermaid diagrams rendered as text graphics, with scrolling and zoom
- Inline images (kitty, iTerm2 and sixel protocols)

**Tools** (open in a side panel next to your shell)
- Git: status, commit, push/pull, checkout, submodules
- Diff and three-way merge tool
- SSH connection list on top of `~/.ssh/config`, with titles, descriptions and
  an easy "add connection" form
- Read-only database viewer for SQLite, PostgreSQL, MySQL and Dolt. Writing
  requires an explicit switch into a clearly marked "danger mode". PostgreSQL
  inside a Docker container on a remote host works over plain `ssh` +
  `docker exec psql`, with no open ports or tunnels
- File browser
- System monitor with load graphs and a process list
- Command suggestions
- Project workspaces (save and restore a set of tabs)

## Security

The fork tries to be safer than the upstream defaults:
- no update checks, no file downloads triggered by escape sequences, no bundled
  unsigned ANGLE/Mesa DLLs;
- escape-sequence payloads and decompressed image data have size limits;
- kitty file-transfer image modes are off by default;
- passwords are kept only in the OS keychain;
- every upstream WezTerm update is reviewed before it is merged. The reports are
  in [security-reviews/](security-reviews/).

Found a security problem? Please open a private
[security advisory](https://github.com/saysdf/saysdfOpenTerm/security/advisories/new)
instead of a public issue (see [SECURITY.md](SECURITY.md)).

## Building

There are no binary releases yet. The bootstrap scripts build the latest release
from source and install it for the current user (no admin rights). They print
every step and ask before installing anything (Git, Rust via rustup, Strawberry
Perl, Linux packages via `./get-deps`, the Xcode command line tools).

Windows (PowerShell):

```powershell
git clone --recurse-submodules https://github.com/saysdf/saysdfOpenTerm.git
powershell -ExecutionPolicy Bypass -File saysdfTerm\bootstrap\bootstrap.ps1
```

Linux and macOS:

```sh
git clone --recurse-submodules https://github.com/saysdf/saysdfOpenTerm.git
bash saysdfOpenTerm/bootstrap/bootstrap.sh
```

Useful options: `-DryRun` / `--dry-run` shows the plan without changing anything,
`-Yes` / `--yes` answers "yes" to every question, `-Uninstall` / `--uninstall`
removes the installed files (your config stays). Without a checkout, the scripts
clone the repository themselves into `~/src/saysdfOpenTerm` (`-Dir` / `--dir`,
`-Ref` / `--ref` for a specific tag).

| OS | Installed to | Start |
|---|---|---|
| Windows | `%LOCALAPPDATA%\Programs\saysdfTerm` | Start menu → saysdfTerm |
| Linux | `~/.local/lib/saysdfTerm`, links in `~/.local/bin` | application menu or `saysdfTerm` |
| macOS | `~/Applications/saysdfTerm.app` | Launchpad / Spotlight or `saysdfTerm` |

The build uses a limited number of parallel jobs so that at least 10% of RAM
and CPU stay free, and runs at low priority. A first build takes 20–40 minutes.

To build by hand: stable Rust, `cargo build --release -p wezterm-gui -p wezterm
-p wezterm-mux-server -p saysdf-cli`; on Linux run `./get-deps` first, on Windows
you need Perl (for example [Strawberry Perl](https://strawberryperl.com/)) and the
MSVC C++ build tools. The binaries land in `target/release/`:

| Binary | What it is |
|---|---|
| `saysdfTerm` | the terminal (GUI) |
| `saysdfTerm-cli` | command-line control of a running terminal |
| `saysdfTerm-mux-server` | multiplexer server |
| `saysdf` | the fork's tools (git, db, files, viewer, monitor…) |

## Configuration

The config file is `~/.config/saysdfTerm/saysdfTerm.lua`:

```lua
local wezterm = require 'wezterm'
local config = wezterm.config_builder()

config.color_scheme = 'Modus-Operandi'

-- enable the fork's features; options go in the first table
return require('saysdf').setup({}, config)
```

All regular [WezTerm configuration](https://wezterm.org/config/files.html)
options are supported. The hotkeys are listed in
[saysdf/keymap.md](saysdf/keymap.md) and in the in-app help (F1).

## Repository layout

| Path | Contents |
|---|---|
| `saysdf/` | everything the fork adds: Lua modules, the `saysdf` CLI, the database, secrets and render crates, tools |
| `bootstrap/` | build-from-source and install scripts |
| `openspec/` | requirements ([OpenSpec](https://github.com/Fission-AI/OpenSpec) specs) |
| `security-reviews/` | security reviews of upstream updates and new dependencies |
| everything else | WezTerm, with small patches marked `// saysdfTerminal:` |

The original WezTerm README is in [README.wezterm.md](README.wezterm.md).

## License

MIT, see [LICENSE.md](LICENSE.md). Based on WezTerm © Wez Furlong; the fork's
changes are © saysdf. The bundled fonts are under the OFL 1.1, and the vendored
crates keep their own licenses.

saysdfTerminal is not affiliated with or endorsed by the WezTerm project.
