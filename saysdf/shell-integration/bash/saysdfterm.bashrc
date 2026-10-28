# saysdfTerminal: bash shell integration.
# Usage: bash --rcfile <dir>/bash/saysdfterm.bashrc -i
# --rcfile replaces the normal startup files, so this file loads them itself:
#   SAYSDFTERM_BASH_LOGIN=1 -> /etc/profile, then the first of ~/.bash_profile|~/.bash_login|~/.profile
#   otherwise              -> /etc/bash.bashrc (if present), then ~/.bashrc
# and then the upstream wezterm.sh (OSC 133, OSC 7, WEZTERM_* user vars, bundled bash-preexec)
# plus saysdfTerm hooks for SAYSDF_LAST_EXIT / SAYSDF_LAST_DURATION_MS.
# User files are never modified. Can also be sourced manually from ~/.bashrc (idempotent).
#
# Disable (any non-empty value): SAYSDFTERM_SHELL_SKIP_ALL | _SEMANTIC_ZONES | _CWD | _USER_VARS
# and the upstream WEZTERM_SHELL_SKIP_* equivalents.
# Multiline input (change multiline-input): Alt+Enter (ESC CR, sent by saysdfTerm for Shift+Enter
# and Ctrl+Enter) inserts a literal newline ("\C-v\C-j"), unless the user already bound it.
# Skipped when SAYSDFTERM_MULTILINE=0, or inside saysdfTerm with saysdf.multiline disabled.
# Command hints (change command-hints): `saysdf hints complete` registered as the completer for
# git/cargo/docker/kubectl/npm unless a completion exists (SAYSDF_HINTS_OVERRIDE=1 overrides).
# Off: SAYSDFTERM_HINTS=0, or inside saysdfTerm with saysdf.hints disabled; per command:
# SAYSDF_HINTS_DISABLED=kubectl,npm ('*' = all). Binary: SAYSDFTERM_HINTS_BIN or saysdf in PATH.
# Prompt suggest (change prompt-suggest): SAYSDFTERM_SUGGEST=1 -> user vars SAYSDF_HISTFILE and
# SAYSDF_PATH at the prompt, only when changed. Not emitted otherwise.

# Directory of this file (resolved before user files can change anything).
__saysdfterm_bash_dir=${BASH_SOURCE[0]%/*}
[[ "$__saysdfterm_bash_dir" == "${BASH_SOURCE[0]}" ]] && __saysdfterm_bash_dir=.

# 1. User startup files (only when we are the rcfile, not when sourced from ~/.bashrc).
if [[ -z "${__saysdfterm_bash_rc_done-}" && "${BASH_SOURCE[0]}" == "${BASH_SOURCE[${#BASH_SOURCE[@]}-1]}" ]]; then
  __saysdfterm_bash_rc_done=1
  if [[ -n "${SAYSDFTERM_BASH_LOGIN-}" && "${SAYSDFTERM_BASH_LOGIN}" != 0 ]]; then
    [[ -r /etc/profile ]] && . /etc/profile
    for __saysdfterm_f in ~/.bash_profile ~/.bash_login ~/.profile; do
      if [[ -r "$__saysdfterm_f" ]]; then . "$__saysdfterm_f"; break; fi
    done
    unset __saysdfterm_f
  else
    [[ -r /etc/bash.bashrc ]] && . /etc/bash.bashrc
    [[ -r ~/.bashrc ]] && . ~/.bashrc
  fi
fi

# 2. Integration (once per shell).
if [[ -z "${__saysdfterm_bash_loaded-}" && $- == *i* \
      && -z "${SAYSDFTERM_SHELL_SKIP_ALL-}" && -z "${WEZTERM_SHELL_SKIP_ALL-}" ]]; then
  __saysdfterm_bash_loaded=1

  # Map our skip variables onto the upstream ones (shell variables, not exported).
  for __saysdfterm_p in SEMANTIC_ZONES CWD USER_VARS; do
    __saysdfterm_v=SAYSDFTERM_SHELL_SKIP_$__saysdfterm_p
    [[ -n "${!__saysdfterm_v-}" ]] && printf -v "WEZTERM_SHELL_SKIP_$__saysdfterm_p" 1
  done
  unset __saysdfterm_p __saysdfterm_v

  # Find wezterm.sh: explicit dir, packaged layout (<dir>/wezterm.sh), source tree.
  __saysdfterm_wsh=
  for __saysdfterm_f in \
      "${SAYSDFTERM_SHELL_INTEGRATION_DIR:+$SAYSDFTERM_SHELL_INTEGRATION_DIR/wezterm.sh}" \
      "$__saysdfterm_bash_dir/../wezterm.sh" \
      "$__saysdfterm_bash_dir/../../../assets/shell-integration/wezterm.sh"; do
    if [[ -n "$__saysdfterm_f" && -r "$__saysdfterm_f" ]]; then __saysdfterm_wsh=$__saysdfterm_f; break; fi
  done
  unset __saysdfterm_f
  [[ -n "$__saysdfterm_wsh" ]] && . "$__saysdfterm_wsh"
  unset __saysdfterm_wsh

  # 3. Duration / exit code user vars (needs bash-preexec from wezterm.sh; not with ble.sh).
  if declare -F __wezterm_set_user_var >/dev/null && declare -F __bp_precmd_invoke_cmd >/dev/null \
      && [[ -z "${WEZTERM_SHELL_SKIP_USER_VARS-}" ]]; then
    __saysdfterm_now_ms() {
      if [[ -n "${EPOCHREALTIME-}" ]]; then
        local t=${EPOCHREALTIME/[.,]/}   # microseconds (decimal separator depends on locale)
        printf -v __saysdfterm_ms '%s' "$(( 10#$t / 1000 ))"
      else
        __saysdfterm_ms=$(( SECONDS * 1000 ))
      fi
    }
    __saysdfterm_preexec() {
      local __saysdfterm_ms
      __saysdfterm_now_ms
      __saysdfterm_cmd_start=$__saysdfterm_ms
    }
    # Runs first among precmd hooks: take the end time before other hooks (which fork) run.
    __saysdfterm_precmd_mark() {
      local ret=$?
      [[ -n "${__saysdfterm_cmd_start-}" ]] && __saysdfterm_now_ms
      return $ret
    }
    # base64 on one line: GNU base64 wraps at 76 columns and PATH is long.
    __saysdfterm_suggest_var() {
      local b64
      b64=$(printf '%s' "$2" | base64 | tr -d '\n')
      if [[ -z "${TMUX-}" ]]; then
        printf '\033]1337;SetUserVar=%s=%s\007' "$1" "$b64"
      else
        printf '\033Ptmux;\033\033]1337;SetUserVar=%s=%s\007\033\\' "$1" "$b64"
      fi
    }
    __saysdfterm_suggest_report() {
      [[ "${SAYSDFTERM_SUGGEST-}" == 1 ]] || return 0
      hash base64 2>/dev/null || return 0
      if [[ "${HISTFILE-}" != "${__saysdfterm_sent_histfile-}" ]]; then
        __saysdfterm_sent_histfile=${HISTFILE-}
        __saysdfterm_suggest_var SAYSDF_HISTFILE "$__saysdfterm_sent_histfile"
      fi
      if [[ "$PATH" != "${__saysdfterm_sent_path-}" ]]; then
        __saysdfterm_sent_path=$PATH
        __saysdfterm_suggest_var SAYSDF_PATH "$__saysdfterm_sent_path"
      fi
    }
    # Runs last: report exit code (bash-preexec restores $? for every hook) and duration.
    __saysdfterm_precmd() {
      local ret=$?
      if [[ -n "${__saysdfterm_cmd_start-}" ]]; then
        __wezterm_set_user_var SAYSDF_LAST_EXIT "$ret"
        __wezterm_set_user_var SAYSDF_LAST_DURATION_MS "$(( __saysdfterm_ms - __saysdfterm_cmd_start ))"
        __saysdfterm_cmd_start=
      fi
      __saysdfterm_suggest_report
      return $ret
    }
    precmd_functions=(__saysdfterm_precmd_mark "${precmd_functions[@]}" __saysdfterm_precmd)
    preexec_functions+=(__saysdfterm_preexec)   # last: start time after other hooks
  fi

  # 4. Multiline input: Alt+Enter -> literal newline in the readline buffer.
  if [[ "${SAYSDFTERM_MULTILINE-}" == 1 \
        || ( -z "${SAYSDFTERM_MULTILINE-}" && -z "${SAYSDFTERM_SHELL_INTEGRATION-}" ) ]]; then
    if ! { bind -p; bind -s; bind -X; } 2>/dev/null | grep -qF '"\e\C-m"'; then
      bind '"\e\r": "\C-v\C-j"' 2>/dev/null
    fi
  fi

  # 5. Command hints (change command-hints): `saysdf hints complete` as `complete -C` for
  #    spec-covered commands without an existing completion (incl. lazily loaded bash-completion).
  if [[ "${SAYSDFTERM_HINTS-}" == 1 \
        || ( -z "${SAYSDFTERM_HINTS-}" && -z "${SAYSDFTERM_SHELL_INTEGRATION-}" ) ]] \
     && [[ "${SAYSDF_HINTS_DISABLED-}" != "*" && "${SAYSDF_HINTS_DISABLED-}" != 1 ]]; then
    __saysdfterm_hints_bin=${SAYSDFTERM_HINTS_BIN:-$(command -v saysdf 2>/dev/null)}
    if [[ -n "$__saysdfterm_hints_bin" ]]; then
      __saysdfterm_hints_active=
      for __saysdfterm_c in ${SAYSDFTERM_HINTS_COMMANDS:-git cargo docker kubectl npm}; do
        __saysdfterm_c=${__saysdfterm_c//,/}
        [[ -z "$__saysdfterm_c" || ",${SAYSDF_HINTS_DISABLED-}," == *",$__saysdfterm_c,"* ]] && continue
        if [[ "${SAYSDF_HINTS_OVERRIDE-}" != 1 ]]; then
          complete -p "$__saysdfterm_c" >/dev/null 2>&1 && continue
          __saysdfterm_has=
          for __saysdfterm_d in "${BASH_COMPLETION_USER_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion}/completions" \
                                /usr/share/bash-completion/completions /usr/local/share/bash-completion/completions; do
            [[ -r "$__saysdfterm_d/$__saysdfterm_c" ]] && { __saysdfterm_has=1; break; }
          done
          [[ -n "$__saysdfterm_has" ]] && continue
        fi
        complete -o default -C "$(printf '%q' "$__saysdfterm_hints_bin") hints complete --shell bash --from-env" "$__saysdfterm_c"
        __saysdfterm_hints_active+=${__saysdfterm_hints_active:+,}$__saysdfterm_c
      done
      export SAYSDF_HINTS_ACTIVE=$__saysdfterm_hints_active SAYSDF_HINTS_SHELL=bash
      unset __saysdfterm_c __saysdfterm_d __saysdfterm_has __saysdfterm_hints_active
    fi
    if [[ -n "${BLE_VERSION-}" ]]; then
      export SAYSDF_HINTS_INLINE="ble.sh auto-complete"
    else
      export SAYSDF_HINTS_INLINE="unavailable: bash has no inline suggestions (ble.sh is optional, not installed by saysdfTerm)"
    fi
  fi
fi
