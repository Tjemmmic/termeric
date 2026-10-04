//! The editing keys every single-line text field shares.

use super::*;
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
        text_edit_for_key(key, self.modifiers, self.alt_graph_active)
    }

    /// Whether a character key types its character under the held
    /// modifiers: alone, with Shift, or through AltGr. Ctrl, Alt, or Super
    /// make it a chord instead.
    pub(super) fn key_types_text(&self) -> bool {
        self.alt_graph_active
            || !self
                .modifiers
                .intersects(ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SUPER)
    }
}

/// Plain arrows, Home, End, Backspace, and Delete edit by grapheme; Ctrl turns
/// the arrows and deletions into word edits.
fn text_edit_for_key(
    key: &Key,
    modifiers: ModifiersState,
    alt_graph_active: bool,
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
    if modifiers.is_empty() {
        return Some(FieldKey::Edit(edit));
    }
    if modifiers != ModifiersState::CONTROL {
        return Some(FieldKey::Consume);
    }
    Some(match edit {
        TextInputEdit::Left => FieldKey::Edit(TextInputEdit::WordLeft),
        TextInputEdit::Right => FieldKey::Edit(TextInputEdit::WordRight),
        TextInputEdit::Backspace => FieldKey::Edit(TextInputEdit::BackspaceWord),
        TextInputEdit::Delete => FieldKey::Edit(TextInputEdit::DeleteWord),
        _ => FieldKey::Consume,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use FieldKey::{Consume, Edit};
    use TextInputEdit::*;

    fn map(key: NamedKey, modifiers: ModifiersState, alt_graph: bool) -> Option<FieldKey> {
        text_edit_for_key(&Key::Named(key), modifiers, alt_graph)
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
                text_edit_for_key(&key, ModifiersState::empty(), false),
                None
            );
        }
    }
}
