# saysdfTerminal: fish shell integration (fish 3.1+).
# Usage: fish --init-command "source <dir>/saysdfterm.fish"   (runs after config.fish)
# Emits OSC 133 (A/B/C/D;<exit>), OSC 7 (cwd) and OSC 1337 SetUserVar
# (WEZTERM_PROG, SAYSDF_LAST_EXIT, SAYSDF_LAST_DURATION_MS).
# Wraps the existing fish_prompt for 133;B; never touches config files. Safe to source twice.
#
# Disable (any non-empty value): SAYSDFTERM_SHELL_SKIP_ALL | _SEMANTIC_ZONES | _CWD | _USER_VARS
# and the upstream WEZTERM_SHELL_SKIP_* equivalents.
# Multiline input (change multiline-input): Alt+Enter (ESC CR, sent by saysdfTerm for Shift+Enter
# and Ctrl+Enter) inserts a newline, unless the user bound it. fish 4: `bind alt-enter`, fish 3: `bind \e\r`.
# Skipped when SAYSDFTERM_MULTILINE=0, or inside saysdfTerm with saysdf.multiline disabled.
# Command hints (change command-hints): `saysdf hints complete` registered as the completer for
# git/cargo/docker/kubectl/npm unless a completion exists (SAYSDF_HINTS_OVERRIDE=1 overrides).
# Off: SAYSDFTERM_HINTS=0, or inside saysdfTerm with saysdf.hints disabled; per command:
# SAYSDF_HINTS_DISABLED=kubectl,npm ('*' = all). Binary: SAYSDFTERM_HINTS_BIN or saysdf in PATH.
# Prompt suggest (change prompt-suggest): SAYSDFTERM_SUGGEST=1 -> user vars SAYSDF_HISTFILE and
# SAYSDF_PATH at the prompt, only when changed. Not emitted otherwise.

if status is-interactive; and not set -q __saysdfterm_fish_loaded
    and test -z "$SAYSDFTERM_SHELL_SKIP_ALL" -a -z "$WEZTERM_SHELL_SKIP_ALL"
    and not contains -- "$TERM" linux dumb

    set -g __saysdfterm_fish_loaded 1

    function __saysdfterm_skip --argument-names part
        set -l a SAYSDFTERM_SHELL_SKIP_$part
        set -l b WEZTERM_SHELL_SKIP_$part
        test -n "$$a" -o -n "$$b"
    end

    function __saysdfterm_user_var --argument-names name value
        __saysdfterm_skip USER_VARS; and return
        command -sq base64; or return
        set -l b64 (printf '%s' "$value" | command base64 | string join '')
        printf '\e]1337;SetUserVar=%s=%s\a' $name "$b64"
    end

    function __saysdfterm_osc7
        __saysdfterm_skip CWD; and return
        set -l host $hostname
        test -n "$host"; or set host (command hostname 2>/dev/null)
        printf '\e]7;file://%s%s\e\' "$host" (string escape --style=url -- $PWD)
    end

    # Before the command runs (non-empty command lines only).
    function __saysdfterm_preexec --on-event fish_preexec
        __saysdfterm_skip SEMANTIC_ZONES; or printf '\e]133;C\a'
        __saysdfterm_user_var WEZTERM_PROG "$argv[1]"
    end

    # After the command: $status and $CMD_DURATION belong to it.
    function __saysdfterm_postexec --on-event fish_postexec
        set -l code $status
        set -l dur $CMD_DURATION
        __saysdfterm_skip SEMANTIC_ZONES; or printf '\e]133;D;%s\a' $code
        __saysdfterm_user_var SAYSDF_LAST_EXIT $code
        test -n "$dur"; and __saysdfterm_user_var SAYSDF_LAST_DURATION_MS $dur
    end

    # SAYSDFTERM_SUGGEST=1 (change prompt-suggest): SAYSDF_HISTFILE / SAYSDF_PATH only when changed.
    # History file: <data dir>/fish/<$fish_history>_history; empty when history is off (fish_history='').
    function __saysdfterm_suggest_report
        test "$SAYSDFTERM_SUGGEST" = 1; or return
        set -l hist ''
        set -l hname fish
        set -q fish_history; and set hname $fish_history
        if test -n "$hname"
            set -l base $XDG_DATA_HOME
            test -n "$base"; or set base $HOME/.local/share
            set hist $base/fish/{$hname}_history
        end
        if not set -q __saysdfterm_sent_histfile; or test "$hist" != "$__saysdfterm_sent_histfile"
            set -g __saysdfterm_sent_histfile $hist
            __saysdfterm_user_var SAYSDF_HISTFILE "$hist"
        end
        set -l p (string join : -- $PATH)
        if not set -q __saysdfterm_sent_path; or test "$p" != "$__saysdfterm_sent_path"
            set -g __saysdfterm_sent_path $p
            __saysdfterm_user_var SAYSDF_PATH "$p"
        end
    end

    # Before each new prompt.
    function __saysdfterm_prompt_start --on-event fish_prompt
        __saysdfterm_user_var WEZTERM_PROG ''
        __saysdfterm_skip SEMANTIC_ZONES; or printf '\e]133;A\a'
        __saysdfterm_osc7
        __saysdfterm_suggest_report
    end

    function __saysdfterm_set_status
        return $argv[1]
    end

    # Wrap fish_prompt (also triggers autoloading of a user/theme prompt) to append 133;B.
    if functions -q fish_prompt
        functions -c fish_prompt __saysdfterm_orig_fish_prompt
    else
        function __saysdfterm_orig_fish_prompt
            printf '%s> ' (prompt_pwd)
        end
    end
    function fish_prompt
        set -l s $status
        __saysdfterm_set_status $s
        __saysdfterm_orig_fish_prompt
        __saysdfterm_skip SEMANTIC_ZONES; or printf '\e]133;B\a'
    end

    # Multiline input: Alt+Enter -> newline (runs after config.fish, so a user binding wins).
    if test "$SAYSDFTERM_MULTILINE" = 1; or begin; test -z "$SAYSDFTERM_MULTILINE"; and test -z "$SAYSDFTERM_SHELL_INTEGRATION"; end
        set -l __sdf_major (string split -m1 . -- $version)[1]
        if test "$__sdf_major" -ge 4 2>/dev/null
            # fish 4 binds alt-enter to a newline by default; keep any user binding
            if not bind --user alt-enter >/dev/null 2>&1
                bind alt-enter 'commandline -i \n'
            end
        else
            if not bind --user \e\r >/dev/null 2>&1
                bind \e\r 'commandline -i \n'
            end
        end
    end

    # Command hints (change command-hints): `saysdf hints complete` via `complete -c -a` for
    # spec-covered commands without fish/user completions; inline suggestions are built into fish.
    if test "$SAYSDFTERM_HINTS" = 1; or begin; test -z "$SAYSDFTERM_HINTS"; and test -z "$SAYSDFTERM_SHELL_INTEGRATION"; end
        if not contains -- "$SAYSDF_HINTS_DISABLED" '*' 1
            set -l bin $SAYSDFTERM_HINTS_BIN
            test -n "$bin"; or set bin (command -s saysdf)
            if test -n "$bin"
                set -g __saysdfterm_hints_bin $bin
                function __saysdfterm_hints_complete
                    set -lx SAYSDF_HINTS_LINE (commandline -cp | string collect)
                    $__saysdfterm_hints_bin hints complete --shell fish --from-env 2>/dev/null
                end
                set -l cmds git cargo docker kubectl npm
                test -n "$SAYSDFTERM_HINTS_COMMANDS"; and set cmds (string split -n ' ' -- (string replace -a ',' ' ' -- $SAYSDFTERM_HINTS_COMMANDS))
                set -l off (string split -n ',' -- "$SAYSDF_HINTS_DISABLED")
                set -l active
                for c in $cmds
                    contains -- $c $off; and continue
                    if test "$SAYSDF_HINTS_OVERRIDE" != 1
                        set -l has
                        for d in $fish_complete_path
                            if test -f "$d/$c.fish"
                                set has 1
                                break
                            end
                        end
                        test -n "$has"; and continue
                    end
                    complete -c $c -f -a '(__saysdfterm_hints_complete)'
                    set -a active $c
                end
                set -gx SAYSDF_HINTS_ACTIVE (string join , -- $active)
                set -gx SAYSDF_HINTS_SHELL fish
            end
        end
        set -gx SAYSDF_HINTS_INLINE "fish autosuggestions (built-in)"
    end
end
