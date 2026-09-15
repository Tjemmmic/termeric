//! Integration eligibility for native Zsh option syntax, not a CLI validator.
//! Unrecognized modes retain their original argv/environment without injection.

pub(super) fn is_interactive(args: &[String]) -> bool {
    let mut state = StartupState {
        interactive: true,
        rcs: true,
        stdin: false,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "-" | "+" | "--" | "+-") {
            return state.eligible(args.next().is_some());
        }
        let enabled = arg.starts_with('-');
        if let Some(name) = arg.strip_prefix("--").or_else(|| arg.strip_prefix("+-")) {
            if !state.named_option(name, enabled, true) {
                return false;
            }
            continue;
        }
        let Some(short) = arg.strip_prefix(['-', '+']) else {
            // With -s these are shell positional parameters, not a script.
            // All remaining words are data, including words that resemble -c.
            return state.eligible(true);
        };
        let mut flags = short
            .trim_end_matches(|ch: char| ch.is_ascii_whitespace())
            .chars();
        if flags.as_str().is_empty() {
            return false;
        }
        let mut ended = false;
        while let Some(flag) = flags.next() {
            match flag {
                'i' => state.interactive = enabled,
                'f' => state.rcs = !enabled,
                's' => state.stdin = enabled,
                'l' | 'd' | 'x' | 'v' | 'u' => {}
                'o' => {
                    let name = if flags.as_str().is_empty() {
                        args.next().map(String::as_str)
                    } else {
                        Some(flags.as_str())
                    };
                    if !name.is_some_and(|name| state.named_option(name, enabled, false)) {
                        return false;
                    }
                    break;
                }
                // -b stops parsing subsequent argv words, but flags following
                // b in this same group still take effect. -i- ends the group.
                'b' => ended = true,
                '-' if flags.as_str().is_empty() => ended = true,
                _ => return false,
            }
        }
        if ended {
            return state.eligible(args.next().is_some());
        }
    }
    state.eligible(false)
}

struct StartupState {
    // A PTY with no command/script operand is interactive by default.
    interactive: bool,
    rcs: bool,
    stdin: bool,
}

impl StartupState {
    fn eligible(&self, has_operands: bool) -> bool {
        self.interactive && self.rcs && (!has_operands || self.stdin)
    }

    fn named_option(&mut self, name: &str, mut enabled: bool, long: bool) -> bool {
        if name.is_empty()
            || !name
                .bytes()
                .all(|ch| ch.is_ascii_alphabetic() || ch == b'_' || (long && ch == b'-'))
        {
            return false;
        }
        let normalized: String = name
            .bytes()
            .filter(|ch| *ch != b'_' && *ch != b'-')
            .map(|ch| char::from(ch.to_ascii_lowercase()))
            .collect();
        let name = if let Some(name) = normalized.strip_prefix("no") {
            enabled = !enabled;
            name
        } else {
            &normalized
        };
        match name {
            "interactive" => self.interactive = enabled,
            "rcs" => self.rcs = enabled,
            "shinstdin" => self.stdin = enabled,
            "login" | "globalrcs" | "xtrace" | "verbose" | "unset" | "zle" => {}
            // In particular, do not guess emulation/privileged/restricted or
            // SH_OPTION_LETTERS modes, which change startup or option syntax.
            _ => return false,
        }
        true
    }
}
