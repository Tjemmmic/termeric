//! Recognized Fish REPL launch syntax, not a full shell CLI validator.
//! Return the insertion point after user init commands and before a final --.

pub(super) fn integration_index(args: &[String]) -> Option<usize> {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            return (index + 1 == args.len()).then_some(index);
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value)));
            match name {
                "interactive" | "login" | "private" | "print-rusage-self" if value.is_none() => {}
                "init-command" | "debug" | "debug-output" | "profile" | "profile-startup" => {
                    if value.is_none() {
                        index += 1;
                        args.get(index)?;
                    }
                }
                // No-config/no-execute, commands, utility modes, feature
                // grammar changes, abbreviations, and unknown options bypass
                // integration. Fish remains responsible for their validation.
                _ => return None,
            }
        } else {
            let short = arg.strip_prefix('-').filter(|short| !short.is_empty())?;
            let mut flags = short.chars();
            while let Some(flag) = flags.next() {
                match flag {
                    'i' | 'l' | 'P' => {}
                    'C' | 'd' | 'o' | 'p' => {
                        if flags.as_str().is_empty() {
                            index += 1;
                            args.get(index)?;
                        }
                        // Attached or separate values are opaque, including
                        // strings that look like flags or end markers.
                        break;
                    }
                    _ => return None,
                }
            }
        }
        index += 1;
    }
    Some(index)
}
