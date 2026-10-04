use super::*;

#[test]
fn bash_posix_environment_is_not_claimed_as_rc_integration() {
    for (name, value) in [
        ("POSIXLY_CORRECT", ""),
        ("POSIXLY_CORRECT", "1"),
        ("SHELLOPTS", "hashall:posix:braceexpand"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("must-not-be-created");
        let mut config = super::tests::shell("bash", &["-i"]);
        config.env.insert(name.into(), value.into());
        let launch = prepare_shell_launch_in(&config, &base).unwrap();
        assert!(!launch.integration.is_enabled());
        assert_eq!(launch.args, config.args);
        assert_eq!(launch.env, config.env.into_iter().collect::<Vec<_>>());
        assert!(!base.exists());
    }
}

#[test]
fn bash_repreparing_a_launch_preserves_the_original_rc_without_recursion() {
    let temp = tempfile::tempdir().unwrap();
    for custom in [None, Some(""), Some("/user/custom rc")] {
        let mut config = super::tests::shell("bash", &[]);
        if let Some(path) = custom {
            config.args = vec!["--rcfile".into(), path.into()];
        }
        for directory in [temp.path().join("first"), temp.path().join("second")] {
            let launch = prepare_shell_launch_in(&config, &directory).unwrap();
            assert_eq!(launch.args.len(), 2);
            assert_eq!(
                launch.args[1],
                directory.join("bash/.bashrc").to_str().unwrap()
            );
            assert_eq!(launch.env.len(), 3);
            assert!(launch.env.contains(&(
                "TERMERIC_BASH_CUSTOM_RC".into(),
                if custom.is_some() { "1" } else { "0" }.into()
            )));
            assert!(launch.env.contains(&(
                "TERMERIC_BASH_RCFILE".into(),
                custom.unwrap_or_default().into()
            )));
            config.args = launch.args;
            config.env = launch.env.into_iter().collect();
        }
    }
}

#[test]
fn bash_custom_rc_startup_matches_native_file_selection_and_child_environment() {
    let Some(program) = super::tests::test_shell("bash") else {
        return;
    };
    for requested in [
        None,
        Some("profile rc"),
        Some("absolute"),
        Some("-rc"),
        Some("rc; $(printf injected)"),
        Some(""),
        Some("missing"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("cwd");
        let decoy = temp.path().join("path-decoy");
        for directory in [&home, &cwd, &decoy] {
            std::fs::create_dir(directory).unwrap();
        }
        let home_rc = "USER_RC_MARK=home\nprintf 'HOME_RC\\n'\n";
        let custom_rc = "USER_RC_MARK=custom\nprintf 'CUSTOM_RC\\n'\n";
        std::fs::write(home.join(".bashrc"), home_rc).unwrap();
        if let Some(name) = requested.filter(|name| !name.is_empty() && *name != "missing") {
            std::fs::write(cwd.join(name), custom_rc).unwrap();
            std::fs::write(decoy.join(name), "printf 'WRONG_PATH_RC\\n'\n").unwrap();
        }
        let mut config = super::tests::shell(&program, &["--noprofile"]);
        if let Some(name) = requested {
            config.args.extend([
                "--rcfile".into(),
                "unused earlier rc".into(),
                "--init-file".into(),
                if name == "absolute" {
                    cwd.join(name).to_str().unwrap().into()
                } else {
                    name.into()
                },
            ]);
        }
        config.args.extend(["-i".into(), "+m".into()]);
        config
            .env
            .insert("HOME".into(), home.to_str().unwrap().into());
        config
            .env
            .insert("PATH".into(), format!("{}:/usr/bin:/bin", decoy.display()));
        config.env.insert("TEST_BASH".into(), program.clone());
        let launch = prepare_shell_launch_in(&config, &temp.path().join("generated")).unwrap();
        let script = r#"printf 'FINAL:%s\n' "${USER_RC_MARK-unset}"
printf 'MARKERS:%s:%s:%s\n' "${TERMERIC_BASH_CUSTOM_RC+x}" "${TERMERIC_BASH_RCFILE+x}" "${TERMERIC_BASH_INTEGRATION_RC+x}"
if [[ $TEST_EXPECT_HOOK == yes ]]; then declare -F __termeric_after_prompt >/dev/null || exit 42; fi
"$TEST_BASH" --noprofile -i +m -c 'printf "CHILD:%s\n" "${USER_RC_MARK-unset}"'
"#;
        let run = |args: &[String], env: &[(String, String)], integrated| {
            let output = std::process::Command::new(&program)
                .args(args)
                .args(["-c", script])
                .env_clear()
                .envs(env.iter().cloned())
                .env("TEST_EXPECT_HOOK", if integrated { "yes" } else { "no" })
                .current_dir(&cwd)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{requested:?}: {:?}",
                output.stderr
            );
            output
        };
        // -c is appended by this startup fixture only; the preparation path
        // itself is separately tested to leave command invocations untouched.
        let expected = run(
            &config.args,
            &config.env.into_iter().collect::<Vec<_>>(),
            false,
        );
        let actual = run(&launch.args, &launch.env, true);
        assert_eq!(
            String::from_utf8_lossy(&actual.stdout),
            String::from_utf8_lossy(&expected.stdout),
            "{requested:?}"
        );
        // Bash warns about absent job control without a PTY. Require the same
        // native diagnostics, not a blanket suppression of arbitrary errors.
        assert_eq!(actual.stderr, expected.stderr, "{requested:?}");
        assert!(String::from_utf8_lossy(&actual.stdout).contains("MARKERS:::\n"));
        assert_eq!(
            std::fs::read_to_string(home.join(".bashrc")).unwrap(),
            home_rc
        );
        if let Some(name) = requested.filter(|name| !name.is_empty() && *name != "missing") {
            assert_eq!(std::fs::read_to_string(cwd.join(name)).unwrap(), custom_rc);
        }
    }
}

#[test]
fn bash_non_rc_launches_are_preserved_without_generating_files() {
    let cases: &[&[&str]] = &[
        &["-c", "printf ok"],
        &["-ic", "printf ok"],
        &["-cprintf ok"],
        &["script.sh"],
        &["login"],
        &["--", "-i"],
        &["--norc"],
        &["--posix"],
        &["--help"],
        &["--version"],
        &["--restricted"],
        &["-p"],
        &["-n"],
        &["--dump-strings"],
        &["--pretty-print"],
        &["--future-option"],
        &["--rcfile"],
        &["--rcfile=config"],
        &["-i", "--rcfile", "config"],
        &["-o"],
        &["-o", "posix"],
        &["-O", "extdebug"],
        &["-ovi"],
        &["+i"],
        &["+"],
        &[""],
    ];
    for args in cases {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("must-not-be-created");
        let mut config = super::tests::shell("bash", args);
        config.env.insert("PROFILE_MARK".into(), "exact".into());
        let launch = prepare_shell_launch_in(&config, &base).unwrap();
        assert_eq!(launch.program, config.program, "{args:?}");
        assert_eq!(launch.args, config.args, "{args:?}");
        assert_eq!(
            launch.env,
            config.env.into_iter().collect::<Vec<_>>(),
            "{args:?}"
        );
        assert!(!launch.integration.is_enabled(), "{args:?}");
        assert!(!base.exists(), "{args:?}");
    }
}

#[test]
fn bash_custom_rc_replaces_only_the_last_rc_value() {
    let temp = tempfile::tempdir().unwrap();
    let args = [
        "--rcfile",
        "first config",
        "--init-file",
        "last 'config; literal",
        "--noprofile",
        "-i",
    ];
    let launch = prepare_shell_launch_in(&super::tests::shell("bash", &args), temp.path()).unwrap();
    assert!(launch.integration.is_enabled());
    assert_eq!(launch.args.len(), args.len());
    for (index, original) in args.iter().enumerate() {
        if index != 3 {
            assert_eq!(&launch.args[index], original);
        }
    }
    assert_eq!(std::fs::read_to_string(&launch.args[3]).unwrap(), BASH_RC);
    assert!(
        launch
            .env
            .contains(&("TERMERIC_BASH_CUSTOM_RC".into(), "1".into()))
    );
    assert!(
        launch
            .env
            .contains(&("TERMERIC_BASH_RCFILE".into(), args[3].into()))
    );
}

#[test]
fn bash_repl_options_and_stdin_parameters_preserve_argument_boundaries() {
    let cases: &[&[&str]] = &[
        &[],
        &["--noprofile", "-i"],
        &["--noediting", "-i"],
        &["-iu"],
        &["-io", "vi"],
        &["-O", "extglob", "-i"],
        &["+i", "-i"],
        &["-is", "argument", "-c", "not a command"],
        &["-is", "--", "-f"],
        &["-i", "--"],
        &["-i", "-"],
    ];
    let temp = tempfile::tempdir().unwrap();
    for args in cases {
        let launch =
            prepare_shell_launch_in(&super::tests::shell("bash", args), temp.path()).unwrap();
        assert!(launch.integration.is_enabled(), "{args:?}");
        assert_eq!(launch.args[0], "--rcfile");
        assert_eq!(
            launch.args[2..],
            args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>()
        );
    }
}
