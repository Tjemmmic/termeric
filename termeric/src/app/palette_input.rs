//! Input for the palette-style modals: the live-domain picker, the Unicode
//! picker, and the command palette. They share the palette's drawing and
//! interaction, so one path handles keys, clicks, the wheel, and query
//! edits for whichever is open; only what a chosen entry means differs.

use super::text_field_keys::FieldKey;
use super::*;
use crate::ui::command_palette::{PaletteLayout, PaletteResult};
use crate::ui::domain_picker::DomainPickerCommand;
use crate::ui::unicode_picker::UnicodePickerSelection;
use winit::keyboard::{Key, NamedKey};

/// A palette-style modal, in the order they own input when several are open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaletteModal {
    Domains,
    Unicode,
    Commands,
}

/// What choosing an entry in a palette-style modal asks for.
pub(super) enum PaletteChoice {
    Domain(DomainPickerCommand),
    Unicode(UnicodePickerSelection),
    Command(PaletteResult),
}

/// The open one of the palette-style modals, in the order they own input.
/// Takes the three by field so callers can hold other borrows of the app.
pub(super) fn open_palette_owner<'a>(
    domains: &'a mut crate::ui::domain_picker::DomainPicker,
    unicode: &'a mut crate::ui::unicode_picker::UnicodePicker,
    commands: &'a mut crate::ui::command_palette::CommandPalette,
) -> Option<&'a mut dyn PaletteOwner> {
    if domains.is_active() {
        Some(domains)
    } else if unicode.is_active() {
        Some(unicode)
    } else if commands.is_active() {
        Some(commands)
    } else {
        None
    }
}

impl App {
    /// The open palette-style modal that owns input, if any.
    pub(super) fn open_palette_modal(&self) -> Option<PaletteModal> {
        if self.domain_picker.is_active() {
            Some(PaletteModal::Domains)
        } else if self.unicode_picker.is_active() {
            Some(PaletteModal::Unicode)
        } else if self.command_palette.is_active() {
            Some(PaletteModal::Commands)
        } else {
            None
        }
    }

    pub(super) fn palette_owner(&self, modal: PaletteModal) -> &dyn PaletteOwner {
        match modal {
            PaletteModal::Domains => &self.domain_picker,
            PaletteModal::Unicode => &self.unicode_picker,
            PaletteModal::Commands => &self.command_palette,
        }
    }

    pub(super) fn palette_owner_mut(&mut self, modal: PaletteModal) -> &mut dyn PaletteOwner {
        match modal {
            PaletteModal::Domains => &mut self.domain_picker,
            PaletteModal::Unicode => &mut self.unicode_picker,
            PaletteModal::Commands => &mut self.command_palette,
        }
    }

    /// Escape: a staged picker steps back; the others close.
    fn back_out_of_palette_modal(&mut self, modal: PaletteModal) {
        match modal {
            PaletteModal::Domains => self.back_or_close_domain_picker(),
            PaletteModal::Unicode => self.close_unicode_picker(),
            PaletteModal::Commands => self.command_palette.close(),
        }
    }

    fn close_palette_modal(&mut self, modal: PaletteModal) {
        match modal {
            PaletteModal::Domains => self.close_domain_picker(),
            PaletteModal::Unicode => self.close_unicode_picker(),
            PaletteModal::Commands => self.command_palette.close(),
        }
    }

    /// Choose the highlighted entry.
    fn activate_palette_modal(&mut self, modal: PaletteModal) -> Option<PaletteChoice> {
        match modal {
            PaletteModal::Domains => self.domain_picker.activate().map(PaletteChoice::Domain),
            PaletteModal::Unicode => self.unicode_picker.activate().map(PaletteChoice::Unicode),
            PaletteModal::Commands => Some(PaletteChoice::Command(self.command_palette.execute())),
        }
    }

    fn click_palette_modal(
        &mut self,
        modal: PaletteModal,
        x: f32,
        y: f32,
        layout: &PaletteLayout,
    ) -> Option<PaletteChoice> {
        match modal {
            PaletteModal::Domains => self
                .domain_picker
                .handle_click(x, y, layout)
                .map(PaletteChoice::Domain),
            PaletteModal::Unicode => self
                .unicode_picker
                .handle_click(x, y, layout)
                .map(PaletteChoice::Unicode),
            PaletteModal::Commands => self
                .command_palette
                .handle_click(x, y, layout)
                .map(PaletteChoice::Command),
        }
    }

    /// Choose an entry by its stable identifier, as assistive technology does.
    pub(super) fn execute_palette_modal_command(
        &mut self,
        modal: PaletteModal,
        command: &str,
    ) -> Option<PaletteChoice> {
        match modal {
            PaletteModal::Domains => self
                .domain_picker
                .execute_command(command)
                .map(PaletteChoice::Domain),
            PaletteModal::Unicode => self
                .unicode_picker
                .execute_command(command)
                .map(PaletteChoice::Unicode),
            PaletteModal::Commands => self
                .command_palette
                .execute_command(command)
                .map(PaletteChoice::Command),
        }
    }

    /// Act on a chosen entry, then let go of a Unicode target whose picker
    /// closed without one.
    pub(super) fn finish_palette_input(&mut self, choice: Option<PaletteChoice>) {
        match choice {
            Some(PaletteChoice::Domain(command)) => self.handle_domain_picker_command(command),
            Some(PaletteChoice::Unicode(selection)) => {
                self.apply_unicode_picker_selection(selection);
            }
            Some(PaletteChoice::Command(result)) => self.handle_palette_result(result),
            None => {}
        }
        if !self.unicode_picker.is_active() {
            self.pending_unicode_picker = None;
        }
    }

    /// True means the key belongs to a palette, even when its busy stage is
    /// read-only. Never let editing keys fall through to a terminal behind it.
    pub(super) fn handle_palette_edit_key(&mut self, key: &Key) -> bool {
        let Some(modal) = self.open_palette_modal() else {
            return false;
        };
        let edit = match self.text_edit_for_key(key) {
            Some(FieldKey::Edit(edit)) => edit,
            Some(FieldKey::Consume) => return true,
            None => return false,
        };
        self.palette_owner_mut(modal).edit_query(edit);
        self.ime.clear_preedit();
        self.request_redraw(RedrawReason::KeyboardInput);
        true
    }

    /// A pressed key in the open palette-style modal, after its editing keys.
    /// Returns false when none is open.
    pub(super) fn handle_palette_modal_key(&mut self, key: &Key) -> bool {
        let Some(modal) = self.open_palette_modal() else {
            return false;
        };
        let typing = !self.modifiers.control_key() && !self.modifiers.alt_key();
        let choice = match key {
            Key::Named(NamedKey::Escape) => {
                self.back_out_of_palette_modal(modal);
                None
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.palette_owner_mut(modal).palette_mut().select_prev();
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.palette_owner_mut(modal).palette_mut().select_next();
                None
            }
            Key::Named(NamedKey::Enter) => self.activate_palette_modal(modal),
            Key::Named(NamedKey::Space) => {
                self.palette_owner_mut(modal).insert_query_text(" ");
                None
            }
            // Ctrl+Shift+P toggles the palette, so it closes any of them.
            Key::Character(text)
                if self.modifiers.control_key()
                    && self.modifiers.shift_key()
                    && text.eq_ignore_ascii_case("p") =>
            {
                self.close_palette_modal(modal);
                None
            }
            Key::Character(text) if typing => {
                self.palette_owner_mut(modal).insert_query_text(text);
                None
            }
            _ => None,
        };
        self.finish_palette_input(choice);
        self.request_redraw(RedrawReason::KeyboardInput);
        true
    }

    /// A pointer press while a palette-style modal is open: an entry is
    /// chosen, the query takes the caret, and a press outside closes it.
    /// Returns false when none is open.
    pub(super) fn handle_palette_modal_press(&mut self) -> bool {
        let Some(modal) = self.open_palette_modal() else {
            return false;
        };
        self.ime.clear_preedit();
        let choice =
            if let (Some(position), Some(renderer)) = (self.last_cursor_pos, &self.renderer) {
                let (width, height) = renderer.window_size();
                let info = self.palette_owner(modal).build_info();
                let layout = PaletteLayout::calculate(
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                    info.animation,
                    info.entries.len(),
                );
                self.click_palette_modal(modal, position.0 as f32, position.1 as f32, &layout)
            } else {
                self.close_palette_modal(modal);
                None
            };
        self.finish_palette_input(choice);
        self.request_redraw(RedrawReason::PointerInput);
        true
    }

    /// The wheel scrolls the open palette-style modal's list. Returns false
    /// when none is open.
    pub(super) fn handle_palette_modal_wheel(
        &mut self,
        delta: winit::event::MouseScrollDelta,
    ) -> bool {
        let Some(modal) = self.open_palette_modal() else {
            return false;
        };
        let lines = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => y,
            winit::event::MouseScrollDelta::PixelDelta(position) => (position.y / 20.0) as f32,
        };
        let palette = self.palette_owner_mut(modal).palette_mut();
        if lines > 0.0 {
            palette.scroll_up(lines.ceil() as usize);
        } else if lines < 0.0 {
            palette.scroll_down((-lines).ceil() as usize);
        }
        self.request_redraw(RedrawReason::PointerInput);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NamedKey;

    fn open(app: &mut App, modal: PaletteModal) {
        match modal {
            PaletteModal::Commands => app.command_palette.open(),
            PaletteModal::Unicode => app.unicode_picker.open(),
            PaletteModal::Domains => app.domain_picker.open(&app.config.domains),
        }
    }

    #[test]
    fn every_palette_modal_types_whole_text_and_closes_on_ctrl_shift_p() {
        for modal in [
            PaletteModal::Commands,
            PaletteModal::Unicode,
            PaletteModal::Domains,
        ] {
            let mut app = App::new_headless(Config::default()).unwrap();
            open(&mut app, modal);
            assert_eq!(app.open_palette_modal(), Some(modal));
            assert!(app.handle_palette_modal_key(&Key::Character("ab".into())));
            assert!(app.handle_palette_modal_key(&Key::Named(NamedKey::Space)));
            assert_eq!(
                app.palette_owner(modal).build_info().input,
                "ab ",
                "{modal:?}"
            );
            app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
            assert!(app.handle_palette_modal_key(&Key::Character("P".into())));
            assert_eq!(app.open_palette_modal(), None, "{modal:?}");
            assert!(!app.handle_palette_modal_key(&Key::Character("x".into())));
        }
    }

    #[test]
    fn palette_query_keys_edit_only_the_active_model_and_insert_at_its_caret() {
        for kind in ["command", "unicode", "domain"] {
            let mut app = App::new_headless(Config::default()).unwrap();
            let query = "a界e\u{301}👩\u{200d}💻z";
            match kind {
                "command" => {
                    app.command_palette.open();
                    app.command_palette.accessibility_set_input(query);
                }
                "unicode" => {
                    app.unicode_picker.open();
                    app.unicode_picker.set_query(query);
                }
                _ => {
                    app.domain_picker.open(&app.config.domains);
                    app.domain_picker.set_query(query);
                }
            }
            for key in [
                NamedKey::Home,
                NamedKey::ArrowRight,
                NamedKey::Delete,
                NamedKey::ArrowRight,
            ] {
                assert!(app.handle_palette_edit_key(&Key::Named(key)));
            }
            let modal = app.open_palette_modal().unwrap();
            assert!(app.palette_owner_mut(modal).insert_query_text("X"));
            let info = app.palette_owner(modal).build_info();
            assert_eq!(info.input, "ae\u{301}X👩\u{200d}💻z");
            assert_eq!(info.cursor, "ae\u{301}X".len());
            app.handle_ime_event(winit::event::Ime::Commit("Q".into()));
            let info = app.palette_owner(modal).build_info();
            assert_eq!(info.input, "ae\u{301}XQ👩\u{200d}💻z");
            assert_eq!(info.cursor, "ae\u{301}XQ".len());
            assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::End)));
            assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::ArrowLeft)));
            assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::Delete)));
            let info = app.palette_owner(modal).build_info();
            assert!(info.input.ends_with("👩\u{200d}💻"));
            assert!(!app.handle_palette_edit_key(&Key::Named(NamedKey::Enter)));
            assert!(app.redraw_scheduler.has_pending());
            app.command_palette.close();
            app.domain_picker.close();
            app.unicode_picker.close();
            assert!(!app.handle_palette_edit_key(&Key::Named(NamedKey::Home)));
            app.unicode_picker.finish_search_for_test();
        }
    }

    #[test]
    fn busy_domain_picker_consumes_edits_without_mutating_it_or_background_palette() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.command_palette.accessibility_set_input("background");
        app.domain_picker.open(&app.config.domains);
        app.domain_picker.set_query("local");
        // UI-only transition; do not dispatch its discovery/network command.
        assert!(app.domain_picker.activate().is_some());
        assert!(app.domain_picker.is_busy());
        let before = app.domain_picker.build_info();
        for key in [
            NamedKey::Home,
            NamedKey::Delete,
            NamedKey::End,
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::Backspace,
        ] {
            assert!(app.handle_palette_edit_key(&Key::Named(key)));
            app.modifiers = ModifiersState::CONTROL;
            assert!(app.handle_palette_edit_key(&Key::Named(key)));
            app.modifiers = ModifiersState::empty();
        }
        assert_eq!(app.domain_picker.build_info().input, before.input);
        assert_eq!(app.domain_picker.build_info().cursor, before.cursor);
        assert_eq!(app.command_palette.build_info().input, "background");
        assert_eq!(app.command_palette.build_info().cursor, "background".len());
    }

    #[test]
    fn unsupported_modified_edits_are_consumed_without_mutation() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.command_palette.accessibility_set_input("two words");
        for modifier in [
            ModifiersState::ALT,
            ModifiersState::SHIFT,
            ModifiersState::SUPER,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
            ModifiersState::CONTROL | ModifiersState::ALT,
        ] {
            app.modifiers = modifier;
            for key in [
                NamedKey::ArrowLeft,
                NamedKey::ArrowRight,
                NamedKey::Home,
                NamedKey::End,
                NamedKey::Backspace,
                NamedKey::Delete,
            ] {
                assert!(app.handle_palette_edit_key(&Key::Named(key)));
                assert_eq!(app.command_palette.build_info().input, "two words");
                assert_eq!(app.command_palette.build_info().cursor, "two words".len());
            }
        }
        app.modifiers = ModifiersState::CONTROL;
        app.alt_graph_active = true;
        assert!(app.handle_palette_edit_key(&Key::Named(NamedKey::Backspace)));
        assert_eq!(app.command_palette.build_info().input, "two words");
        app.command_palette.close();
        assert!(!app.handle_palette_edit_key(&Key::Named(NamedKey::Backspace)));
    }

    #[test]
    fn palette_word_edits_preserve_graphemes_and_clear_preedit() {
        for kind in ["command", "unicode", "domain"] {
            let mut app = App::new_headless(Config::default()).unwrap();
            let query = "one  e\u{301} 👩\u{200d}💻 tail";
            match kind {
                "command" => {
                    app.command_palette.open();
                    app.command_palette.accessibility_set_input(query);
                }
                "unicode" => {
                    app.unicode_picker.open();
                    app.unicode_picker.set_query(query);
                }
                _ => {
                    app.domain_picker.open(&app.config.domains);
                    app.domain_picker.set_query(query);
                }
            }
            app.modifiers = ModifiersState::CONTROL;
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            for (key, expected, cursor) in [
                (
                    NamedKey::ArrowLeft,
                    query,
                    "one  e\u{301} 👩\u{200d}💻 ".len(),
                ),
                (
                    NamedKey::Backspace,
                    "one  e\u{301} tail",
                    "one  e\u{301} ".len(),
                ),
                (NamedKey::Delete, "one  e\u{301} ", "one  e\u{301} ".len()),
                (NamedKey::ArrowLeft, "one  e\u{301} ", 5),
                (NamedKey::Backspace, "e\u{301} ", 0),
                (NamedKey::ArrowRight, "e\u{301} ", "e\u{301}".len()),
                (NamedKey::Home, "e\u{301} ", "e\u{301}".len()),
                (NamedKey::End, "e\u{301} ", "e\u{301}".len()),
            ] {
                assert!(app.handle_palette_edit_key(&Key::Named(key)));
                let info = app
                    .palette_owner(app.open_palette_modal().unwrap())
                    .build_info();
                assert_eq!(info.input, expected, "{kind}: {key:?}");
                assert_eq!(info.cursor, cursor, "{kind}: {key:?}");
                assert!(app.ime.preedit().is_none());
            }
            app.unicode_picker.finish_search_for_test();
        }
    }
}
