//! Modals drawn and navigated as the command palette: the palette itself,
//! and the pickers that lend theirs. Navigation, drawing, and query editing
//! are the palette's; the owner decides when its query may change and
//! learns when it did.

use super::{CommandPalette, PaletteInfo};
use crate::ui::text_input::TextInputEdit;

pub trait PaletteOwner {
    fn palette(&self) -> &CommandPalette;

    fn palette_mut(&mut self) -> &mut CommandPalette;

    /// Whether the query may change now. A picker between stages, or one
    /// showing a fixed list, keeps its query.
    fn query_editable(&self) -> bool {
        true
    }

    /// Called after the query changed.
    fn query_changed(&mut self) {}

    /// What to draw: the palette's info plus anything the owner adds.
    fn build_info(&self) -> PaletteInfo {
        self.palette().build_info()
    }

    fn is_active(&self) -> bool {
        self.palette().is_active()
    }

    /// Insert typed, committed, or pasted text at the query's caret, whole
    /// or not at all.
    fn insert_query_text(&mut self, text: &str) -> bool {
        if !self.is_active()
            || !self.query_editable()
            || !self.palette_mut().insert_query_text(text)
        {
            return false;
        }
        self.query_changed();
        true
    }

    /// Apply a caret move or deletion. Returns whether the query changed.
    fn edit_query(&mut self, edit: TextInputEdit) -> bool {
        if !self.is_active() || !self.query_editable() || !self.palette_mut().edit_input(edit) {
            return false;
        }
        self.query_changed();
        true
    }

    /// Replace the query, as assistive technology does.
    fn set_query(&mut self, value: &str) -> bool {
        if !self.query_editable() || !self.palette_mut().accessibility_set_input(value) {
            return false;
        }
        self.query_changed();
        true
    }
}

impl PaletteOwner for CommandPalette {
    fn palette(&self) -> &CommandPalette {
        self
    }

    fn palette_mut(&mut self) -> &mut CommandPalette {
        self
    }
}
