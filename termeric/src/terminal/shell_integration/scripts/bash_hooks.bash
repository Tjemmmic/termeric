# Installation must not automatically export private functions or turn the
# user's unexported prompt variables into child-shell configuration.
builtin declare +x __termeric_allexport=$-
builtin set +a
builtin unset __termeric_seen_command
__termeric_before_prompt() {
    local __termeric_status=$?
    if [ -n "${__termeric_seen_command-}" ]; then
        printf '\033]133;D;%s\007' "$__termeric_status"
    fi
    return "$__termeric_status"
}
__termeric_after_prompt() {
    local __termeric_status=$?
    # Encode bytes, independent of the user's locale. Raw controls in a file
    # name must never terminate OSC 7 or begin another terminal command.
    local LC_ALL=C
    local __termeric_cwd=${PWD-}
    # Match the receiver's 4096-byte URI ceiling, including "file://".
    if [[ $__termeric_cwd == /* && ${#__termeric_cwd} -le 4089 ]]; then
        if [[ $__termeric_cwd == *[!a-zA-Z0-9/._~-]* ]]; then
            local __termeric_raw=$__termeric_cwd
            local __termeric_char __termeric_hex __termeric_index
            __termeric_cwd=
            for ((__termeric_index=0; __termeric_index<${#__termeric_raw}; __termeric_index++)); do
                __termeric_char=${__termeric_raw:__termeric_index:1}
                case $__termeric_char in
                    [a-zA-Z0-9/._~-]) __termeric_cwd+=$__termeric_char ;;
                    *) printf -v __termeric_hex '%%%02X' "'$__termeric_char"
                       __termeric_cwd+=$__termeric_hex ;;
                esac
                if [[ ${#__termeric_cwd} -gt 4089 ]]; then
                    __termeric_cwd=
                    break
                fi
            done
        fi
        if [[ -n $__termeric_cwd ]]; then
            printf '\033]7;file://%s\007' "$__termeric_cwd"
        fi
    fi
    printf '\033]133;A\007\033]133;B\007'
    __termeric_seen_command=1
    # The user may enable allexport after installation, too. This flag belongs
    # only to this shell, never to a child shell's first prompt.
    builtin export -n __termeric_seen_command
    return "$__termeric_status"
}
# Let Bash execute each original command, rather than evaluating it inside a
# wrapper whose conditionals overwrite $? and whose local scope changes it.
if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )) &&
    [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a'* ]]; then
    PROMPT_COMMAND=(__termeric_before_prompt "${PROMPT_COMMAND[@]}" __termeric_after_prompt)
else
    # Preserve scalar commands as scalars, also supporting older Bash. Newlines
    # preserve trailing comments instead of appending unsafe semicolons.
    PROMPT_COMMAND=$'__termeric_before_prompt\n'"${PROMPT_COMMAND-}"$'\n__termeric_after_prompt'
fi
PS0="${PS0-}"$'\033]133;C\007'
if [[ $__termeric_allexport == *a* ]]; then
    builtin unset __termeric_allexport
    builtin set -a
else
    builtin unset __termeric_allexport
fi
