_termeric() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="termeric"
                ;;
            termeric,control)
                cmd="termeric__subcmd__control"
                ;;
            termeric,domain)
                cmd="termeric__subcmd__domain"
                ;;
            termeric,mcp)
                cmd="termeric__subcmd__mcp"
                ;;
            termeric,plugin)
                cmd="termeric__subcmd__plugin"
                ;;
            termeric,workspace)
                cmd="termeric__subcmd__workspace"
                ;;
            termeric__subcmd__control,close-pane)
                cmd="termeric__subcmd__control__subcmd__close__subcmd__pane"
                ;;
            termeric__subcmd__control,close-tab)
                cmd="termeric__subcmd__control__subcmd__close__subcmd__tab"
                ;;
            termeric__subcmd__control,close-window)
                cmd="termeric__subcmd__control__subcmd__close__subcmd__window"
                ;;
            termeric__subcmd__control,focus-pane)
                cmd="termeric__subcmd__control__subcmd__focus__subcmd__pane"
                ;;
            termeric__subcmd__control,focus-window)
                cmd="termeric__subcmd__control__subcmd__focus__subcmd__window"
                ;;
            termeric__subcmd__control,get-text)
                cmd="termeric__subcmd__control__subcmd__get__subcmd__text"
                ;;
            termeric__subcmd__control,launch-status)
                cmd="termeric__subcmd__control__subcmd__launch__subcmd__status"
                ;;
            termeric__subcmd__control,launch-tab)
                cmd="termeric__subcmd__control__subcmd__launch__subcmd__tab"
                ;;
            termeric__subcmd__control,launch-window)
                cmd="termeric__subcmd__control__subcmd__launch__subcmd__window"
                ;;
            termeric__subcmd__control,list)
                cmd="termeric__subcmd__control__subcmd__list"
                ;;
            termeric__subcmd__control,move-pane)
                cmd="termeric__subcmd__control__subcmd__move__subcmd__pane"
                ;;
            termeric__subcmd__control,move-tab)
                cmd="termeric__subcmd__control__subcmd__move__subcmd__tab"
                ;;
            termeric__subcmd__control,new-tab)
                cmd="termeric__subcmd__control__subcmd__new__subcmd__tab"
                ;;
            termeric__subcmd__control,new-window)
                cmd="termeric__subcmd__control__subcmd__new__subcmd__window"
                ;;
            termeric__subcmd__control,resize-divider)
                cmd="termeric__subcmd__control__subcmd__resize__subcmd__divider"
                ;;
            termeric__subcmd__control,resize-window)
                cmd="termeric__subcmd__control__subcmd__resize__subcmd__window"
                ;;
            termeric__subcmd__control,select-tab)
                cmd="termeric__subcmd__control__subcmd__select__subcmd__tab"
                ;;
            termeric__subcmd__control,send-text)
                cmd="termeric__subcmd__control__subcmd__send__subcmd__text"
                ;;
            termeric__subcmd__control,split-pane)
                cmd="termeric__subcmd__control__subcmd__split__subcmd__pane"
                ;;
            termeric__subcmd__control,wait-launch)
                cmd="termeric__subcmd__control__subcmd__wait__subcmd__launch"
                ;;
            termeric__subcmd__domain,attach)
                cmd="termeric__subcmd__domain__subcmd__attach"
                ;;
            termeric__subcmd__domain,balance-tab)
                cmd="termeric__subcmd__domain__subcmd__balance__subcmd__tab"
                ;;
            termeric__subcmd__domain,close-pane)
                cmd="termeric__subcmd__domain__subcmd__close__subcmd__pane"
                ;;
            termeric__subcmd__domain,close-tab)
                cmd="termeric__subcmd__domain__subcmd__close__subcmd__tab"
                ;;
            termeric__subcmd__domain,create)
                cmd="termeric__subcmd__domain__subcmd__create"
                ;;
            termeric__subcmd__domain,focus)
                cmd="termeric__subcmd__domain__subcmd__focus"
                ;;
            termeric__subcmd__domain,get-text)
                cmd="termeric__subcmd__domain__subcmd__get__subcmd__text"
                ;;
            termeric__subcmd__domain,hosts)
                cmd="termeric__subcmd__domain__subcmd__hosts"
                ;;
            termeric__subcmd__domain,kill)
                cmd="termeric__subcmd__domain__subcmd__kill"
                ;;
            termeric__subcmd__domain,layout-tab)
                cmd="termeric__subcmd__domain__subcmd__layout__subcmd__tab"
                ;;
            termeric__subcmd__domain,list)
                cmd="termeric__subcmd__domain__subcmd__list"
                ;;
            termeric__subcmd__domain,move-pane)
                cmd="termeric__subcmd__domain__subcmd__move__subcmd__pane"
                ;;
            termeric__subcmd__domain,move-tab)
                cmd="termeric__subcmd__domain__subcmd__move__subcmd__tab"
                ;;
            termeric__subcmd__domain,panes)
                cmd="termeric__subcmd__domain__subcmd__panes"
                ;;
            termeric__subcmd__domain,pin-tab)
                cmd="termeric__subcmd__domain__subcmd__pin__subcmd__tab"
                ;;
            termeric__subcmd__domain,protocol-version)
                cmd="termeric__subcmd__domain__subcmd__protocol__subcmd__version"
                ;;
            termeric__subcmd__domain,proxy-stdio)
                cmd="termeric__subcmd__domain__subcmd__proxy__subcmd__stdio"
                ;;
            termeric__subcmd__domain,relay)
                cmd="termeric__subcmd__domain__subcmd__relay"
                ;;
            termeric__subcmd__domain,rename-tab)
                cmd="termeric__subcmd__domain__subcmd__rename__subcmd__tab"
                ;;
            termeric__subcmd__domain,resize-split)
                cmd="termeric__subcmd__domain__subcmd__resize__subcmd__split"
                ;;
            termeric__subcmd__domain,send)
                cmd="termeric__subcmd__domain__subcmd__send"
                ;;
            termeric__subcmd__domain,server)
                cmd="termeric__subcmd__domain__subcmd__server"
                ;;
            termeric__subcmd__domain,show)
                cmd="termeric__subcmd__domain__subcmd__show"
                ;;
            termeric__subcmd__domain,spawn)
                cmd="termeric__subcmd__domain__subcmd__spawn"
                ;;
            termeric__subcmd__domain,split)
                cmd="termeric__subcmd__domain__subcmd__split"
                ;;
            termeric__subcmd__domain,stop-server)
                cmd="termeric__subcmd__domain__subcmd__stop__subcmd__server"
                ;;
            termeric__subcmd__domain,swap-pane)
                cmd="termeric__subcmd__domain__subcmd__swap__subcmd__pane"
                ;;
            termeric__subcmd__domain,unpin-tab)
                cmd="termeric__subcmd__domain__subcmd__unpin__subcmd__tab"
                ;;
            termeric__subcmd__domain,upload)
                cmd="termeric__subcmd__domain__subcmd__upload"
                ;;
            termeric__subcmd__plugin,configure)
                cmd="termeric__subcmd__plugin__subcmd__configure"
                ;;
            termeric__subcmd__plugin,disable)
                cmd="termeric__subcmd__plugin__subcmd__disable"
                ;;
            termeric__subcmd__plugin,enable)
                cmd="termeric__subcmd__plugin__subcmd__enable"
                ;;
            termeric__subcmd__plugin,grant)
                cmd="termeric__subcmd__plugin__subcmd__grant"
                ;;
            termeric__subcmd__plugin,install)
                cmd="termeric__subcmd__plugin__subcmd__install"
                ;;
            termeric__subcmd__plugin,list)
                cmd="termeric__subcmd__plugin__subcmd__list"
                ;;
            termeric__subcmd__plugin,remove)
                cmd="termeric__subcmd__plugin__subcmd__remove"
                ;;
            termeric__subcmd__plugin,revoke)
                cmd="termeric__subcmd__plugin__subcmd__revoke"
                ;;
            termeric__subcmd__plugin,show)
                cmd="termeric__subcmd__plugin__subcmd__show"
                ;;
            termeric__subcmd__plugin,update)
                cmd="termeric__subcmd__plugin__subcmd__update"
                ;;
            termeric__subcmd__plugin,validate)
                cmd="termeric__subcmd__plugin__subcmd__validate"
                ;;
            termeric__subcmd__workspace,list)
                cmd="termeric__subcmd__workspace__subcmd__list"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        termeric)
            opts="-h -V --help --new-window --separate-process --quick-access --tab --split --cwd --working-directory --command --terminal-profile --profile --workspace --title --geometry --class --app-id --renderer --version domain control plugin workspace mcp"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --split)
                    COMPREPLY=($(compgen -W "horizontal vertical" -- "${cur}"))
                    return 0
                    ;;
                --working-directory)
                    COMPREPLY=()
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o plusdirs
                    fi
                    return 0
                    ;;
                --cwd)
                    COMPREPLY=()
                    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                        compopt -o plusdirs
                    fi
                    return 0
                    ;;
                --command)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --terminal-profile)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --profile)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --workspace)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --geometry)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --app-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --class)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --renderer)
                    COMPREPLY=($(compgen -W "auto gpu software" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control)
            opts="list wait-launch launch-status get-text send-text new-window launch-window new-tab launch-tab split-pane focus-window select-tab focus-pane close-window close-tab close-pane move-tab move-pane resize-window resize-divider"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__close__subcmd__pane)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__close__subcmd__tab)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__close__subcmd__window)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__focus__subcmd__pane)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__focus__subcmd__window)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__get__subcmd__text)
            opts="--history-lines --bytes --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --history-lines)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bytes)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__launch__subcmd__status)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__launch__subcmd__tab)
            opts="--background --cwd --title --env --env-stdin --unset-env --clear-env --wait --timeout --request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --cwd)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --env)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --unset-env)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --timeout)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__launch__subcmd__window)
            opts="--cwd --title --env --env-stdin --unset-env --clear-env --wait --timeout --request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --cwd)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --env)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --unset-env)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --timeout)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__list)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__move__subcmd__pane)
            opts="--request-id --json horizontal vertical"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__move__subcmd__tab)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__new__subcmd__tab)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__new__subcmd__window)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__resize__subcmd__divider)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__resize__subcmd__window)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__select__subcmd__tab)
            opts="--request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__send__subcmd__text)
            opts="--paste --enter --request-id --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__split__subcmd__pane)
            opts="--request-id --json horizontal vertical"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --request-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__control__subcmd__wait__subcmd__launch)
            opts="--timeout --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --timeout)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain)
            opts="--host --host-profile protocol-version hosts list show panes create spawn split resize-split rename-tab pin-tab unpin-tab move-tab swap-pane balance-tab layout-tab move-pane focus close-pane close-tab send get-text upload attach kill stop-server server proxy-stdio relay"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --host-profile)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__attach)
            opts="--geometry --app-id"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --geometry)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --app-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__balance__subcmd__tab)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__close__subcmd__pane)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__close__subcmd__tab)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__create)
            opts="--cwd --working-directory --title --size --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --working-directory)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --cwd)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --size)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__get__subcmd__text)
            opts="--bytes --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --bytes)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__hosts)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__kill)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__layout__subcmd__tab)
            opts="columns rows grid tall fat"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__list)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__move__subcmd__pane)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__move__subcmd__tab)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__panes)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__pin__subcmd__tab)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__protocol__subcmd__version)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__proxy__subcmd__stdio)
            opts="--no-start"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__relay)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__rename__subcmd__tab)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__resize__subcmd__split)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__send)
            opts="--enter"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__server)
            opts="--foreground"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__show)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__spawn)
            opts="--cwd --working-directory --title --size --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --working-directory)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --cwd)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --size)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__split)
            opts="--cwd --working-directory --title --size --json horizontal vertical"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --working-directory)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --cwd)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --size)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__stop__subcmd__server)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__swap__subcmd__pane)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__unpin__subcmd__tab)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__domain__subcmd__upload)
            opts="--name --json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__mcp)
            opts="--process"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --process)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin)
            opts="list show validate install update enable disable configure grant revoke remove"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__configure)
            opts="--default-profile --clear-default-profile --set --unset --reset-settings"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --default-profile)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --set)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --unset)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__disable)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__enable)
            opts="--allow"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --allow)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__grant)
            opts="--allow"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --allow)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__install)
            opts="--allow --enable --replace"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --allow)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__list)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__remove)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__revoke)
            opts="--allow"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --allow)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__show)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__update)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__plugin__subcmd__validate)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__workspace)
            opts="list"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        termeric__subcmd__workspace__subcmd__list)
            opts="--json"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _termeric -o nosort -o bashdefault -o default termeric
else
    complete -F _termeric -o bashdefault -o default termeric
fi
