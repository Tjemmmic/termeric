# Generated login bootstrap. Restore Bash mode before user startup.
if [[ $- != *i* || ${ENV-} != '${TERMERIC_BASH_LOGIN_RC}' || -z ${TERMERIC_BASH_LOGIN_RC-} ]] || ! builtin shopt -q login_shell; then
    builtin return
fi
builtin set +o posix
if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )) && [[ ${TERMERIC_BASH_LOGIN_INHERIT_ERREXIT-0} != 1 ]]; then
    builtin shopt -u inherit_errexit
fi
if [[ ${TERMERIC_BASH_LOGIN_HISTFILE_SET-0} != 1 ]]; then
    builtin declare +x HISTFILE="$HOME/.bash_history"
fi
if [[ ${TERMERIC_BASH_LOGIN_ENV_SET-0} == 1 ]]; then
    builtin export ENV="${TERMERIC_BASH_LOGIN_ENV-}"
else
    builtin unset ENV
fi
builtin declare +x __termeric_no_profile="${TERMERIC_BASH_LOGIN_NO_PROFILE-0}"
builtin unset TERMERIC_BASH_LOGIN_ENV_SET TERMERIC_BASH_LOGIN_ENV TERMERIC_BASH_LOGIN_RC TERMERIC_BASH_LOGIN_NO_PROFILE TERMERIC_BASH_LOGIN_HISTFILE_SET TERMERIC_BASH_LOGIN_INHERIT_ERREXIT
if [[ $__termeric_no_profile != 1 ]]; then
    builtin unset __termeric_no_profile
    if [[ -r /etc/profile ]]; then builtin source /etc/profile; fi
    if [[ -f "$HOME/.bash_profile" && -r "$HOME/.bash_profile" ]]; then
        builtin source "$HOME/.bash_profile"
    elif [[ -f "$HOME/.bash_login" && -r "$HOME/.bash_login" ]]; then
        builtin source "$HOME/.bash_login"
    elif [[ -f "$HOME/.profile" && -r "$HOME/.profile" ]]; then
        builtin source "$HOME/.profile"
    fi
else
    builtin unset __termeric_no_profile
fi
