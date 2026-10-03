use super::*;

#[test]
fn elvish_non_repl_and_invalid_launches_have_no_integration_side_effects() {
    let cases: &[&[&str]] = &[
        &["--lsp"],
        &["--daemon"],
        &["--version"],
        &["--buildinfo"],
        &["--help"],
        &["-h"],
        &["--h=false"],
        &["--norc"],
        &["--norc=false", "-norc"],
        &["-version=0", "--version=True"],
        &["--c", "echo hi"],
        &["--c=false", "script.elv"],
        &["-i", "script.elv"],
        &["login"],
        &["-"],
        &[""],
        &["--", "-i"],
        &["--", "--"],
        &["--", "-rc=config"],
        &["--rc", "custom", "--", "--norc=false"],
        &["--future-mode"],
        &["-il"],
        &["-rcfile"],
        &["---i"],
        &["-rc"],
        &["--log"],
        &["--cpuprofile"],
        &["-db"],
        &["-i=false", "false"],
        &["-i=no"],
        &["-norc="],
        &["--version=FaLsE"],
        &["-i=true=false"],
        &["--deprecation-level"],
    ];
    for args in cases {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("must-not-be-created");
        let mut config = super::tests::shell("elvish", args);
        config
            .env
            .insert("XDG_DATA_DIRS".into(), "/user/data".into());
        let launch = prepare_shell_launch_in(&config, &base).unwrap();
        assert_ne!(
            launch.integration,
            ShellIntegrationSupport::Unavailable(ELVISH_MANUAL_INSTRUCTIONS.into()),
            "{args:?}"
        );
        assert_eq!(launch.program, config.program);
        assert_eq!(launch.args, config.args);
        assert_eq!(
            launch.env,
            config.env.into_iter().collect::<Vec<_>>(),
            "{args:?}"
        );
        assert!(!base.exists(), "{args:?}");
    }
}

#[test]
fn elvish_repl_options_follow_boolean_order_and_opaque_value_boundaries() {
    let cases: &[&[&str]] = &[
        &[],
        &["--"],
        &["-i", "--l"],
        &["--i=false", "-l=TRUE"],
        &["-norc", "--norc=false"],
        &["-lsp", "-lsp=0", "--daemon", "--daemon=F"],
        &[
            "--help",
            "-help=false",
            "-version",
            "--version=f",
            "-buildinfo",
            "-buildinfo=False",
        ],
        &["-c"],
        &["--c=1", "--compileonly", "--json"],
        &["--rc", "-norc", "--"],
        &["--rc=-version"],
        &["-rc", "earlier", "--rc", "custom rc"],
        &["--rc="],
        &["--log", "--norc", "--db=path", "-sock", "--lsp"],
        &[
            "--cpuprofile=cpu file",
            "--allocsprofile",
            "allocation file",
        ],
        &["--deprecation-level", "-1"],
        &["--deprecation-level", "-i"],
        &["-deprecation-level=0x_15"],
        &["-deprecation-level=not-a-number"],
    ];
    for args in cases {
        let temp = tempfile::tempdir().unwrap();
        let config = super::tests::shell("elvish", args);
        let launch = prepare_shell_launch_in(&config, temp.path()).unwrap();
        assert_eq!(
            launch.integration,
            ShellIntegrationSupport::Unavailable(ELVISH_MANUAL_INSTRUCTIONS.into()),
            "{args:?}"
        );
        assert_eq!(launch.args, config.args);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("elvish/lib/termeric-integration.elv"))
                .unwrap(),
            ELVISH_MODULE
        );
        assert!(launch.env.iter().any(|(key, _)| key == "XDG_DATA_DIRS"));
    }
}
