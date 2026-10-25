begin
    set -l __termeric_init_status $status
    function __termeric_restore_init_status
        functions --erase __termeric_restore_init_status
        return $argv[1]
    end
    function __termeric_report_cwd --argument-names __termeric_path
        string match --quiet '/*' -- "$__termeric_path"; or return 0
        # Fish counts characters here, not bytes. This bounds encoding work;
        # the ASCII URL below supplies the authoritative byte-length check.
        test (string length -- "$__termeric_path") -le 4089; or return 0
        set -l __termeric_encoded (string escape --style=url -- "$__termeric_path")
        test (count $__termeric_encoded) -eq 1; or return 0
        # Reserve seven bytes for "file://" within the receiver's 4-KiB URI cap.
        test (string length -- "$__termeric_encoded") -le 4089; or return 0
        printf '\e]7;file://%s\a' "$__termeric_encoded"
    end
    function __termeric_cwd --on-event fish_prompt
        __termeric_report_cwd "$PWD"
    end
    # Fish 4 marks its own prompts, with the command's start after the
    # prompt. Only where it doesn't (Fish 3, or mark-prompt turned off) does
    # Termeric mark them; the command then starts where typing begins.
    if not status test-feature mark-prompt 2>/dev/null
        function __termeric_preexec --on-event fish_preexec; printf '\e]133;C\a'; end
        function __termeric_postexec --on-event fish_postexec; printf '\e]133;D;%s\a' $status; end
        function __termeric_prompt --on-event fish_prompt; printf '\e]133;A\a\e]133;B\a'; end
    end
    __termeric_restore_init_status $__termeric_init_status
end
