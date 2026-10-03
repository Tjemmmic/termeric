use super::elvish_environment::configure as expose_elvish_module;
use super::*;

const TRANSPORT: &str = "TERMERIC_ELVISH_INTEGRATION_XDG_DATA_DIRS";
const HAD: &str = "TERMERIC_ELVISH_HAD_XDG_DATA_DIRS";
const ORIGINAL: &str = "TERMERIC_ELVISH_ORIGINAL_XDG_DATA_DIRS";

fn value(env: &[(String, String)], key: &str) -> Option<String> {
    env.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

#[test]
fn elvish_profile_path_overrides_stale_transport_markers() {
    let temp = tempfile::tempdir().unwrap();
    for transport in [None, Some("/old/transport:/old/data")] {
        let mut env = vec![
            ("XDG_DATA_DIRS".into(), "/profile/one:/profile/two".into()),
            (HAD.into(), "1".into()),
            (ORIGINAL.into(), "/old/data".into()),
        ];
        if let Some(transport) = transport {
            env.push((TRANSPORT.into(), transport.into()));
        }
        expose_elvish_module(temp.path(), &mut env).unwrap();
        assert_eq!(
            value(&env, ORIGINAL).as_deref(),
            Some("/profile/one:/profile/two")
        );
        assert_eq!(value(&env, "XDG_DATA_DIRS"), value(&env, TRANSPORT));
        assert_eq!(
            std::env::split_paths(&value(&env, "XDG_DATA_DIRS").unwrap()).collect::<Vec<_>>(),
            [
                temp.path().to_path_buf(),
                PathBuf::from("/profile/one"),
                PathBuf::from("/profile/two")
            ]
        );
    }
}

#[test]
fn elvish_repreparation_unwraps_only_identified_transports() {
    let temp = tempfile::tempdir().unwrap();
    let mut env = vec![("XDG_DATA_DIRS".into(), "/original:/original:/last".into())];
    for base in [temp.path().join("one"), temp.path().join("two")] {
        expose_elvish_module(&base, &mut env).unwrap();
        assert_eq!(
            value(&env, ORIGINAL).as_deref(),
            Some("/original:/original:/last")
        );
        assert_eq!(value(&env, "XDG_DATA_DIRS"), value(&env, TRANSPORT));
        assert_eq!(
            std::env::split_paths(&value(&env, "XDG_DATA_DIRS").unwrap()).collect::<Vec<_>>(),
            [
                base,
                PathBuf::from("/original"),
                PathBuf::from("/original"),
                PathBuf::from("/last")
            ]
        );
    }
    // Without a transport marker the inherited path is the user's own.
    let mut ambiguous = vec![
        ("XDG_DATA_DIRS".into(), "/unknown/root:/original".into()),
        (HAD.into(), "1".into()),
        (ORIGINAL.into(), "/original".into()),
    ];
    expose_elvish_module(temp.path(), &mut ambiguous).unwrap();
    assert_eq!(
        value(&ambiguous, ORIGINAL).as_deref(),
        Some("/unknown/root:/original")
    );
}

#[test]
fn elvish_empty_search_environment_keeps_native_default_roots() {
    let temp = tempfile::tempdir().unwrap();
    let mut env = vec![("XDG_DATA_DIRS".into(), String::new())];
    expose_elvish_module(temp.path(), &mut env).unwrap();
    assert_eq!(value(&env, HAD).as_deref(), Some("1"));
    assert_eq!(value(&env, ORIGINAL).as_deref(), Some(""));
    assert_eq!(
        std::env::split_paths(&value(&env, "XDG_DATA_DIRS").unwrap()).collect::<Vec<_>>(),
        [
            temp.path().to_path_buf(),
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share")
        ]
    );
    // Re-preparation must distinguish an originally absent value from an
    // explicitly empty one, while both retain the same default search roots.
    for present in [false, true] {
        let mut env = vec![
            ("XDG_DATA_DIRS".into(), "/known/transport".into()),
            (TRANSPORT.into(), "/known/transport".into()),
            (HAD.into(), if present { "1" } else { "0" }.into()),
            (ORIGINAL.into(), String::new()),
        ];
        expose_elvish_module(temp.path(), &mut env).unwrap();
        assert_eq!(
            value(&env, HAD).as_deref(),
            Some(if present { "1" } else { "0" })
        );
        assert_eq!(value(&env, ORIGINAL).as_deref(), Some(""));
        assert_eq!(
            value(&env, "XDG_DATA_DIRS").unwrap(),
            format!("{}:/usr/local/share:/usr/share", temp.path().display())
        );
    }
}

#[test]
fn elvish_module_restores_only_unchanged_transport_and_cleans_child_environment() {
    let Some(program) = super::tests::test_shell("elvish") else {
        return;
    };
    for (original, mutation, prepared) in [
        (None, None, true),
        (Some(""), None, true),
        (Some("/one:/two"), None, true),
        (Some("/path with space:/quote'\"/é:/one:/one"), None, true),
        (Some("/original"), Some(Some("/rc/override")), true),
        (Some("/original"), Some(Some("")), true),
        (Some("/original"), Some(None), true),
        (Some("/manual/path"), None, false),
        (Some(""), None, false),
        (None, None, false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut env: Vec<(String, String)> = Vec::new();
        // Construct an explicit transport so inherited host variables cannot
        // change the absence/empty cases in this native module test.
        if prepared {
            env.extend([
                ("XDG_DATA_DIRS".into(), "/test/transport:/test/data".into()),
                (TRANSPORT.into(), "/test/transport:/test/data".into()),
                (
                    HAD.into(),
                    if original.is_some() { "1" } else { "0" }.into(),
                ),
                (ORIGINAL.into(), original.unwrap_or_default().into()),
            ]);
        } else if let Some(original) = original {
            env.push(("XDG_DATA_DIRS".into(), original.into()));
        }
        let expected = mutation.unwrap_or(original);
        let prefix = match mutation {
            Some(Some(_)) => "set-env XDG_DATA_DIRS $E:TEST_EXPECTED\n",
            Some(None) => "unset-env XDG_DATA_DIRS\n",
            None => "",
        };
        // As in the cwd byte tests, only edit:'s unavailable script-mode hook
        // registry is stubbed. The whole production module runs unchanged.
        let script = format!(
            r#"var edit: = (ns [&before-readline=[] &after-readline=[] &after-command=[]])
{prefix}{ELVISH_MODULE}
if (!= (count $edit:before-readline) 1) {{ fail missing-hook }}
/bin/sh -c 'if [ "$TEST_PRESENT" = 1 ]; then test "${{XDG_DATA_DIRS+x}}" = x && test "$XDG_DATA_DIRS" = "$TEST_EXPECTED" || exit 81; else test "${{XDG_DATA_DIRS+x}}" != x || exit 82; fi; test "${{TERMERIC_ELVISH_HAD_XDG_DATA_DIRS+x}}${{TERMERIC_ELVISH_ORIGINAL_XDG_DATA_DIRS+x}}${{TERMERIC_ELVISH_INTEGRATION_XDG_DATA_DIRS+x}}" = "" || exit 83'
put [&present=(has-env XDG_DATA_DIRS) &value=$E:XDG_DATA_DIRS] | to-json
"#
        );
        let output = std::process::Command::new(&program)
            .args(["-norc", "-c", &script])
            .env_clear()
            .envs(env)
            .env("HOME", temp.path())
            .env("TEST_EXPECTED", expected.unwrap_or_default())
            .env("TEST_PRESENT", if expected.is_some() { "1" } else { "0" })
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{original:?}/{mutation:?}/{prepared}: {}",
            String::from_utf8_lossy(&output.stderr).escape_debug()
        );
        assert!(output.stderr.is_empty(), "{:?}", output.stderr);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
            serde_json::json!({"present": expected.is_some(), "value": expected.unwrap_or_default()})
        );
    }
}
