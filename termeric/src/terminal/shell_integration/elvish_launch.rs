//! Eligibility for Elvish's Go-style flags and rc-loading REPL mode.
//! Keep unrecognized/malformed invocations untouched; Elvish owns diagnostics.

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
            "rc" | "log" | "db" | "sock" | "cpuprofile" | "allocsprofile" => {
                // String values may be empty or look like flags/end markers.
                if inline.is_none() && args.next().is_none() {
                    return false;
                }
            }
            "deprecation-level" => {
                if !inline
                    .or_else(|| args.next().map(String::as_str))
                    .is_some_and(valid_go_int)
                {
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

fn valid_go_int(value: &str) -> bool {
    let negative = value.starts_with('-');
    let digits = value.strip_prefix(['-', '+']).unwrap_or(value);
    let (radix, digits, prefix) = match digits.as_bytes() {
        [b'0', b'x' | b'X', ..] => (16, &digits[2..], true),
        [b'0', b'o' | b'O', ..] => (8, &digits[2..], true),
        [b'0', b'b' | b'B', ..] => (2, &digits[2..], true),
        [b'0', _, ..] => (8, digits, false),
        _ => (10, digits, false),
    };
    let limit = (isize::MAX as u64) + u64::from(negative);
    let mut magnitude = 0u64;
    let mut previous_digit = prefix;
    let mut any_digit = false;
    for ch in digits.chars() {
        if ch == '_' && previous_digit {
            previous_digit = false;
            continue;
        }
        let Some(digit) = ch.to_digit(radix) else {
            return false;
        };
        let Some(next) = magnitude
            .checked_mul(u64::from(radix))
            .and_then(|value| value.checked_add(u64::from(digit)))
            .filter(|value| *value <= limit)
        else {
            return false;
        };
        magnitude = next;
        previous_digit = true;
        any_digit = true;
    }
    any_digit && previous_digit
}
