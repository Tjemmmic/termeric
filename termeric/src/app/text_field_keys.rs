//! The editing keys every single-line text field shares, and the keys that
//! step focus through a panel's controls.

use super::*;
use crate::input::platform::{Reach, System};
use crate::ui::focus_step::FocusStep;
use crate::ui::text_input::TextInputEdit;
use winit::keyboard::{Key, NamedKey};

/// What a single-line field does with one of the shared editing keys.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum FieldKey {
    Edit(TextInputEdit),
    /// An editing key under modifiers no field supports (selection, AltGr).
    /// The field swallows it, so it never becomes a plain edit or reaches a
    /// terminal behind the field.
    Consume,
}

impl App {
    /// Map the shared editing keys for the current modifiers. `None` means the
    /// key is not an editing key and the field handles it itself.
    pub(super) fn text_edit_for_key(&self, key: &Key) -> Option<FieldKey> {
        text_edit_for_key(key, self.modifiers, self.alt_graph_active, System::CURRENT)
    }
}

/// Whether a character key (or Space) types its text under the held
/// modifiers: alone, with Shift, or through AltGr. Ctrl, Alt, or Super make
/// it a chord instead. Every text field and copy mode share this one rule.
pub(super) fn types_text(modifiers: ModifiersState, alt_graph_active: bool) -> bool {
    alt_graph_active
        || !modifiers
            .intersects(ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SUPER)
}

/// Tab and Shift+Tab, the vertical arrows, Home, and End step keyboard focus
/// through a panel's controls.
pub(super) fn focus_step_for_key(key: &Key, modifiers: ModifiersState) -> Option<FocusStep> {
    let plain = modifiers.is_empty();
    match key {
        Key::Named(NamedKey::Tab) if modifiers == ModifiersState::SHIFT => {
            Some(FocusStep::Previous)
        }
        Key::Named(NamedKey::Tab | NamedKey::ArrowDown) if plain => Some(FocusStep::Next),
        Key::Named(NamedKey::ArrowUp) if plain => Some(FocusStep::Previous),
        Key::Named(NamedKey::Home) if plain => Some(FocusStep::First),
        Key::Named(NamedKey::End) if plain => Some(FocusStep::Last),
        _ => None,
    }
}

/// Plain arrows, Home, End, Backspace, and Delete edit by grapheme. The
/// system's word modifier (Ctrl, or ⌥ on macOS) turns the horizontal arrows
/// and deletions into word edits, and on macOS ⌘ turns the arrows into Home
/// and End and Backspace into deleting to the line's start.
fn text_edit_for_key(
    key: &Key,
    modifiers: ModifiersState,
    alt_graph_active: bool,
    system: System,
) -> Option<FieldKey> {
    let edit = match key {
        Key::Named(NamedKey::ArrowLeft) => TextInputEdit::Left,
        Key::Named(NamedKey::ArrowRight) => TextInputEdit::Right,
        Key::Named(NamedKey::Home) => TextInputEdit::Home,
        Key::Named(NamedKey::End) => TextInputEdit::End,
        Key::Named(NamedKey::Backspace) => TextInputEdit::Backspace,
        Key::Named(NamedKey::Delete) => TextInputEdit::Delete,
        _ => return None,
    };
    if alt_graph_active {
        return Some(FieldKey::Consume);
    }
    let Some(reach) = system.reach(modifiers) else {
        return Some(FieldKey::Consume);
    };
    Some(match (reach, edit) {
        (Reach::Character, edit) => FieldKey::Edit(edit),
        (Reach::Word, TextInputEdit::Left) => FieldKey::Edit(TextInputEdit::WordLeft),
        (Reach::Word, TextInputEdit::Right) => FieldKey::Edit(TextInputEdit::WordRight),
        (Reach::Word, TextInputEdit::Backspace) => FieldKey::Edit(TextInputEdit::BackspaceWord),
        (Reach::Word, TextInputEdit::Delete) => FieldKey::Edit(TextInputEdit::DeleteWord),
        (Reach::Line, TextInputEdit::Left) => FieldKey::Edit(TextInputEdit::Home),
        (Reach::Line, TextInputEdit::Right) => FieldKey::Edit(TextInputEdit::End),
        (Reach::Line, TextInputEdit::Backspace) => FieldKey::Edit(TextInputEdit::BackspaceLine),
        _ => FieldKey::Consume,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use FieldKey::{Consume, Edit};
    use TextInputEdit::*;

    fn map(key: NamedKey, modifiers: ModifiersState, alt_graph: bool) -> Option<FieldKey> {
        text_edit_for_key(&Key::Named(key), modifiers, alt_graph, System::Linux)
    }

    fn map_on_mac(key: NamedKey, modifiers: ModifiersState) -> Option<FieldKey> {
        text_edit_for_key(&Key::Named(key), modifiers, false, System::Mac)
    }

    #[test]
    fn plain_keys_edit_by_grapheme_and_ctrl_edits_by_word() {
        let plain = ModifiersState::empty();
        let ctrl = ModifiersState::CONTROL;
        for (key, plain_edit, ctrl_edit) in [
            (NamedKey::ArrowLeft, Edit(Left), Edit(WordLeft)),
            (NamedKey::ArrowRight, Edit(Right), Edit(WordRight)),
            (NamedKey::Home, Edit(Home), Consume),
            (NamedKey::End, Edit(End), Consume),
            (NamedKey::Backspace, Edit(Backspace), Edit(BackspaceWord)),
            (NamedKey::Delete, Edit(Delete), Edit(DeleteWord)),
        ] {
            assert_eq!(map(key, plain, false), Some(plain_edit));
            assert_eq!(map(key, ctrl, false), Some(ctrl_edit));
        }
    }

    #[test]
    fn mac_fields_edit_by_word_with_option_and_by_line_with_command() {
        let plain = ModifiersState::empty();
        let option = ModifiersState::ALT;
        let command = ModifiersState::SUPER;
        for (key, plain_edit, option_edit, command_edit) in [
            (NamedKey::ArrowLeft, Edit(Left), Edit(WordLeft), Edit(Home)),
            (
                NamedKey::ArrowRight,
                Edit(Right),
                Edit(WordRight),
                Edit(End),
            ),
            (NamedKey::Home, Edit(Home), Consume, Consume),
            (NamedKey::End, Edit(End), Consume, Consume),
            (
                NamedKey::Backspace,
                Edit(Backspace),
                Edit(BackspaceWord),
                Edit(BackspaceLine),
            ),
            (NamedKey::Delete, Edit(Delete), Edit(DeleteWord), Consume),
        ] {
            assert_eq!(map_on_mac(key, plain), Some(plain_edit), "{key:?}");
            assert_eq!(map_on_mac(key, option), Some(option_edit), "{key:?}");
            assert_eq!(map_on_mac(key, command), Some(command_edit), "{key:?}");
        }
        // Ctrl is the terminal's on a Mac, so a field swallows it.
        for modifiers in [
            ModifiersState::CONTROL,
            ModifiersState::SHIFT,
            option | command,
        ] {
            assert_eq!(map_on_mac(NamedKey::ArrowLeft, modifiers), Some(Consume));
        }
        // On Linux the same keys keep their meanings: Alt and Super are no
        // field's.
        assert_eq!(map(NamedKey::ArrowLeft, option, false), Some(Consume));
        assert_eq!(map(NamedKey::Backspace, command, false), Some(Consume));
    }

    #[test]
    fn unsupported_modifiers_and_alt_graph_are_consumed_and_other_keys_pass() {
        for modifiers in [
            ModifiersState::SHIFT,
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        ] {
            assert_eq!(map(NamedKey::Backspace, modifiers, false), Some(Consume));
        }
        assert_eq!(
            map(NamedKey::ArrowLeft, ModifiersState::empty(), true),
            Some(Consume)
        );
        for key in [
            Key::Named(NamedKey::Escape),
            Key::Named(NamedKey::ArrowUp),
            Key::Character("a".into()),
        ] {
            assert_eq!(
                text_edit_for_key(&key, ModifiersState::empty(), false, System::Linux),
                None
            );
        }
    }

    #[test]
    fn text_types_alone_with_shift_or_through_altgr_but_not_in_a_chord() {
        assert!(types_text(ModifiersState::empty(), false));
        assert!(types_text(ModifiersState::SHIFT, false));
        assert!(types_text(
            ModifiersState::CONTROL | ModifiersState::ALT,
            true
        ));
        for modifiers in [
            ModifiersState::CONTROL,
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::SUPER | ModifiersState::SHIFT,
        ] {
            assert!(!types_text(modifiers, false), "{modifiers:?}");
        }
    }
}
