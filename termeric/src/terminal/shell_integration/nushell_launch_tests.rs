use super::*;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

#[test]
fn nushell_non_repl_and_unknown_launches_are_preserved_without_side_effects() {
    let cases: &[&[&str]] = &[
        &["--mcp"],
        &["--mcp", "--mcp-transport", "http", "--mcp-port", "3000"],
        &["--mcp-transport=stdio"],
        &["--help"],
        &["-h"],
        &["--version"],
        &["-iv"],
        &["--lsp"],
        &["--ide-ast"],
        &["--ide-check=1"],
        &["--testbin", "echo_env"],
        &["--commands", "print 1"],
        &["-ic", "print 1"],
        &["--commands=print 1"],
        &["--execute", "print setup"],
        &["-ie", "print setup"],
        &["--execute=print setup"],
        &["script.nu", "--interactive"],
        &["--", "-i"],
        &["--", "--mcp"],
        &["--stdin"],
        &["--no-newline"],
        &["--future-server"],
        &["--future-server=stdio"],
        &["-x"],
        &["-"],
        &[""],
        &["--config-home"],
        &["--config-home="],
        &["--interactive=false"],
        &["-mrounded"],
        &["-iI"],
        &["--config", "config.nu", "script.nu"],
        &["--log-include=nu_cli"],
    ];
    for values in cases {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("must-not-be-created");
        let mut config = super::tests::shell("nu", values);
        config.env.insert("PROFILE_MARK".into(), "unchanged".into());
        let launch = prepare_shell_launch_in(&config, &base).unwrap();
        assert_eq!(launch.program, config.program, "{values:?}");
        assert_eq!(launch.args, config.args, "{values:?}");
        assert_eq!(
            launch.env,
            vec![("PROFILE_MARK".into(), "unchanged".into())]
        );
        assert!(!launch.integration.is_enabled(), "{values:?}");
        assert!(!base.exists(), "{values:?}: unexpected generated files");
    }
}

#[test]
fn nushell_repl_options_keep_their_exact_argument_boundaries() {
    let cases: &[&[&str]] = &[
        &[],
        &["-iln"],
        &["--interactive", "--login", "--no-history", "--no-std-lib"],
        &["--no-config-file"],
        &["--config-home", "/tmp/custom config"],
        &["--config-home=/tmp/custom config"],
        &["--config", "--mcp"], // A value, not a server flag.
        &["--env-config", "env.nu", "--config", "config.nu"],
        &["-I", "library", "-m", "rounded"],
        &["-im", "rounded"],
        &["-iI=library"],
        &["--include-path=library", "--table-mode=rounded"],
        &[
            "--log-level",
            "info",
            "--log-target=file",
            "--log-file",
            "log",
        ],
        &[
            "--plugin-config",
            "registry",
            "--plugins",
            "[/tmp/a /tmp/b]",
        ],
        &["--error-style", "plain"],
        &["--threads", "2"], // Older supported Nushell releases.
        &["--"],
        &["-i", "--"],
    ];
    let temp = tempfile::tempdir().unwrap();
    for values in cases {
        let config = super::tests::shell("nu", values);
        let launch = prepare_shell_launch_in(&config, temp.path()).unwrap();
        assert_eq!(
            launch.integration,
            ShellIntegrationSupport::Enabled("nushell"),
            "{values:?}"
        );
        assert_eq!(&launch.args[..2], &["--execute", NUSHELL_INIT]);
        assert_eq!(&launch.args[2..], &args(values), "{values:?}");
        assert_eq!(launch.program, config.program);
        assert!(launch.env.is_empty());
    }
}
