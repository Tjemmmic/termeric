if [[ -r "${ZDOTDIR-$HOME}/@STAGE_FILE@" ]]; then
    builtin source "${ZDOTDIR-$HOME}/@STAGE_FILE@"
fi
# Respect user suppression and readonly configuration. Native startup continues
# from the user's path when the temporary transport can no longer be used.
if [[ -o rcs && -o interactive && -n $__termeric_zsh_transport_dir && ${(t)ZDOTDIR-} != *readonly* ]]; then
    export TERMERIC_ZSH_HAD_ZDOTDIR=${+ZDOTDIR}
    export TERMERIC_ORIGINAL_ZDOTDIR="${ZDOTDIR-}"
    if [[ ${(t)ZDOTDIR-} == *export* ]]; then
        export TERMERIC_ZSH_ZDOTDIR_EXPORTED=1
    else
        export TERMERIC_ZSH_ZDOTDIR_EXPORTED=0
    fi
    export TERMERIC_ZSH_INTEGRATION_DIR="$__termeric_zsh_transport_dir"
    export ZDOTDIR="$__termeric_zsh_transport_dir"
fi
unset __termeric_zsh_transport_dir
