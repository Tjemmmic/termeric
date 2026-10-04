//! The toolbar Workspaces menu: opening, keyboard ownership, pointer
//! handling, and activation.

use winit::keyboard::{Key, NamedKey};

use super::{App, RedrawReason};
use crate::renderer::{PopupMenuInfo, TabBarClick};
use crate::ui::popup_menu::{PopupMenu, PopupMenuLayout};
use crate::ui::workspace_menu::{WorkspaceMenuAction, workspace_menu_rows};

impl App {
    /// Open the menu with fresh workspace entries, or close it when open.
    /// Keyboard opening highlights the first entry so Enter works at once.
    pub(super) fn toggle_workspace_menu(&mut self, from_keyboard: bool) {
        if self.workspace_menu.take().is_some() {
            self.request_redraw(RedrawReason::Action);
            return;
        }
        let rows = workspace_menu_rows(
            &crate::workspace::list_workspaces(),
            self.current_workspace_id.as_deref(),
            self.is_live_domain_window(),
        );
        self.workspace_menu = Some(PopupMenu::new(rows, from_keyboard));
        self.request_redraw(RedrawReason::Action);
    }

    pub(super) fn close_workspace_menu(&mut self) {
        if self.workspace_menu.take().is_some() {
            self.request_redraw(RedrawReason::Action);
        }
    }

    /// Geometry of the open menu, anchored under the Workspaces button.
    pub(super) fn workspace_menu_layout(&self) -> Option<PopupMenuLayout> {
        let menu = self.workspace_menu.as_ref()?;
        let renderer = self.renderer.as_ref()?;
        let size = self.window.as_ref()?.inner_size();
        let (width, height) = (size.width as f32, size.height as f32);
        let button =
            renderer.tab_bar_element_rect(TabBarClick::SessionDropdown, &[], width, height)?;
        Some(PopupMenuLayout::calculate(
            menu.rows(),
            button.right(),
            renderer.tab_bar_height(),
            width,
            renderer.scale_factor(),
        ))
    }

    pub(super) fn workspace_menu_render_info(&self) -> Option<PopupMenuInfo> {
        let menu = self.workspace_menu.as_ref()?;
        let layout = self.workspace_menu_layout()?;
        Some(PopupMenuInfo::new(menu.rows(), menu.highlighted(), layout))
    }

    /// While open, the menu owns every key so nothing reaches the terminal.
    pub(super) fn handle_workspace_menu_key(&mut self, key: &Key) {
        let Some(menu) = self.workspace_menu.as_mut() else {
            return;
        };
        match key {
            Key::Named(NamedKey::Escape) => {
                self.close_workspace_menu();
                return;
            }
            Key::Named(NamedKey::ArrowDown) => menu.move_highlight(true),
            Key::Named(NamedKey::ArrowUp) => menu.move_highlight(false),
            Key::Named(NamedKey::Home) => menu.highlight_edge(false),
            Key::Named(NamedKey::End) => menu.highlight_edge(true),
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                if let Some(action) = menu.highlighted_action() {
                    self.activate_workspace_menu(action);
                }
                return;
            }
            _ => return,
        }
        self.request_redraw(RedrawReason::Action);
    }

    /// Highlight the row under the pointer. Returns whether it changed.
    pub(super) fn hover_workspace_menu(&mut self, x: f32, y: f32) -> bool {
        let row = self
            .workspace_menu_layout()
            .and_then(|layout| layout.row_at(x, y));
        let Some(menu) = self.workspace_menu.as_mut() else {
            return false;
        };
        let before = menu.highlighted();
        menu.set_highlight(row);
        before != menu.highlighted()
    }

    /// Activate the clicked row, or dismiss the menu on an outside click.
    pub(super) fn click_workspace_menu(&mut self, x: f32, y: f32) {
        let Some(layout) = self.workspace_menu_layout() else {
            return;
        };
        if !layout.contains(x, y) {
            self.close_workspace_menu();
            return;
        }
        let action = layout.row_at(x, y).and_then(|row| {
            self.workspace_menu
                .as_ref()
                .and_then(|menu| menu.action_at(row))
        });
        if let Some(action) = action {
            self.activate_workspace_menu(action);
        }
    }

    pub(super) fn activate_workspace_menu(&mut self, action: WorkspaceMenuAction) {
        self.close_workspace_menu();
        match action {
            WorkspaceMenuAction::Open(id) => self.request_open_workspace(&id),
            WorkspaceMenuAction::Save => {
                self.force_save_workspace(super::workspaces::WorkspaceSave::Update)
            }
            WorkspaceMenuAction::SaveAs => {
                self.force_save_workspace(super::workspaces::WorkspaceSave::New)
            }
            WorkspaceMenuAction::Manage => {
                self.settings_panel.close();
                self.commands_panel.close();
                self.agents_panel.close();
                self.session_panel
                    .open(crate::session::listing::SessionListing::workspaces());
            }
        }
        self.request_redraw(RedrawReason::Action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn app_with_empty_workspace_store() -> (App, tempfile::TempDir) {
        let store = tempfile::tempdir().unwrap();
        crate::workspace::set_test_workspaces_base_dir(Some(store.path().to_path_buf()));
        (App::new_headless(Config::default()).unwrap(), store)
    }

    #[test]
    fn keyboard_opened_menu_navigates_and_escape_only_closes_it() {
        let (mut app, _store) = app_with_empty_workspace_store();
        app.toggle_workspace_menu(true);
        let menu = app.workspace_menu.as_ref().expect("menu opens");
        assert_eq!(
            menu.highlighted_action(),
            Some(WorkspaceMenuAction::Save),
            "keyboard opening starts on the first entry"
        );
        app.handle_workspace_menu_key(&Key::Named(NamedKey::ArrowDown));
        assert_eq!(
            app.workspace_menu.as_ref().unwrap().highlighted_action(),
            Some(WorkspaceMenuAction::SaveAs)
        );
        // Printable keys are swallowed rather than typed into the terminal.
        app.handle_workspace_menu_key(&Key::Character("x".into()));
        assert!(app.workspace_menu.is_some());
        app.handle_workspace_menu_key(&Key::Named(NamedKey::Escape));
        assert!(app.workspace_menu.is_none());
        assert!(!app.session_panel.is_open());
        crate::workspace::set_test_workspaces_base_dir(None);
    }

    #[test]
    fn enter_on_manage_opens_the_workspace_manager() {
        let (mut app, _store) = app_with_empty_workspace_store();
        app.toggle_workspace_menu(true);
        app.handle_workspace_menu_key(&Key::Named(NamedKey::End));
        app.handle_workspace_menu_key(&Key::Named(NamedKey::Enter));
        assert!(app.workspace_menu.is_none());
        assert!(app.session_panel.is_open() && app.session_panel.is_workspace_mode());
        crate::workspace::set_test_workspaces_base_dir(None);
    }

    #[test]
    fn toggling_again_closes_and_pointer_menus_start_unhighlighted() {
        let (mut app, _store) = app_with_empty_workspace_store();
        app.toggle_workspace_menu(false);
        assert_eq!(app.workspace_menu.as_ref().unwrap().highlighted(), None);
        app.toggle_workspace_menu(false);
        assert!(app.workspace_menu.is_none());
        crate::workspace::set_test_workspaces_base_dir(None);
    }
}
