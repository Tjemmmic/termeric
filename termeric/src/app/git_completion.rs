//! Keyboard ownership for the inline Git completion popup.

use super::*;
use winit::keyboard::{Key, NamedKey};

impl App {
    pub(super) fn git_completion_has_room(&self) -> bool {
        self.renderer
            .as_ref()
            .is_none_or(crate::renderer::Renderer::git_completion_fits)
    }

    pub(super) fn dismiss_git_completion_popup(&mut self) {
        self.git_completions_visible = false;
        self.git_completions.clear();
        self.git_completion_index = None;
    }

    /// Called for pressed keys after modal/chrome owners and before shortcuts.
    pub(super) fn handle_git_completion_key(&mut self, key: &Key) -> bool {
        if !self.git_completions_visible {
            return false;
        }
        if !self.git_completion_has_room() {
            self.dismiss_git_completion_popup();
            return false;
        }
        if self.focused_pane_read_only() {
            self.clear_terminal_input_tracking();
            return false;
        }
        if !matches!(self.broadcast, BroadcastState::Inactive)
            || !self.modifiers.is_empty()
            || self.alt_graph_active
            || self.git_completions.is_empty()
            || self
                .git_completion_index
                .is_some_and(|index| index >= self.git_completions.len())
        {
            // Shortcuts, broadcast controls, and modified terminal keys keep
            // ownership. Drop the popup before a legacy modified Enter can
            // encode as CR and reach the editable-input completion path.
            self.dismiss_git_completion_popup();
            return false;
        }
        match key {
            Key::Named(NamedKey::Tab) => {
                let handled = self.accept_git_completion(false);
                self.dismiss_git_completion_popup();
                handled
            }
            Key::Named(NamedKey::ArrowDown) => {
                let max = self.git_completions.len() - 1;
                self.git_completion_index = Some(
                    self.git_completion_index
                        .map_or(0, |index| index.saturating_add(1).min(max)),
                );
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.git_completion_index = Some(
                    self.git_completion_index
                        .map_or(self.git_completions.len() - 1, |index| {
                            index.saturating_sub(1)
                        }),
                );
                true
            }
            Key::Named(NamedKey::Escape) => {
                self.dismiss_git_completion_popup();
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completion_app() -> App {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.input_line_buffer = "git checkout ma".into();
        app.git_completions = vec!["main".into(), "maintenance".into()].into();
        app.git_completion_index = Some(0);
        app.git_completions_visible = true;
        app
    }

    #[test]
    fn modified_completion_keys_preserve_shortcut_and_terminal_ownership() {
        for modifiers in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::SHIFT,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        ] {
            for key in [
                NamedKey::Tab,
                NamedKey::ArrowDown,
                NamedKey::ArrowUp,
                NamedKey::Escape,
                NamedKey::Enter,
            ] {
                let mut app = completion_app();
                app.modifiers = modifiers;
                assert!(
                    !app.handle_git_completion_key(&Key::Named(key)),
                    "{modifiers:?} {key:?} was intercepted"
                );
                assert_eq!(app.git_completion_index, None);
                assert!(!app.git_completions_visible);
                assert!(app.git_completions.is_empty());
                assert_eq!(app.input_line_buffer, "git checkout ma");
            }
        }
    }

    #[test]
    fn altgraph_and_broadcast_ownership_retire_only_the_popup() {
        for mode in ["altgraph", "armed", "active"] {
            for key in [
                NamedKey::Tab,
                NamedKey::ArrowDown,
                NamedKey::ArrowUp,
                NamedKey::Enter,
            ] {
                let mut app = completion_app();
                match mode {
                    "altgraph" => app.alt_graph_active = true,
                    "armed" => {
                        app.broadcast = BroadcastState::Armed {
                            tab_id: 0,
                            targets: vec![],
                            expires: Instant::now() + Duration::from_secs(5),
                        }
                    }
                    _ => {
                        app.broadcast = BroadcastState::Active {
                            tab_id: 0,
                            targets: vec![],
                        }
                    }
                }
                assert!(!app.handle_git_completion_key(&Key::Named(key)));
                assert!(!app.git_completions_visible);
                assert_eq!(app.input_line_buffer, "git checkout ma");
                assert_eq!(app.alt_graph_active, mode == "altgraph");
                assert_eq!(
                    matches!(app.broadcast, BroadcastState::Inactive),
                    mode == "altgraph"
                );
            }
        }
    }

    #[test]
    fn unmodified_navigation_is_bounded_and_stale_popups_fall_through() {
        let mut app = completion_app();
        for expected in [1, 1] {
            assert!(app.handle_git_completion_key(&Key::Named(NamedKey::ArrowDown)));
            assert_eq!(app.git_completion_index, Some(expected));
        }
        for expected in [0, 0] {
            assert!(app.handle_git_completion_key(&Key::Named(NamedKey::ArrowUp)));
            assert_eq!(app.git_completion_index, Some(expected));
        }
        app.git_completion_index = None;
        assert!(app.handle_git_completion_key(&Key::Named(NamedKey::ArrowUp)));
        assert_eq!(app.git_completion_index, Some(1));
        app.git_completion_index = None;
        assert!(app.handle_git_completion_key(&Key::Named(NamedKey::ArrowDown)));
        assert_eq!(app.git_completion_index, Some(0));
        assert!(!app.handle_git_completion_key(&Key::Named(NamedKey::Enter)));
        assert!(app.git_completions_visible);
        assert!(app.handle_git_completion_key(&Key::Named(NamedKey::Escape)));
        assert_eq!(app.input_line_buffer, "git checkout ma");
        assert!(!app.git_completions_visible);
        for invalid in [None, Some(usize::MAX)] {
            let mut app = completion_app();
            if invalid.is_none() {
                app.git_completions.clear();
            }
            app.git_completion_index = invalid;
            assert!(!app.handle_git_completion_key(&Key::Named(NamedKey::ArrowDown)));
            assert!(!app.git_completions_visible);
            assert_eq!(app.input_line_buffer, "git checkout ma");
        }
        let mut app = completion_app();
        app.git_completion_index = None;
        assert!(!app.handle_git_completion_key(&Key::Named(NamedKey::Tab)));
        assert!(!app.git_completions_visible);
    }
}
