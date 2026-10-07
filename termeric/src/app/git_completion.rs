//! Keyboard ownership for the inline Git completion popup.

use super::*;
use winit::keyboard::{Key, NamedKey};

/// The inline Git branch completion popup. It exists only while it has
/// suggestions, and its highlighted row is always one of them, so dismissing
/// it is one assignment and no half-reset state can be written.
#[derive(Debug, Clone)]
pub(super) struct GitCompletionPopup {
    completions: crate::git::BranchCompletions,
    selected: usize,
}

impl GitCompletionPopup {
    /// A popup over `branches` with the first highlighted, or `None` when
    /// there is nothing to suggest.
    pub(super) fn new(branches: Vec<String>) -> Option<Self> {
        (!branches.is_empty()).then(|| Self {
            completions: branches.into(),
            selected: 0,
        })
    }

    pub(super) fn completions(&self) -> &crate::git::BranchCompletions {
        &self.completions
    }

    pub(super) fn selected(&self) -> usize {
        self.selected
    }

    pub(super) fn selected_branch(&self) -> &str {
        &self.completions[self.selected]
    }

    fn select_next(&mut self) {
        self.selected = (self.selected + 1).min(self.completions.len() - 1);
    }

    fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

impl App {
    pub(super) fn git_completion_has_room(&self) -> bool {
        self.renderer
            .as_ref()
            .is_none_or(crate::renderer::Renderer::git_completion_fits)
    }

    pub(super) fn dismiss_git_completion_popup(&mut self) {
        self.git_completion = None;
    }

    /// Called for pressed keys after modal/chrome owners and before shortcuts.
    pub(super) fn handle_git_completion_key(&mut self, key: &Key) -> bool {
        if self.git_completion.is_none() {
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
                if let Some(popup) = &mut self.git_completion {
                    popup.select_next();
                }
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(popup) = &mut self.git_completion {
                    popup.select_previous();
                }
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
        app.git_completion = GitCompletionPopup::new(vec!["main".into(), "maintenance".into()]);
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
                assert!(app.git_completion.is_none());
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
                assert!(app.git_completion.is_none());
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
    fn unmodified_navigation_is_bounded() {
        let mut app = completion_app();
        let selected = |app: &App| {
            app.git_completion
                .as_ref()
                .map(GitCompletionPopup::selected)
        };
        for expected in [1, 1] {
            assert!(app.handle_git_completion_key(&Key::Named(NamedKey::ArrowDown)));
            assert_eq!(selected(&app), Some(expected));
        }
        for expected in [0, 0] {
            assert!(app.handle_git_completion_key(&Key::Named(NamedKey::ArrowUp)));
            assert_eq!(selected(&app), Some(expected));
        }
        assert!(!app.handle_git_completion_key(&Key::Named(NamedKey::Enter)));
        assert!(app.git_completion.is_some());
        assert!(app.handle_git_completion_key(&Key::Named(NamedKey::Escape)));
        assert_eq!(app.input_line_buffer, "git checkout ma");
        assert!(app.git_completion.is_none());
        assert!(!app.handle_git_completion_key(&Key::Named(NamedKey::ArrowDown)));
    }

    #[test]
    fn a_popup_needs_suggestions() {
        assert!(GitCompletionPopup::new(Vec::new()).is_none());
        let popup = GitCompletionPopup::new(vec!["main".into()]).unwrap();
        assert_eq!(popup.selected_branch(), "main");
    }
}
