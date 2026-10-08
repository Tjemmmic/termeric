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

    /// Whether the action belongs to agent features, which are offered only
    /// while they are on.
    pub fn is_agent_feature(self) -> bool {
        matches!(self, Self::NewAgent | Self::ToggleAgentView)
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
/// reverse-lookup key in the registry. Every one is built by
/// [`KeyChord::new`], which normalizes it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// Lowercased key name: "c", "tab", "pageup", "f1", etc.
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
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

/// One `[[shortcuts]]` entry in config.toml. Entries apply in order on top
/// of the defaults, each as if it had been recorded in the shortcuts editor.
/// Entries of two different actions never give one chord
/// (`first_shared_chord`), so an action with entries is bound as its last
/// entry says, whatever the order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The entry that gives `action` the binding `chord`, spelled as the
    /// registry holds it.
    fn new(action: BindableAction, chord: Option<&KeyChord>) -> Self {
        Self {
            action: action.meta().id.to_string(),
            key: chord.map(|chord| chord.key.clone()),
            ctrl: chord.is_some_and(|chord| chord.ctrl),
            alt: chord.is_some_and(|chord| chord.alt),
            shift: chord.is_some_and(|chord| chord.shift),
            super_key: chord.is_some_and(|chord| chord.super_key),
        }
    }

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

/// The first two entries, as indexes into `entries`, that give one chord to
/// two different actions: `(earlier, later)`. Entries naming an unknown
/// action are left to the caller.
pub fn first_shared_chord(entries: &[ShortcutOverride]) -> Option<(usize, usize)> {
    let bindings: Vec<_> = entries.iter().map(ShortcutOverride::to_binding).collect();
    bindings.iter().enumerate().find_map(|(later, binding)| {
        let Some((action, Some(chord))) = binding else {
            return None;
        };
        bindings[..later]
            .iter()
            .position(|earlier| {
                matches!(earlier, Some((other, Some(taken))) if other != action && taken == chord)
            })
            .map(|earlier| (earlier, later))
    })
}

/// Remove each entry that a later entry of its own action overrides and whose
/// chord an entry of another action gives. It binds nothing any more, and
/// kept, it would give that chord to two actions.
fn drop_overridden_shared_chords(entries: &mut Vec<ShortcutOverride>) {
    let bindings: Vec<_> = entries.iter().map(ShortcutOverride::to_binding).collect();
    let keep: Vec<bool> = bindings
        .iter()
        .enumerate()
        .map(|(index, binding)| {
            let Some((action, Some(chord))) = binding else {
                return true;
            };
            let overridden = bindings[index + 1..]
                .iter()
                .any(|later| matches!(later, Some((other, _)) if other == action));
            let shared = bindings.iter().any(
                |entry| matches!(entry, Some((other, Some(taken))) if other != action && taken == chord),
            );
            !(overridden && shared)
        })
        .collect();
    let mut keep = keep.into_iter();
    entries.retain(|_| keep.next().unwrap_or(true));
}

/// An action left without its default shortcut by another action's
/// `[[shortcuts]]` entry, though no entry of its own unbinds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplacedDefault {
    pub action: BindableAction,
    pub chord: KeyChord,
    /// The action the chord runs now; none when the entry that took it was
    /// overridden by a later entry of its own action, or belongs to an
    /// agent action held back.
    pub taken_by: Option<BindableAction>,
}

impl DisplacedDefault {
    pub fn message(&self) -> String {
        let lost = self.action.meta().label;
        match self.taken_by {
            Some(taken_by) => format!(
                "{} now runs {}; {lost} has no shortcut",
                self.chord,
                taken_by.meta().label
            ),
            None => format!(
                "{lost} has no shortcut; a [[shortcuts]] entry took {}",
                self.chord
            ),
        }
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
    /// Whether agent actions are offered. While they are not, they behave
    /// as if they had no shortcut: their chords run nothing and conflict
    /// with nothing. Their bindings are kept for when they return.
    agent_features: bool,
}

impl ShortcutRegistry {
    /// Create a new registry populated with all default bindings, holding
    /// back agent actions until agent features are turned on.
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
            agent_features: false,
        }
    }

    /// Offer agent actions or hold them back; see `agent_features`.
    pub fn set_agent_features(&mut self, enabled: bool) {
        self.agent_features = enabled;
    }

    /// Whether `action` is offered: every action is, except agent actions
    /// while agent features are off.
    pub fn offers(&self, action: BindableAction) -> bool {
        self.agent_features || !action.is_agent_feature()
    }

    /// The defaults with `overrides` applied.
    pub fn with_overrides(overrides: &[ShortcutOverride]) -> Self {
        let mut registry = Self::new();
        registry.apply_overrides(overrides);
        registry
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
        self.offered_holder(&chord)
    }

    /// The offered action `chord` runs.
    fn offered_holder(&self, chord: &KeyChord) -> Option<BindableAction> {
        self.key_map
            .get(chord)
            .copied()
            .filter(|action| self.offers(*action))
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
                }
            }
            self.key_map.insert(new_chord.clone(), action);
        }

        self.bindings.insert(action, chord);
    }

    /// The offered action a chord is currently bound to (for conflict
    /// detection). An agent action held back is no conflict: giving its
    /// chord to another action leaves it without one.
    pub fn conflict_for(&self, chord: &KeyChord) -> Option<BindableAction> {
        self.offered_holder(chord)
    }

    /// How many offered actions are bound differently from their defaults.
    pub fn changed_count(&self) -> usize {
        BindableAction::all()
            .iter()
            .filter(|action| self.offers(**action))
            .filter(|action| self.binding_for(**action) != action.meta().default_binding.as_ref())
            .count()
    }

    /// Bring `entries`, the configured `[[shortcuts]]`, to the bindings this
    /// registry holds, touching only the entries of actions bound
    /// differently: each such action's last entry is changed in place, and
    /// an action without an entry gets a new one at the end. Every other
    /// entry keeps its place and spelling, so the file a save writes changes
    /// only where the bindings did. An action that lost its shortcut here
    /// gets an entry that unbinds it, so the file says so rather than
    /// leaving it to whichever entry took the chord.
    pub fn record_overrides(&self, entries: &mut Vec<ShortcutOverride>) {
        debug_assert!(
            first_shared_chord(entries).is_none(),
            "configured shortcuts give no chord to two actions"
        );
        // Each pass leaves every action that has an entry bound as here, so
        // only actions without one can still differ, and each of those gets
        // an entry: the passes settle within one per action.
        for _ in 0..=BindableAction::all().len() {
            drop_overridden_shared_chords(entries);
            let configured = Self::with_overrides(entries);
            let differing: Vec<BindableAction> = BindableAction::all()
                .iter()
                .copied()
                .filter(|action| configured.binding_for(*action) != self.binding_for(*action))
                .collect();
            if differing.is_empty() {
                debug_assert!(
                    first_shared_chord(entries).is_none(),
                    "recorded shortcuts give no chord to two actions"
                );
                return;
            }
            for action in differing {
                let recorded = ShortcutOverride::new(action, self.binding_for(action));
                match entries
                    .iter_mut()
                    .rev()
                    .find(|entry| BindableAction::from_id(&entry.action) == Some(action))
                {
                    Some(entry) => *entry = recorded,
                    None => entries.push(recorded),
                }
            }
        }
        debug_assert!(false, "recording shortcuts did not settle: {entries:?}");
        log::error!("Recording shortcuts did not settle; a save may not keep every change");
    }

    /// The actions that `entries` leave without their default shortcut
    /// because another action's entry took it, while no entry of their own
    /// says to unbind them. With `agent_features` off, agent actions are
    /// neither listed nor named as having taken a chord.
    pub fn displaced_defaults(
        entries: &[ShortcutOverride],
        agent_features: bool,
    ) -> Vec<DisplacedDefault> {
        let mut configured = Self::with_overrides(entries);
        configured.set_agent_features(agent_features);
        BindableAction::all()
            .iter()
            .filter(|&&action| configured.offers(action))
            .filter_map(|&action| {
                let chord = action.meta().default_binding?;
                let has_entry = entries
                    .iter()
                    .any(|entry| BindableAction::from_id(&entry.action) == Some(action));
                (!has_entry && configured.binding_for(action).is_none()).then(|| DisplacedDefault {
                    action,
                    taken_by: configured.conflict_for(&chord),
                    chord,
                })
            })
            .collect()
    }
}

impl Default for ShortcutRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Plugin / custom bindings ────────────────────────────────────────────────

/// One `[[keybindings]]` entry in config.toml: a key for a plugin or custom
/// action (SendText, RunCommand, PluginCommand). Its modifiers are written
/// as a `[[shortcuts]]` entry writes them, flat beside the key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyBinding {
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default, rename = "super")]
    pub super_key: bool,
    pub action: KeyAction,
}

impl KeyBinding {
    /// The chord this entry binds, normalized as every chord is.
    fn chord(&self) -> KeyChord {
        KeyChord::new(&self.key, self.ctrl, self.alt, self.shift, self.super_key)
    }
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
            self.key_map
                .entry(binding.chord())
                .or_insert(binding.action);
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
            ctrl,
            alt,
            shift,
            super_key: false,
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

    /// The bindings of every action, to compare two registries.
    fn all_bindings(registry: &ShortcutRegistry) -> Vec<Option<KeyChord>> {
        BindableAction::all()
            .iter()
            .map(|action| registry.binding_for(*action).cloned())
            .collect()
    }

    fn entry(action: &str, key: Option<&str>, ctrl: bool, shift: bool) -> ShortcutOverride {
        ShortcutOverride {
            action: action.into(),
            key: key.map(Into::into),
            ctrl,
            alt: false,
            shift,
            super_key: false,
        }
    }

    #[test]
    fn recorded_shortcuts_load_as_the_bindings_they_record() {
        let mut reg = ShortcutRegistry::new();
        reg.set_binding(
            BindableAction::Copy,
            Some(KeyChord::new("k", true, false, false, false)),
        );
        reg.set_binding(BindableAction::Paste, None);
        let mut entries = Vec::new();
        reg.record_overrides(&mut entries);
        assert_eq!(
            entries,
            [
                entry("copy", Some("k"), true, false),
                entry("paste", None, false, false)
            ]
        );
        assert_eq!(
            all_bindings(&ShortcutRegistry::with_overrides(&entries)),
            all_bindings(&reg)
        );
        assert_eq!(reg.changed_count(), 2);
    }

    #[test]
    fn recording_a_shortcut_changes_only_the_entries_of_changed_actions() {
        // Out of order and spelled as a person writes them.
        let original = vec![
            entry("toggle_pane_zoom", Some("z"), true, true),
            entry("navigate_pane_left", None, false, false),
            entry("scroll_page_up", Some("PageUp"), true, false),
        ];
        let mut entries = original.clone();
        let mut reg = ShortcutRegistry::with_overrides(&entries);
        reg.record_overrides(&mut entries);
        assert_eq!(entries, original, "nothing changed, so nothing is recorded");

        reg.set_binding(
            BindableAction::Copy,
            Some(KeyChord::new("k", true, false, false, false)),
        );
        reg.record_overrides(&mut entries);
        assert_eq!(entries[..3], original[..]);
        assert_eq!(entries[3..], [entry("copy", Some("k"), true, false)]);

        // Rebinding an action with an entry changes that entry in place.
        reg.set_binding(
            BindableAction::TogglePaneZoom,
            Some(KeyChord::new("m", true, false, true, false)),
        );
        reg.record_overrides(&mut entries);
        assert_eq!(entries[0], entry("toggle_pane_zoom", Some("m"), true, true));
        assert_eq!(entries[1..3], original[1..3]);
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn recording_keeps_the_bindings_an_entry_took_from_others() {
        // Copy took Paste's chord, which left Paste unbound.
        let mut entries = vec![entry("copy", Some("v"), true, true)];
        let mut reg = ShortcutRegistry::with_overrides(&entries);
        assert_eq!(reg.binding_for(BindableAction::Paste), None);

        // Moving Copy elsewhere would give Paste its chord back on the next
        // load, but in the editor Paste stays unbound, so the file says so.
        reg.set_binding(
            BindableAction::Copy,
            Some(KeyChord::new("j", true, false, false, false)),
        );
        reg.record_overrides(&mut entries);
        assert_eq!(
            entries,
            [
                entry("copy", Some("j"), true, false),
                entry("paste", None, false, false)
            ]
        );

        // Taking a chord in the editor says in the file that the action
        // which had it is unbound, so loading it is no surprise.
        let find = KeyChord::new("f", true, false, true, false);
        assert_eq!(reg.binding_for(BindableAction::ToggleSearch), Some(&find));
        reg.set_binding(BindableAction::NewTab, Some(find));
        reg.record_overrides(&mut entries);
        assert_eq!(
            all_bindings(&ShortcutRegistry::with_overrides(&entries)),
            all_bindings(&reg)
        );
        assert_eq!(
            entries[2..],
            [
                entry("toggle_search", None, false, false),
                entry("new_tab", Some("f"), true, true)
            ]
        );
        assert_eq!(reg.binding_for(BindableAction::ToggleSearch), None);
        assert!(ShortcutRegistry::displaced_defaults(&entries, true).is_empty());
    }

    #[test]
    fn recording_drops_overridden_entries_rather_than_share_a_chord() {
        // Copy's first entry is overridden by its second, so Ctrl+J is free.
        let mut entries = vec![
            entry("copy", Some("j"), true, false),
            entry("scroll_page_up", Some("PageUp"), true, false),
            entry("copy", Some("k"), true, false),
        ];
        let mut reg = ShortcutRegistry::with_overrides(&entries);
        assert_eq!(
            reg.conflict_for(&KeyChord::new("j", true, false, false, false)),
            None
        );

        // Giving Ctrl+J to Paste would leave two actions' entries naming it.
        reg.set_binding(
            BindableAction::Paste,
            Some(KeyChord::new("j", true, false, false, false)),
        );
        reg.record_overrides(&mut entries);
        assert_eq!(
            entries,
            [
                entry("scroll_page_up", Some("PageUp"), true, false),
                entry("copy", Some("k"), true, false),
                entry("paste", Some("j"), true, false),
            ]
        );
        assert_eq!(first_shared_chord(&entries), None);
        assert_eq!(
            all_bindings(&ShortcutRegistry::with_overrides(&entries)),
            all_bindings(&reg)
        );
    }

    #[test]
    fn entries_of_two_actions_may_not_share_a_chord() {
        let new_tab = entry("new_tab", Some("T"), true, true);
        let rename = entry("rename_tab", Some("t"), true, true);
        assert_eq!(
            first_shared_chord(&[
                new_tab.clone(),
                entry("copy", Some("k"), true, false),
                rename.clone()
            ]),
            Some((0, 2))
        );
        // One action may be given a chord twice, the second entry winning.
        assert_eq!(
            first_shared_chord(&[new_tab.clone(), new_tab.clone()]),
            None
        );
        // Unbinding entries name no chord.
        assert_eq!(
            first_shared_chord(&[
                entry("copy", None, false, false),
                entry("paste", None, false, false)
            ]),
            None
        );
        // Shift is no part of a caseless character, so these are one chord.
        assert_eq!(
            first_shared_chord(&[
                entry("copy", Some("?"), true, true),
                entry("paste", Some("?"), true, false)
            ]),
            Some((0, 1))
        );
    }

    #[test]
    fn agent_actions_held_back_have_no_shortcut_until_agent_features_return() {
        let agent_view = KeyChord::new("a", true, false, true, false);
        let pressed = |registry: &ShortcutRegistry| {
            registry.check(
                &Key::Character("A".into()),
                ModifiersState::CONTROL | ModifiersState::SHIFT,
            )
        };
        let mut reg = ShortcutRegistry::new();
        assert!(!reg.offers(BindableAction::ToggleAgentView));
        assert!(reg.offers(BindableAction::ToggleSearch));
        assert_eq!(pressed(&reg), None);
        assert_eq!(reg.conflict_for(&agent_view), None);
        // The binding is kept, so it comes back.
        assert_eq!(
            reg.binding_for(BindableAction::ToggleAgentView),
            Some(&agent_view)
        );
        reg.set_binding(BindableAction::ToggleAgentView, None);
        assert_eq!(reg.changed_count(), 0, "a held-back change is not counted");
        reg.set_binding(BindableAction::ToggleAgentView, Some(agent_view.clone()));

        reg.set_agent_features(true);
        assert_eq!(pressed(&reg), Some(BindableAction::ToggleAgentView));
        assert_eq!(
            reg.conflict_for(&agent_view),
            Some(BindableAction::ToggleAgentView)
        );

        // A chord given to another action while they are held back is that
        // action's when they return.
        reg.set_agent_features(false);
        reg.set_binding(BindableAction::RenameTab, Some(agent_view.clone()));
        reg.set_agent_features(true);
        assert_eq!(reg.binding_for(BindableAction::ToggleAgentView), None);
        assert_eq!(pressed(&reg), Some(BindableAction::RenameTab));
    }

    #[test]
    fn held_back_agent_actions_are_left_out_of_displaced_defaults() {
        // Rename Tab takes Agent View's default.
        let taken = [entry("rename_tab", Some("a"), true, true)];
        assert!(ShortcutRegistry::displaced_defaults(&taken, false).is_empty());
        assert_eq!(
            ShortcutRegistry::displaced_defaults(&taken, true)[0].action,
            BindableAction::ToggleAgentView
        );

        // Agent View takes New Tab's default: the chord runs nothing while
        // agent features are off, so no action is named as running it.
        let taking = [entry("toggle_agent_view", Some("t"), true, true)];
        let off = ShortcutRegistry::displaced_defaults(&taking, false);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].action, BindableAction::NewTab);
        assert_eq!(off[0].taken_by, None);
        assert_eq!(
            ShortcutRegistry::displaced_defaults(&taking, true)[0].taken_by,
            Some(BindableAction::ToggleAgentView)
        );
    }

    #[test]
    fn an_entry_taking_a_default_names_the_action_that_lost_it() {
        // Rename Tab takes New Tab's Ctrl+Shift+T.
        let entries = [entry("rename_tab", Some("t"), true, true)];
        let displaced = ShortcutRegistry::displaced_defaults(&entries, true);
        assert_eq!(
            displaced,
            [DisplacedDefault {
                action: BindableAction::NewTab,
                chord: KeyChord::new("t", true, false, true, false),
                taken_by: Some(BindableAction::RenameTab),
            }]
        );
        assert_eq!(
            displaced[0].message(),
            "Ctrl+Shift+T now runs Rename Tab; New Tab has no shortcut"
        );

        // An entry of New Tab's own says what it wants: nothing is lost.
        let unbound = [
            entry("rename_tab", Some("t"), true, true),
            entry("new_tab", None, false, false),
        ];
        assert!(ShortcutRegistry::displaced_defaults(&unbound, true).is_empty());
        let moved = [
            entry("rename_tab", Some("t"), true, true),
            entry("new_tab", Some("n"), true, true),
        ];
        assert!(ShortcutRegistry::displaced_defaults(&moved, true).is_empty());

        // A later entry moving Rename Tab on leaves the chord to nobody.
        let overridden = [
            entry("rename_tab", Some("t"), true, true),
            entry("rename_tab", Some("r"), true, true),
        ];
        let displaced = ShortcutRegistry::displaced_defaults(&overridden, true);
        assert_eq!(displaced.len(), 1);
        assert_eq!(displaced[0].taken_by, None);
        assert_eq!(
            displaced[0].message(),
            "New Tab has no shortcut; a [[shortcuts]] entry took Ctrl+Shift+T"
        );
        assert!(ShortcutRegistry::displaced_defaults(&[], true).is_empty());
    }

    #[test]
    fn any_sequence_of_edits_records_entries_that_load_as_the_editor_left_them() {
        let chords: Vec<Option<KeyChord>> = vec![
            None,
            Some(KeyChord::new("c", true, false, true, false)),
            Some(KeyChord::new("v", true, false, true, false)),
            Some(KeyChord::new("t", true, false, true, false)),
            Some(KeyChord::new("k", true, false, false, false)),
            Some(KeyChord::new("pageup", false, false, true, false)),
        ];
        let actions = [
            BindableAction::Copy,
            BindableAction::Paste,
            BindableAction::NewTab,
            BindableAction::ScrollPageUp,
            BindableAction::TogglePaneZoom,
        ];
        // A fixed linear congruential sequence keeps the test deterministic.
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |bound: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize % bound
        };
        // Files as a person may write them: one entry per action, and one
        // with an overridden entry that took a default (New Tab's) for good.
        let files = [
            vec![
                entry("scroll_page_up", Some("PageUp"), true, false),
                entry("new_tab", Some("k"), true, false),
            ],
            vec![
                entry("copy", Some("t"), true, true),
                entry("scroll_page_up", Some("PageUp"), true, false),
                entry("copy", Some("k"), true, false),
            ],
        ];
        for file in &files {
            assert_eq!(first_shared_chord(file), None);
            for _ in 0..200 {
                let mut entries = file.clone();
                let mut reg = ShortcutRegistry::with_overrides(&entries);
                let mut displaced = ShortcutRegistry::displaced_defaults(&entries, true);
                for _ in 0..6 {
                    let action = actions[next(actions.len())];
                    reg.set_binding(action, chords[next(chords.len())].clone());
                    reg.record_overrides(&mut entries);
                    assert_eq!(
                        all_bindings(&ShortcutRegistry::with_overrides(&entries)),
                        all_bindings(&reg),
                        "{entries:?}"
                    );
                    assert_eq!(first_shared_chord(&entries), None, "{entries:?}");
                    // The editor never leaves a new action unbound without
                    // an entry saying so.
                    let now = ShortcutRegistry::displaced_defaults(&entries, true);
                    assert!(
                        now.iter().all(|lost| displaced
                            .iter()
                            .any(|before| before.action == lost.action)),
                        "{entries:?}"
                    );
                    displaced = now;
                }
            }
        }
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
        let mut entries = Vec::new();
        set.record_overrides(&mut entries);
        let loaded = ShortcutRegistry::with_overrides(&entries);

        assert_eq!(loaded.binding_for(BindableAction::Copy), Some(&paste));
        assert_eq!(loaded.binding_for(BindableAction::Paste), None);
        assert_eq!(all_bindings(&loaded), all_bindings(&set));
        // The file says both what Copy took and that Paste lost it.
        assert_eq!(
            entries,
            [
                entry("copy", Some("v"), true, true),
                entry("paste", None, false, false)
            ]
        );
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
            ctrl: true,
            alt: false,
            shift: true,
            super_key: false,
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
