use super::*;

#[test]
fn bash_login_bootstrap_ignores_unprepared_manual_sources() {
    let Some(program) = super::tests::test_shell("bash") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("bootstrap");
    std::fs::write(&path, format!("{}{BASH_HOOKS}", bash_login::STARTUP)).unwrap();
    for args in [
        vec!["--noprofile", "--norc"],
        vec!["--noprofile", "--norc", "--login", "-i"],
    ] {
        let output = std::process::Command::new(&program).args(args)
            .args(["-c", "builtin source \"$TEST_BOOTSTRAP\"; declare -F __termeric_after_prompt >/dev/null && exit 91; printf 'UNCHANGED\\n'"])
            .env_clear().env("HOME", temp.path()).env("TEST_BOOTSTRAP", &path).output().unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        assert_eq!(output.stdout, b"UNCHANGED\n");
    }
}

#[test]
fn bash_login_routing_and_repreparation_preserve_original_environment() {
    let temp = tempfile::tempdir().unwrap();
    for args in [
        vec!["--login"],
        vec!["-il"],
        vec!["+l"],
        vec!["-l", "+l"],
        vec!["--noprofile", "--login"],
        vec!["--login", "--rcfile", "ignored"],
        vec!["-ls", "parameter", "-c"],
    ] {
        let mut config = super::tests::shell("bash", &args);
        config.env.insert("ENV".into(), "/user/original env".into());
        for directory in [temp.path().join("one"), temp.path().join("two")] {
            let launch = prepare_shell_launch_in(&config, &directory).unwrap();
            assert!(launch.integration.is_enabled(), "{args:?}");
            assert_eq!(&launch.args[1..], args);
            assert_eq!(launch.args[0], "--posix");
            assert!(launch.env.contains(&(
                "TERMERIC_BASH_LOGIN_ENV".into(),
                "/user/original env".into()
            )));
            assert!(directory.join("bash/.bash-login").is_file());
            assert!(!directory.join("bash/.bashrc").exists());
            config.args = launch.args;
            config.env = launch.env.into_iter().collect();
        }
    }
    for args in [
        vec!["--login", "-c", "printf ok"],
        vec!["-lcprintf ok"],
        vec!["--login", "script"],
        vec!["--login", "--posix"],
        vec!["--login", "+i"],
        vec!["--norc", "--login"],
    ] {
        let base = temp.path().join("must-not-exist");
        let config = super::tests::shell("bash", &args);
        let launch = prepare_shell_launch_in(&config, &base).unwrap();
        assert!(!launch.integration.is_enabled());
        assert_eq!(launch.args, config.args);
        assert_eq!(launch.env, config.env.into_iter().collect::<Vec<_>>());
        assert!(!base.exists());
    }
}

#[test]
fn bash_login_preserves_exported_history_and_inherited_errexit_option() {
    let Some(program) = super::tests::test_shell("bash") else {
        return;
    };
    for history in [None, Some(""), Some("/dev/null")] {
        let temp = tempfile::tempdir().unwrap();
        let mut config = super::tests::shell(&program, &["--noprofile", "--login", "-i"]);
        config
            .env
            .insert("HOME".into(), temp.path().display().to_string());
        config
            .env
            .insert("BASHOPTS".into(), "inherit_errexit".into());
        if let Some(value) = history {
            config.env.insert("HISTFILE".into(), value.into());
        }
        let launch = prepare_shell_launch_in(&config, &temp.path().join("generated")).unwrap();
        let run = |args: &[String], env: &[(String, String)]| {
            let output = std::process::Command::new(&program)
                .args(args)
                .args([
                    "-c",
                    "declare -p HISTFILE; shopt -p inherit_errexit; exit 0",
                ])
                .env_clear()
                .envs(env.iter().cloned())
                .output()
                .unwrap();
            assert!(output.status.success());
            output
        };
        let native = run(&config.args, &config.env.into_iter().collect::<Vec<_>>());
        let actual = run(&launch.args, &launch.env);
        assert_eq!(actual.stdout, native.stdout, "{history:?}");
        assert_eq!(actual.stderr, native.stderr);
    }
}

#[test]
fn bash_login_bootstrap_matches_native_profiles_environment_and_logout() {
    let Some(program) = super::tests::test_shell("bash") else {
        return;
    };
    for (profile, no_profile) in [
        (".bash_profile", false),
        (".bash_login", false),
        (".profile", false),
        ("none", false),
        (".bash_profile", true),
    ] {
        for original_env in [None, Some(""), Some("/unused/ENV $(printf SHOULD_NOT_RUN)")] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            std::fs::create_dir(&home).unwrap();
            let snapshot = r#"printf 'OPTIONS:%s:%s\n' "$SHELLOPTS" "$BASHOPTS"
printf 'ENV:%s:%s\n' "${ENV+x}" "${ENV-}"
declare -p HISTFILE
shopt -q login_shell || exit 91
[[ -z $(compgen -A variable TERMERIC_BASH_LOGIN_) ]] || exit 92
"#;
            let profile_body = format!(
                "printf 'PROFILE:{profile}\\n'\n{snapshot}\ndeclare LOGIN_GLOBAL=visible\nreturn 17\n"
            );
            if profile != "none" {
                std::fs::write(home.join(profile), &profile_body).unwrap();
            }
            if profile == ".bash_profile" {
                std::fs::write(
                    home.join(".bash_login"),
                    "printf 'WRONG_LOWER_PROFILE\\n'\n",
                )
                .unwrap();
                std::fs::write(home.join(".profile"), "printf 'WRONG_LOWER_PROFILE\\n'\n").unwrap();
            }
            std::fs::write(home.join(".bashrc"), "printf 'WRONG_RC\\n'\n").unwrap();
            std::fs::write(home.join(".bash_logout"), "printf 'LOGOUT\\n'\n").unwrap();
            let generated = home.join("generated $(touch SHOULD_NOT_EXIST) é");
            std::fs::write(&generated, format!("{}{BASH_HOOKS}", bash_login::STARTUP)).unwrap();
            let mut args: Vec<String> = if no_profile {
                vec!["--noprofile".into()]
            } else {
                vec![]
            };
            args.extend([
                "--login".into(),
                "--rcfile".into(),
                "ignored rc".into(),
                "-i".into(),
            ]);
            let native_args = args.clone();
            let mut env = vec![
                ("HOME".into(), home.display().to_string()),
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("TERM".into(), "dumb".into()),
            ];
            if let Some(value) = original_env {
                env.push(("ENV".into(), value.into()));
            }
            let native_env = env.clone();
            bash_login::configure(&mut args, &mut env, &generated, no_profile).unwrap();
            let inspect =
                format!("{snapshot}\nprintf 'GLOBAL:%s\\n' \"${{LOGIN_GLOBAL-unset}}\"\nexit 0\n");
            let run = |args: &[String], env: &[(String, String)]| {
                let output = std::process::Command::new(&program)
                    .args(args)
                    .args(["-c", &inspect])
                    .env_clear()
                    .envs(env.iter().cloned())
                    .current_dir(&home)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{profile}/{no_profile}/{original_env:?}: {}",
                    String::from_utf8_lossy(&output.stderr).escape_debug()
                );
                output
            };
            let native = run(&native_args, &native_env);
            let integrated = run(&args, &env);
            assert_eq!(
                String::from_utf8_lossy(&integrated.stdout),
                String::from_utf8_lossy(&native.stdout),
                "{profile}/{no_profile}/{original_env:?}"
            );
            assert_eq!(integrated.stderr, native.stderr);
            assert!(!home.join("SHOULD_NOT_EXIST").exists());
            if profile != "none" {
                assert_eq!(
                    std::fs::read_to_string(home.join(profile)).unwrap(),
                    profile_body
                );
            }
        }
    }
}
