//! Native Bash child-environment checks; no user startup files or providers.
use super::*;
use std::process::Command;

#[test]
fn bash_allexport_does_not_export_private_hooks_or_change_prompt_export_choices() {
    let Some(program) = super::tests::test_shell("bash") else {
        return;
    };
    for setup in [
        "unset PROMPT_COMMAND PS0",
        "PROMPT_COMMAND=':'; PS0='user scalar '",
        "PROMPT_COMMAND=([2]=':' [9]='true'); PS0='user array '",
        "export PROMPT_COMMAND=':' PS0='user exported '",
    ] {
        for initially_enabled in [false, true] {
            // The runtime path also exercises enabling allexport *after*
            // installation, rather than only the startup option snapshot.
            let script = format!(
                r#"{setup}
before_options=$-
if [[ $ENABLE_ALLEXPORT == yes ]]; then set -a; fi
{BASH_HOOKS}
if [[ $ENABLE_ALLEXPORT == yes ]]; then
    [[ $- == *a* ]] || exit 41
else
    [[ $- == "$before_options" ]] || exit 42
    set -a
fi
USER_AFTER_INSTALL=retained
__termeric_before_prompt
__termeric_after_prompt
false
__termeric_before_prompt
[[ $? == 1 ]] || exit 43
"$TEST_BASH" --noprofile --norc -c '
    for name in __termeric_seen_command __termeric_allexport; do
        [[ ! -v $name ]] || {{ printf "LEAKED_VARIABLE:%s\n" "$name"; exit 44; }}
    done
    for name in __termeric_before_prompt __termeric_after_prompt; do
        if declare -F "$name" >/dev/null; then printf "LEAKED_FUNCTION:%s\n" "$name"; exit 45; fi
    done
    [[ $USER_AFTER_INSTALL == retained ]] || exit 46
    for name in PROMPT_COMMAND PS0; do
        if [[ $EXPECT_EXPORTED == yes ]]; then
            [[ -v $name ]] || exit 47
        else
            [[ ! -v $name ]] || {{ printf "ACCIDENTAL_EXPORT:%s\n" "$name"; exit 48; }}
        fi
    done
    printf "CHILD_OK\n"
' || exit $?
[[ $- == *a* ]] || exit 49
"#
            );
            let output = Command::new(&program)
                .args(["--noprofile", "--norc", "-c", &script])
                .env_clear()
                .env("TEST_BASH", &program)
                .env(
                    "ENABLE_ALLEXPORT",
                    if initially_enabled { "yes" } else { "no" },
                )
                .env(
                    "EXPECT_EXPORTED",
                    if setup.starts_with("export") {
                        "yes"
                    } else {
                        "no"
                    },
                )
                // Simulate stale private state exported by an outer shell.
                .env("__termeric_seen_command", "1")
                .env("PWD", "/test")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "setup={setup:?}, allexport={initially_enabled}: {output:?}"
            );
            assert!(output.stderr.is_empty(), "{output:?}");
            let bytes = &output.stdout;
            assert!(bytes.ends_with(b"CHILD_OK\n"), "{output:?}");
            assert_eq!(
                bytes
                    .windows(8)
                    .filter(|part| *part == b"\x1b]133;D;")
                    .count(),
                1,
                "stale inherited state must not end a nonexistent command: {output:?}"
            );
            assert!(
                bytes.windows(10).any(|part| part == b"\x1b]133;D;1\x07"),
                "{output:?}"
            );
        }
    }
}
