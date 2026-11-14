//! `[quick_access]`: how the quick-access terminal is reached. On Linux a
//! desktop shortcut runs `termeric --quick-access`. On macOS the running
//! quick-access terminal also listens for a system-wide hotkey of its own,
//! since macOS gives a key press the right to bring an app forward but not
//! a request from another program.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuickAccessConfig {
    /// macOS only: the key that shows and hides the quick-access terminal
    /// while it runs, whichever app is in front. Checked on every system, so
    /// a configuration shared between machines is checked everywhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macos_hotkey: Option<Hotkey>,
}

/// A key with its modifiers, as `[[shortcuts]]` entries write them. The key
/// is the one at that place on a US keyboard, whatever the layout.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Hotkey {
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default, rename = "super")]
    pub super_key: bool,
}

impl QuickAccessConfig {
    pub fn validate(&self) -> Result<()> {
        let Some(hotkey) = &self.macos_hotkey else {
            return Ok(());
        };
        if hotkey_key_code(&hotkey.key).is_none() {
            bail!(
                "quick_access.macos_hotkey key {:?} is not one of: {}",
                hotkey.key,
                HOTKEY_KEYS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        // A plain key would stop working everywhere else; function keys
        // alone are the usual exception.
        if !(hotkey.ctrl || hotkey.alt || hotkey.super_key) && !hotkey.key.starts_with('f') {
            bail!("quick_access.macos_hotkey needs ctrl, alt, or super unless its key is F1–F20");
        }
        Ok(())
    }
}

/// The macOS virtual key code (`kVK_*` in Carbon's `Events.h`) of `key`,
/// a name from [`HOTKEY_KEYS`], compared without regard to case.
pub fn hotkey_key_code(key: &str) -> Option<u16> {
    let key = key.to_ascii_lowercase();
    HOTKEY_KEYS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, code)| *code)
}

/// Every key a hotkey may use, with its macOS virtual key code.
pub const HOTKEY_KEYS: &[(&str, u16)] = &[
    ("a", 0x00),
    ("s", 0x01),
    ("d", 0x02),
    ("f", 0x03),
    ("h", 0x04),
    ("g", 0x05),
    ("z", 0x06),
    ("x", 0x07),
    ("c", 0x08),
    ("v", 0x09),
    ("b", 0x0B),
    ("q", 0x0C),
    ("w", 0x0D),
    ("e", 0x0E),
    ("r", 0x0F),
    ("y", 0x10),
    ("t", 0x11),
    ("1", 0x12),
    ("2", 0x13),
    ("3", 0x14),
    ("4", 0x15),
    ("6", 0x16),
    ("5", 0x17),
    ("=", 0x18),
    ("9", 0x19),
    ("7", 0x1A),
    ("-", 0x1B),
    ("8", 0x1C),
    ("0", 0x1D),
    ("]", 0x1E),
    ("o", 0x1F),
    ("u", 0x20),
    ("[", 0x21),
    ("i", 0x22),
    ("p", 0x23),
    ("l", 0x25),
    ("j", 0x26),
    ("'", 0x27),
    ("k", 0x28),
    (";", 0x29),
    ("\\", 0x2A),
    (",", 0x2B),
    ("/", 0x2C),
    ("n", 0x2D),
    ("m", 0x2E),
    (".", 0x2F),
    ("`", 0x32),
    ("return", 0x24),
    ("tab", 0x30),
    ("space", 0x31),
    ("escape", 0x35),
    ("f1", 0x7A),
    ("f2", 0x78),
    ("f3", 0x63),
    ("f4", 0x76),
    ("f5", 0x60),
    ("f6", 0x61),
    ("f7", 0x62),
    ("f8", 0x64),
    ("f9", 0x65),
    ("f10", 0x6D),
    ("f11", 0x67),
    ("f12", 0x6F),
    ("f13", 0x69),
    ("f14", 0x6B),
    ("f15", 0x71),
    ("f16", 0x6A),
    ("f17", 0x40),
    ("f18", 0x4F),
    ("f19", 0x50),
    ("f20", 0x5A),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn hotkey(key: &str, ctrl: bool) -> QuickAccessConfig {
        QuickAccessConfig {
            macos_hotkey: Some(Hotkey {
                key: key.into(),
                ctrl,
                alt: false,
                shift: false,
                super_key: false,
            }),
        }
    }

    #[test]
    fn hotkeys_name_a_known_key_and_a_modifier() {
        assert!(QuickAccessConfig::default().validate().is_ok());
        assert!(hotkey("`", true).validate().is_ok());
        assert!(hotkey("Space", true).validate().is_ok());
        assert!(hotkey("f12", false).validate().is_ok());
        assert!(hotkey("a", false).validate().is_err());
        assert!(hotkey("enter", true).validate().is_err());
    }

    #[test]
    fn every_hotkey_key_has_one_code() {
        let mut codes: Vec<u16> = HOTKEY_KEYS.iter().map(|(_, code)| *code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), HOTKEY_KEYS.len());
        assert_eq!(hotkey_key_code("`"), Some(0x32));
        assert_eq!(hotkey_key_code("SPACE"), Some(0x31));
    }

    #[test]
    fn the_setting_reads_as_documented() {
        let config: QuickAccessConfig =
            toml::from_str("macos_hotkey = { key = \"`\", ctrl = true }").unwrap();
        assert_eq!(config, hotkey("`", true));
    }
}
