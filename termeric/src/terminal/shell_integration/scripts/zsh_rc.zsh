if [[ -r "${ZDOTDIR-$HOME}/.zshrc" ]]; then
    source "${ZDOTDIR-$HOME}/.zshrc"
fi
__termeric_preexec() { printf '\033]133;C\007' }
__termeric_report_cwd() {
    # Localize options as well as locale: Zsh substring/pattern semantics must
    # not depend on user multibyte, indexing, or word-splitting preferences.
    emulate -L zsh
    setopt no_multibyte
    local LC_ALL=C
    local __termeric_cwd=${PWD-}
    # Bound the complete URI, including the seven bytes in "file://".
    [[ $__termeric_cwd == /* && ${#__termeric_cwd} -le 4089 ]] || return 0
    if [[ $__termeric_cwd == *[^a-zA-Z0-9/._~-]* ]]; then
        local __termeric_raw=$__termeric_cwd
        local __termeric_char __termeric_hex __termeric_index
        __termeric_cwd=
        for ((__termeric_index=0; __termeric_index<${#__termeric_raw}; __termeric_index++)); do
            __termeric_char=${__termeric_raw:$__termeric_index:1}
            case $__termeric_char in
                [a-zA-Z0-9/._~-]) __termeric_cwd+=$__termeric_char ;;
                *) printf -v __termeric_hex '%%%02X' "'$__termeric_char"
                   __termeric_cwd+=$__termeric_hex ;;
            esac
            [[ ${#__termeric_cwd} -le 4089 ]] || return 0
        done
    fi
    printf '\033]7;file://%s\007' "$__termeric_cwd"
}
__termeric_precmd() {
    local __termeric_status=$?
    if [[ -n "${__termeric_seen_command-}" ]]; then
        printf '\033]133;D;%s\007' "$__termeric_status"
    fi
    __termeric_report_cwd
    printf '\033]133;A\007'
    # The command starts where the prompt ends, so this mark goes at the end
    # of PS1, again on every prompt: themes may have just rebuilt it. Without
    # %{…%} the mark would count as prompt width, so then it goes here and
    # Termeric takes the command's start from where typing begins.
    if [[ -o prompt_percent ]]; then
        local __termeric_command_start=$'%{\033]133;B\007%}'
        if [[ ${PS1-} != *"$__termeric_command_start" ]]; then
            PS1="${PS1-}$__termeric_command_start"
        fi
    else
        printf '\033]133;B\007'
    fi
    # Run after hooks added later, such as a theme's, next time too.
    precmd_functions=(${precmd_functions:#__termeric_precmd} __termeric_precmd)
    __termeric_seen_command=1
}
typeset -ga preexec_functions precmd_functions
preexec_functions+=(__termeric_preexec)
precmd_functions+=(__termeric_precmd)
