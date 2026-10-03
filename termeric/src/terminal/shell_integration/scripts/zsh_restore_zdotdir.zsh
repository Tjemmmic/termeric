# ZDOTDIR is a temporary startup transport, not the user's configuration path.
# Restore it before user code runs, including the distinction between unset
# and explicitly empty. Do not leak transport markers into nested shells.
if [[ ${TERMERIC_ZSH_HAD_ZDOTDIR-} == 1 ]]; then
    if [[ ${TERMERIC_ZSH_ZDOTDIR_EXPORTED-1} == 0 ]]; then
        typeset -g +x ZDOTDIR="${TERMERIC_ORIGINAL_ZDOTDIR-}"
    else
        export ZDOTDIR="${TERMERIC_ORIGINAL_ZDOTDIR-}"
    fi
elif [[ ${TERMERIC_ZSH_HAD_ZDOTDIR-} == 0 ]]; then
    unset ZDOTDIR
fi
unset TERMERIC_ORIGINAL_ZDOTDIR TERMERIC_ZSH_HAD_ZDOTDIR TERMERIC_ZSH_INTEGRATION_DIR TERMERIC_ZSH_ZDOTDIR_EXPORTED
