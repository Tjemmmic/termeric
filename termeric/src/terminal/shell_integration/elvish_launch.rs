//! Eligibility for Elvish's Go-style flags and rc-loading REPL mode.
//! Unrecognized invocations stay untouched; Elvish owns diagnostics.

pub(super) fn is_interactive(args: &[String]) -> bool {
    // These flags select a different program or suppress user rc loading.
    // Go's flag parser stores the last value, rather than acting immediately.
    let mut disabled = [false; 6];
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args.next().is_none() && !disabled.contains(&true);
        }
        let Some(flag) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) else {
            return false;
        };
        let (name, inline) = flag
            .split_once('=')
            .map_or((flag, None), |(name, value)| (name, Some(value)));
        match name {
            "rc" | "log" | "db" | "sock" | "cpuprofile" | "allocsprofile" | "deprecation-level" => {
                // Values may be empty or look like flags/end markers. A
                // malformed deprecation level is Elvish's error to report;
                // integration only adds the module search path either way.
                if inline.is_none() && args.next().is_none() {
                    return false;
                }
            }
            "norc" | "lsp" | "daemon" | "version" | "buildinfo" | "help" | "i" | "l" | "c"
            | "compileonly" | "json" => {
                let enabled = match inline {
                    None | Some("1" | "t" | "T" | "true" | "TRUE" | "True") => true,
                    Some("0" | "f" | "F" | "false" | "FALSE" | "False") => false,
                    _ => return false,
                };
                match name {
                    "norc" => disabled[0] = enabled,
                    "lsp" => disabled[1] = enabled,
                    "daemon" => disabled[2] = enabled,
                    "version" => disabled[3] = enabled,
                    "buildinfo" => disabled[4] = enabled,
                    "help" => disabled[5] = enabled,
                    // i/l are compatibility no-ops; c/compileonly/json do not
                    // prevent a REPL with no operands. Any operand still makes
                    // this a script/command launch, even with -i.
                    _ => {}
                }
            }
            // Includes lone -, grouped flags, abbreviations, and Go's special
            // undefined -h help alias (which exits even with =false).
            _ => return false,
        }
    }
    !disabled.contains(&true)
}
