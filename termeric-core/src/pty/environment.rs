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
#[cfg(unix)]
pub(super) fn new_terminal_id() -> Option<String> {
    let random = crate::hex::random(16)
        .map_err(|error| log::warn!("No terminal identity: {error}"))
        .ok()?;
    Some(format!("{}.{random}", std::process::id()))
}

#[cfg(not(unix))]
pub(super) fn new_terminal_id() -> Option<String> {
    None
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
