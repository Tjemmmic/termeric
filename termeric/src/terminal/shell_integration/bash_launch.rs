//! Eligibility and custom-rc routing for normal interactive, non-login Bash.
use super::{Context, Path, Result, effective_env, upsert_env};

pub(super) struct InteractiveLaunch {
    rcfile_arg: Option<usize>,
}

pub(super) fn classify(args: &[String]) -> Option<InteractiveLaunch> {
    let mut interactive = true;
    let mut stdin = false;
    let mut short_options = false;
    let mut rcfile_arg = None;
    let mut args = args.iter().enumerate();
    while let Some((_, arg)) = args.next() {
        if matches!(arg.as_str(), "-" | "--") {
            return (interactive && (args.next().is_none() || stdin))
                .then_some(InteractiveLaunch { rcfile_arg });
        }
        if arg.starts_with("--") {
            // Bash requires its long options before all short options.
            if short_options {
                return None;
            }
            match arg.as_str() {
                "--rcfile" | "--init-file" => rcfile_arg = Some(args.next()?.0),
                "--noprofile" | "--noediting" | "--verbose" => {}
                // Login/POSIX/no-rc modes do not read our generated rc.
                // Unknown/debug/restricted modes keep their native launch.
                _ => return None,
            }
            continue;
        }
        let enabled = arg.starts_with('-');
        let Some(short) = arg.strip_prefix(['-', '+']) else {
            return (interactive && stdin).then_some(InteractiveLaunch { rcfile_arg });
        };
        if short.is_empty() {
            return None;
        }
        short_options = true;
        let mut flags = short.chars();
        while let Some(flag) = flags.next() {
            match flag {
                'i' => interactive = enabled,
                's' => stdin = enabled,
                'a' | 'b' | 'f' | 'h' | 'k' | 'm' | 'u' | 'v' | 'x' | 'B' | 'C' | 'E' | 'H'
                | 'P' | 'T' => {}
                'o' | 'O' if flags.as_str().is_empty() => {
                    // Bash takes the value from the following argv word,
                    // unlike Zsh's attached -oname form.
                    let (_, name) = args.next()?;
                    let known = if flag == 'o' {
                        matches!(
                            name.as_str(),
                            "vi" | "emacs"
                                | "history"
                                | "ignoreeof"
                                | "interactive-comments"
                                | "pipefail"
                                | "nounset"
                                | "noglob"
                                | "noclobber"
                                | "braceexpand"
                                | "hashall"
                                | "histexpand"
                                | "monitor"
                                | "notify"
                                | "verbose"
                                | "xtrace"
                                | "physical"
                                | "allexport"
                                | "keyword"
                                | "errtrace"
                                | "functrace"
                        )
                    } else {
                        matches!(
                            name.as_str(),
                            "extglob"
                                | "globstar"
                                | "nullglob"
                                | "dotglob"
                                | "nocaseglob"
                                | "checkwinsize"
                                | "histappend"
                        )
                    };
                    if !known {
                        return None;
                    }
                }
                _ => return None,
            }
        }
    }
    interactive.then_some(InteractiveLaunch { rcfile_arg })
}

pub(super) fn configure(
    args: &mut Vec<String>,
    env: &mut Vec<(String, String)>,
    path: &Path,
) -> Result<()> {
    let options = classify(args).context("Bash arguments cannot load integration")?;
    let generated = path
        .to_str()
        .context("Bash integration path is not valid UTF-8")?;
    let original = if let Some(index) = options.rcfile_arg {
        let requested = std::mem::replace(&mut args[index], generated.into());
        let previous = effective_env(env, "TERMERIC_BASH_INTEGRATION_RC");
        if requested == generated || Some(&requested) == previous.as_ref() {
            // Re-preparing an unconsumed launch must not source a generated
            // wrapper recursively or install prompt hooks twice.
            if effective_env(env, "TERMERIC_BASH_CUSTOM_RC").as_deref() == Some("1") {
                effective_env(env, "TERMERIC_BASH_RCFILE")
                    .filter(|value| value != generated && Some(value) != previous.as_ref())
            } else {
                None
            }
        } else {
            Some(requested)
        }
    } else {
        args.splice(0..0, ["--rcfile".into(), generated.into()]);
        None
    };
    upsert_env(
        env,
        "TERMERIC_BASH_CUSTOM_RC",
        if original.is_some() { "1" } else { "0" }.into(),
    );
    upsert_env(env, "TERMERIC_BASH_RCFILE", original.unwrap_or_default());
    upsert_env(env, "TERMERIC_BASH_INTEGRATION_RC", generated.into());
    Ok(())
}
