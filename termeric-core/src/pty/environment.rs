//! One-shot environment inheritance policy for explicitly launched children.

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPolicy {
    pub clear: bool,
    pub unset: Vec<String>,
}

/// Names this terminal's processes as `PID.HEX`: the process that owns the
/// PTY, then a random identifier unique to it. Programs such as agent hooks
/// report back to that process under this identity.
pub const TERMINAL_ID_VARIABLE: &str = "TERMERIC_TERMINAL_ID";

/// Random bytes in a terminal identity, written as twice as many hex digits.
const TERMINAL_ID_RANDOM_BYTES: usize = 16;

/// The process a [`TERMINAL_ID_VARIABLE`] identity names, or `None` when
/// `id` is not spelled as Termeric writes one: decimal digits, a dot, and
/// the random part in lowercase hex.
pub fn terminal_id_process(id: &str) -> Option<u32> {
    let (process, random) = id.split_once('.')?;
    let digits = !process.is_empty() && process.bytes().all(|byte| byte.is_ascii_digit());
    if !digits || !crate::hex::is_lower(random, TERMINAL_ID_RANDOM_BYTES * 2) {
        return None;
    }
    process.parse().ok()
}

pub fn terminal_identity_variable(name: &str) -> bool {
    matches!(
        name,
        "TERM"
            | "COLORTERM"
            | "TERMINFO"
            | "TERMINFO_DIRS"
            | "TERM_PROGRAM"
            | "TERM_PROGRAM_VERSION"
            | TERMINAL_ID_VARIABLE
    )
}

/// A fresh identity for one terminal's processes, or `None` when the system
/// cannot supply randomness.
pub(super) fn new_terminal_id() -> Option<String> {
    let random = crate::hex::random(TERMINAL_ID_RANDOM_BYTES)
        .map_err(|error| log::warn!("No terminal identity: {error}"))
        .ok()?;
    Some(format!("{}.{random}", std::process::id()))
}

impl EnvironmentPolicy {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.clear && !self.unset.is_empty() {
            return Err("Cleared environments cannot also specify variable removals");
        }
        for (index, name) in self.unset.iter().enumerate() {
            let mut bytes = name.bytes();
            if !bytes
                .next()
                .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
                || !bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
                || terminal_identity_variable(name)
                || matches!(name.as_str(), "PWD" | "SHELL")
            {
                return Err("Invalid or reserved environment removal name");
            }
            if self.unset[..index].contains(name) {
                return Err("Duplicate environment removal name");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_ids_name_the_process_that_wrote_them() {
        let id = new_terminal_id().expect("randomness");
        assert_eq!(terminal_id_process(&id), Some(std::process::id()));
        let random = "0123456789abcdef0123456789abcdef";
        assert_eq!(terminal_id_process(&format!("42.{random}")), Some(42));
        for malformed in [
            String::new(),
            random.to_owned(),
            format!(".{random}"),
            format!("+42.{random}"),
            format!("-1.{random}"),
            format!("4 2.{random}"),
            format!("42.{}", random.to_uppercase()),
            format!("42.{}", &random[1..]),
            format!("42.{random}0"),
            format!("42.{random}.7"),
            // More than a process id can hold.
            format!("4294967296.{random}"),
        ] {
            assert_eq!(terminal_id_process(&malformed), None, "{malformed:?}");
        }
    }
}
