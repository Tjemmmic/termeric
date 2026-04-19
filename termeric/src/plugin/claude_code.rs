//! Claude Code plugin — launch Claude Code AI assistant from Termeric
//!
//! This plugin provides a deep integration with Claude Code:
//!
//! - **Clean launch**: `claude` runs directly as the tab's process (no shell echo)
//! - **Context-aware**: Inherits the active terminal's working directory automatically
//! - **Prompt support**: Launch with an initial prompt via command palette
//! - **Configurable**: Model, binary path, extra args, all via config.toml
//!
//! ## Commands
//!
//! | Command | Description |
//! |---------|-------------|
//! | `Open Claude Code` | Launch Claude Code in the active terminal's cwd |
//! | `Claude Code: Resume` | Resume the last conversation (`--continue`) |
//!
//! ## Default Keybinding
//!
//! `Ctrl+Shift+I` — Open Claude Code

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ai_common;
use super::claude_profiles::{ClaudeProfile, resolve_repo_profile};
use super::hooks::{
    Hook, HookResponse, Plugin, PluginAction, PluginCommand, PluginContext, PluginKeybinding,
};

/// Configuration for the Claude Code plugin.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ClaudeCodeConfig {
    /// Enable the Claude Code plugin
    pub enabled: bool,
    /// Path to claude binary (default: "claude" — assumes it's in PATH)
    pub binary: String,
    /// Default model to use (e.g., "sonnet", "opus")
    pub model: Option<String>,
    /// Whether to auto-set working directory to active terminal's cwd
    pub inherit_cwd: bool,
    /// Additional CLI arguments to pass to claude
    #[serde(default)]
    pub args: Vec<String>,
    /// Named profiles for different session configurations
    #[serde(default)]
    pub profiles: HashMap<String, ClaudeProfile>,
    /// Default profile to use when no repo-specific config exists
    pub default_profile: Option<String>,
    /// Approval mode: None (default interactive), "auto-accept" (skip permissions)
    pub approval_mode: Option<String>,
    /// Number of lines to copy with the copy-last command (default: 50)
    #[serde(default = "default_copy_lines")]
    pub copy_lines: usize,
}

fn default_copy_lines() -> usize {
    50
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            binary: "claude".to_string(),
            model: None,
            inherit_cwd: true,
            args: Vec::new(),
            profiles: HashMap::new(),
            default_profile: None,
            approval_mode: None,
            copy_lines: 50,
        }
    }
}

/// The Claude Code plugin.
#[derive(Debug)]
pub struct ClaudeCodePlugin {
    config: ClaudeCodeConfig,
}

impl ClaudeCodePlugin {
    /// Create a new Claude Code plugin with the given configuration.
    pub fn new(config: ClaudeCodeConfig) -> Self {
        Self { config }
    }

    /// Build the args list for launching Claude Code.
    ///
    /// `profile_extra_args` are merged from the resolved profile.
    /// `profile_approval_mode` overrides the config-level approval mode if set.
    fn build_args(
        &self,
        extra: &[&str],
        profile_extra_args: &[String],
        profile_approval_mode: Option<&str>,
    ) -> Vec<String> {
        // Collect approval-mode flag + config args + profile args into one slice
        let effective_approval = profile_approval_mode.or(self.config.approval_mode.as_deref());

        let mut combined_extra: Vec<String> = Vec::new();
        if effective_approval == Some("auto-accept") {
            combined_extra.push("--dangerously-skip-permissions".to_string());
        }
        combined_extra.extend(profile_extra_args.iter().cloned());
        combined_extra.extend(extra.iter().map(|s| s.to_string()));

        ai_common::build_args_with_model(
            self.config.model.as_deref(),
            &self.config.args,
            &combined_extra,
        )
    }

    /// Resolve the working directory: use context cwd if inherit_cwd is enabled.
    fn resolve_cwd<'a>(&self, ctx: &'a PluginContext) -> Option<&'a str> {
        ai_common::resolve_cwd(self.config.inherit_cwd, ctx)
    }

    /// Resolve the active profile for the current context.
    ///
    /// Priority: repo-specific `.termeric/config` > `default_profile` config > no profile.
    /// Returns the resolved profile (if any) and its name.
    fn resolve_profile(&self, cwd: Option<&str>) -> (Option<&ClaudeProfile>, Option<String>) {
        // 1. Try repo-specific profile from .termeric/config
        let profile_name = cwd
            .and_then(|c| resolve_repo_profile(c, "claude_code"))
            .or_else(|| self.config.default_profile.clone());

        if let Some(ref name) = profile_name {
            if let Some(profile) = self.config.profiles.get(name) {
                return (Some(profile), Some(name.clone()));
            }
            log::warn!(
                "Claude Code profile '{}' not found in config, using defaults",
                name
            );
        }

        (None, profile_name)
    }

    /// Build the launch action(s). Emits a `ShowNotice` alongside `OpenTab`
    /// when a requested profile name wasn't found in config, so the user
    /// doesn't silently get defaults.
    fn build_launch_actions(
        &self,
        ctx: &PluginContext,
        extra_cli_args: &[&str],
        title: &str,
    ) -> Vec<PluginAction> {
        let cwd = self.resolve_cwd(ctx);
        let (profile, profile_name) = self.resolve_profile(cwd);

        let (profile_extra_args, profile_env, profile_approval) = match profile {
            Some(p) => (
                p.extra_args.as_slice(),
                p.env.clone(),
                p.approval_mode.as_deref(),
            ),
            None => (&[] as &[String], HashMap::new(), None),
        };

        let mut env = profile_env;
        // Pass terminal context to the spawned process
        env.insert("COLUMNS".to_string(), ctx.terminal_size.0.to_string());
        env.insert("LINES".to_string(), ctx.terminal_size.1.to_string());

        let mut actions = Vec::with_capacity(2);
        // If a profile name was requested but not found, surface it.
        if profile.is_none() {
            if let Some(name) = profile_name {
                actions.push(PluginAction::ShowNotice {
                    title: "Claude Code profile not found".into(),
                    lines: vec![
                        format!("Profile '{}' is not defined in your config.", name),
                        String::new(),
                        "Falling back to defaults for this tab.".into(),
                    ],
                });
            }
        }
        actions.push(PluginAction::OpenTab {
            program: self.config.binary.clone(),
            args: self.build_args(extra_cli_args, profile_extra_args, profile_approval),
            cwd: cwd.map(|s| s.to_string()),
            title: Some(title.to_string()),
            plugin_id: Some("claude-code".to_string()),
            env,
        });
        actions
    }
}

impl Plugin for ClaudeCodePlugin {
    fn id(&self) -> &str {
        "claude-code"
    }

    fn name(&self) -> &str {
        "Claude Code"
    }

    fn on_hook(&mut self, hook: &Hook, ctx: &PluginContext) -> HookResponse {
        match hook {
            Hook::CommandInvoked { command_id } => match command_id.as_str() {
                "claude-code.open" => {
                    HookResponse::Actions(self.build_launch_actions(ctx, &[], "Claude Code"))
                }
                "claude-code.resume" => HookResponse::Actions(self.build_launch_actions(
                    ctx,
                    &["--continue"],
                    "Claude Code (resume)",
                )),
                "claude-code.copy-last" => HookResponse::Action(PluginAction::CopyFromPluginTab {
                    plugin_id: "claude-code".to_string(),
                    lines: self.config.copy_lines,
                }),
                "claude-code.clear" => HookResponse::Action(PluginAction::ClearPluginTab {
                    plugin_id: "claude-code".to_string(),
                }),
                "claude-code.restart" => HookResponse::Action(PluginAction::RestartPlugin {
                    plugin_id: "claude-code".to_string(),
                }),
                "claude-code.toggle-transcript" => {
                    HookResponse::Action(PluginAction::ToggleTranscript)
                }
                // review-diff, copy-diff, insert-context, export-session, jump-to-error,
                // and select-profile are handled directly by the App
                "claude-code.review-diff"
                | "claude-code.copy-diff"
                | "claude-code.insert-context"
                | "claude-code.export-session"
                | "claude-code.jump-to-error"
                | "claude-code.select-profile" => HookResponse::Handled,
                _ => HookResponse::Ignored,
            },
            _ => HookResponse::Ignored,
        }
    }

    fn commands(&self) -> Vec<PluginCommand> {
        ai_common::standard_ai_commands("claude-code", "Claude Code")
    }

    fn keybindings(&self) -> Vec<PluginKeybinding> {
        vec![
            PluginKeybinding {
                key: "i".to_string(),
                ctrl: true,
                alt: false,
                shift: true,
                command_id: "claude-code.open".to_string(),
            },
            PluginKeybinding {
                key: "r".to_string(),
                ctrl: true,
                alt: false,
                shift: true,
                command_id: "claude-code.resume".to_string(),
            },
            PluginKeybinding {
                key: "l".to_string(),
                ctrl: true,
                alt: false,
                shift: true,
                command_id: "claude-code.clear".to_string(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Return the first `OpenTab` action from a `HookResponse` regardless of
    /// whether it was emitted as `Action(_)` or `Actions(vec![...])`.
    fn extract_open_tab(response: HookResponse) -> Option<PluginAction> {
        let actions: Vec<PluginAction> = match response {
            HookResponse::Action(a) => vec![a],
            HookResponse::Actions(v) => v,
            _ => vec![],
        };
        actions
            .into_iter()
            .find(|a| matches!(a, PluginAction::OpenTab { .. }))
    }

    #[test]
    fn test_default_config() {
        let config = ClaudeCodeConfig::default();
        assert!(config.enabled);
        assert_eq!(config.binary, "claude");
        assert!(config.model.is_none());
        assert!(config.inherit_cwd);
        assert!(config.args.is_empty());
        assert!(config.approval_mode.is_none());
        assert_eq!(config.copy_lines, 50);
    }

    #[test]
    fn test_build_args_basic() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let args = plugin.build_args(&[], &[], None);
        assert!(args.is_empty()); // No model, no extra args
    }

    #[test]
    fn test_build_args_with_model() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            model: Some("sonnet".to_string()),
            ..Default::default()
        });
        let args = plugin.build_args(&[], &[], None);
        assert_eq!(args, vec!["--model", "sonnet"]);
    }

    #[test]
    fn test_build_args_with_extra() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            args: vec!["--verbose".to_string()],
            ..Default::default()
        });
        let args = plugin.build_args(&["--continue"], &[], None);
        assert_eq!(args, vec!["--verbose", "--continue"]);
    }

    #[test]
    fn test_plugin_metadata() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        assert_eq!(plugin.id(), "claude-code");
        assert_eq!(plugin.name(), "Claude Code");
    }

    #[test]
    fn test_plugin_commands() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let commands = plugin.commands();
        assert_eq!(commands.len(), 12);
        assert_eq!(commands[0].id, "claude-code.open");
        assert_eq!(commands[1].id, "claude-code.resume");
        assert_eq!(commands[2].id, "claude-code.copy-last");
        assert_eq!(commands[3].id, "claude-code.clear");
        assert_eq!(commands[4].id, "claude-code.restart");
        assert_eq!(commands[5].id, "claude-code.review-diff");
        assert_eq!(commands[6].id, "claude-code.copy-diff");
        assert_eq!(commands[7].id, "claude-code.insert-context");
        assert_eq!(commands[8].id, "claude-code.toggle-transcript");
        assert_eq!(commands[9].id, "claude-code.export-session");
        assert_eq!(commands[10].id, "claude-code.jump-to-error");
        assert_eq!(commands[11].id, "claude-code.select-profile");
    }

    #[test]
    fn test_plugin_keybindings() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let bindings = plugin.keybindings();
        assert_eq!(bindings.len(), 3);
        assert_eq!(bindings[0].key, "i");
        assert_eq!(bindings[0].command_id, "claude-code.open");
        assert!(bindings[0].ctrl);
        assert!(bindings[0].shift);
        assert_eq!(bindings[1].key, "r");
        assert_eq!(bindings[1].command_id, "claude-code.resume");
        assert!(bindings[1].ctrl);
        assert!(bindings[1].shift);
        assert_eq!(bindings[2].key, "l");
        assert_eq!(bindings[2].command_id, "claude-code.clear");
        assert!(bindings[2].ctrl);
        assert!(bindings[2].shift);
    }

    #[test]
    fn test_hook_open() {
        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let ctx = PluginContext {
            cwd: Some("/tmp/project".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "claude-code.open".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab {
                program,
                args,
                cwd,
                title,
                plugin_id,
                env,
            }) => {
                assert_eq!(program, "claude");
                assert!(args.is_empty());
                assert_eq!(cwd, Some("/tmp/project".to_string()));
                assert_eq!(title, Some("Claude Code".to_string()));
                assert_eq!(plugin_id, Some("claude-code".to_string()));
                // env contains terminal size (COLUMNS, LINES)
                assert_eq!(env.get("COLUMNS"), Some(&"80".to_string()));
                assert_eq!(env.get("LINES"), Some(&"24".to_string()));
            }
            _ => panic!("Expected OpenTab action"),
        }
    }

    #[test]
    fn test_hook_resume() {
        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let ctx = PluginContext {
            cwd: Some("/tmp".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "claude-code.resume".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab { args, title, .. }) => {
                assert!(args.contains(&"--continue".to_string()));
                assert_eq!(title, Some("Claude Code (resume)".to_string()));
            }
            _ => panic!("Expected OpenTab with --continue"),
        }
    }

    #[test]
    fn test_hook_ignores_other_commands() {
        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "other.command".to_string(),
            },
            &ctx,
        );
        assert!(matches!(response, HookResponse::Ignored));
    }

    #[test]
    fn test_cwd_disabled() {
        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            inherit_cwd: false,
            ..Default::default()
        });
        let ctx = PluginContext {
            cwd: Some("/tmp/project".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "claude-code.open".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab { cwd, .. }) => {
                assert_eq!(cwd, None); // cwd not passed when inherit_cwd is false
            }
            _ => panic!("Expected OpenTab action"),
        }
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            enabled = true
            binary = "/usr/local/bin/claude"
            model = "opus"
            inherit_cwd = false
            args = ["--verbose"]
        "#;
        let config: ClaudeCodeConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.binary, "/usr/local/bin/claude");
        assert_eq!(config.model, Some("opus".to_string()));
        assert!(!config.inherit_cwd);
        assert_eq!(config.args, vec!["--verbose"]);
        // Defaults for omitted fields
        assert!(config.approval_mode.is_none());
        assert_eq!(config.copy_lines, 50);
    }

    #[test]
    fn test_config_deserialization_with_new_fields() {
        let toml_str = r#"
            enabled = true
            binary = "claude"
            approval_mode = "auto-accept"
            copy_lines = 100
        "#;
        let config: ClaudeCodeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.approval_mode, Some("auto-accept".to_string()));
        assert_eq!(config.copy_lines, 100);
    }

    #[test]
    fn test_build_args_with_approval_mode() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            approval_mode: Some("auto-accept".to_string()),
            ..Default::default()
        });
        let args = plugin.build_args(&[], &[], None);
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn test_build_args_without_approval_mode() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let args = plugin.build_args(&[], &[], None);
        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn test_build_args_with_non_auto_accept_approval_mode() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            approval_mode: Some("interactive".to_string()),
            ..Default::default()
        });
        let args = plugin.build_args(&[], &[], None);
        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn test_hook_copy_last_uses_config_lines() {
        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            copy_lines: 100,
            ..Default::default()
        });
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "claude-code.copy-last".to_string(),
            },
            &ctx,
        );

        match response {
            HookResponse::Action(PluginAction::CopyFromPluginTab { lines, .. }) => {
                assert_eq!(lines, 100);
            }
            _ => panic!("Expected Action(CopyFromPluginTab)"),
        }
    }

    #[test]
    fn test_hook_toggle_transcript() {
        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "claude-code.toggle-transcript".to_string(),
            },
            &ctx,
        );
        assert!(matches!(
            response,
            HookResponse::Action(PluginAction::ToggleTranscript)
        ));
    }

    #[test]
    fn test_hook_app_handled_commands() {
        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        for cmd in &[
            "claude-code.review-diff",
            "claude-code.copy-diff",
            "claude-code.insert-context",
            "claude-code.export-session",
            "claude-code.jump-to-error",
            "claude-code.select-profile",
        ] {
            let response = plugin.on_hook(
                &Hook::CommandInvoked {
                    command_id: cmd.to_string(),
                },
                &ctx,
            );
            assert!(
                matches!(response, HookResponse::Handled),
                "Expected Handled for {}, got {:?}",
                cmd,
                response
            );
        }
    }

    // --- Profile resolution tests ---

    #[test]
    fn test_resolve_profile_no_profiles_configured() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let (profile, name) = plugin.resolve_profile(Some("/tmp/nonexistent"));
        assert!(profile.is_none());
        assert!(name.is_none());
    }

    #[test]
    fn test_resolve_profile_default_profile() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "safe".to_string(),
            ClaudeProfile {
                description: "Safe mode".to_string(),
                approval_mode: Some("interactive".to_string()),
                ..Default::default()
            },
        );
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            default_profile: Some("safe".to_string()),
            profiles,
            ..Default::default()
        });
        let (profile, name) = plugin.resolve_profile(None);
        assert!(profile.is_some());
        assert_eq!(name, Some("safe".to_string()));
        assert_eq!(
            profile.unwrap().approval_mode,
            Some("interactive".to_string())
        );
    }

    #[test]
    fn test_resolve_profile_missing_named_profile() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            default_profile: Some("nonexistent".to_string()),
            ..Default::default()
        });
        let (profile, name) = plugin.resolve_profile(None);
        assert!(profile.is_none());
        assert_eq!(name, Some("nonexistent".to_string()));
    }

    #[test]
    fn test_build_args_with_profile_extra_args() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let profile_args = vec!["--verbose".to_string(), "--no-color".to_string()];
        let args = plugin.build_args(&[], &profile_args, None);
        assert_eq!(args, vec!["--verbose", "--no-color"]);
    }

    #[test]
    fn test_build_args_profile_approval_overrides_config() {
        // Config has no approval mode, but profile sets auto-accept
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let args = plugin.build_args(&[], &[], Some("auto-accept"));
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn test_build_args_profile_approval_overrides_config_interactive() {
        // Config has auto-accept, but profile overrides to interactive
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            approval_mode: Some("auto-accept".to_string()),
            ..Default::default()
        });
        let args = plugin.build_args(&[], &[], Some("interactive"));
        // Profile says "interactive", so no --dangerously-skip-permissions
        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn test_build_args_profile_none_falls_back_to_config() {
        // Config has auto-accept, profile has no override
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            approval_mode: Some("auto-accept".to_string()),
            ..Default::default()
        });
        let args = plugin.build_args(&[], &[], None);
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn test_open_with_profile_env_passthrough() {
        let mut env = HashMap::new();
        env.insert("CLAUDE_API_KEY".to_string(), "test-key".to_string());
        env.insert("CUSTOM_VAR".to_string(), "custom-val".to_string());

        let mut profiles = HashMap::new();
        profiles.insert(
            "work".to_string(),
            ClaudeProfile {
                description: "Work profile".to_string(),
                env: env.clone(),
                extra_args: vec!["--verbose".to_string()],
                approval_mode: Some("auto-accept".to_string()),
                ..Default::default()
            },
        );

        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            default_profile: Some("work".to_string()),
            profiles,
            ..Default::default()
        });

        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "claude-code.open".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab {
                args, env: tab_env, ..
            }) => {
                // Profile env should be passed through
                assert_eq!(tab_env.get("CLAUDE_API_KEY"), Some(&"test-key".to_string()));
                assert_eq!(tab_env.get("CUSTOM_VAR"), Some(&"custom-val".to_string()));
                // Profile extra_args should be in args
                assert!(args.contains(&"--verbose".to_string()));
                // Profile approval_mode should inject --dangerously-skip-permissions
                assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
            }
            _ => panic!("Expected OpenTab action"),
        }
    }

    #[test]
    fn test_resume_with_profile_env_passthrough() {
        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "my-val".to_string());

        let mut profiles = HashMap::new();
        profiles.insert(
            "dev".to_string(),
            ClaudeProfile {
                env: env.clone(),
                ..Default::default()
            },
        );

        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            default_profile: Some("dev".to_string()),
            profiles,
            ..Default::default()
        });

        let ctx = PluginContext {
            cwd: Some("/tmp/project".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "claude-code.resume".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab {
                args, env: tab_env, ..
            }) => {
                assert_eq!(tab_env.get("MY_VAR"), Some(&"my-val".to_string()));
                assert!(args.contains(&"--continue".to_string()));
            }
            _ => panic!("Expected OpenTab with --continue"),
        }
    }

    #[test]
    fn test_open_no_profile_empty_env() {
        let mut plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let ctx = PluginContext {
            cwd: Some("/tmp".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "claude-code.open".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab { env, .. }) => {
                // No profile env, but terminal size is always passed
                assert_eq!(env.len(), 2);
                assert_eq!(env.get("COLUMNS"), Some(&"80".to_string()));
                assert_eq!(env.get("LINES"), Some(&"24".to_string()));
            }
            _ => panic!("Expected OpenTab action"),
        }
    }
}
