//! Codex profiles — per-repo and global configuration rulesets for OpenAI Codex
//!
//! Profiles define environment, permission, sandbox, and targeting rules for Codex
//! sessions. A repo can auto-select a profile via `.termeric/config` under the
//! `[codex]` section.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::claude_profiles::ProfileTarget;

/// A Codex profile defining behavior for a session
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CodexProfile {
    /// Human-readable description
    #[serde(default)]
    pub description: String,

    /// Approval mode override: "suggest", "auto-edit", "full-auto". These map
    /// to Codex policies "untrusted", "on-request", and "never", respectively.
    #[serde(default)]
    pub approval_mode: Option<String>,

    /// Sandbox mode override: "read-only", "workspace-write", "full-access"
    #[serde(default)]
    pub sandbox_mode: Option<String>,

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_profile() {
        let profile = CodexProfile::default();
        assert!(profile.description.is_empty());
        assert!(profile.approval_mode.is_none());
        assert!(profile.sandbox_mode.is_none());
        assert!(profile.env.is_empty());
        assert!(profile.extra_args.is_empty());
        assert!(profile.target.is_none());
        assert!(!profile.auto_record);
    }

    #[test]
    fn test_profile_deserialization() {
        let toml_str = r#"
            description = "Safe Codex mode"
            approval_mode = "suggest"
            sandbox_mode = "read-only"
            auto_record = true

            [env]
            OPENAI_API_KEY = "test-key"

            [target]
            type = "ssh"
            host = "dev01"
            user = "deploy"
        "#;
        let profile: CodexProfile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.description, "Safe Codex mode");
        assert_eq!(profile.approval_mode, Some("suggest".to_string()));
        assert_eq!(profile.sandbox_mode, Some("read-only".to_string()));
        assert!(profile.auto_record);
        assert_eq!(
            profile.env.get("OPENAI_API_KEY"),
            Some(&"test-key".to_string())
        );
        match profile.target.unwrap() {
            ProfileTarget::Ssh { host, user } => {
                assert_eq!(host, "dev01");
                assert_eq!(user, Some("deploy".to_string()));
            }
            _ => panic!("Expected SSH target"),
        }
    }

    #[test]
    fn test_profile_deserialization_full_auto() {
        let toml_str = r#"
            description = "Full auto profile"
            approval_mode = "full-auto"
            sandbox_mode = "full-access"
            extra_args = ["--notify"]

            [env]
            API_KEY = "prod-key"
        "#;
        let profile: CodexProfile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.approval_mode, Some("full-auto".to_string()));
        assert_eq!(profile.sandbox_mode, Some("full-access".to_string()));
        assert_eq!(profile.extra_args, vec!["--notify"]);
        assert_eq!(profile.env.get("API_KEY"), Some(&"prod-key".to_string()));
    }

    #[test]
    fn test_profile_deserialization_minimal() {
        let toml_str = r#"
            description = "Minimal"
        "#;
        let profile: CodexProfile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.description, "Minimal");
        assert!(profile.approval_mode.is_none());
        assert!(profile.sandbox_mode.is_none());
        assert!(profile.env.is_empty());
        assert!(profile.extra_args.is_empty());
    }
}
