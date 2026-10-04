//! Unified keyboard shortcut system
//!
//! Single registry for all bindable actions with defaults, user overrides,
//! and O(1) key-chord lookup. Also retains the plugin/custom `KeyBindingManager`
//! for dynamic bindings (SendText, RunCommand, PluginCommand).

use crate::input::Action;
use crate::panes::PaneDir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use winit::keyboard::{Key, ModifiersState, NamedKey};

// ── BindableAction ──────────────────────────────────────────────────────────

/// Declares every bindable action once: its variant, config id, Settings
/// label, category, default chord, and the app `Action` it runs.
macro_rules! bindable_actions {
    ($(
        $variant:ident => $id:literal, $label:literal, $category:ident, $binding:expr, $action:expr;
    )*) => {
        /// Every operation that can be bound to a keyboard shortcut.
        ///
        /// Parameterized actions are expanded to concrete variants so each has
        /// exactly one binding slot (e.g. `SwitchToTab1` through `SwitchToTab9`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum BindableAction {
            $($variant,)*
        }

        impl BindableAction {
            /// Every variant, in display order.
            pub fn all() -> &'static [BindableAction] {
                &[$(Self::$variant,)*]
            }

            /// Metadata: string id, display label, category, and default binding.
            pub fn meta(self) -> ActionMeta {
                match self {
                    $(Self::$variant => ActionMeta {
                        id: $id,
                        label: $label,
                        category: ShortcutCategory::$category,
                        default_binding: $binding,
                    },)*
                }
            }

            /// Convert to the internal `Action` enum used by app dispatch.
            pub fn to_action(self) -> Action {
                match self {
                    $(Self::$variant => $action,)*
                }
            }
        }
    };
}

const NO_MODIFIERS: ModifiersState = ModifiersState::empty();
const CTRL: ModifiersState = ModifiersState::CONTROL;
const ALT: ModifiersState = ModifiersState::ALT;
const SHIFT: ModifiersState = ModifiersState::SHIFT;

fn chord(key: &str, modifiers: ModifiersState) -> Option<KeyChord> {
    Some(KeyChord::new(
        key,
        modifiers.control_key(),
        modifiers.alt_key(),
        modifiers.shift_key(),
        modifiers.super_key(),
    ))
}

bindable_actions! {
    // General
    Copy => "copy", "Copy Selection", General, chord("c", CTRL | SHIFT), Action::Copy;
    Paste => "paste", "Paste", General, chord("v", CTRL | SHIFT), Action::Paste;
    ToggleSearch => "toggle_search", "Find", General,
        chord("f", CTRL | SHIFT), Action::ToggleSearch;
    NewAgent => "new_agent", "New Agent", General, chord("i", CTRL | SHIFT), Action::NewAgent;
    ToggleAgentView => "toggle_agent_view", "Agent View", General,
        chord("a", CTRL | SHIFT), Action::ToggleAgentView;
    ToggleCopyMode => "toggle_copy_mode", "Keyboard Copy Mode", Navigation,
        chord("x", CTRL | SHIFT), Action::ToggleCopyMode;
    QuickSelect => "quick_select", "Quick Select Links and Matches", Navigation,
        chord("u", CTRL | SHIFT), Action::QuickSelect;
    OpenUnicodePicker => "open_unicode_picker", "Insert Unicode Character", General,
        None, Action::OpenUnicodePicker;
    ToggleCommandPalette => "toggle_command_palette", "Command Palette", General,
        chord("p", CTRL | SHIFT), Action::ToggleCommandPalette;
    ToggleSettings => "toggle_settings", "Open Settings", General,
        chord(",", CTRL), Action::ToggleSettings;
    ToggleFullscreen => "toggle_fullscreen", "Toggle Fullscreen", General,
        chord("F11", NO_MODIFIERS), Action::ToggleFullscreen;
    ClearSelection => "clear_selection", "Clear Selection", General,
        chord("Escape", NO_MODIFIERS), Action::ClearSelection;

    // Font
    IncreaseFontSize => "increase_font_size", "Increase Font Size", Font,
        chord("=", CTRL), Action::IncreaseFontSize;
    DecreaseFontSize => "decrease_font_size", "Decrease Font Size", Font,
        chord("-", CTRL), Action::DecreaseFontSize;
    ResetFontSize => "reset_font_size", "Reset Font Size", Font,
        chord("0", CTRL), Action::ResetFontSize;

    // Tabs
    NewTab => "new_tab", "New Tab", Tabs, chord("t", CTRL | SHIFT), Action::NewTab;
    CloseTab => "close_tab", "Close Tab", Tabs, chord("w", CTRL | SHIFT), Action::CloseTab;
    NextTab => "next_tab", "Next Tab", Tabs, chord("Tab", CTRL), Action::NextTab;
    PrevTab => "prev_tab", "Previous Tab", Tabs, chord("Tab", CTRL | SHIFT), Action::PrevTab;
    SwitchToTab1 => "switch_to_tab_1", "Switch to Tab 1", Tabs,
        chord("1", CTRL), Action::SwitchToTab(0);
    SwitchToTab2 => "switch_to_tab_2", "Switch to Tab 2", Tabs,
        chord("2", CTRL), Action::SwitchToTab(1);
    SwitchToTab3 => "switch_to_tab_3", "Switch to Tab 3", Tabs,
        chord("3", CTRL), Action::SwitchToTab(2);
    SwitchToTab4 => "switch_to_tab_4", "Switch to Tab 4", Tabs,
        chord("4", CTRL), Action::SwitchToTab(3);
    SwitchToTab5 => "switch_to_tab_5", "Switch to Tab 5", Tabs,
        chord("5", CTRL), Action::SwitchToTab(4);
    SwitchToTab6 => "switch_to_tab_6", "Switch to Tab 6", Tabs,
        chord("6", CTRL), Action::SwitchToTab(5);
    SwitchToTab7 => "switch_to_tab_7", "Switch to Tab 7", Tabs,
        chord("7", CTRL), Action::SwitchToTab(6);
    SwitchToTab8 => "switch_to_tab_8", "Switch to Tab 8", Tabs,
        chord("8", CTRL), Action::SwitchToTab(7);
    SwitchToTab9 => "switch_to_tab_9", "Switch to Tab 9", Tabs,
        chord("9", CTRL), Action::SwitchToTab(8);

    // Panes
    SplitVertical => "split_vertical", "Split Pane Right", Panes,
        chord("right", ALT | SHIFT), Action::SplitVertical;
    SplitHorizontal => "split_horizontal", "Split Pane Down", Panes,
        chord("down", ALT | SHIFT), Action::SplitHorizontal;
    NavigatePaneUp => "navigate_pane_up", "Navigate Pane Up", Panes,
        chord("up", ALT), Action::NavigatePane(PaneDir::Up);
    NavigatePaneDown => "navigate_pane_down", "Navigate Pane Down", Panes,
        chord("down", ALT), Action::NavigatePane(PaneDir::Down);
    NavigatePaneLeft => "navigate_pane_left", "Navigate Pane Left", Panes,
        chord("left", ALT), Action::NavigatePane(PaneDir::Left);
    NavigatePaneRight => "navigate_pane_right", "Navigate Pane Right", Panes,
        chord("right", ALT), Action::NavigatePane(PaneDir::Right);
    ClosePane => "close_pane", "Close Pane", Panes, chord("w", ALT | SHIFT), Action::ClosePane;
    RenameTab => "rename_tab", "Rename Tab", Tabs,
        chord("t", CTRL | ALT | SHIFT), Action::RenameTab;
    MoveTabLeft => "move_tab_left", "Move Tab Left", Tabs, None, Action::MoveTabLeft;
    MoveTabRight => "move_tab_right", "Move Tab Right", Tabs, None, Action::MoveTabRight;
    MoveTabToNextWindow => "move_tab_to_next_window", "Move Tab to Next Window", Tabs,
        None, Action::MoveTabToNextWindow;
    TogglePinTab => "toggle_pin_tab", "Pin or Unpin Tab", Tabs, None, Action::TogglePinTab;
    TogglePaneZoom => "toggle_pane_zoom", "Zoom Pane", Panes, None, Action::TogglePaneZoom;
    TogglePaneReadOnly => "toggle_pane_read_only", "Toggle Pane Read-only", Panes,
        None, Action::TogglePaneReadOnly;
    SwapPaneNext => "swap_pane_next", "Swap Pane with Next", Panes, None, Action::SwapPaneNext;
    BalancePanes => "balance_panes", "Balance Panes", Panes, None, Action::BalancePanes;
    MovePaneToNextTab => "move_pane_to_next_tab", "Move Pane to Next Tab", Panes,
        None, Action::MovePaneToNextTab;
    MovePaneToNextWindow => "move_pane_to_next_window", "Move Pane to Next Window", Panes,
        None, Action::MovePaneToNextWindow;
    LayoutColumns => "layout_columns", "Layout: Columns", Panes, None, Action::LayoutColumns;
    LayoutRows => "layout_rows", "Layout: Rows", Panes, None, Action::LayoutRows;
    LayoutGrid => "layout_grid", "Layout: Grid", Panes, None, Action::LayoutGrid;
    LayoutTall => "layout_tall", "Layout: Tall", Panes, None, Action::LayoutTall;
    LayoutFat => "layout_fat", "Layout: Fat", Panes, None, Action::LayoutFat;
    CyclePaneCwdMode => "cycle_pane_cwd_mode", "Cycle New Pane Directory", Panes,
        None, Action::CyclePaneCwdMode;
    ToggleBroadcast => "toggle_broadcast", "Toggle Broadcast Input", Panes,
        None, Action::ToggleBroadcast;

    // Scrolling
    ScrollPageUp => "scroll_page_up", "Scroll Page Up", Scrolling,
        chord("PageUp", SHIFT), Action::ScrollPageUp;
    ScrollPageDown => "scroll_page_down", "Scroll Page Down", Scrolling,
        chord("PageDown", SHIFT), Action::ScrollPageDown;
    ScrollToTop => "scroll_to_top", "Scroll to Top", Scrolling,
        chord("Home", CTRL | SHIFT), Action::ScrollToTop;
    ScrollToBottom => "scroll_to_bottom", "Scroll to Bottom", Scrolling,
        chord("End", CTRL | SHIFT), Action::ScrollToBottom;

    // Navigation
    PreviousPrompt => "previous_prompt", "Previous Prompt", Navigation,
        chord("Up", CTRL | SHIFT), Action::PreviousPrompt;
    NextPrompt => "next_prompt", "Next Prompt", Navigation,
        chord("Down", CTRL | SHIFT), Action::NextPrompt;
    PreviousFailedCommand =>
        "previous_failed_command", "Jump to Previous Failed Command", Navigation,
        None, Action::PreviousFailedCommand;
    NextFailedCommand => "next_failed_command", "Jump to Next Failed Command", Navigation,
        None, Action::NextFailedCommand;

    // Terminal
    ClearScrollback => "clear_scrollback", "Clear Scrollback", Terminal,
        None, Action::ClearScrollback;
    ResetTerminal => "reset_terminal", "Reset Terminal", Terminal, None, Action::ResetTerminal;
}

impl BindableAction {
    /// Look up a `BindableAction` by its config string id.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::all().iter().find(|a| a.meta().id == id).copied()
    }
}

// ── ShortcutCategory ────────────────────────────────────────────────────────

/// Grouping for the settings UI sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutCategory {
    General,
    Font,
    Tabs,
    Panes,
    Scrolling,
    Navigation,
    Terminal,
}

impl ShortcutCategory {
    /// Display order: everyday actions first, specialist ones last.
    pub const ORDERED: [Self; 7] = [
        Self::General,
        Self::Tabs,
        Self::Panes,
        Self::Navigation,
        Self::Scrolling,
        Self::Font,
        Self::Terminal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Font => "Font",
            Self::Tabs => "Tabs",
            Self::Panes => "Panes",
            Self::Scrolling => "Scrolling",
            Self::Navigation => "Navigation",
            Self::Terminal => "Terminal",
        }
    }
}

// ── ActionMeta ──────────────────────────────────────────────────────────────

/// Static metadata for a `BindableAction`.
pub struct ActionMeta {
    /// String identifier used in config TOML.
    pub id: &'static str,
    /// Human-readable label shown in settings.
    pub label: &'static str,
    /// Category for UI grouping.
    pub category: ShortcutCategory,
    /// Default key chord (None = unset by default).
    pub default_binding: Option<KeyChord>,
}

// ── KeyChord ────────────────────────────────────────────────────────────────

/// A key + modifier combination. Used as both the binding value and the
/// reverse-lookup key in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyChord {
    /// Lowercased key name: "c", "tab", "pageup", "f1", etc.
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default, rename = "super")]
    pub super_key: bool,
}

impl KeyChord {
    /// Construct from parts. Every chord is built here, so recorded,
    /// configured, and pressed keys normalize the same way: lowercase, `+`
    /// as `=`, which share a physical key on most layouts, and no Shift on a
    /// character without case. Shift there is already part of the character
    /// the layout produced (`?`, or `1` on AZERTY), so counting it again
    /// would make the chord depend on the layout; on a letter it is the only
    /// thing telling Ctrl+Shift+C from Ctrl+C.
    pub fn new(key: &str, ctrl: bool, alt: bool, shift: bool, super_key: bool) -> Self {
        let key = if key == "+" { "=" } else { key };
        let shift = shift && !is_caseless_character(key);
        Self {
            key: key.to_lowercase(),
            ctrl,
            alt,
            shift,
            super_key,
        }
    }

    /// Build from a winit key event + modifier state, for recording a
    /// shortcut or looking one up.
    pub fn from_key_event(key: &Key, modifiers: ModifiersState) -> Option<Self> {
        Some(Self::new(
            &key_to_string(key)?,
            modifiers.control_key(),
            modifiers.alt_key(),
            modifiers.shift_key(),
            modifiers.super_key(),
        ))
    }

    /// Human-readable string for settings UI: "Ctrl+Shift+C"
    pub fn display_string(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.super_key {
            parts.push("Super");
        }
        // Capitalize first letter of the key for display
        let key_display = capitalize_key(&self.key);
        parts.push(&key_display);
        parts.join("+")
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_string())
    }
}

/// A single character with no upper and lower case: a digit or a symbol.
fn is_caseless_character(key: &str) -> bool {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(character), None) => character.to_lowercase().eq(character.to_uppercase()),
        _ => false,
    }
}

/// Capitalize a key name for display (e.g. "pageup" → "PageUp", "c" → "C")
fn capitalize_key(key: &str) -> String {
    match key {
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        "escape" => "Esc".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "insert" => "Insert".to_string(),
        "enter" => "Enter".to_string(),
        "space" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        k if k.starts_with('f') && k.len() <= 3 && k[1..].parse::<u8>().is_ok() => k.to_uppercase(),
        k => {
            let mut c = k.chars();
            match c.next() {
                None => String::new(),
                Some(first) => {
                    let upper: String = first.to_uppercase().collect();
                    upper + c.as_str()
                }
            }
        }
    }
}

// ── ShortcutOverride (config persistence) ───────────────────────────────────

/// A user override persisted in config.toml.
/// Only bindings that differ from defaults are saved.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShortcutOverride {
    /// The `BindableAction` string id (e.g. "copy", "new_tab").
    pub action: String,
    /// Key name, or absent/null to explicitly unbind.
    pub key: Option<String>,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default, rename = "super")]
    pub super_key: bool,
}

impl ShortcutOverride {
    /// Convert to a `(BindableAction, Option<KeyChord>)` pair.
    pub fn to_binding(&self) -> Option<(BindableAction, Option<KeyChord>)> {
        let action = BindableAction::from_id(&self.action)?;
        let chord = self
            .key
            .as_ref()
            .map(|key| KeyChord::new(key, self.ctrl, self.alt, self.shift, self.super_key));
        Some((action, chord))
    }
}

// ── ShortcutRegistry ────────────────────────────────────────────────────────

/// Unified shortcut registry — the single source of truth for all key bindings.
///
/// Holds defaults + user overrides. Provides O(1) lookup on the hot path
/// (every keypress) and methods for the settings UI to read/write bindings.
#[derive(Debug)]
pub struct ShortcutRegistry {
    /// action → current chord (None = unbound)
    bindings: HashMap<BindableAction, Option<KeyChord>>,
    /// chord → action (reverse index for O(1) dispatch)
    key_map: HashMap<KeyChord, BindableAction>,
    /// Only user-changed bindings, for minimal config persistence
    user_overrides: HashMap<BindableAction, Option<KeyChord>>,
}

impl ShortcutRegistry {
    /// Create a new registry populated with all default bindings.
    pub fn new() -> Self {
        let mut bindings = HashMap::new();
        let mut key_map = HashMap::new();

        for &action in BindableAction::all() {
            let meta = action.meta();
            if let Some(chord) = &meta.default_binding {
                key_map.insert(chord.clone(), action);
                bindings.insert(action, Some(chord.clone()));
            } else {
                bindings.insert(action, None);
            }
        }

        Self {
            bindings,
            key_map,
            user_overrides: HashMap::new(),
        }
    }

    /// Apply user overrides from config on top of defaults, in order,
    /// exactly as if each had been set in Settings.
    pub fn apply_overrides(&mut self, overrides: &[ShortcutOverride]) {
        for (action, chord) in overrides.iter().filter_map(ShortcutOverride::to_binding) {
            self.set_binding(action, chord);
        }
    }

    /// O(1) lookup — the hot path called on every keypress.
    pub fn check(&self, key: &Key, modifiers: ModifiersState) -> Option<BindableAction> {
        let chord = KeyChord::from_key_event(key, modifiers)?;
        self.key_map.get(&chord).copied()
    }

    /// Get the current binding for an action (for settings UI display).
    pub fn binding_for(&self, action: BindableAction) -> Option<&KeyChord> {
        self.bindings.get(&action).and_then(|opt| opt.as_ref())
    }

    /// Set a binding, handling conflicts automatically.
    /// If `chord` is already bound to a different action, that action is unbound.
    pub fn set_binding(&mut self, action: BindableAction, chord: Option<KeyChord>) {
        // Remove old chord from reverse map
        if let Some(Some(old)) = self.bindings.get(&action) {
            self.key_map.remove(old);
        }

        // Handle conflict
        if let Some(new_chord) = &chord {
            if let Some(&conflicting) = self.key_map.get(new_chord) {
                if conflicting != action {
                    self.bindings.insert(conflicting, None);
                    self.user_overrides.insert(conflicting, None);
                }
            }
            self.key_map.insert(new_chord.clone(), action);
        }

        self.bindings.insert(action, chord.clone());
        self.user_overrides.insert(action, chord);
    }

    /// Check what action a chord is currently bound to (for conflict detection).
    pub fn conflict_for(&self, chord: &KeyChord) -> Option<BindableAction> {
        self.key_map.get(chord).copied()
    }

    /// Produce the minimal set of overrides for config persistence.
    pub fn user_overrides(&self) -> Vec<ShortcutOverride> {
        let mut out = Vec::new();
        for (&action, chord_opt) in &self.user_overrides {
            let meta = action.meta();
            match chord_opt {
                Some(chord) => {
                    out.push(ShortcutOverride {
                        action: meta.id.to_string(),
                        key: Some(chord.key.clone()),
                        ctrl: chord.ctrl,
                        alt: chord.alt,
                        shift: chord.shift,
                        super_key: chord.super_key,
                    });
                }
                None => {
                    // Explicitly unbound
                    out.push(ShortcutOverride {
                        action: meta.id.to_string(),
                        key: None,
                        ctrl: false,
                        alt: false,
                        shift: false,
                        super_key: false,
                    });
                }
            }
        }
        out.sort_by(|a, b| a.action.cmp(&b.action));
        out
    }
}

impl Default for ShortcutRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Plugin / custom bindings ────────────────────────────────────────────────

/// A key binding definition for plugin/custom actions (SendText, RunCommand, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyBinding {
    pub key: String,
    #[serde(default)]
    pub modifiers: KeyModifiers,
    pub action: KeyAction,
}

/// Modifier keys for a binding
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyModifiers {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default, rename = "super")]
    pub super_key: bool,
}

/// Action to perform when a plugin/custom key binding is triggered.
/// These are dynamic/parameterized and cannot be part of BindableAction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum KeyAction {
    SendText { text: String },
    RunCommand { command: String },
    PluginCommand { command_id: String },
}

impl KeyAction {
    pub fn to_action(&self) -> Option<Action> {
        match self {
            KeyAction::PluginCommand { command_id } => {
                Some(Action::PluginCommand(command_id.clone()))
            }
            _ => None,
        }
    }
}

/// Manager for plugin/custom keybindings (SendText, RunCommand, PluginCommand).
#[derive(Debug, Default)]
pub struct KeyBindingManager {
    /// Chord → action, keyed by the same `KeyChord` normalization as the
    /// shortcut registry. The first binding for a chord wins.
    key_map: HashMap<KeyChord, KeyAction>,
}

impl KeyBindingManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(&mut self, bindings: Vec<KeyBinding>) {
        self.key_map.clear();
        for binding in bindings {
            let modifiers = &binding.modifiers;
            let chord = KeyChord::new(
                &binding.key,
                modifiers.ctrl,
                modifiers.alt,
                modifiers.shift,
                modifiers.super_key,
            );
            self.key_map.entry(chord).or_insert(binding.action);
        }
    }

    pub fn check(&self, key: &Key, modifiers: ModifiersState) -> Option<&KeyAction> {
        self.key_map.get(&KeyChord::from_key_event(key, modifiers)?)
    }
}

// ── Key name conversion ─────────────────────────────────────────────────────

/// Convert a winit Key to a string representation
pub fn key_to_string(key: &Key) -> Option<String> {
    match key {
        Key::Named(named) => Some(named_key_to_string(named)),
        Key::Character(c) => Some(c.to_string()),
        _ => None,
    }
}

fn named_key_to_string(key: &NamedKey) -> String {
    match key {
        NamedKey::Enter => "Enter".to_string(),
        NamedKey::Tab => "Tab".to_string(),
        NamedKey::Space => "Space".to_string(),
        NamedKey::Backspace => "Backspace".to_string(),
        NamedKey::Escape => "Escape".to_string(),
        NamedKey::Delete => "Delete".to_string(),
        NamedKey::Insert => "Insert".to_string(),
        NamedKey::Home => "Home".to_string(),
        NamedKey::End => "End".to_string(),
        NamedKey::PageUp => "PageUp".to_string(),
        NamedKey::PageDown => "PageDown".to_string(),
        NamedKey::ArrowUp => "Up".to_string(),
        NamedKey::ArrowDown => "Down".to_string(),
        NamedKey::ArrowLeft => "Left".to_string(),
        NamedKey::ArrowRight => "Right".to_string(),
        NamedKey::F1 => "F1".to_string(),
        NamedKey::F2 => "F2".to_string(),
        NamedKey::F3 => "F3".to_string(),
        NamedKey::F4 => "F4".to_string(),
        NamedKey::F5 => "F5".to_string(),
        NamedKey::F6 => "F6".to_string(),
        NamedKey::F7 => "F7".to_string(),
        NamedKey::F8 => "F8".to_string(),
        NamedKey::F9 => "F9".to_string(),
        NamedKey::F10 => "F10".to_string(),
        NamedKey::F11 => "F11".to_string(),
        NamedKey::F12 => "F12".to_string(),
        _ => format!("{:?}", key),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_on_plus_fires_however_it_was_set() {
        let plus = Key::Character("+".into());
        let ctrl = ModifiersState::CONTROL;

        let mut recorded = ShortcutRegistry::new();
        let chord = KeyChord::from_key_event(&plus, ctrl).unwrap();
        recorded.set_binding(BindableAction::ToggleFullscreen, Some(chord));
        assert_eq!(
            recorded.check(&plus, ctrl),
            Some(BindableAction::ToggleFullscreen)
        );

        let mut configured = ShortcutRegistry::new();
        configured.apply_overrides(&[ShortcutOverride {
            action: BindableAction::ToggleFullscreen.meta().id.into(),
            key: Some("+".into()),
            ctrl: true,
            alt: false,
            shift: false,
            super_key: false,
        }]);
        assert_eq!(
            configured.check(&plus, ctrl),
            Some(BindableAction::ToggleFullscreen)
        );
        assert_eq!(
            configured.check(&Key::Character("=".into()), ctrl),
            Some(BindableAction::ToggleFullscreen)
        );
    }

    #[test]
    fn shift_is_part_of_the_character_on_symbol_and_digit_keys() {
        let reg = ShortcutRegistry::new();
        let ctrl = ModifiersState::CONTROL;
        let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;

        // US layout: Ctrl+Shift+= arrives as "+" with Shift still held.
        assert_eq!(
            reg.check(&Key::Character("+".into()), ctrl_shift),
            Some(BindableAction::IncreaseFontSize)
        );
        // AZERTY: the digit row needs Shift to type "1".
        assert_eq!(
            reg.check(&Key::Character("1".into()), ctrl_shift),
            Some(BindableAction::SwitchToTab1)
        );
        // Letters keep Shift: Ctrl+Shift+C copies, Ctrl+C is the terminal's.
        assert_eq!(
            reg.check(&Key::Character("C".into()), ctrl_shift),
            Some(BindableAction::Copy)
        );
        assert_eq!(reg.check(&Key::Character("c".into()), ctrl), None);
        // Named keys keep Shift too.
        assert_eq!(
            reg.check(&Key::Named(NamedKey::Tab), ctrl_shift),
            Some(BindableAction::PrevTab)
        );
        assert_eq!(
            KeyChord::new("?", true, false, true, false).display_string(),
            "Ctrl+?"
        );
    }

    #[test]
    fn a_shifted_symbol_from_config_matches_the_recorded_chord() {
        let question = Key::Character("?".into());
        let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;
        let recorded = KeyChord::from_key_event(&question, ctrl_shift).unwrap();
        let mut configured = ShortcutRegistry::new();
        configured.apply_overrides(&[ShortcutOverride {
            action: BindableAction::ToggleFullscreen.meta().id.into(),
            key: Some("?".into()),
            ctrl: true,
            alt: false,
            shift: true,
            super_key: false,
        }]);
        assert_eq!(
            configured.binding_for(BindableAction::ToggleFullscreen),
            Some(&recorded)
        );
        for modifiers in [ctrl_shift, ModifiersState::CONTROL] {
            assert_eq!(
                configured.check(&question, modifiers),
                Some(BindableAction::ToggleFullscreen)
            );
        }
    }

    #[test]
    fn custom_keybindings_share_the_shortcut_normalization() {
        let binding = |key: &str, ctrl: bool, alt: bool, shift: bool, text: &str| KeyBinding {
            key: key.into(),
            modifiers: KeyModifiers {
                ctrl,
                alt,
                shift,
                super_key: false,
            },
            action: KeyAction::SendText { text: text.into() },
        };
        let mut manager = KeyBindingManager::new();
        manager.load(vec![
            binding("?", true, false, true, "question"),
            binding("+", false, true, false, "plus"),
            binding("1", true, true, false, "one"),
            binding("K", true, false, false, "letter"),
            // A later binding on the same chord never shadows the first.
            binding("k", true, false, false, "shadowed"),
        ]);
        let sent = |key: Key, modifiers: ModifiersState| match manager.check(&key, modifiers) {
            Some(KeyAction::SendText { text }) => Some(text.as_str()),
            _ => None,
        };
        let char = |text: &str| Key::Character(text.into());
        let ctrl = ModifiersState::CONTROL;
        let shift = ModifiersState::SHIFT;
        let alt = ModifiersState::ALT;

        assert_eq!(sent(char("?"), ctrl | shift), Some("question"));
        assert_eq!(sent(char("?"), ctrl), Some("question"));
        assert_eq!(sent(char("+"), alt | shift), Some("plus"));
        assert_eq!(sent(char("="), alt), Some("plus"));
        // AZERTY digits need Shift.
        assert_eq!(sent(char("1"), ctrl | alt | shift), Some("one"));
        assert_eq!(sent(char("k"), ctrl), Some("letter"));
        assert_eq!(sent(char("K"), ctrl | shift), None);
        assert_eq!(sent(Key::Named(NamedKey::Enter), ctrl), None);
    }

    #[test]
    fn test_registry_defaults() {
        let reg = ShortcutRegistry::new();
        // Copy should be Ctrl+Shift+C
        let chord = reg.binding_for(BindableAction::Copy).unwrap();
        assert_eq!(chord.key, "c");
        assert!(chord.ctrl);
        assert!(chord.shift);
        assert!(!chord.alt);

        // Pane actions should be usable without first configuring shortcuts.
        assert_eq!(
            reg.binding_for(BindableAction::SplitVertical)
                .unwrap()
                .display_string(),
            "Alt+Shift+Right"
        );
        assert_eq!(
            reg.binding_for(BindableAction::NavigatePaneLeft)
                .unwrap()
                .display_string(),
            "Alt+Left"
        );
        assert_eq!(
            reg.binding_for(BindableAction::ToggleFullscreen)
                .unwrap()
                .display_string(),
            "F11"
        );
    }

    #[test]
    fn default_bindings_are_unique() {
        let mut seen = HashMap::new();
        for &action in BindableAction::all() {
            let Some(chord) = action.meta().default_binding else {
                continue;
            };
            assert!(
                seen.insert(chord.clone(), action).is_none(),
                "duplicate default binding {}",
                chord.display_string()
            );
        }
    }

    #[test]
    fn unicode_picker_is_discoverable_without_reserving_a_default_chord() {
        let meta = BindableAction::OpenUnicodePicker.meta();
        assert_eq!(meta.id, "open_unicode_picker");
        assert_eq!(meta.label, "Insert Unicode Character");
        assert!(meta.default_binding.is_none());
        assert!(BindableAction::all().contains(&BindableAction::OpenUnicodePicker));
    }

    #[test]
    fn test_registry_lookup() {
        let reg = ShortcutRegistry::new();
        let key = Key::Character("c".into());
        let mut mods = ModifiersState::empty();
        mods |= ModifiersState::CONTROL;
        mods |= ModifiersState::SHIFT;
        let result = reg.check(&key, mods);
        assert_eq!(result, Some(BindableAction::Copy));
    }

    #[test]
    fn test_set_binding_with_conflict() {
        let mut reg = ShortcutRegistry::new();
        // Rebind Copy's chord (Ctrl+Shift+C) to Paste
        let chord = KeyChord::new("c", true, false, true, false);
        reg.set_binding(BindableAction::Paste, Some(chord.clone()));

        // Paste now has Ctrl+Shift+C
        assert_eq!(reg.binding_for(BindableAction::Paste), Some(&chord));
        // Copy was auto-unbound due to conflict
        assert!(reg.binding_for(BindableAction::Copy).is_none());
    }

    #[test]
    fn unbinding_an_action_removes_its_chord() {
        let mut reg = ShortcutRegistry::new();
        reg.set_binding(BindableAction::Copy, None);
        assert!(reg.binding_for(BindableAction::Copy).is_none());
        // Lookup should return None
        let key = Key::Character("c".into());
        let mut mods = ModifiersState::empty();
        mods |= ModifiersState::CONTROL;
        mods |= ModifiersState::SHIFT;
        assert!(reg.check(&key, mods).is_none());
    }

    #[test]
    fn test_user_overrides_persistence() {
        let mut reg = ShortcutRegistry::new();
        reg.set_binding(
            BindableAction::Copy,
            Some(KeyChord::new("k", true, false, false, false)),
        );
        reg.set_binding(BindableAction::Paste, None);

        let overrides = reg.user_overrides();
        assert_eq!(overrides.len(), 2); // Copy rebind + Paste cleared

        let copy_ovr = overrides.iter().find(|o| o.action == "copy").unwrap();
        assert_eq!(copy_ovr.key.as_deref(), Some("k"));
        assert!(copy_ovr.ctrl);

        let paste_ovr = overrides.iter().find(|o| o.action == "paste").unwrap();
        assert!(paste_ovr.key.is_none());
    }

    #[test]
    fn test_apply_overrides() {
        let mut reg = ShortcutRegistry::new();
        reg.apply_overrides(&[ShortcutOverride {
            action: "copy".into(),
            key: Some("k".into()),
            ctrl: true,
            alt: false,
            shift: false,
            super_key: false,
        }]);
        let chord = reg.binding_for(BindableAction::Copy).unwrap();
        assert_eq!(chord.key, "k");
        assert!(chord.ctrl);
    }

    #[test]
    fn a_conflicting_override_from_config_matches_setting_it_in_settings() {
        let paste = ShortcutRegistry::new()
            .binding_for(BindableAction::Paste)
            .unwrap()
            .clone();
        let mut set = ShortcutRegistry::new();
        set.set_binding(BindableAction::Copy, Some(paste.clone()));
        let mut loaded = ShortcutRegistry::new();
        loaded.apply_overrides(&set.user_overrides());

        assert_eq!(loaded.binding_for(BindableAction::Copy), Some(&paste));
        assert_eq!(loaded.binding_for(BindableAction::Paste), None);
        let actions = |registry: &ShortcutRegistry| {
            registry
                .user_overrides()
                .into_iter()
                .map(|ovr| (ovr.action, ovr.key))
                .collect::<Vec<_>>()
        };
        assert_eq!(actions(&loaded), actions(&set));
    }

    #[test]
    fn test_from_id_roundtrip() {
        for &action in BindableAction::all() {
            let id = action.meta().id;
            assert_eq!(
                BindableAction::from_id(id),
                Some(action),
                "roundtrip failed for {id}"
            );
        }
    }

    #[test]
    fn test_display_string() {
        let chord = KeyChord::new("c", true, false, true, false);
        assert_eq!(chord.display_string(), "Ctrl+Shift+C");

        let chord2 = KeyChord::new("pageup", false, false, true, false);
        assert_eq!(chord2.display_string(), "Shift+PageUp");

        let chord3 = KeyChord::new("f1", false, false, false, false);
        assert_eq!(chord3.display_string(), "F1");
    }

    #[test]
    fn test_plus_equals_normalization() {
        let reg = ShortcutRegistry::new();
        // IncreaseFontSize is bound to Ctrl+=
        // Pressing Ctrl++ should also match
        let key_plus = Key::Character("+".into());
        let mut mods = ModifiersState::empty();
        mods |= ModifiersState::CONTROL;
        assert_eq!(
            reg.check(&key_plus, mods),
            Some(BindableAction::IncreaseFontSize)
        );
    }

    #[test]
    fn test_keybinding_manager_plugin() {
        let mut manager = KeyBindingManager::new();
        manager.load(vec![KeyBinding {
            key: "t".to_string(),
            modifiers: KeyModifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
            action: KeyAction::PluginCommand {
                command_id: "test_plugin".into(),
            },
        }]);

        let mut mods = ModifiersState::empty();
        mods |= ModifiersState::CONTROL;
        mods |= ModifiersState::SHIFT;
        let key = Key::Character("t".into());
        let action = manager.check(&key, mods);
        assert!(matches!(
            action,
            Some(KeyAction::PluginCommand { command_id }) if command_id == "test_plugin"
        ));
    }
}
