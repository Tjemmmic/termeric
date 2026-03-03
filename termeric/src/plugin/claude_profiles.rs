//! Claude Code profiles — per-repo and global configuration rulesets
//!
//! Profiles define environment, permission, and targeting rules for Claude Code
//! sessions. A repo can auto-select a profile via `.termeric/config`.

use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};

use crate::ui::claude_chrome::HostTarget;

/// A Claude Code profile defining behavior for a session
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ClaudeProfile {
    /// Human-readable description
    #[serde(default)]
    pub description: String,

    /// Whether to require confirmation for all commands (safe mode)
    #[serde(default)]
    pub confirm_all_commands: bool,

    /// Extra environment variables to set for this profile
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Extra CLI arguments for this profile
    #[serde(default)]
    pub extra_args: Vec<String>,

    /// Target host (if set, overrides the default)
    #[serde(default)]
    pub target: Option<ProfileTarget>,

    /// Whether to auto-record transcript
    #[serde(default)]
    pub auto_record: bool,

    /// Approval mode override (e.g., "auto-accept" for --dangerously-skip-permissions)
    #[serde(default)]
    pub approval_mode: Option<String>,
}

/// Target specification for a profile
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProfileTarget {
    /// Run locally (default)
    Local,
    /// Run via SSH
    Ssh { host: String, user: Option<String> },
    /// Run inside a container
    Container { name: String, #[serde(default = "default_runtime")] runtime: String },
}

fn default_runtime() -> String {
    "docker".to_string()
}

impl ProfileTarget {
    /// Convert to a HostTarget for the chrome bar
    pub fn to_host_target(&self) -> HostTarget {
        match self {
            ProfileTarget::Local => HostTarget::Local,
            ProfileTarget::Ssh { host, user } => HostTarget::Ssh {
                host: host.clone(),
                user: user.clone(),
            },
            ProfileTarget::Container { name, runtime } => HostTarget::Container {
                name: name.clone(),
                runtime: runtime.clone(),
            },
        }
    }
}

/// Try to resolve a profile name from a `.termeric/config` file in the given directory.
///
/// Walks up from `cwd` to find the nearest `.termeric/config` file.
/// If found, reads the `[section]` (e.g. `[claude_code]` or `[codex]`) for a `profile` key.
///
/// The `section` parameter selects which TOML section to read from
/// (e.g. `"claude_code"` or `"codex"`).
pub fn resolve_repo_profile(cwd: &str, section: &str) -> Option<String> {
    let mut dir = Path::new(cwd);
    loop {
        let config_path = dir.join(".termeric").join("config");
        if config_path.exists()
            && let Ok(contents) = std::fs::read_to_string(&config_path)
            && let Some(val) = parse_section_profile(&contents, section)
        {
            return Some(val);
        }
        dir = dir.parent()?;
    }
}

/// Parse a `profile = "name"` key from a specific `[section]` in TOML-like content.
///
/// Returns `None` if the section or key is not found.
fn parse_section_profile(contents: &str, section: &str) -> Option<String> {
    let section_header = format!("[{}]", section);
    let mut in_section = false;

    for line in contents.lines() {
        let trimmed = line.trim();

        // Detect section headers
        if trimmed.starts_with('[') {
            in_section = trimmed == section_header;
            continue;
        }

        if !in_section {
            continue;
        }

        // Look for `profile = "value"` within the target section
        if let Some(rest) = trimmed.strip_prefix("profile") {
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix('=') {
                let val = rest.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_profile() {
        let profile = ClaudeProfile::default();
        assert!(!profile.confirm_all_commands);
        assert!(profile.env.is_empty());
        assert!(profile.extra_args.is_empty());
        assert!(profile.target.is_none());
        assert!(!profile.auto_record);
    }

    #[test]
    fn test_profile_target_to_host() {
        let local = ProfileTarget::Local;
        assert_eq!(local.to_host_target(), HostTarget::Local);

        let ssh = ProfileTarget::Ssh { host: "dev01".into(), user: Some("root".into()) };
        match ssh.to_host_target() {
            HostTarget::Ssh { host, user } => {
                assert_eq!(host, "dev01");
                assert_eq!(user, Some("root".to_string()));
            }
            _ => panic!("Expected SSH"),
        }

        let container = ProfileTarget::Container { name: "devbox".into(), runtime: "docker".into() };
        match container.to_host_target() {
            HostTarget::Container { name, runtime } => {
                assert_eq!(name, "devbox");
                assert_eq!(runtime, "docker");
            }
            _ => panic!("Expected Container"),
        }
    }

    #[test]
    fn test_resolve_repo_profile_nonexistent() {
        assert!(resolve_repo_profile("/tmp/nonexistent_repo_xyzzy", "claude_code").is_none());
    }

    #[test]
    fn test_parse_section_profile_found() {
        let contents = r#"
[claude_code]
profile = "work"

[codex]
profile = "dev"
"#;
        assert_eq!(
            super::parse_section_profile(contents, "claude_code"),
            Some("work".to_string())
        );
        assert_eq!(
            super::parse_section_profile(contents, "codex"),
            Some("dev".to_string())
        );
    }

    #[test]
    fn test_parse_section_profile_missing_section() {
        let contents = r#"
[claude_code]
profile = "work"
"#;
        assert_eq!(super::parse_section_profile(contents, "codex"), None);
    }

    #[test]
    fn test_parse_section_profile_no_key() {
        let contents = r#"
[claude_code]
model = "opus"
"#;
        assert_eq!(super::parse_section_profile(contents, "claude_code"), None);
    }

    #[test]
    fn test_parse_section_profile_empty() {
        assert_eq!(super::parse_section_profile("", "claude_code"), None);
    }

    #[test]
    fn test_parse_section_profile_wrong_section_key_ignored() {
        // profile key in [other] section should not be returned for [claude_code]
        let contents = r#"
[other]
profile = "wrong"

[claude_code]
model = "opus"
"#;
        assert_eq!(super::parse_section_profile(contents, "claude_code"), None);
    }

    #[test]
    fn test_profile_deserialization() {
        let toml_str = r#"
            description = "Safe mode"
            confirm_all_commands = true
            auto_record = true

            [env]
            CLAUDE_SAFE = "1"

            [target]
            type = "ssh"
            host = "dev01"
            user = "deploy"
        "#;
        let profile: ClaudeProfile = toml::from_str(toml_str).unwrap();
        assert!(profile.confirm_all_commands);
        assert!(profile.auto_record);
        assert_eq!(profile.env.get("CLAUDE_SAFE"), Some(&"1".to_string()));
        assert!(profile.approval_mode.is_none());
        match profile.target.unwrap() {
            ProfileTarget::Ssh { host, user } => {
                assert_eq!(host, "dev01");
                assert_eq!(user, Some("deploy".to_string()));
            }
            _ => panic!("Expected SSH target"),
        }
    }

    #[test]
    fn test_profile_deserialization_with_approval_mode() {
        let toml_str = r#"
            description = "Auto-accept profile"
            approval_mode = "auto-accept"
            extra_args = ["--verbose"]

            [env]
            API_KEY = "test-key"
        "#;
        let profile: ClaudeProfile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.approval_mode, Some("auto-accept".to_string()));
        assert_eq!(profile.extra_args, vec!["--verbose"]);
        assert_eq!(profile.env.get("API_KEY"), Some(&"test-key".to_string()));
    }
}
