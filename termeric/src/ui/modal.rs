//! The modal layers and the one order they stack in. The layer on top owns
//! keyboard, pointer, wheel, IME, paste, and assistive input, and is drawn
//! above the others, so what a person sees in front is what they talk to.

/// A surface that takes input from everything beneath it while open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModalLayer {
    Guardrail,
    InfoOverlay,
    DiffReview,
    ContextPicker,
    SproutMenu,
    Cellar,
    DomainPicker,
    UnicodePicker,
    CommandPalette,
    ShortcutsPanel,
    Settings,
    WorkspaceMenu,
    SessionPanel,
    CommandsPanel,
    AgentsPanel,
}

impl ModalLayer {
    /// Every layer, the one on top first.
    pub const INPUT_PRIORITY: [ModalLayer; 15] = [
        ModalLayer::Guardrail,
        ModalLayer::InfoOverlay,
        ModalLayer::DiffReview,
        ModalLayer::ContextPicker,
        ModalLayer::SproutMenu,
        ModalLayer::Cellar,
        ModalLayer::DomainPicker,
        ModalLayer::UnicodePicker,
        ModalLayer::CommandPalette,
        ModalLayer::ShortcutsPanel,
        ModalLayer::Settings,
        // A popup over the panel it may have opened from.
        ModalLayer::WorkspaceMenu,
        ModalLayer::SessionPanel,
        ModalLayer::CommandsPanel,
        ModalLayer::AgentsPanel,
    ];

    /// The side panels, settings, and toolbar menu: the pointer reaches them
    /// through the terminal's own dispatch, after the overlays above them.
    pub fn is_panel(self) -> bool {
        matches!(
            self,
            ModalLayer::Settings
                | ModalLayer::SessionPanel
                | ModalLayer::WorkspaceMenu
                | ModalLayer::CommandsPanel
                | ModalLayer::AgentsPanel
        )
    }

    /// Layers to draw, the one at the bottom first.
    pub fn draw_order() -> impl Iterator<Item = ModalLayer> {
        Self::INPUT_PRIORITY.into_iter().rev()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_layer_has_one_place_and_drawing_is_input_order_reversed() {
        let unique: HashSet<_> = ModalLayer::INPUT_PRIORITY.iter().collect();
        assert_eq!(unique.len(), ModalLayer::INPUT_PRIORITY.len());
        let drawn: Vec<_> = ModalLayer::draw_order().collect();
        assert_eq!(drawn.first(), Some(&ModalLayer::AgentsPanel));
        assert_eq!(drawn.last(), Some(&ModalLayer::Guardrail));
    }
}
