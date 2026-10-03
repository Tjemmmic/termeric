//! Login startup bootstrap, independent of the ordinary --rcfile path.
use super::{Context, Path, Result, effective_env, upsert_env};

const ENV_INDIRECTION: &str = "${TERMERIC_BASH_LOGIN_RC}";

pub(super) fn is_prepared(args: &[String], env: &[(String, String)]) -> bool {
    args.first().is_some_and(|arg| arg == "--posix")
        && effective_env(env, "ENV").as_deref() == Some(ENV_INDIRECTION)
        && effective_env(env, "TERMERIC_BASH_LOGIN_RC").is_some_and(|path| !path.is_empty())
}

pub(super) fn configure(
    args: &mut Vec<String>,
    env: &mut Vec<(String, String)>,
    path: &Path,
    no_profile: bool,
) -> Result<()> {
    let prepared = is_prepared(args, env);
    let original_env = if prepared {
        if effective_env(env, "TERMERIC_BASH_LOGIN_ENV_SET").as_deref() == Some("1") {
            effective_env(env, "TERMERIC_BASH_LOGIN_ENV")
        } else {
            None
        }
    } else {
        effective_env(env, "ENV")
    };
    let generated = path
        .to_str()
        .context("Bash login integration path is not valid UTF-8")?;
    let had_history = effective_env(env, "HISTFILE").is_some();
    let inherit_errexit = effective_env(env, "BASHOPTS")
        .is_some_and(|value| value.split(':').any(|option| option == "inherit_errexit"));
    upsert_env(
        env,
        "TERMERIC_BASH_LOGIN_ENV_SET",
        if original_env.is_some() { "1" } else { "0" }.into(),
    );
    upsert_env(
        env,
        "TERMERIC_BASH_LOGIN_ENV",
        original_env.unwrap_or_default(),
    );
    upsert_env(env, "TERMERIC_BASH_LOGIN_RC", generated.into());
    upsert_env(
        env,
        "TERMERIC_BASH_LOGIN_NO_PROFILE",
        if no_profile { "1" } else { "0" }.into(),
    );
    upsert_env(
        env,
        "TERMERIC_BASH_LOGIN_HISTFILE_SET",
        if had_history { "1" } else { "0" }.into(),
    );
    upsert_env(
        env,
        "TERMERIC_BASH_LOGIN_INHERIT_ERREXIT",
        if inherit_errexit { "1" } else { "0" }.into(),
    );
    // ENV undergoes shell expansion. One level of parameter indirection keeps
    // spaces, dollar signs, and command-substitution syntax in paths literal.
    upsert_env(env, "ENV", ENV_INDIRECTION.into());
    if !prepared {
        args.insert(0, "--posix".into());
    }
    Ok(())
}
