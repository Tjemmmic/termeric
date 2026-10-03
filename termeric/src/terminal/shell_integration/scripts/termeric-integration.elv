{
    use str
    use re

    # Restore only the unchanged value that this launch actually installed.
    # User rc overrides/unsets and imports without transport markers are kept.
    if (and (has-env TERMERIC_ELVISH_INTEGRATION_XDG_DATA_DIRS) (has-env XDG_DATA_DIRS)) {
        var termeric-transport = $E:TERMERIC_ELVISH_INTEGRATION_XDG_DATA_DIRS
        if (and (not-eq $termeric-transport '') (eq $E:XDG_DATA_DIRS $termeric-transport)) {
            if (eq $E:TERMERIC_ELVISH_HAD_XDG_DATA_DIRS 1) {
                if (has-env TERMERIC_ELVISH_ORIGINAL_XDG_DATA_DIRS) {
                    set-env XDG_DATA_DIRS $E:TERMERIC_ELVISH_ORIGINAL_XDG_DATA_DIRS
                }
            } elif (eq $E:TERMERIC_ELVISH_HAD_XDG_DATA_DIRS 0) {
                unset-env XDG_DATA_DIRS
            }
        }
    }
    unset-env TERMERIC_ELVISH_HAD_XDG_DATA_DIRS
    unset-env TERMERIC_ELVISH_ORIGINAL_XDG_DATA_DIRS
    unset-env TERMERIC_ELVISH_INTEGRATION_XDG_DATA_DIRS

    fn termeric-report-pwd {
        var termeric-raw = $pwd
        # count measures bytes in Elvish strings. Reserve seven bytes for
        # file:// within the receiver's 4096-byte URI ceiling.
        if (or (not (str:has-prefix $termeric-raw /)) (> (count $termeric-raw) 4089)) {
            return
        }
        # Ordinary ASCII paths need no per-byte conversion.
        if (re:match '^[a-zA-Z0-9/._~-]+$' $termeric-raw) {
            printf "\e]7;file://%s\a" $termeric-raw
            return
        }
        var termeric-cwd = ''
        for termeric-byte [(str:to-utf8-bytes $termeric-raw)] {
            # Decode only known-safe ASCII. A single UTF-8 continuation or
            # leading byte is not a valid argument to str:from-utf8-bytes.
            var termeric-alnum = (or (<= 48 $termeric-byte 57) (<= 65 $termeric-byte 90) (<= 97 $termeric-byte 122))
            if (or $termeric-alnum (<= 45 $termeric-byte 47) (== $termeric-byte 95) (== $termeric-byte 126)) {
                set termeric-cwd = $termeric-cwd(str:from-utf8-bytes $termeric-byte)
            } else {
                set termeric-cwd = $termeric-cwd(printf '%%%02X' $termeric-byte)
            }
            # Do not emit a partial report when encoding exceeds the cap.
            if (> (count $termeric-cwd) 4089) { return }
        }
        printf "\e]7;file://%s\a" $termeric-cwd
    }
    fn termeric-before-readline {
        termeric-report-pwd
        printf "\e]133;A\a\e]133;B\a"
    }
    fn termeric-after-readline {|_| printf "\e]133;C\a" }
    fn termeric-after-command {|command-info|
        var termeric-status = 0
        if (not-eq $nil $command-info[error]) {
            set termeric-status = 1
            if (has-key $command-info[error] reason) {
                if (has-key $command-info[error][reason] exit-status) {
                    set termeric-status = $command-info[error][reason][exit-status]
                }
            }
        }
        printf "\e]133;D;%s\a" $termeric-status
    }

    set edit:before-readline = [ $@edit:before-readline $termeric-before-readline~ ]
    set edit:after-readline = [ $@edit:after-readline $termeric-after-readline~ ]
    set edit:after-command = [ $@edit:after-command $termeric-after-command~ ]
    set after-chdir = [ $@after-chdir {|_| termeric-report-pwd } ]
}
