//! The keyboard conventions of each system Termeric runs on, in one place.
//!
//! Which modifier runs Termeric's own commands, how chords are named, and
//! the other rules that differ between Linux and macOS are methods of
//! [`System`]. Input code asks [`System::CURRENT`] rather than testing the
//! target itself, and code that decides with a rule takes the `System` it
//! follows, so each system's tests also check the other's conventions.

use winit::keyboard::ModifiersState;

/// A system whose conventions Termeric follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    #[cfg_attr(
        target_os = "macos",
        allow(
            dead_code,
            reason = "a macOS build follows Linux's conventions only in tests"
        )
    )]
    Linux,
    #[cfg_attr(
        target_os = "linux",
        allow(
            dead_code,
            reason = "a Linux build follows macOS's conventions only in tests"
        )
    )]
    Mac,
}

impl System {
    /// The system this build runs on.
    #[cfg(target_os = "linux")]
    pub const CURRENT: Self = Self::Linux;
    /// The system this build runs on.
    #[cfg(target_os = "macos")]
    pub const CURRENT: Self = Self::Mac;

    /// The modifier of Termeric's own commands in its fields and dialogs,
    /// such as select all, copy, paste, undo, saving Settings, or confirming
    /// a guarded command: Ctrl on Linux, ⌘ on macOS.
    pub const fn command(self) -> ModifiersState {
        match self {
            Self::Linux => ModifiersState::CONTROL,
            Self::Mac => ModifiersState::SUPER,
        }
    }

    /// The modifier that moves and deletes by word in a text field: Ctrl on
    /// Linux, ⌥ on macOS.
    pub const fn word(self) -> ModifiersState {
        match self {
            Self::Linux => ModifiersState::CONTROL,
            Self::Mac => ModifiersState::ALT,
        }
    }

    /// The modifier that moves to the start or end of the line and deletes
    /// to its start in a text field: ⌘ on macOS. Linux uses Home and End.
    pub const fn line(self) -> Option<ModifiersState> {
        match self {
            Self::Linux => None,
            Self::Mac => Some(ModifiersState::SUPER),
        }
    }

    /// How far an editing key (an arrow, Backspace, or Delete) reaches in a
    /// text field with `modifiers` held, Shift aside: `None` when the field
    /// doesn't edit with them.
    pub fn reach(self, modifiers: ModifiersState) -> Option<Reach> {
        if modifiers.is_empty() {
            Some(Reach::Character)
        } else if modifiers == self.word() {
            Some(Reach::Word)
        } else if Some(modifiers) == self.line() {
            Some(Reach::Line)
        } else {
            None
        }
    }

    /// The editing command a character key runs in a field that selects and
    /// undoes, with `modifiers` held: the command modifier with A, C, X, V,
    /// or Z, Shift with Z to redo, and on Linux also Y to redo. Alt must not
    /// be held; any other modifier is ignored.
    pub fn edit_command(self, key: &str, modifiers: ModifiersState) -> Option<EditCommand> {
        if !modifiers.contains(self.command()) || modifiers.alt_key() {
            return None;
        }
        Some(match key.to_lowercase().as_str() {
            "a" => EditCommand::SelectAll,
            "c" => EditCommand::Copy,
            "x" => EditCommand::Cut,
            "v" => EditCommand::Paste,
            "z" if modifiers.shift_key() => EditCommand::Redo,
            "z" => EditCommand::Undo,
            "y" if self == Self::Linux => EditCommand::Redo,
            _ => return None,
        })
    }

    /// Whether the chord of `key` and `modifiers` is one of the editing
    /// commands exactly as `edit_command` names them: the command modifier
    /// alone, or with Shift only to redo. A field that holds the keyboard
    /// takes these before any application shortcut.
    pub fn is_exact_edit_command(self, key: &str, modifiers: ModifiersState) -> bool {
        if modifiers == self.command() {
            self.edit_command(key, modifiers).is_some()
        } else if modifiers == self.command() | ModifiersState::SHIFT {
            key.eq_ignore_ascii_case("z")
        } else {
            false
        }
    }

    /// The label of a chord of `modifiers` and the key named `key` (a
    /// lowercase key name such as `"t"`, `"pageup"`, or `"f1"`): "Ctrl+Shift+T"
    /// on Linux, and "⇧⌘T" on macOS, whose glyphs follow Apple's order
    /// (⌃⌥⇧⌘) with no separators.
    pub fn chord_label(self, modifiers: ModifiersState, key: &str) -> String {
        let modifiers = Modifier::held(modifiers).map(|modifier| modifier.label(self));
        let key = self.key_label(key);
        match self {
            Self::Linux => modifiers
                .chain([key.as_str()])
                .collect::<Vec<_>>()
                .join("+"),
            Self::Mac => modifiers.chain([key.as_str()]).collect(),
        }
    }

    /// How a key is named in a chord's label: "PageUp" or "F1", and on macOS
    /// the glyphs Apple's menus use for the arrows and other named keys.
    fn key_label(self, key: &str) -> String {
        if self == Self::Mac {
            let glyph = match key {
                "up" => Some("↑"),
                "down" => Some("↓"),
                "left" => Some("←"),
                "right" => Some("→"),
                "enter" => Some("↩"),
                "tab" => Some("⇥"),
                "escape" => Some("⎋"),
                "backspace" => Some("⌫"),
                "delete" => Some("⌦"),
                "home" => Some("↖"),
                "end" => Some("↘"),
                "pageup" => Some("⇞"),
                "pagedown" => Some("⇟"),
                _ => None,
            };
            if let Some(glyph) = glyph {
                return glyph.to_string();
            }
        }
        match key {
            "pageup" => "PageUp".to_string(),
            "pagedown" => "PageDown".to_string(),
            "escape" => "Esc".to_string(),
            k if k.starts_with('f') && k.len() <= 3 && k[1..].parse::<u8>().is_ok() => {
                k.to_uppercase()
            }
            k => {
                let mut characters = k.chars();
                match characters.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().chain(characters).collect(),
                }
            }
        }
    }

    /// Whether `filter`, typed in lowercase into the Shortcuts panel, finds
    /// the chord of `modifiers` and `key`. Its label is searched as typed;
    /// on macOS, whose labels are glyphs, every word of the filter may also
    /// begin a name of a held modifier (cmd, command, opt, option, alt, ctrl,
    /// control, shift) or the key's name, so "cmd t" finds ⌘T.
    pub fn chord_matches_filter(self, modifiers: ModifiersState, key: &str, filter: &str) -> bool {
        if self
            .chord_label(modifiers, key)
            .to_lowercase()
            .contains(filter)
        {
            return true;
        }
        if self != Self::Mac {
            return false;
        }
        let names: Vec<&str> = Modifier::held(modifiers)
            .flat_map(|modifier| modifier.mac_names().iter().copied())
            .chain([key])
            .collect();
        let mut words = filter
            .split(|character: char| character.is_whitespace() || character == '+')
            .filter(|word| !word.is_empty())
            .peekable();
        words.peek().is_some() && words.all(|word| names.iter().any(|name| name.starts_with(word)))
    }
}

/// How far an editing key reaches in a text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// One character (grapheme).
    Character,
    /// To the next word boundary.
    Word,
    /// To the start or end of the line.
    Line,
}

/// A command of a field that selects and undoes, such as the agent view's
/// composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
}

/// One modifier key, for naming chords.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Modifier {
    Control,
    Alt,
    Shift,
    Super,
}

impl Modifier {
    /// The modifiers held in `modifiers`, in the order chords name them,
    /// which is also Apple's.
    fn held(modifiers: ModifiersState) -> impl Iterator<Item = Self> {
        [
            (modifiers.control_key(), Self::Control),
            (modifiers.alt_key(), Self::Alt),
            (modifiers.shift_key(), Self::Shift),
            (modifiers.super_key(), Self::Super),
        ]
        .into_iter()
        .filter_map(|(held, modifier)| held.then_some(modifier))
    }

    fn label(self, system: System) -> &'static str {
        match (system, self) {
            (System::Linux, Self::Control) => "Ctrl",
            (System::Linux, Self::Alt) => "Alt",
            (System::Linux, Self::Shift) => "Shift",
            (System::Linux, Self::Super) => "Super",
            (System::Mac, Self::Control) => "⌃",
            (System::Mac, Self::Alt) => "⌥",
            (System::Mac, Self::Shift) => "⇧",
            (System::Mac, Self::Super) => "⌘",
        }
    }

    /// The words a Mac user may search a modifier by.
    fn mac_names(self) -> &'static [&'static str] {
        match self {
            Self::Control => &["ctrl", "control"],
            Self::Alt => &["opt", "option", "alt"],
            Self::Shift => &["shift"],
            Self::Super => &["cmd", "command"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: ModifiersState = ModifiersState::CONTROL;
    const ALT: ModifiersState = ModifiersState::ALT;
    const SHIFT: ModifiersState = ModifiersState::SHIFT;
    const SUPER: ModifiersState = ModifiersState::SUPER;

    #[test]
    fn linux_names_chords_with_words_joined_by_plus() {
        let linux = System::Linux;
        assert_eq!(linux.chord_label(CTRL | SHIFT, "c"), "Ctrl+Shift+C");
        assert_eq!(linux.chord_label(SHIFT, "pageup"), "Shift+PageUp");
        assert_eq!(linux.chord_label(ModifiersState::empty(), "f11"), "F11");
        assert_eq!(linux.chord_label(ALT, "left"), "Alt+Left");
        assert_eq!(
            linux.chord_label(CTRL | ALT | SHIFT | SUPER, "escape"),
            "Ctrl+Alt+Shift+Super+Esc"
        );
        assert_eq!(linux.chord_label(CTRL, "enter"), "Ctrl+Enter");
    }

    #[test]
    fn macos_names_chords_with_glyphs_in_apples_order() {
        let mac = System::Mac;
        assert_eq!(mac.chord_label(SUPER | SHIFT, "p"), "⇧⌘P");
        assert_eq!(mac.chord_label(CTRL | ALT | SHIFT | SUPER, "t"), "⌃⌥⇧⌘T");
        assert_eq!(mac.chord_label(SUPER, "}"), "⌘}");
        assert_eq!(mac.chord_label(SUPER, "enter"), "⌘↩");
        assert_eq!(mac.chord_label(ALT | SUPER, "left"), "⌥⌘←");
        assert_eq!(mac.chord_label(SHIFT, "pagedown"), "⇧⇟");
        assert_eq!(mac.chord_label(SUPER, "home"), "⌘↖");
        assert_eq!(mac.chord_label(SUPER, "backspace"), "⌘⌫");
        assert_eq!(mac.chord_label(CTRL, "s"), "⌃S");
        assert_eq!(mac.chord_label(CTRL | SUPER, "f"), "⌃⌘F");
        assert_eq!(mac.chord_label(ModifiersState::empty(), "f11"), "F11");
        assert_eq!(mac.chord_label(SUPER, "space"), "⌘Space");
    }

    #[test]
    fn the_command_modifier_is_ctrl_on_linux_and_command_on_macos() {
        assert_eq!(System::Linux.command(), CTRL);
        assert_eq!(System::Mac.command(), SUPER);
    }

    #[test]
    fn fields_reach_by_word_with_ctrl_on_linux_and_option_on_macos() {
        use Reach::{Character, Line, Word};
        let none = ModifiersState::empty();
        assert_eq!(System::Linux.reach(none), Some(Character));
        assert_eq!(System::Linux.reach(CTRL), Some(Word));
        assert_eq!(System::Linux.reach(ALT), None);
        assert_eq!(System::Linux.reach(SUPER), None);
        assert_eq!(System::Mac.reach(none), Some(Character));
        assert_eq!(System::Mac.reach(ALT), Some(Word));
        assert_eq!(System::Mac.reach(SUPER), Some(Line));
        assert_eq!(System::Mac.reach(CTRL), None);
        assert_eq!(System::Mac.reach(ALT | SUPER), None);
        assert_eq!(System::Linux.line(), None);
    }

    #[test]
    fn editing_commands_follow_the_command_modifier() {
        use EditCommand::{Copy, Cut, Paste, Redo, SelectAll, Undo};
        for (system, command) in [(System::Linux, CTRL), (System::Mac, SUPER)] {
            for (key, edit) in [
                ("a", SelectAll),
                ("C", Copy),
                ("x", Cut),
                ("v", Paste),
                ("z", Undo),
            ] {
                assert_eq!(system.edit_command(key, command), Some(edit), "{key}");
                assert!(system.is_exact_edit_command(key, command), "{key}");
                assert_eq!(system.edit_command(key, command | ALT), None);
            }
            assert_eq!(system.edit_command("z", command | SHIFT), Some(Redo));
            assert!(system.is_exact_edit_command("Z", command | SHIFT));
            assert!(!system.is_exact_edit_command("A", command | SHIFT));
            assert_eq!(system.edit_command("k", command), None);
            assert_eq!(system.edit_command("a", SHIFT), None);
        }
        assert_eq!(System::Linux.edit_command("y", CTRL), Some(Redo));
        assert_eq!(System::Mac.edit_command("y", SUPER), None);
        assert_eq!(System::Mac.edit_command("c", CTRL), None);
        assert!(!System::Linux.is_exact_edit_command("y", CTRL | SHIFT));
    }

    #[test]
    fn a_linux_filter_searches_the_label() {
        let linux = System::Linux;
        assert!(linux.chord_matches_filter(CTRL | SHIFT, "t", "ctrl+sh"));
        assert!(linux.chord_matches_filter(CTRL | SHIFT, "t", "shift+t"));
        assert!(!linux.chord_matches_filter(CTRL | SHIFT, "t", "cmd"));
        assert!(!linux.chord_matches_filter(CTRL, "t", "ctrl t"));
    }

    #[test]
    fn a_macos_filter_also_finds_modifiers_and_keys_by_name() {
        let mac = System::Mac;
        assert!(mac.chord_matches_filter(SUPER | SHIFT, "p", "⇧⌘"));
        for filter in ["cmd", "command", "com", "cmd p", "shift+cmd+p", "p shift"] {
            assert!(
                mac.chord_matches_filter(SUPER | SHIFT, "p", filter),
                "{filter}"
            );
        }
        for filter in ["opt", "option", "alt", "ctrl", "control", "cmd t", "cmd  w"] {
            assert!(
                !mac.chord_matches_filter(SUPER | SHIFT, "p", filter),
                "{filter}"
            );
        }
        assert!(mac.chord_matches_filter(ALT | SUPER, "left", "option left"));
        assert!(mac.chord_matches_filter(CTRL | SUPER, "f", "control"));
        assert!(!mac.chord_matches_filter(SUPER, "t", " "));
    }
}
