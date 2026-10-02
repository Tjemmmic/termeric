use super::*;

#[test]
fn fish_non_repl_launches_are_preserved_without_side_effects() {
    let cases: &[&[&str]] = &[
        &["-c", "printf ok"],
        &["-cprintf ok"],
        &["-ic", "printf ok"],
        &["--command=printf ok"],
        &["--command", "printf ok"],
        &["script.fish"],
        &["login"],
        &["-"],
        &["-i", "script.fish", "--private"],
        &["--", "-i"],
        &["--", "--"],
        &["--no-config"],
        &["-iN"],
        &["-n"],
        &["-in"],
        &["--no-execute"],
        &["--help"],
        &["-ih"],
        &["--version"],
        &["-Pv"],
        &["--print-debug-categories"],
        &["--features", "qmark-noglob"],
        &["--future-option"],
        &["--interact"],
        &["--interactive=yes"],
        &["--init-command"],
        &["-iC"],
        &["--debug"],
        &["-o"],
        &["--profile-startup"],
        &["--init-command=printf ok", "script.fish"],
        &["-C", "--private", "-cprintf ok"],
        &["-C", "--", "script.fish"],
        &[""],
    ];
    for args in cases {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("must-not-be-created");
        let mut config = super::tests::shell("fish", args);
        config.env.insert("TEST_VALUE".into(), "unchanged".into());
        let launch = prepare_shell_launch_in(&config, &base).unwrap();
        assert!(!launch.integration.is_enabled(), "{args:?}");
        assert_eq!(launch.program, config.program);
        assert_eq!(launch.args, config.args, "{args:?}");
        assert_eq!(launch.env, config.env.into_iter().collect::<Vec<_>>());
        assert!(!base.exists());
    }
}

#[test]
fn fish_repl_options_preserve_user_words_before_separate_integration() {
    let cases: &[(&[&str], usize)] = &[
        (&[], 0),
        (&["-ilP"], 1),
        (&["--interactive", "--login", "--private"], 3),
        (&["--init-command", "printf '%s' 'a; b' # comment"], 2),
        (&["-iCset -g test one", "--init-command=set -a test two"], 2),
        (&["-C", "", "--init-command="], 3),
        (&["--debug", "--version", "-o", "--no-config"], 4),
        (&["-dreader", "--debug-output=debug file"], 2),
        (&["-p", "profile file", "--profile-startup=startup file"], 3),
        (&["-ipprofile", "--profile", "other file"], 3),
        (&["--debug=reader", "--print-rusage-self"], 2),
        (&["--profile-startup", "--", "--"], 2),
        (&["--"], 0),
        (&["-C", "--"], 2),
        (&["-C", "--", "--"], 2),
        (&["--init-command", "--command", "--private"], 3),
    ];
    for (args, insertion) in cases {
        let temp = tempfile::tempdir().unwrap();
        let config = super::tests::shell("fish", args);
        let launch = prepare_shell_launch_in(&config, temp.path()).unwrap();
        assert_eq!(
            launch.integration,
            ShellIntegrationSupport::Enabled("fish"),
            "{args:?}"
        );
        let mut expected = config.args;
        expected.splice(
            *insertion..*insertion,
            ["--init-command".into(), FISH_INIT.into()],
        );
        assert_eq!(launch.args, expected, "{args:?}");
        assert!(launch.env.is_empty());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[cfg(unix)]
#[test]
fn fish_startup_commands_preserve_native_order_status_and_user_hooks() {
    let Some(program) = super::tests::test_shell("fish") else {
        return;
    };
    for final_status in [0, 1, 23] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let rc = home.join(".config/fish/config.fish");
        std::fs::create_dir_all(rc.parent().unwrap()).unwrap();
        let rc_contents = "set -g startup_order rc\nprintf 'RC\\n'\n";
        std::fs::write(&rc, rc_contents).unwrap();
        let first = "set -a startup_order 'first; literal'\nfunctions --query __termeric_cwd; and exit 91\n# trailing comment";
        let second = format!(
            "set -a startup_order second\nfunction user_prompt_hook --on-event fish_prompt; set -g user_hook_ran yes; end\nfunction user_status; return {final_status}; end\nuser_status"
        );
        for args in [
            vec![
                "-C".into(),
                first.into(),
                "--init-command".into(),
                second.clone(),
            ],
            vec![
                format!("--init-command={first}"),
                format!("-C{second}"),
                "--".into(),
            ],
        ] {
            let mut config = super::tests::shell(&program, &[]);
            config.args = args;
            let launch = prepare_shell_launch_in(&config, &temp.path().join("generated")).unwrap();
            assert!(launch.integration.is_enabled());
            let run = |args: &[String], integrated| {
                let mut args = args.to_vec();
                // -c is appended only by this startup inspection fixture;
                // actual command invocations must bypass integration.
                let insertion =
                    args.len() - usize::from(args.last().is_some_and(|arg| arg == "--"));
                args.splice(
                    insertion..insertion,
                    [
                        "-c".into(),
                        r#"printf 'STATUS:%s\n' $status
printf 'ORDER:%s\n' $startup_order
user_prompt_hook
test "$user_hook_ran" = yes; or exit 92
set --query __termeric_init_status; and exit 93
functions --query __termeric_restore_init_status; and exit 94
if test "$TEST_INTEGRATED" = yes
    functions --query __termeric_cwd; or exit 95
else
    functions --query __termeric_cwd; and exit 96
end
printf 'DONE\n'
"#
                        .into(),
                    ],
                );
                let output = std::process::Command::new(&program)
                    .args(args)
                    .env_clear()
                    .env("HOME", &home)
                    .env("XDG_CONFIG_HOME", home.join(".config"))
                    .env("PATH", "/usr/bin:/bin")
                    .env("LC_ALL", "C.UTF-8")
                    .env("TEST_INTEGRATED", if integrated { "yes" } else { "no" })
                    .current_dir(&home)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{final_status}: {:?}",
                    output.stderr
                );
                assert!(output.stderr.is_empty(), "{:?}", output.stderr);
                output.stdout
            };
            let expected = run(&config.args, false);
            let actual = run(&launch.args, true);
            assert_eq!(
                actual, expected,
                "status={final_status}, args={:?}",
                config.args
            );
            assert_eq!(
                String::from_utf8(actual).unwrap(),
                format!(
                    "RC\nSTATUS:{final_status}\nORDER:rc\nORDER:first; literal\nORDER:second\nDONE\n"
                )
            );
            assert_eq!(std::fs::read_to_string(&rc).unwrap(), rc_contents);
        }
    }
}
