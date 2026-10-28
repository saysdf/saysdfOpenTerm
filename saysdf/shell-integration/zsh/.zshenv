# saysdfTerminal: zsh shell integration, ZDOTDIR trick (as in kitty / VS Code).
# The terminal starts zsh with ZDOTDIR=<dir>/zsh and the user's original value (if any) in
# SAYSDFTERM_ORIG_ZDOTDIR. This file restores ZDOTDIR first, then loads the user's .zshenv, so the
# rest of the startup (.zprofile, .zshrc, .zlogin) comes from the user's own ZDOTDIR as usual.
# For interactive shells, the upstream wezterm.sh and saysdfTerm hooks are installed from a
# one-shot precmd hook, i.e. after the user's .zshrc. User files are never modified.
#
# Disable (any non-empty value): SAYSDFTERM_SHELL_SKIP_ALL | _SEMANTIC_ZONES | _CWD | _USER_VARS
# and the upstream WEZTERM_SHELL_SKIP_* equivalents.
# Multiline input (change multiline-input): Alt+Enter (ESC CR, sent by saysdfTerm for Shift+Enter
# and Ctrl+Enter) -> self-insert-unmeta (inserts a newline) in the emacs and viins keymaps, unless
# the user bound ^[^M to something else. Skipped when SAYSDFTERM_MULTILINE=0, or inside saysdfTerm
# with saysdf.multiline disabled.
# Command hints (change command-hints): `saysdf hints complete` registered as the completer for
# git/cargo/docker/kubectl/npm unless a completion exists (SAYSDF_HINTS_OVERRIDE=1 overrides).
# Off: SAYSDFTERM_HINTS=0, or inside saysdfTerm with saysdf.hints disabled; per command:
# SAYSDF_HINTS_DISABLED=kubectl,npm ('*' = all). Binary: SAYSDFTERM_HINTS_BIN or saysdf in PATH.
# Prompt suggest (change prompt-suggest): SAYSDFTERM_SUGGEST=1 -> user vars SAYSDF_HISTFILE and
# SAYSDF_PATH at the prompt, only when changed. Not emitted otherwise.

# Our directory, taken before ZDOTDIR changes.
typeset -g __saysdfterm_zsh_dir=${${(%):-%x}:A:h}

# 1. Restore the original ZDOTDIR (unset or empty means $HOME).
if [[ -n "${SAYSDFTERM_ORIG_ZDOTDIR-}" ]]; then
  export ZDOTDIR=$SAYSDFTERM_ORIG_ZDOTDIR
else
  unset ZDOTDIR
fi
unset SAYSDFTERM_ORIG_ZDOTDIR

# 2. The user's .zshenv (it may change ZDOTDIR itself; that is respected).
if [[ -r "${ZDOTDIR:-$HOME}/.zshenv" && "${ZDOTDIR:-$HOME}/.zshenv" -ef "$__saysdfterm_zsh_dir/.zshenv" ]]; then
  : # ZDOTDIR still points at us: nothing to load (avoid recursion)
elif [[ -r "${ZDOTDIR:-$HOME}/.zshenv" ]]; then
  source "${ZDOTDIR:-$HOME}/.zshenv"
fi

# 3. Integration loader, shared with our .zshrc (fallback when ZDOTDIR still points here).
__saysdfterm_zsh_install() {
  emulate -L zsh
  [[ -n "${__saysdfterm_zsh_loaded-}" ]] && return 0
  [[ -o interactive ]] || return 0
  [[ -n "${SAYSDFTERM_SHELL_SKIP_ALL-}" || -n "${WEZTERM_SHELL_SKIP_ALL-}" ]] && return 0
  typeset -g __saysdfterm_zsh_loaded=1

  local p v f wsh=
  for p in SEMANTIC_ZONES CWD USER_VARS; do
    v=SAYSDFTERM_SHELL_SKIP_$p
    [[ -n "${(P)v-}" ]] && typeset -g WEZTERM_SHELL_SKIP_$p=1
  done
  for f in "${SAYSDFTERM_SHELL_INTEGRATION_DIR:+$SAYSDFTERM_SHELL_INTEGRATION_DIR/wezterm.sh}" \
           "$__saysdfterm_zsh_dir/../wezterm.sh" \
           "$__saysdfterm_zsh_dir/../../../assets/shell-integration/wezterm.sh"; do
    if [[ -n "$f" && -r "$f" ]]; then wsh=$f; break; fi
  done
  # wezterm.sh only assigns globals at top level (no typeset/local), so sourcing it here is safe.
  [[ -n "$wsh" ]] && source "$wsh"

  # Duration / exit code user vars.
  if (( $+functions[__wezterm_set_user_var] )) && [[ -z "${WEZTERM_SHELL_SKIP_USER_VARS-}" ]]; then
    zmodload -F zsh/datetime p:EPOCHREALTIME 2>/dev/null
    __saysdfterm_now_ms() {
      if [[ -n "${EPOCHREALTIME-}" ]]; then
        typeset -g __saysdfterm_ms=$(( ${EPOCHREALTIME/[.,]/} / 1000 ))
      else
        typeset -g __saysdfterm_ms=$(( SECONDS * 1000 ))
      fi
    }
    __saysdfterm_preexec() { __saysdfterm_now_ms; typeset -g __saysdfterm_cmd_start=$__saysdfterm_ms }
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
      (( $+commands[base64] )) || return 0
      if [[ "${HISTFILE-}" != "${__saysdfterm_sent_histfile-}" ]]; then
        typeset -g __saysdfterm_sent_histfile=${HISTFILE-}
        __saysdfterm_suggest_var SAYSDF_HISTFILE "$__saysdfterm_sent_histfile"
      fi
      if [[ "$PATH" != "${__saysdfterm_sent_path-}" ]]; then
        typeset -g __saysdfterm_sent_path=$PATH
        __saysdfterm_suggest_var SAYSDF_PATH "$__saysdfterm_sent_path"
      fi
    }
    __saysdfterm_precmd() {
      local ret=$?
      if [[ -n "${__saysdfterm_cmd_start-}" ]]; then
        __saysdfterm_now_ms
        __wezterm_set_user_var SAYSDF_LAST_EXIT "$ret"
        __wezterm_set_user_var SAYSDF_LAST_DURATION_MS "$(( __saysdfterm_ms - __saysdfterm_cmd_start ))"
        typeset -g __saysdfterm_cmd_start=
      fi
      __saysdfterm_suggest_report
      return $ret
    }
    # precmd first (end time and $? before other hooks run), preexec last (start time after them).
    precmd_functions=(__saysdfterm_precmd $precmd_functions)
    preexec_functions+=(__saysdfterm_preexec)
  fi
}

# Multiline input: Alt+Enter inserts a newline (after the user's .zshrc, so their binding wins).
__saysdfterm_zsh_multiline() {
  [[ -n "${SAYSDFTERM_SHELL_SKIP_ALL-}" || -n "${WEZTERM_SHELL_SKIP_ALL-}" ]] && return 0
  [[ "${SAYSDFTERM_MULTILINE-}" == 1 \
     || ( -z "${SAYSDFTERM_MULTILINE-}" && -z "${SAYSDFTERM_SHELL_INTEGRATION-}" ) ]] || return 0
  local km cur
  for km in emacs viins; do
    cur=${${(z)"$(bindkey -M $km '^[^M' 2>/dev/null)"}[2]}
    if [[ -z "$cur" || "$cur" == undefined-key || "$cur" == self-insert-unmeta ]]; then
      bindkey -M $km '^[^M' self-insert-unmeta
    fi
  done
}

# Command hints (change command-hints): `saysdf hints complete` via compdef for spec-covered
# commands without an existing zsh completion (after the user's .zshrc, so compinit has run).
__saysdfterm_hints_complete() {
  local -a c
  c=("${(@f)$(SAYSDF_HINTS_LINE=$LBUFFER "$__saysdfterm_hints_bin" hints complete --shell zsh --from-env 2>/dev/null)}")
  c=(${c:#})
  if (( $#c )); then
    _describe -t saysdf-hints 'saysdf hints' c && return 0
  fi
  _files
}

__saysdfterm_zsh_hints() {
  [[ -n "${SAYSDFTERM_SHELL_SKIP_ALL-}" || -n "${WEZTERM_SHELL_SKIP_ALL-}" ]] && return 0
  [[ "${SAYSDFTERM_HINTS-}" == 1 \
     || ( -z "${SAYSDFTERM_HINTS-}" && -z "${SAYSDFTERM_SHELL_INTEGRATION-}" ) ]] || return 0
  [[ "${SAYSDF_HINTS_DISABLED-}" == "*" || "${SAYSDF_HINTS_DISABLED-}" == 1 ]] && return 0
  typeset -g __saysdfterm_hints_bin=${SAYSDFTERM_HINTS_BIN:-${commands[saysdf]-}}
  local -a active
  local c
  if [[ -n "$__saysdfterm_hints_bin" ]] && (( $+functions[compdef] )); then
    for c in ${(s:,:)${${SAYSDFTERM_HINTS_COMMANDS:-git cargo docker kubectl npm}// /,}}; do
      [[ -z "$c" || ",${SAYSDF_HINTS_DISABLED-}," == *",$c,"* ]] && continue
      [[ "${SAYSDF_HINTS_OVERRIDE-}" != 1 && -n "${_comps[$c]-}" ]] && continue
      compdef __saysdfterm_hints_complete $c
      active+=$c
    done
  fi
  export SAYSDF_HINTS_ACTIVE=${(j:,:)active} SAYSDF_HINTS_SHELL=zsh
  if [[ "${SAYSDFTERM_HINTS_INLINE-}" == 0 ]]; then
    export SAYSDF_HINTS_INLINE="off (SAYSDFTERM_HINTS_INLINE=0)"
  elif (( $+functions[_zsh_autosuggest_start] || $+functions[_zsh_autosuggest_bind_widgets] )); then
    export SAYSDF_HINTS_INLINE=zsh-autosuggestions
  else
    export SAYSDF_HINTS_INLINE="unavailable: install zsh-autosuggestions (not installed by saysdfTerm)"
  fi
}

# One-shot precmd hook: runs before the first prompt, after the user's .zshrc.
__saysdfterm_zsh_deferred() {
  local ret=$?
  precmd_functions=(${precmd_functions:#__saysdfterm_zsh_deferred})
  __saysdfterm_zsh_install
  __saysdfterm_zsh_multiline
  __saysdfterm_zsh_hints
  # Hooks added now are not run by zsh for this prompt: run the upstream ones once by hand.
  local h
  for h in __wezterm_semantic_precmd __wezterm_user_vars_precmd __wezterm_osc7; do
    if (( ${precmd_functions[(Ie)$h]} )); then
      (exit $ret); $h
    fi
  done
  return $ret
}

if [[ -o interactive && -z "${__saysdfterm_zsh_loaded-}" ]]; then
  precmd_functions+=(__saysdfterm_zsh_deferred)
fi
