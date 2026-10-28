# saysdfTerminal: fallback .zshrc for the ZDOTDIR trick.
# Normally never read: our .zshenv restores the user's ZDOTDIR, so zsh reads the user's .zshrc.
# It is read only if ZDOTDIR still points here (e.g. the user's .zshenv set it back to us or
# the ZDOTDIR restore was bypassed). Then: restore ZDOTDIR and load the user's .zshrc; the
# integration itself comes from the deferred precmd hook registered by our .zshenv.

typeset -g __saysdfterm_zsh_dir=${__saysdfterm_zsh_dir:-${${(%):-%x}:A:h}}
if [[ "${ZDOTDIR-}" == "$__saysdfterm_zsh_dir" ]]; then
  if [[ -n "${SAYSDFTERM_ORIG_ZDOTDIR-}" ]]; then
    export ZDOTDIR=$SAYSDFTERM_ORIG_ZDOTDIR
  else
    unset ZDOTDIR
  fi
  unset SAYSDFTERM_ORIG_ZDOTDIR
fi
if [[ -r "${ZDOTDIR:-$HOME}/.zshrc" && ! "${ZDOTDIR:-$HOME}/.zshrc" -ef "$__saysdfterm_zsh_dir/.zshrc" ]]; then
  source "${ZDOTDIR:-$HOME}/.zshrc"
fi
