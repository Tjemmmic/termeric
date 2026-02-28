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

use super::hooks::{
    Hook, HookResponse, Plugin, PluginAction, PluginCommand, PluginContext, PluginKeybinding,
};
use super::claude_profiles::ClaudeProfile;

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
    fn build_args(&self, extra: &[&str]) -> Vec<String> {
        let mut args = Vec::new();

        if let Some(model) = &self.config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        args.extend(self.config.args.iter().cloned());
        args.extend(extra.iter().map(|s| s.to_string()));

        args
    }

    /// Resolve the working directory: use context cwd if inherit_cwd is enabled.
    fn resolve_cwd<'a>(&self, ctx: &'a PluginContext) -> Option<&'a str> {
        if self.config.inherit_cwd {
            ctx.cwd.as_deref()
        } else {
            None
        }
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
                "claude-code.open" => HookResponse::Action(PluginAction::OpenTab {
                    program: self.config.binary.clone(),
                    args: self.build_args(&[]),
                    cwd: self.resolve_cwd(ctx).map(|s| s.to_string()),
                    title: Some("Claude Code".to_string()),
                    plugin_id: Some("claude-code".to_string()),
                }),
                "claude-code.resume" => HookResponse::Action(PluginAction::OpenTab {
                    program: self.config.binary.clone(),
                    args: self.build_args(&["--continue"]),
                    cwd: self.resolve_cwd(ctx).map(|s| s.to_string()),
                    title: Some("Claude Code (resume)".to_string()),
                    plugin_id: Some("claude-code".to_string()),
                }),
                "claude-code.copy-last" => HookResponse::Action(PluginAction::CopyFromPluginTab {
                    plugin_id: "claude-code".to_string(),
                    lines: 50,
                }),
                "claude-code.clear" => HookResponse::Action(PluginAction::ClearPluginTab {
                    plugin_id: "claude-code".to_string(),
                }),
                "claude-code.restart" => HookResponse::Action(PluginAction::RestartPlugin {
                    plugin_id: "claude-code".to_string(),
                }),
                // review-diff, copy-diff, and insert-context are handled directly by the App
                "claude-code.review-diff" | "claude-code.copy-diff" | "claude-code.insert-context" => HookResponse::Handled,
                _ => HookResponse::Ignored,
            },
            _ => HookResponse::Ignored,
        }
    }

    fn commands(&self) -> Vec<PluginCommand> {
        vec![
            PluginCommand {
                id: "claude-code.open".to_string(),
                name: "Open Claude Code".to_string(),
                description: "Launch Claude Code AI assistant in a new tab".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.resume".to_string(),
                name: "Claude Code: Resume".to_string(),
                description: "Resume the last Claude Code conversation".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.copy-last".to_string(),
                name: "Claude Code: Copy Last Response".to_string(),
                description: "Copy the last 50 lines from the Claude Code tab".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.clear".to_string(),
                name: "Claude Code: Clear Session".to_string(),
                description: "Send Ctrl+C and clear the Claude Code tab".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.restart".to_string(),
                name: "Claude Code: Restart".to_string(),
                description: "Close and reopen Claude Code in the same directory".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.review-diff".to_string(),
                name: "Claude Code: Review Diff".to_string(),
                description: "Open the diff review overlay for detected changes".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.copy-diff".to_string(),
                name: "Claude Code: Copy Diff".to_string(),
                description: "Copy the latest detected diff to clipboard".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.insert-context".to_string(),
                name: "Claude Code: Insert Context...".to_string(),
                description: "Open context picker to assemble and inject context into Claude".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.toggle-transcript".to_string(),
                name: "Claude Code: Toggle Transcript".to_string(),
                description: "Start or stop recording the session transcript".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.export-session".to_string(),
                name: "Claude Code: Export Session".to_string(),
                description: "Export the current session as Markdown or JSONL".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.jump-to-error".to_string(),
                name: "Claude Code: Jump to Last Error".to_string(),
                description: "Scroll to the last detected error in terminal output".to_string(),
                category: Some("AI".to_string()),
            },
            PluginCommand {
                id: "claude-code.select-profile".to_string(),
                name: "Claude Code: Select Profile".to_string(),
                description: "Choose a profile for the Claude Code session".to_string(),
                category: Some("AI".to_string()),
            },
        ]
    }

    fn keybindings(&self) -> Vec<PluginKeybinding> {
        vec![PluginKeybinding {
            key: "i".to_string(),
            ctrl: true,
            alt: false,
            shift: true,
            command_id: "claude-code.open".to_string(),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ClaudeCodeConfig::default();
        assert!(config.enabled);
        assert_eq!(config.binary, "claude");
        assert!(config.model.is_none());
        assert!(config.inherit_cwd);
        assert!(config.args.is_empty());
    }

    #[test]
    fn test_build_args_basic() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig::default());
        let args = plugin.build_args(&[]);
        assert!(args.is_empty()); // No model, no extra args
    }

    #[test]
    fn test_build_args_with_model() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            model: Some("sonnet".to_string()),
            ..Default::default()
        });
        let args = plugin.build_args(&[]);
        assert_eq!(args, vec!["--model", "sonnet"]);
    }

    #[test]
    fn test_build_args_with_extra() {
        let plugin = ClaudeCodePlugin::new(ClaudeCodeConfig {
            args: vec!["--verbose".to_string()],
            ..Default::default()
        });
        let args = plugin.build_args(&["--continue"]);
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
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].key, "i");
        assert!(bindings[0].ctrl);
        assert!(bindings[0].shift);
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

        match response {
            HookResponse::Action(PluginAction::OpenTab {
                program,
                args,
                cwd,
                title,
                plugin_id,
            }) => {
                assert_eq!(program, "claude");
                assert!(args.is_empty());
                assert_eq!(cwd, Some("/tmp/project".to_string()));
                assert_eq!(title, Some("Claude Code".to_string()));
                assert_eq!(plugin_id, Some("claude-code".to_string()));
            }
            _ => panic!("Expected Action(OpenTab)"),
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

        match response {
            HookResponse::Action(PluginAction::OpenTab { args, title, .. }) => {
                assert!(args.contains(&"--continue".to_string()));
                assert_eq!(title, Some("Claude Code (resume)".to_string()));
            }
            _ => panic!("Expected Action(OpenTab) with --continue"),
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

        match response {
            HookResponse::Action(PluginAction::OpenTab { cwd, .. }) => {
                assert_eq!(cwd, None); // cwd not passed when inherit_cwd is false
            }
            _ => panic!("Expected Action(OpenTab)"),
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
    }
}
