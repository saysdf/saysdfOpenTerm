# Shell integration (saysdfTerm)

Scripts that report the prompt, input, output, exit code, command duration
(OSC 133, OSC 1337 `SetUserVar`) and the current folder (OSC 7) to the terminal. Change: `shell-integration`.

| Shell | File | Status |
|---|---|---|
| Windows PowerShell 5.1, pwsh 7+ | `saysdfterm.ps1` | ready |
| bash 4+ | `bash/saysdfterm.bashrc` (on top of `assets/shell-integration/wezterm.sh`) | ready |
| zsh | `zsh/.zshenv`, `zsh/.zshrc` (ZDOTDIR trick) | written, untested (no zsh available) |
| fish 3.1+ | `saysdfterm.fish` | written, untested (no fish available) |

cmd.exe has no integration.

## Automatic setup: the `saysdf.shell` module

The built-in Lua module `saysdf/lua/saysdf/shell.lua` is enabled through `require("saysdf").setup(...)`
(the built-in default config does this itself) and:

- looks for the scripts folder: `opts.dir` -> `$SAYSDFTERM_SHELL_INTEGRATION_DIR` -> `<exe folder>/shell-integration`
  (Windows, Linux archive) -> `<App>/Contents/Resources/shell-integration` (macOS) ->
  `<exe folder>/../share/saysdfterm/shell-integration` (Linux packages); if not found, a warning goes to the log
  and the shell runs without integration;
- if `config.default_prog` is not set, picks a default shell with integration: Windows - `pwsh.exe`
  from PATH, otherwise `powershell.exe`; Unix - `$SHELL` (bash, zsh, fish; any other shell is left alone);
- adds the shells found with integration to `launch_menu`, sets `SAYSDFTERM_SHELL_INTEGRATION=1`
  (and on macOS also `SAYSDFTERM_BASH_LOGIN=1`);
- bindings: `Alt+Shift+Up/Down` - previous/next prompt, `Ctrl+Shift+O` - copy the output of the
  last command (if empty, a notification), triple click - select the zone (the whole command output).

```lua
local wezterm = require("wezterm")
local config = wezterm.config_builder()
require("saysdf").setup({
  shell = {
    shell = "powershell.exe",       -- instead of auto-detection
    launch_menu = false,            -- no menu entries
    copy_output = { key = "Y", mods = "CTRL|SHIFT" },
    triple_click = false,           -- keep the upstream line selection
    -- keys = false                 -- no bindings at all
    -- default_prog = false         -- do not change default_prog
  },
}, config)
return config
```

`require("saysdf").setup({ shell = false }, config)` disables the whole module (no launch command,
no bindings, no handler). The scripts still work when sourced by hand.

The module does not touch your own `default_prog`; you can take the integrated command from the module:

```lua
config.default_prog = require("saysdf.shell").command("pwsh.exe")   -- nil if the scripts are missing
```

API for other modules:

```lua
local shell = require("saysdf.shell")
-- { exit_code, duration_ms, finished_at, pane_id } of the pane's last finished command, or nil
local info = shell.last_command(pane)

wezterm.on("saysdf-command-finished", function(window, pane, info)
  if info.exit_code ~= 0 then
    wezterm.log_info("exit " .. info.exit_code .. " after " .. tostring(info.duration_ms) .. " ms")
  end
end)
```

`duration_ms` may be `nil` (the shell did not send a duration). The data comes from terminal output
and is untrusted: for display only.

## Manual setup

PowerShell (in the profile or in the current session; calling it again duplicates nothing):

```powershell
. 'C:\path\to\saysdfterm\shell-integration\saysdfterm.ps1'
```

The script wraps the existing `prompt` and `PSConsoleHostReadLine` (PSReadLine) and does not change the profile.
Without PSReadLine, A/B/D, OSC 7 and user vars work; `133;C` and `WEZTERM_PROG` require PSReadLine.

bash / zsh / fish (the terminal does this itself; by hand):

```sh
bash --rcfile <dir>/bash/saysdfterm.bashrc -i      # SAYSDFTERM_BASH_LOGIN=1 - as a login shell
SAYSDFTERM_ORIG_ZDOTDIR="$ZDOTDIR" ZDOTDIR=<dir>/zsh zsh -i
fish --init-command 'source <dir>/saysdfterm.fish'
```

Or from your own rc: `. <dir>/bash/saysdfterm.bashrc` at the end of `~/.bashrc` (repeating it duplicates nothing).
bash/zsh look for `wezterm.sh` in `$SAYSDFTERM_SHELL_INTEGRATION_DIR`, then in `<dir>/wezterm.sh`,
then in the source tree (`assets/shell-integration/`).

User vars: `WEZTERM_PROG` (the current command, empty at the prompt), `SAYSDF_LAST_EXIT`,
`SAYSDF_LAST_DURATION_MS`.

## Multi-line input (the `saysdf.multiline` module)

On Shift+Enter and Ctrl+Enter the terminal sends the sequence ESC CR (`\x1b\r`, the same as
Alt+Enter) straight to the pty. The scripts bind Alt+Enter to inserting a line break; Enter runs the whole
multi-line command:

| Shell | Binding |
|---|---|
| PowerShell (PSReadLine) | `Set-PSReadLineKeyHandler -Chord Alt+Enter -Function AddLine` |
| bash (readline) | `bind '"\e\r": "\C-v\C-j"'` (a literal LF in the line) |
| zsh (zle) | `bindkey -M emacs/viins '^[^M' self-insert-unmeta` |
| fish | 4.x: `bind alt-enter 'commandline -i \n'`, 3.x: `bind \e\r 'commandline -i \n'` |

The binding is installed only if the user has not assigned their own action to Alt+Enter (PowerShell -
checked with `Get-PSReadLineKeyHandler`, bash - `bind -p/-s/-X`, zsh - `bindkey`, fish - `bind --user`;
zsh and fish check after `.zshrc`/`config.fish`). If a custom binding exists, Shift+Enter runs it.

When it is installed: `SAYSDFTERM_MULTILINE=1` (set by the `saysdf.multiline` module) or the script is
loaded outside saysdfTerm (neither `SAYSDFTERM_MULTILINE` nor `SAYSDFTERM_SHELL_INTEGRATION` is set, for example a
manual `source` on an SSH host). It is not installed with `SAYSDFTERM_MULTILINE=0`, with
`SAYSDFTERM_SHELL_SKIP_ALL`, and inside saysdfTerm when the module is disabled:

```lua
local config = require("saysdf").setup({
  multiline = false,                     -- Shift/Ctrl+Enter as upstream, no Alt+Enter in shells
  -- multiline = { keys = { "SHIFT" } }, -- Shift+Enter only
})
return config
```

Notes:
- In PSReadLine, Ctrl+Enter is `InsertLineAbove` by default and Shift+Enter is `AddLine`; with the module both
  give `AddLine` (via Alt+Enter).
- Programs that distinguish Shift+Enter via kitty/CSI u (vim with a `<S-CR>` mapping, Helix) receive
  Alt+Enter; get upstream behavior back with `multiline = false`.
- Programs without Alt+Enter support (less, man, old TUIs) see Escape + Enter - usually harmless.
- Alt+Enter in the terminal itself is the upstream `ToggleFullScreen` (a GUI action that does not reach the shell);
  the shell bindings fire on the ESC CR that Shift/Ctrl+Enter sends.
- cmd.exe has no multi-line input.

## Command hints (the `saysdf.hints` module, `saysdf hints`)

Hints work in the shell's own line editor (design command-hints D1): the scripts register
`saysdf hints complete` as the completer for commands with built-in specs (`saysdf/hints/specs/*.json`:
git, cargo, docker, kubectl, npm - `saysdf hints list`). Candidates: subcommands and flags with descriptions,
flag values, paths (`filepaths`/`folders`), git branches/remotes/tags. Dynamic data comes only from an allow-list,
and git runs in safe mode (`--no-optional-locks`, no fsmonitor, hooks are not invoked) with a 500 ms timeout;
`script` entries in the specs are ignored, there is no network access.

| Shell | Completer | Gray history suffix (-> accepts) |
|---|---|---|
| PowerShell 7 / 5.1 | `Register-ArgumentCompleter -Native`; Tab -> `MenuComplete` (if Tab has the default binding) | PSReadLine `PredictionSource History`/`HistoryAndPlugin` (7.2+), if it was `None`; F2 - ListView; PSReadLine < 2.1 - a message once |
| bash | `complete -o default -C 'saysdf hints complete --shell bash --from-env'` | none (ble.sh is optional, we do not install it) |
| zsh | `compdef` (function `__saysdfterm_hints_complete`, `_describe`), after `.zshrc` | zsh-autosuggestions if installed (we do not install it) |
| fish | `complete -c <cmd> -f -a '(__saysdfterm_hints_complete)'` | built-in autosuggestions |

Existing completers are not overridden (`SAYSDF_HINTS_OVERRIDE=1` overrides them): PowerShell -
posh-git for git and an already registered `-Native` completer (carapace, your own); bash - `complete -p`
and bash-completion files; zsh - `$_comps[cmd]` (zsh completes git/docker/... itself, so it takes over);
fish - `<cmd>.fish` in `$fish_complete_path`.

Variables (set by the module, can be set by hand when sourcing on an SSH host):
- `SAYSDFTERM_HINTS=1|0` - enable/disable; without it the script enables hints only outside
  saysdfTerm (like `SAYSDFTERM_MULTILINE`);
- `SAYSDF_HINTS_DISABLED=kubectl,npm` - no hints from us for these commands (`*` - all);
- `SAYSDFTERM_HINTS_INLINE=0` - do not touch PredictionSource; `SAYSDFTERM_HINTS_MENU=0` - do not touch Tab;
- `SAYSDFTERM_HINTS_COMMANDS`, `SAYSDFTERM_HINTS_BIN` - the list of commands and the path to `saysdf`.

```lua
local config = require("saysdf").setup({
  hints = { disabled = { "kubectl" }, inline = true },  -- hints = false - disable
})
```

`saysdf hints doctor` in the shell shows whether hints are enabled, which commands have the completer
registered, and whether the gray suffix is available (the scripts export `SAYSDF_HINTS_ACTIVE`, `SAYSDF_HINTS_SHELL`,
`SAYSDF_HINTS_INLINE`). Full-screen programs (vim, Claude Code) are not affected: everything works
only in the shell's line editor.

## Disabling

Any non-empty value of the environment variable (before the shell starts / before the script is sourced):

- `SAYSDFTERM_SHELL_SKIP_ALL` - the script does nothing;
- `SAYSDFTERM_SHELL_SKIP_SEMANTIC_ZONES` - no OSC 133;
- `SAYSDFTERM_SHELL_SKIP_CWD` - no OSC 7;
- `SAYSDFTERM_SHELL_SKIP_USER_VARS` - no OSC 1337 `SetUserVar`.

The upstream `WEZTERM_SHELL_SKIP_*` variables with the same suffixes also apply.

## Syntax check

```powershell
powershell -NoProfile -File saysdf/tools/check-shell-scripts.ps1
```
