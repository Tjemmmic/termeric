//! Which modal layer owns input. Every consumer — keys, pointer, wheel,
//! hover, IME, paste, and assistive technology — asks [`App::top_modal_layer`]
//! instead of keeping its own order, so they cannot disagree.

use super::*;
pub(super) use crate::ui::modal::ModalLayer;

impl App {
    /// Whether `layer` is open and taking input; a layer closing with an
    /// animation keeps input until its animation ends.
    pub(super) fn modal_layer_open(&self, layer: ModalLayer) -> bool {
        match layer {
            ModalLayer::Guardrail => self.guardrail.is_active(),
            ModalLayer::Dialog => self.dialog.is_active(),
            ModalLayer::DiffReview => self.diff_review.is_active(),
            ModalLayer::ContextPicker => self.context_picker.is_active(),
            ModalLayer::SproutMenu => self
                .harvest
                .as_ref()
                .is_some_and(|harvest| harvest.sprout_menu.is_some()),
            ModalLayer::Cellar => self
                .harvest
                .as_ref()
                .is_some_and(|harvest| harvest.cellar.is_expanded()),
            ModalLayer::DomainPicker => self.domain_picker.is_active(),
            ModalLayer::UnicodePicker => self.unicode_picker.is_active(),
            ModalLayer::CommandPalette => self.command_palette.is_active(),
            ModalLayer::ShortcutsPanel => self.shortcuts_panel.is_active(),
            ModalLayer::Settings => self.settings_panel.is_active(),
            ModalLayer::WorkspacePanel => self.workspace_panel.is_open(),
            ModalLayer::WorkspaceMenu => self.workspace_menu.is_some(),
            ModalLayer::CommandsPanel => self.commands_panel.is_open(),
            ModalLayer::AgentsPanel => self.agents_panel.is_open(),
        }
    }

    /// The open modal layer on top, which owns input.
    pub(super) fn top_modal_layer(&self) -> Option<ModalLayer> {
        ModalLayer::INPUT_PRIORITY
            .into_iter()
            .find(|layer| self.modal_layer_open(*layer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_highest_open_layer_owns_input_and_panels_yield_to_overlays() {
        let mut app = App::new_headless(Config::default()).unwrap();
        assert_eq!(app.top_modal_layer(), None);
        assert!(!app.pointer_panel_owns_input());

        app.workspace_panel.set_test_entries(Vec::new(), true);
        assert_eq!(app.top_modal_layer(), Some(ModalLayer::WorkspacePanel));
        assert!(app.pointer_panel_owns_input());

        app.workspace_menu = Some(crate::ui::popup_menu::PopupMenu::new(
            crate::ui::workspace_menu::workspace_menu_rows(&[], None, false),
            false,
        ));
        assert_eq!(app.top_modal_layer(), Some(ModalLayer::WorkspaceMenu));

        app.command_palette.open();
        assert_eq!(app.top_modal_layer(), Some(ModalLayer::CommandPalette));
        assert!(!app.pointer_panel_owns_input());
        assert!(app.modal_layer_open(ModalLayer::WorkspacePanel));

        app.command_palette.close();
        app.workspace_menu = None;
        assert_eq!(app.top_modal_layer(), Some(ModalLayer::WorkspacePanel));
    }
}
