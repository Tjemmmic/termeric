//! Shared utilities for AI-tool plugins (Claude Code, Codex, etc.)
//!
//! Provides common building blocks so each AI plugin only needs to define
//! its tool-specific config and CLI mapping while reusing:
//!
//! - Argument construction (`build_args_with_model`)
//! - Working-directory resolution (`resolve_cwd`)
//! - The standard 12-command set (`standard_ai_commands`)

use super::hooks::{PluginCommand, PluginContext};

/// Build a CLI argument list with an optional `--model` flag, config-level
/// args, and extra per-invocation args.
///
/// The resulting order is: `[--model <m>] [config_args...] [extra...]`
pub fn build_args_with_model(
    model: Option<&str>,
    config_args: &[String],
    extra: &[String],
) -> Vec<String> {
    let mut args = Vec::new();

    if let Some(m) = model {
        args.push("--model".to_string());
        args.push(m.to_string());
    }

    args.extend(config_args.iter().cloned());
    args.extend(extra.iter().cloned());

    args
}

/// Resolve the working directory from the plugin context.
///
/// Returns `Some(cwd)` when `inherit_cwd` is `true` and the context has a cwd,
/// `None` otherwise.
pub fn resolve_cwd(inherit_cwd: bool, ctx: &PluginContext) -> Option<&str> {
    if inherit_cwd {
        ctx.cwd.as_deref()
    } else {
        None
    }
}

/// Generate the standard 12-command set shared by all AI-tool plugins.
///
/// Each command ID is prefixed with `prefix` (e.g. `"claude-code"` or `"codex"`)
/// and the display name uses `display_name` (e.g. `"Claude Code"` or `"Codex"`).
pub fn standard_ai_commands(prefix: &str, display_name: &str) -> Vec<PluginCommand> {
    vec![
        PluginCommand {
            id: format!("{}.open", prefix),
            name: format!("Open {}", display_name),
            description: format!("Launch {} AI assistant in a new tab", display_name),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.resume", prefix),
            name: format!("{}: Resume", display_name),
            description: format!("Resume the last {} conversation", display_name),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.copy-last", prefix),
            name: format!("{}: Copy Last Response", display_name),
            description: format!("Copy the last lines from the {} tab", display_name),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.clear", prefix),
            name: format!("{}: Clear Session", display_name),
            description: format!("Send Ctrl+C and clear the {} tab", display_name),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.restart", prefix),
            name: format!("{}: Restart", display_name),
            description: format!("Close and reopen {} in the same directory", display_name),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.review-diff", prefix),
            name: format!("{}: Review Diff", display_name),
            description: "Open the diff review overlay for detected changes".to_string(),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.copy-diff", prefix),
            name: format!("{}: Copy Diff", display_name),
            description: "Copy the latest detected diff to clipboard".to_string(),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.insert-context", prefix),
            name: format!("{}: Insert Context...", display_name),
            description: format!(
                "Open context picker to assemble and inject context into {}",
                display_name
            ),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.toggle-transcript", prefix),
            name: format!("{}: Toggle Transcript", display_name),
            description: "Start or stop recording the session transcript".to_string(),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.export-session", prefix),
            name: format!("{}: Export Session", display_name),
            description: "Export the current session as Markdown or JSONL".to_string(),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.jump-to-error", prefix),
            name: format!("{}: Jump to Last Error", display_name),
            description: "Scroll to the last detected error in terminal output".to_string(),
            category: Some("AI".to_string()),
        },
        PluginCommand {
            id: format!("{}.select-profile", prefix),
            name: format!("{}: Select Profile", display_name),
            description: format!("Choose a {} configuration profile", display_name),
            category: Some("AI".to_string()),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- build_args_with_model tests ---

    #[test]
    fn test_build_args_empty() {
        let args = build_args_with_model(None, &[], &[]);
        assert!(args.is_empty());
    }

    #[test]
    fn test_build_args_model_only() {
        let args = build_args_with_model(Some("sonnet"), &[], &[]);
        assert_eq!(args, vec!["--model", "sonnet"]);
    }

    #[test]
    fn test_build_args_config_only() {
        let config = vec!["--verbose".to_string()];
        let args = build_args_with_model(None, &config, &[]);
        assert_eq!(args, vec!["--verbose"]);
    }

    #[test]
    fn test_build_args_extra_only() {
        let extra = vec!["--continue".to_string()];
        let args = build_args_with_model(None, &[], &extra);
        assert_eq!(args, vec!["--continue"]);
    }

    #[test]
    fn test_build_args_all() {
        let config = vec!["--verbose".to_string()];
        let extra = vec!["--continue".to_string()];
        let args = build_args_with_model(Some("opus"), &config, &extra);
        assert_eq!(args, vec!["--model", "opus", "--verbose", "--continue"]);
    }

    #[test]
    fn test_build_args_ordering() {
        // Model comes first, then config, then extra
        let config = vec!["c1".to_string(), "c2".to_string()];
        let extra = vec!["e1".to_string(), "e2".to_string()];
        let args = build_args_with_model(Some("m"), &config, &extra);
        assert_eq!(args, vec!["--model", "m", "c1", "c2", "e1", "e2"]);
    }

    // --- resolve_cwd tests ---

    #[test]
    fn test_resolve_cwd_inherit_true_with_cwd() {
        let ctx = PluginContext {
            cwd: Some("/tmp/project".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        assert_eq!(resolve_cwd(true, &ctx), Some("/tmp/project"));
    }

    #[test]
    fn test_resolve_cwd_inherit_true_without_cwd() {
        let ctx = PluginContext {
            cwd: None,
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        assert_eq!(resolve_cwd(true, &ctx), None);
    }

    #[test]
    fn test_resolve_cwd_inherit_false() {
        let ctx = PluginContext {
            cwd: Some("/tmp/project".to_string()),
            tab_count: 1,
            active_tab: 0,
            terminal_size: (80, 24),
        };
        assert_eq!(resolve_cwd(false, &ctx), None);
    }

    // --- standard_ai_commands tests ---

    #[test]
    fn test_standard_commands_count() {
        let cmds = standard_ai_commands("test-tool", "Test Tool");
        assert_eq!(cmds.len(), 12);
    }

    #[test]
    fn test_standard_commands_ids() {
        let cmds = standard_ai_commands("claude-code", "Claude Code");
        let ids: Vec<&str> = cmds.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "claude-code.open",
                "claude-code.resume",
                "claude-code.copy-last",
                "claude-code.clear",
                "claude-code.restart",
                "claude-code.review-diff",
                "claude-code.copy-diff",
                "claude-code.insert-context",
                "claude-code.toggle-transcript",
                "claude-code.export-session",
                "claude-code.jump-to-error",
                "claude-code.select-profile",
            ]
        );
    }

    #[test]
    fn test_standard_commands_codex_prefix() {
        let cmds = standard_ai_commands("codex", "Codex");
        assert_eq!(cmds[0].id, "codex.open");
        assert_eq!(cmds[0].name, "Open Codex");
        assert_eq!(cmds[1].id, "codex.resume");
        assert_eq!(cmds[1].name, "Codex: Resume");
    }

    #[test]
    fn test_standard_commands_category() {
        let cmds = standard_ai_commands("foo", "Foo");
        for cmd in &cmds {
            assert_eq!(cmd.category, Some("AI".to_string()));
        }
    }
}
