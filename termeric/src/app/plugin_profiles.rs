//! Exact-owner profile selection. Profiles configure future launches only.
use super::*;
use crate::tabs::TerminalIdentity;

impl App {
    pub(super) fn apply_plugin_profile(
        &mut self,
        terminal: TerminalIdentity,
        plugin_id: &str,
        profile_name: String,
    ) {
        let configured = match plugin_id {
            "codex" => {
                profile_name == "default"
                    || self
                        .config
                        .plugins
                        .codex
                        .profiles
                        .contains_key(&profile_name)
            }
            "claude-code" => {
                profile_name == "default"
                    || self
                        .config
                        .plugins
                        .claude_code
                        .profiles
                        .contains_key(&profile_name)
            }
            _ => false,
        };
        if !configured {
            self.toasts
                .warning("Profile is no longer configured; no terminal was changed");
            return;
        }
        let applied = self.tabs.as_mut().is_some_and(|tabs| {
            let Some(origin) = tabs.origin_for_pane(terminal.tab_id, terminal.pane_id) else {
                return false;
            };
            if tabs.origin_read_only(origin) != Some(false) {
                return false;
            }
            let tab = tabs
                .tab_mut_by_id(terminal.tab_id)
                .expect("resolved profile owner");
            tab.live_domain.is_none()
                && tab.set_plugin_profile(terminal.pane_id, plugin_id, profile_name.clone())
        });
        if !applied {
            self.toasts.warning(
                "Original plugin terminal is unavailable or read-only; no profile was changed",
            );
            return;
        }
        self.info_overlay.show(
            "Restart Profile Saved".to_string(),
            vec![
                format!("Next restart profile: {profile_name}"),
                String::new(),
                "The running session has not changed.".into(),
                "Other terminals keep their own profiles.".into(),
            ],
        );
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
