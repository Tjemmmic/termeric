//! Exact-owner profile selection. Profiles configure future launches only.
use super::*;
use crate::tabs::TerminalIdentity;

impl App {
    pub(super) fn show_plugin_profiles(&mut self, plugin_id: String) {
        let mut names: Vec<String> = match plugin_id.as_str() {
            "codex" => self.config.plugins.codex.profiles.keys().cloned().collect(),
            "claude-code" => self
                .config
                .plugins
                .claude_code
                .profiles
                .keys()
                .cloned()
                .collect(),
            _ => {
                self.toasts
                    .warning("Profile selection is unavailable for this plugin");
                return;
            }
        };
        let Some(terminal) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.plugin_terminal_identity(&plugin_id))
        else {
            self.toasts
                .warning(format!("No active {plugin_id} terminal found"));
            return;
        };
        names.sort();
        let truncated = names.len() >= crate::ui::info_overlay::MAX_INFO_CHOICES;
        names.truncate(crate::ui::info_overlay::MAX_INFO_CHOICES - 1);
        let mut profiles = std::collections::BTreeMap::new();
        let mut choices = Vec::new();
        let mut lines = vec![
            "Select a profile for the next restart:".into(),
            String::new(),
        ];
        // Request-local opaque IDs cannot collide with profile names or be
        // truncated by the overlay, even for unusually long configured names.
        for (i, profile) in std::iter::once(None)
            .chain(names.into_iter().map(Some))
            .enumerate()
        {
            let id = format!("profile-choice:{i}");
            let label = profile_label(profile.as_deref());
            let shortcut = (i < 9).then(|| char::from_digit((i + 1) as u32, 10).unwrap());
            let source_line = lines.len();
            lines.push(format!("  {}  {label}", shortcut.unwrap_or(' ')));
            choices.push(crate::ui::info_overlay::InfoOverlayChoice::new(
                &id,
                label,
                source_line,
                shortcut,
            ));
            profiles.insert(id, profile);
        }
        let profile = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(terminal.tab_id))
            .and_then(|tab| tab.pane_plugin_launch(terminal.pane_id))
            .and_then(|(_, profile)| profile);
        lines.push(String::new());
        lines.push(format!("Next restart: {}", profile_label(profile)));
        if truncated {
            lines.push(format!(
                "Only the first {} named profiles are shown.",
                crate::ui::info_overlay::MAX_INFO_CHOICES - 1
            ));
        }
        let request_id =
            self.info_overlay
                .show_choices(format!("Select Profile — {plugin_id}"), lines, choices);
        self.pending_profile_selection = Some(PendingProfileSelection {
            request_id,
            terminal,
            plugin_id,
            profiles,
        });
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn apply_plugin_profile(
        &mut self,
        terminal: TerminalIdentity,
        plugin_id: &str,
        profile: Option<String>,
    ) {
        let configured = match plugin_id {
            "codex" => profile
                .as_ref()
                .is_none_or(|name| self.config.plugins.codex.profiles.contains_key(name)),
            "claude-code" => profile
                .as_ref()
                .is_none_or(|name| self.config.plugins.claude_code.profiles.contains_key(name)),
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
                && tab.set_plugin_profile_reference(terminal.pane_id, plugin_id, profile.clone())
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
                format!("Next restart: {}", profile_label(profile.as_deref())),
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

fn profile_label(profile: Option<&str>) -> String {
    profile.map_or_else(
        || "Automatic profile".into(),
        |name| format!("Profile: {name}"),
    )
}
