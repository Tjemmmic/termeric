use super::*;
use std::process::Command;

#[test]
fn zsh_launches_that_cannot_run_integration_are_preserved() {
    let cases: &[&[&str]] = &[
        &["-f"],
        &["-dfi"],
        &["+o", "rcs"],
        &["-o", "No_Rcs"],
        &["--no-rcs"],
        &["+-rcs"],
        &["+i"],
        &["--no-interactive"],
        &["--help"],
        &["--version"],
        &["--emulate", "sh"],
        &["-c", "print ok"],
        &["-ic", "print ok"],
        &["-cprint ok"],
        &["script.zsh"],
        &["login"],
        &["--", "-i"],
        &["-b", "-i"],
        &["-i-", "script.zsh"],
        &["-i-f"],
        &["--future-option"],
        &["-o"],
        &["-o", "shoptionletters"],
        &["--privileged"],
        &["--rcs=false"],
        &["-f "],
        &["- ", "-i"],
        &["-bfi", "+f"],
    ];
    for args in cases {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("must-not-be-created");
        let mut config = super::tests::shell("zsh", args);
        config.env.insert("ZDOTDIR".into(), "/user/config".into());
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
fn zsh_launch_eligibility_respects_option_order_and_stdin_operands() {
    let cases: &[&[&str]] = &[
        &[],
        &["-dil"],
        &["-f", "+f"],
        &["+i", "-i"],
        &["+o", "rcs", "-o", "R_C_S"],
        &["-ono_rcs", "+ono_rcs"],
        &["--no-rcs", "--rcs"],
        &["+-no-rcs"],
        &["--interactive", "--login", "--no-global-rcs"],
        &["-i", "--"],
        &["-i-"],
        &["-bdi"],
        &["-is", "argument", "-c", "not a command"],
        &["-is", "--", "-f"],
        &["-is", "-b", "-f"],
        &["--shin-stdin", "--", "argument"],
    ];
    let temp = tempfile::tempdir().unwrap();
    for args in cases {
        let config = super::tests::shell("zsh", args);
        let launch = prepare_shell_launch_in(&config, temp.path()).unwrap();
        assert!(launch.integration.is_enabled(), "{args:?}");
        assert_eq!(launch.args, config.args);
        assert_eq!(launch.program, config.program);
    }
}

#[cfg(unix)]
#[test]
fn zsh_user_startup_chain_matches_native_order_scope_and_directory_changes() {
    let Some(program) = super::tests::test_shell("zsh") else {
        return;
    };
    for case in [
        "interactive",
        "login",
        "redirect",
        "env-stop",
        "profile-stop",
        "nounset",
        "unset-nounset",
        "unexported",
        "readonly",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let initial = temp.path().join("initial config");
        let profile = temp.path().join("profile config");
        let rc = temp.path().join("rc config");
        for dir in [&home, &initial, &profile, &rc] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let mut files = Vec::new();
        for (name, phase) in [
            (".zshenv", "env"),
            (".zprofile", "profile"),
            (".zshrc", "rc"),
            (".zlogin", "login"),
            (".zlogout", "logout"),
        ] {
            let mut contents =
                format!("print -r -- \"{phase}:${{ZDOTDIR-UNSET}}\"\ntypeset USER_STAGE={phase}\n");
            match (case, phase) {
                ("redirect", "env") => contents.push_str("export ZDOTDIR=$TEST_PROFILE\n"),
                ("redirect", "profile") => contents.push_str("export ZDOTDIR=$TEST_RC\n"),
                ("env-stop", "env") | ("profile-stop", "profile") => {
                    contents.push_str("unsetopt rcs\n")
                }
                ("nounset", "env") => contents.push_str("setopt nounset\n"),
                ("unset-nounset", "env") => contents.push_str("unset ZDOTDIR\nsetopt nounset\n"),
                ("unexported", "env") => contents.push_str("typeset -g +x ZDOTDIR=$TEST_PROFILE\n"),
                ("readonly", "env") => contents.push_str("readonly ZDOTDIR=$TEST_RC\n"),
                _ => {}
            }
            let dir = match (case, phase) {
                ("unset-nounset", "profile" | "rc" | "login" | "logout") => &home,
                ("unexported", "profile" | "rc" | "login" | "logout") => &profile,
                ("redirect", "profile") => &profile,
                ("redirect", "rc" | "login" | "logout")
                | ("readonly", "profile" | "rc" | "login" | "logout") => &rc,
                _ => &initial,
            };
            let path = dir.join(name);
            std::fs::write(&path, &contents).unwrap();
            files.push((path, contents));
        }
        let mut config = super::tests::shell(
            &program,
            if case == "interactive" {
                &["-d", "-i"]
            } else {
                &["-d", "-i", "-l"]
            },
        );
        for (key, value) in [
            ("HOME", &home),
            ("ZDOTDIR", &initial),
            ("TEST_PROFILE", &profile),
            ("TEST_RC", &rc),
        ] {
            config
                .env
                .insert(key.into(), value.to_str().unwrap().into());
        }
        let launch = prepare_shell_launch_in(&config, &temp.path().join("generated")).unwrap();
        let run = |env: &[(String, String)], integrated: bool| {
            let script = r#"print -r -- "body:${USER_STAGE-missing}:${ZDOTDIR-UNSET}:${(t)ZDOTDIR-}"
print -r -- "markers:${+TERMERIC_ORIGINAL_ZDOTDIR}:${+TERMERIC_ZSH_HAD_ZDOTDIR}:${+TERMERIC_ZSH_INTEGRATION_DIR}:${+TERMERIC_ZSH_ZDOTDIR_EXPORTED}"
if whence -w __termeric_precmd >/dev/null; then __termeric_precmd >/dev/null; fi
if [[ $TEST_EXPECT_HOOK == yes ]]; then
    whence -w __termeric_precmd >/dev/null || exit 42
elif whence -w __termeric_precmd >/dev/null; then
    exit 43
fi
"#;
            let output = Command::new(&program)
                .args(&launch.args)
                .args(["-c", script])
                .env_clear()
                .envs(env.iter().cloned())
                .env(
                    "TEST_EXPECT_HOOK",
                    if integrated && !matches!(case, "env-stop" | "profile-stop" | "readonly") {
                        "yes"
                    } else {
                        "no"
                    },
                )
                .current_dir(&home)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{case} integrated={integrated}: {:?}",
                output.stderr
            );
            assert!(
                output.stderr.is_empty(),
                "{case} integrated={integrated}: {:?}",
                output.stderr
            );
            String::from_utf8(output.stdout).unwrap()
        };
        let expected = run(&config.env.into_iter().collect::<Vec<_>>(), false);
        let actual = run(&launch.env, true);
        assert_eq!(actual, expected, "{case}");
        assert!(actual.contains("markers:0:0:0:0"), "{case}: {actual}");
        for (path, contents) in files {
            assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
        }
    }
}

fn value<'a>(launch: &'a ShellLaunch, key: &str) -> &'a str {
    let values: Vec<_> = launch.env.iter().filter(|(name, _)| name == key).collect();
    assert_eq!(values.len(), 1, "{key} must have one authoritative value");
    &values[0].1
}

#[test]
fn zsh_profile_directory_wins_over_stale_transport_markers() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = super::tests::shell("zsh", &[]);
    for (key, value) in [
        ("ZDOTDIR", "/profile/config with spaces"),
        ("TERMERIC_ZSH_INTEGRATION_DIR", "/old/generated/zsh"),
        ("TERMERIC_ZSH_HAD_ZDOTDIR", "1"),
        ("TERMERIC_ORIGINAL_ZDOTDIR", "/old/user/config"),
    ] {
        config.env.insert(key.into(), value.into());
    }
    let launch = prepare_shell_launch_in(&config, temp.path()).unwrap();
    assert_eq!(
        value(&launch, "TERMERIC_ORIGINAL_ZDOTDIR"),
        "/profile/config with spaces"
    );
    assert_eq!(value(&launch, "TERMERIC_ZSH_HAD_ZDOTDIR"), "1");
    assert_eq!(
        value(&launch, "ZDOTDIR"),
        value(&launch, "TERMERIC_ZSH_INTEGRATION_DIR")
    );
}

#[test]
fn zsh_nested_prepared_launch_recovers_unset_empty_and_custom_directories() {
    let temp = tempfile::tempdir().unwrap();
    for original in [None, Some(""), Some("/original/config")] {
        let mut config = super::tests::shell("zsh", &[]);
        for (key, value) in [
            ("ZDOTDIR", "/previous/generated/zsh"),
            ("TERMERIC_ZSH_INTEGRATION_DIR", "/previous/generated/zsh"),
            (
                "TERMERIC_ZSH_HAD_ZDOTDIR",
                if original.is_some() { "1" } else { "0" },
            ),
            ("TERMERIC_ORIGINAL_ZDOTDIR", original.unwrap_or_default()),
        ] {
            config.env.insert(key.into(), value.into());
        }
        for directory in [temp.path().join("first"), temp.path().join("second")] {
            let launch = prepare_shell_launch_in(&config, &directory).unwrap();
            assert_eq!(
                value(&launch, "ZDOTDIR"),
                directory.join("zsh").to_str().unwrap()
            );
            assert_eq!(
                value(&launch, "ZDOTDIR"),
                value(&launch, "TERMERIC_ZSH_INTEGRATION_DIR")
            );
            assert_eq!(
                value(&launch, "TERMERIC_ORIGINAL_ZDOTDIR"),
                original.unwrap_or_default()
            );
            assert_eq!(
                value(&launch, "TERMERIC_ZSH_HAD_ZDOTDIR"),
                if original.is_some() { "1" } else { "0" }
            );
            config.env = launch.env.into_iter().collect();
        }
    }
}

#[test]
fn zsh_legacy_transport_does_not_recursively_source_its_own_rc() {
    let temp = tempfile::tempdir().unwrap();
    let transport = temp.path().join("zsh");
    let transport = transport.to_str().unwrap();
    for (original, expected) in [
        (None, None),
        (Some(""), None),
        (Some("/user/config"), Some("/user/config")),
        (Some(transport), None),
    ] {
        let mut env = vec![("ZDOTDIR".into(), transport.into())];
        if let Some(original) = original {
            env.push(("TERMERIC_ORIGINAL_ZDOTDIR".into(), original.into()));
        }
        // Test the environment helper directly without mutating process-wide
        // variables; explicit marker values isolate it from parent markers.
        env.push(("TERMERIC_ZSH_HAD_ZDOTDIR".into(), "legacy".into()));
        env.push(("TERMERIC_ZSH_INTEGRATION_DIR".into(), String::new()));
        if original.is_none() {
            env.push(("TERMERIC_ORIGINAL_ZDOTDIR".into(), String::new()));
        }
        configure_zsh_startup(Path::new(transport), &mut env).unwrap();
        let launch = ShellLaunch {
            program: None,
            args: vec![],
            env,
            integration: ShellIntegrationSupport::Enabled("zsh"),
        };
        assert_eq!(
            value(&launch, "TERMERIC_ORIGINAL_ZDOTDIR"),
            expected.unwrap_or_default()
        );
        assert_eq!(
            value(&launch, "TERMERIC_ZSH_HAD_ZDOTDIR"),
            if expected.is_some() { "1" } else { "0" }
        );
    }
}

#[cfg(unix)]
#[test]
fn zsh_startup_restores_user_directory_before_rc_and_nested_shells() {
    let Some(program) = super::tests::test_shell("zsh") else {
        return;
    };
    for case in ["custom", "missing", "redirect", "unset"] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let custom = temp.path().join("custom config");
        let redirected = temp.path().join("redirected config");
        let base = temp.path().join("generated");
        for directory in [&home, &custom, &redirected] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let home_rc = "print -r -- \"HOME_RC:${ZDOTDIR-UNSET}\"\n";
        let custom_rc = if case == "redirect" {
            "print -r -- \"CUSTOM_RC:${ZDOTDIR-UNSET}\"\nexport ZDOTDIR=$TEST_REDIRECT\n"
        } else {
            "print -r -- \"CUSTOM_RC:${ZDOTDIR-UNSET}\"\n"
        };
        std::fs::write(home.join(".zshrc"), home_rc).unwrap();
        if case != "missing" {
            std::fs::write(custom.join(".zshrc"), custom_rc).unwrap();
        }
        std::fs::write(redirected.join(".zshrc"), "print -r -- REDIRECTED_RC\n").unwrap();

        let mut config = super::tests::shell(&program, &["-d", "-i"]);
        for (key, value) in [
            ("HOME", home.to_str().unwrap()),
            ("TEST_ZSH", &program),
            ("TEST_REDIRECT", redirected.to_str().unwrap()),
        ] {
            config.env.insert(key.into(), value.into());
        }
        let mut baseline = config.env.clone();
        if case == "unset" {
            // Model a nested launch before the previous generated rc ran.
            // This also isolates the fixture from the host's ZDOTDIR.
            config
                .env
                .insert("ZDOTDIR".into(), "/previous/generated/zsh".into());
            config.env.insert(
                "TERMERIC_ZSH_INTEGRATION_DIR".into(),
                "/previous/generated/zsh".into(),
            );
            config
                .env
                .insert("TERMERIC_ZSH_HAD_ZDOTDIR".into(), "0".into());
            config
                .env
                .insert("TERMERIC_ORIGINAL_ZDOTDIR".into(), String::new());
        } else {
            config
                .env
                .insert("ZDOTDIR".into(), custom.to_str().unwrap().into());
            baseline.insert("ZDOTDIR".into(), custom.to_str().unwrap().into());
        }
        let launch = prepare_shell_launch_in(&config, &base).unwrap();
        // -c is appended only by this fixture, after eligibility preparation:
        // it runs assertions following real interactive .zshrc startup without
        // requiring a GUI or synthesizing prompt keystrokes.
        let script = r#"print -r -- "FINAL:${ZDOTDIR-UNSET}"
print -r -- "MARKERS:${+TERMERIC_ORIGINAL_ZDOTDIR}:${+TERMERIC_ZSH_HAD_ZDOTDIR}:${+TERMERIC_ZSH_INTEGRATION_DIR}"
"$TEST_ZSH" -d -i -c 'print -r -- "CHILD:${ZDOTDIR-UNSET}"'
"#;
        let run = |env: &[(String, String)]| {
            let output = Command::new(&program)
                .args(&launch.args)
                .args(["-c", script])
                .env_clear()
                .envs(env.iter().cloned())
                .current_dir(&home)
                .output()
                .unwrap();
            assert!(output.status.success(), "{case}: {:?}", output.stderr);
            assert!(output.stderr.is_empty(), "{case}: {:?}", output.stderr);
            output.stdout
        };
        let expected = run(&baseline.into_iter().collect::<Vec<_>>());
        let actual = run(&launch.env);
        assert_eq!(
            String::from_utf8_lossy(&actual),
            String::from_utf8_lossy(&expected),
            "{case}"
        );
        assert!(String::from_utf8_lossy(&actual).contains("MARKERS:0:0:0"));
        assert_eq!(
            std::fs::read_to_string(home.join(".zshrc")).unwrap(),
            home_rc
        );
        if case != "missing" {
            assert_eq!(
                std::fs::read_to_string(custom.join(".zshrc")).unwrap(),
                custom_rc
            );
        }
    }
}
