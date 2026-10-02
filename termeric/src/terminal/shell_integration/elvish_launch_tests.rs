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
        &["--deprecation-level", "-i"],
        &["-deprecation-level=08"],
        &["-deprecation-level=0x"],
        &["-deprecation-level=1__0"],
        &["-deprecation-level=1_"],
        &["-deprecation-level=_1"],
        &["-deprecation-level=0x__1"],
        &["-deprecation-level=999999999999999999999999"],
        &["-deprecation-level=+"],
        &["-deprecation-level="],
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
        &["-deprecation-level=+21"],
        &["--deprecation-level=025"],
        &["-deprecation-level=0x_15"],
        &["--deprecation-level=0B1_0101"],
        &["-deprecation-level=-0o25"],
        &["--deprecation-level=0_25"],
        &["--deprecation-level=0"],
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

#[cfg(unix)]
#[test]
fn elvish_typed_option_values_match_the_native_parser() {
    let Some(program) = super::tests::test_shell("elvish") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let integers = [
        "0",
        "-0",
        "+21",
        "-21",
        "025",
        "-0x15",
        "0X_FF",
        "0o_25",
        "0b_10101",
        "0_25",
        "2_1",
        "",
        "08",
        "-09",
        "0x",
        "_1",
        "1_",
        "0x__1",
        "1__0",
        "++1",
        "-+1",
        "0b2",
        "0o8",
        "0xg",
        " 21",
        "21 ",
        "２１",
        "999999999999999999999999999999",
    ]
    .map(String::from);
    let limits = [
        isize::MAX.to_string(),
        isize::MIN.to_string(),
        ((isize::MAX as i128) + 1).to_string(),
        ((isize::MIN as i128) - 1).to_string(),
    ];
    let booleans = [
        "1", "0", "t", "f", "T", "F", "true", "false", "TRUE", "FALSE", "True", "False", "", "yes",
        "no", "FaLsE", " true", "2",
    ];
    for (name, value) in integers
        .iter()
        .chain(&limits)
        .map(|value| ("deprecation-level", value.as_str()))
        .chain(booleans.iter().map(|value| ("i", *value)))
    {
        let argument = format!("--{name}={value}");
        // Version is a local utility; parsing still validates every supplied
        // flag, but this never starts an editor, daemon, or user rc.
        let native = std::process::Command::new(&program)
            .args([&argument, "--version"])
            .env_clear()
            .env("HOME", temp.path())
            .output()
            .unwrap();
        let eligible = elvish_launch::is_interactive(std::slice::from_ref(&argument));
        assert_eq!(
            eligible,
            native.status.success(),
            "{argument}: {:?}",
            native.stderr
        );
        if eligible {
            assert!(native.stderr.is_empty(), "{argument}: {:?}", native.stderr);
            assert!(!native.stdout.is_empty());
        }
    }
}
