//! Codex plugin — launch OpenAI Codex CLI assistant from Termeric
//!
//! This plugin provides a deep integration with Codex:
//!
//! - **Clean launch**: `codex` runs directly as the tab's process (no shell echo)
//! - **Context-aware**: Inherits the active terminal's working directory automatically
//! - **Configurable**: Model, binary path, approval/sandbox modes, extra args, all via config.toml
//!
//! ## Commands
//!
//! | Command | Description |
//! |---------|-------------|
//! | `Open Codex` | Launch Codex in the active terminal's cwd |
//! | `Codex: Resume` | Resume the last conversation (`--continue`) |
//!
//! ## Default Keybinding
//!
//! `Ctrl+Shift+K` — Open Codex

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ai_common;
use super::claude_profiles::resolve_repo_profile;
use super::codex_profiles::CodexProfile;
use super::hooks::{
    Hook, HookResponse, Plugin, PluginAction, PluginCommand, PluginContext, PluginKeybinding,
};

/// Configuration for the Codex plugin.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CodexConfig {
    /// Enable the Codex plugin
    pub enabled: bool,
    /// Path to codex binary (default: "codex" — assumes it's in PATH)
    pub binary: String,
    /// Default model to use (e.g., "o3-mini", "o4-mini")
    pub model: Option<String>,
    /// Whether to auto-set working directory to active terminal's cwd
    pub inherit_cwd: bool,
    /// Additional CLI arguments to pass to codex
    #[serde(default)]
    pub args: Vec<String>,
    /// Approval mode: "suggest" (default), "auto-edit", "full-auto"
    #[serde(default = "default_approval_mode")]
    pub approval_mode: String,
    /// Sandbox mode: "read-only", "workspace-write" (default), "full-access"
    #[serde(default = "default_sandbox_mode")]
    pub sandbox_mode: Option<String>,
    /// Named profiles for different session configurations
    #[serde(default)]
    pub profiles: HashMap<String, CodexProfile>,
    /// Default profile to use when no repo-specific config exists
    pub default_profile: Option<String>,
    /// Number of lines to copy with the copy-last command (default: 50)
    #[serde(default = "default_copy_lines")]
    pub copy_lines: usize,
}

fn default_approval_mode() -> String {
    "suggest".to_string()
}

fn default_sandbox_mode() -> Option<String> {
    Some("workspace-write".to_string())
}

fn default_copy_lines() -> usize {
    50
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            binary: "codex".to_string(),
            model: None,
            inherit_cwd: true,
            args: Vec::new(),
            approval_mode: "suggest".to_string(),
            sandbox_mode: Some("workspace-write".to_string()),
            profiles: HashMap::new(),
            default_profile: None,
            copy_lines: 50,
        }
    }
}

/// The Codex plugin.
#[derive(Debug)]
pub struct CodexPlugin {
    config: CodexConfig,
}

impl CodexPlugin {
    /// Create a new Codex plugin with the given configuration.
    pub fn new(config: CodexConfig) -> Self {
        Self { config }
    }

    /// Map an approval mode string to the corresponding Codex CLI flag.
    fn approval_mode_flag(mode: &str) -> Option<&'static str> {
        match mode {
            "suggest" => Some("--suggest"),
            "auto-edit" => Some("--auto-edit"),
            "full-auto" => Some("--full-auto"),
            _ => None,
        }
    }

    /// Map a sandbox mode string to the corresponding Codex CLI arguments.
    fn sandbox_mode_args(mode: &str) -> Option<(&'static str, &'static str)> {
        match mode {
            "read-only" => Some(("--sandbox", "read-only")),
            "workspace-write" => Some(("--sandbox", "workspace-write")),
            "full-access" => Some(("--sandbox", "danger-full-access")),
            _ => None,
        }
    }

    /// Build the args list for launching Codex.
    ///
    /// `profile_extra_args` are merged from the resolved profile.
    /// `profile_approval_mode` overrides the config-level approval mode if set.
    /// `profile_sandbox_mode` overrides the config-level sandbox mode if set.
    fn build_args(
        &self,
        extra: &[&str],
        profile_extra_args: &[String],
        profile_approval_mode: Option<&str>,
        profile_sandbox_mode: Option<&str>,
    ) -> Vec<String> {
        let effective_approval = profile_approval_mode
            .unwrap_or(self.config.approval_mode.as_str());
        let effective_sandbox = profile_sandbox_mode
            .or(self.config.sandbox_mode.as_deref());

        let mut combined_extra: Vec<String> = Vec::new();

        // Add approval mode flag
        if let Some(flag) = Self::approval_mode_flag(effective_approval) {
            combined_extra.push(flag.to_string());
        }

        // Add sandbox mode flags
        if let Some((flag, value)) = effective_sandbox.and_then(Self::sandbox_mode_args) {
            combined_extra.push(flag.to_string());
            combined_extra.push(value.to_string());
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
    fn resolve_profile(&self, cwd: Option<&str>) -> (Option<&CodexProfile>, Option<String>) {
        // 1. Try repo-specific profile from .termeric/config [codex] section
        let profile_name = cwd
            .and_then(|c| resolve_repo_profile(c, "codex"))
            .or_else(|| self.config.default_profile.clone());

        if let Some(ref name) = profile_name {
            if let Some(profile) = self.config.profiles.get(name) {
                return (Some(profile), Some(name.clone()));
            }
            log::warn!(
                "Codex profile '{}' not found in config, using defaults",
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

        let (profile_extra_args, profile_env, profile_approval, profile_sandbox) = match profile {
            Some(p) => (
                p.extra_args.as_slice(),
                p.env.clone(),
                p.approval_mode.as_deref(),
                p.sandbox_mode.as_deref(),
            ),
            None => (&[] as &[String], HashMap::new(), None, None),
        };

        let mut env = profile_env;
        // Pass terminal context to the spawned process
        env.insert("COLUMNS".to_string(), ctx.terminal_size.0.to_string());
        env.insert("LINES".to_string(), ctx.terminal_size.1.to_string());

        let mut actions = Vec::with_capacity(2);
        if profile.is_none() {
            if let Some(name) = profile_name {
                actions.push(PluginAction::ShowNotice {
                    title: "Codex profile not found".into(),
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
            args: self.build_args(extra_cli_args, profile_extra_args, profile_approval, profile_sandbox),
            cwd: cwd.map(|s| s.to_string()),
            title: Some(title.to_string()),
            plugin_id: Some("codex".to_string()),
            env,
        });
        actions
    }
}

impl Plugin for CodexPlugin {
    fn id(&self) -> &str {
        "codex"
    }

    fn name(&self) -> &str {
        "Codex"
    }

    fn on_hook(&mut self, hook: &Hook, ctx: &PluginContext) -> HookResponse {
        match hook {
            Hook::CommandInvoked { command_id } => match command_id.as_str() {
                "codex.open" => {
                    HookResponse::Actions(self.build_launch_actions(ctx, &[], "Codex"))
                }
                "codex.resume" => {
                    HookResponse::Actions(self.build_launch_actions(
                        ctx,
                        &["--continue"],
                        "Codex (resume)",
                    ))
                }
                "codex.copy-last" => HookResponse::Action(PluginAction::CopyFromPluginTab {
                    plugin_id: "codex".to_string(),
                    lines: self.config.copy_lines,
                }),
                "codex.clear" => HookResponse::Action(PluginAction::ClearPluginTab {
                    plugin_id: "codex".to_string(),
                }),
                "codex.restart" => HookResponse::Action(PluginAction::RestartPlugin {
                    plugin_id: "codex".to_string(),
                }),
                "codex.toggle-transcript" => HookResponse::Action(PluginAction::ToggleTranscript),
                // review-diff, copy-diff, insert-context, export-session, jump-to-error,
                // and select-profile are handled directly by the App
                "codex.review-diff"
                | "codex.copy-diff"
                | "codex.insert-context"
                | "codex.export-session"
                | "codex.jump-to-error"
                | "codex.select-profile" => HookResponse::Handled,
                _ => HookResponse::Ignored,
            },
            _ => HookResponse::Ignored,
        }
    }

    fn commands(&self) -> Vec<PluginCommand> {
        ai_common::standard_ai_commands("codex", "Codex")
    }

    fn keybindings(&self) -> Vec<PluginKeybinding> {
        vec![
            PluginKeybinding {
                key: "k".to_string(),
                ctrl: true,
                alt: false,
                shift: true,
                command_id: "codex.open".to_string(),
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
        let config = CodexConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.binary, "codex");
        assert!(config.model.is_none());
        assert!(config.inherit_cwd);
        assert!(config.args.is_empty());
        assert_eq!(config.approval_mode, "suggest");
        assert_eq!(config.sandbox_mode, Some("workspace-write".to_string()));
        assert!(config.profiles.is_empty());
        assert!(config.default_profile.is_none());
        assert_eq!(config.copy_lines, 50);
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            enabled = true
            binary = "/usr/local/bin/codex"
            model = "o4-mini"
            inherit_cwd = false
            args = ["--notify"]
            approval_mode = "auto-edit"
            sandbox_mode = "read-only"
            copy_lines = 100
        "#;
        let config: CodexConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.binary, "/usr/local/bin/codex");
        assert_eq!(config.model, Some("o4-mini".to_string()));
        assert!(!config.inherit_cwd);
        assert_eq!(config.args, vec!["--notify"]);
        assert_eq!(config.approval_mode, "auto-edit");
        assert_eq!(config.sandbox_mode, Some("read-only".to_string()));
        assert_eq!(config.copy_lines, 100);
    }

    #[test]
    fn test_config_deserialization_defaults() {
        let toml_str = r#"
            enabled = false
        "#;
        let config: CodexConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.binary, "codex");
        assert_eq!(config.approval_mode, "suggest");
        assert_eq!(config.sandbox_mode, Some("workspace-write".to_string()));
        assert_eq!(config.copy_lines, 50);
    }

    #[test]
    fn test_plugin_metadata() {
        let plugin = CodexPlugin::new(CodexConfig::default());
        assert_eq!(plugin.id(), "codex");
        assert_eq!(plugin.name(), "Codex");
    }

    #[test]
    fn test_plugin_commands() {
        let plugin = CodexPlugin::new(CodexConfig::default());
        let commands = plugin.commands();
        assert_eq!(commands.len(), 12);
        assert_eq!(commands[0].id, "codex.open");
        assert_eq!(commands[1].id, "codex.resume");
        assert_eq!(commands[2].id, "codex.copy-last");
        assert_eq!(commands[3].id, "codex.clear");
        assert_eq!(commands[4].id, "codex.restart");
        assert_eq!(commands[5].id, "codex.review-diff");
        assert_eq!(commands[6].id, "codex.copy-diff");
        assert_eq!(commands[7].id, "codex.insert-context");
        assert_eq!(commands[8].id, "codex.toggle-transcript");
        assert_eq!(commands[9].id, "codex.export-session");
        assert_eq!(commands[10].id, "codex.jump-to-error");
        assert_eq!(commands[11].id, "codex.select-profile");
    }

    #[test]
    fn test_plugin_keybindings() {
        let plugin = CodexPlugin::new(CodexConfig::default());
        let bindings = plugin.keybindings();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].key, "k");
        assert_eq!(bindings[0].command_id, "codex.open");
        assert!(bindings[0].ctrl);
        assert!(bindings[0].shift);
        assert!(!bindings[0].alt);
    }

    // --- Approval mode mapping tests ---

    #[test]
    fn test_approval_mode_suggest() {
        assert_eq!(CodexPlugin::approval_mode_flag("suggest"), Some("--suggest"));
    }

    #[test]
    fn test_approval_mode_auto_edit() {
        assert_eq!(CodexPlugin::approval_mode_flag("auto-edit"), Some("--auto-edit"));
    }

    #[test]
    fn test_approval_mode_full_auto() {
        assert_eq!(CodexPlugin::approval_mode_flag("full-auto"), Some("--full-auto"));
    }

    #[test]
    fn test_approval_mode_unknown() {
        assert_eq!(CodexPlugin::approval_mode_flag("unknown"), None);
    }

    // --- Sandbox mode mapping tests ---

    #[test]
    fn test_sandbox_mode_read_only() {
        assert_eq!(CodexPlugin::sandbox_mode_args("read-only"), Some(("--sandbox", "read-only")));
    }

    #[test]
    fn test_sandbox_mode_workspace_write() {
        assert_eq!(
            CodexPlugin::sandbox_mode_args("workspace-write"),
            Some(("--sandbox", "workspace-write"))
        );
    }

    #[test]
    fn test_sandbox_mode_full_access() {
        assert_eq!(
            CodexPlugin::sandbox_mode_args("full-access"),
            Some(("--sandbox", "danger-full-access"))
        );
    }

    #[test]
    fn test_sandbox_mode_unknown() {
        assert_eq!(CodexPlugin::sandbox_mode_args("unknown"), None);
    }

    // --- build_args tests ---

    #[test]
    fn test_build_args_defaults() {
        let plugin = CodexPlugin::new(CodexConfig::default());
        let args = plugin.build_args(&[], &[], None, None);
        // Default: --suggest + --sandbox workspace-write
        assert_eq!(args, vec!["--suggest", "--sandbox", "workspace-write"]);
    }

    #[test]
    fn test_build_args_with_model() {
        let plugin = CodexPlugin::new(CodexConfig {
            model: Some("o4-mini".to_string()),
            ..Default::default()
        });
        let args = plugin.build_args(&[], &[], None, None);
        assert_eq!(args, vec!["--model", "o4-mini", "--suggest", "--sandbox", "workspace-write"]);
    }

    #[test]
    fn test_build_args_full_auto_full_access() {
        let plugin = CodexPlugin::new(CodexConfig {
            approval_mode: "full-auto".to_string(),
            sandbox_mode: Some("full-access".to_string()),
            ..Default::default()
        });
        let args = plugin.build_args(&[], &[], None, None);
        assert_eq!(args, vec!["--full-auto", "--sandbox", "danger-full-access"]);
    }

    #[test]
    fn test_build_args_no_sandbox() {
        let plugin = CodexPlugin::new(CodexConfig {
            sandbox_mode: None,
            ..Default::default()
        });
        let args = plugin.build_args(&[], &[], None, None);
        // Only approval mode flag, no sandbox
        assert_eq!(args, vec!["--suggest"]);
    }

    #[test]
    fn test_build_args_with_extra() {
        let plugin = CodexPlugin::new(CodexConfig {
            args: vec!["--notify".to_string()],
            ..Default::default()
        });
        let args = plugin.build_args(&["--continue"], &[], None, None);
        assert_eq!(
            args,
            vec!["--notify", "--suggest", "--sandbox", "workspace-write", "--continue"]
        );
    }

    #[test]
    fn test_build_args_profile_overrides_approval() {
        let plugin = CodexPlugin::new(CodexConfig::default());
        let args = plugin.build_args(&[], &[], Some("full-auto"), None);
        assert!(args.contains(&"--full-auto".to_string()));
        assert!(!args.contains(&"--suggest".to_string()));
    }

    #[test]
    fn test_build_args_profile_overrides_sandbox() {
        let plugin = CodexPlugin::new(CodexConfig::default());
        let args = plugin.build_args(&[], &[], None, Some("full-access"));
        assert!(args.contains(&"danger-full-access".to_string()));
        assert!(!args.contains(&"workspace-write".to_string()));
    }

    #[test]
    fn test_build_args_profile_extra_args() {
        let plugin = CodexPlugin::new(CodexConfig::default());
        let profile_args = vec!["--notify".to_string(), "--no-color".to_string()];
        let args = plugin.build_args(&[], &profile_args, None, None);
        assert!(args.contains(&"--notify".to_string()));
        assert!(args.contains(&"--no-color".to_string()));
    }

    // --- Hook tests ---

    #[test]
    fn test_hook_open() {
        let mut plugin = CodexPlugin::new(CodexConfig::default());
        let ctx = PluginContext {
            cwd: Some("/tmp/project".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "codex.open".to_string(),
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
                assert_eq!(program, "codex");
                assert!(args.contains(&"--suggest".to_string()));
                assert!(args.contains(&"--sandbox".to_string()));
                assert!(args.contains(&"workspace-write".to_string()));
                assert_eq!(cwd, Some("/tmp/project".to_string()));
                assert_eq!(title, Some("Codex".to_string()));
                assert_eq!(plugin_id, Some("codex".to_string()));
                assert_eq!(env.get("COLUMNS"), Some(&"80".to_string()));
                assert_eq!(env.get("LINES"), Some(&"24".to_string()));
            }
            _ => panic!("Expected OpenTab action"),
        }
    }

    #[test]
    fn test_hook_resume() {
        let mut plugin = CodexPlugin::new(CodexConfig::default());
        let ctx = PluginContext {
            cwd: Some("/tmp".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "codex.resume".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab { args, title, .. }) => {
                assert!(args.contains(&"--continue".to_string()));
                assert_eq!(title, Some("Codex (resume)".to_string()));
            }
            _ => panic!("Expected OpenTab with --continue"),
        }
    }

    #[test]
    fn test_hook_copy_last() {
        let mut plugin = CodexPlugin::new(CodexConfig {
            copy_lines: 75,
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
                command_id: "codex.copy-last".to_string(),
            },
            &ctx,
        );

        match response {
            HookResponse::Action(PluginAction::CopyFromPluginTab { plugin_id, lines }) => {
                assert_eq!(plugin_id, "codex");
                assert_eq!(lines, 75);
            }
            _ => panic!("Expected Action(CopyFromPluginTab)"),
        }
    }

    #[test]
    fn test_hook_clear() {
        let mut plugin = CodexPlugin::new(CodexConfig::default());
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "codex.clear".to_string(),
            },
            &ctx,
        );

        match response {
            HookResponse::Action(PluginAction::ClearPluginTab { plugin_id }) => {
                assert_eq!(plugin_id, "codex");
            }
            _ => panic!("Expected Action(ClearPluginTab)"),
        }
    }

    #[test]
    fn test_hook_restart() {
        let mut plugin = CodexPlugin::new(CodexConfig::default());
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "codex.restart".to_string(),
            },
            &ctx,
        );

        match response {
            HookResponse::Action(PluginAction::RestartPlugin { plugin_id }) => {
                assert_eq!(plugin_id, "codex");
            }
            _ => panic!("Expected Action(RestartPlugin)"),
        }
    }

    #[test]
    fn test_hook_toggle_transcript() {
        let mut plugin = CodexPlugin::new(CodexConfig::default());
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "codex.toggle-transcript".to_string(),
            },
            &ctx,
        );
        assert!(matches!(response, HookResponse::Action(PluginAction::ToggleTranscript)));
    }

    #[test]
    fn test_hook_app_handled_commands() {
        let mut plugin = CodexPlugin::new(CodexConfig::default());
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        for cmd in &[
            "codex.review-diff",
            "codex.copy-diff",
            "codex.insert-context",
            "codex.export-session",
            "codex.jump-to-error",
            "codex.select-profile",
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

    #[test]
    fn test_hook_ignores_other_commands() {
        let mut plugin = CodexPlugin::new(CodexConfig::default());
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
    fn test_hook_ignores_non_command_hooks() {
        let mut plugin = CodexPlugin::new(CodexConfig::default());
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(&Hook::AppReady, &ctx);
        assert!(matches!(response, HookResponse::Ignored));
    }

    #[test]
    fn test_cwd_disabled() {
        let mut plugin = CodexPlugin::new(CodexConfig {
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
                command_id: "codex.open".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab { cwd, .. }) => {
                assert_eq!(cwd, None);
            }
            _ => panic!("Expected OpenTab action"),
        }
    }

    // --- Profile resolution tests ---

    #[test]
    fn test_resolve_profile_no_profiles_configured() {
        let plugin = CodexPlugin::new(CodexConfig::default());
        let (profile, name) = plugin.resolve_profile(Some("/tmp/nonexistent"));
        assert!(profile.is_none());
        assert!(name.is_none());
    }

    #[test]
    fn test_resolve_profile_default_profile() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "safe".to_string(),
            CodexProfile {
                description: "Safe mode".to_string(),
                approval_mode: Some("suggest".to_string()),
                sandbox_mode: Some("read-only".to_string()),
                ..Default::default()
            },
        );
        let plugin = CodexPlugin::new(CodexConfig {
            default_profile: Some("safe".to_string()),
            profiles,
            ..Default::default()
        });
        let (profile, name) = plugin.resolve_profile(None);
        assert!(profile.is_some());
        assert_eq!(name, Some("safe".to_string()));
        assert_eq!(
            profile.unwrap().approval_mode,
            Some("suggest".to_string())
        );
    }

    #[test]
    fn test_resolve_profile_missing_named_profile() {
        let plugin = CodexPlugin::new(CodexConfig {
            default_profile: Some("nonexistent".to_string()),
            ..Default::default()
        });
        let (profile, name) = plugin.resolve_profile(None);
        assert!(profile.is_none());
        assert_eq!(name, Some("nonexistent".to_string()));
    }

    #[test]
    fn test_open_with_profile_env_passthrough() {
        let mut env = HashMap::new();
        env.insert("OPENAI_API_KEY".to_string(), "test-key".to_string());
        env.insert("CUSTOM_VAR".to_string(), "custom-val".to_string());

        let mut profiles = HashMap::new();
        profiles.insert(
            "work".to_string(),
            CodexProfile {
                description: "Work profile".to_string(),
                env: env.clone(),
                extra_args: vec!["--notify".to_string()],
                approval_mode: Some("full-auto".to_string()),
                sandbox_mode: Some("full-access".to_string()),
                ..Default::default()
            },
        );

        let mut plugin = CodexPlugin::new(CodexConfig {
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
                command_id: "codex.open".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab {
                args,
                env: tab_env,
                ..
            }) => {
                assert_eq!(tab_env.get("OPENAI_API_KEY"), Some(&"test-key".to_string()));
                assert_eq!(tab_env.get("CUSTOM_VAR"), Some(&"custom-val".to_string()));
                assert!(args.contains(&"--full-auto".to_string()));
                assert!(!args.contains(&"--suggest".to_string()));
                assert!(args.contains(&"danger-full-access".to_string()));
                assert!(!args.contains(&"workspace-write".to_string()));
                assert!(args.contains(&"--notify".to_string()));
            }
            _ => panic!("Expected OpenTab action"),
        }
    }

    #[test]
    fn test_open_no_profile_empty_env() {
        let mut plugin = CodexPlugin::new(CodexConfig::default());
        let ctx = PluginContext {
            cwd: Some("/tmp".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };

        let response = plugin.on_hook(
            &Hook::CommandInvoked {
                command_id: "codex.open".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab { env, .. }) => {
                assert_eq!(env.len(), 2);
                assert_eq!(env.get("COLUMNS"), Some(&"80".to_string()));
                assert_eq!(env.get("LINES"), Some(&"24".to_string()));
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
            CodexProfile {
                env: env.clone(),
                ..Default::default()
            },
        );

        let mut plugin = CodexPlugin::new(CodexConfig {
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
                command_id: "codex.resume".to_string(),
            },
            &ctx,
        );

        match extract_open_tab(response) {
            Some(PluginAction::OpenTab {
                args,
                env: tab_env,
                ..
            }) => {
                assert_eq!(tab_env.get("MY_VAR"), Some(&"my-val".to_string()));
                assert!(args.contains(&"--continue".to_string()));
            }
            _ => panic!("Expected OpenTab with --continue"),
        }
    }
}
