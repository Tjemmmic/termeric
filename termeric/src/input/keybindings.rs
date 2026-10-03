//! Unified keyboard shortcut system
//!
//! Single registry for all bindable actions with defaults, user overrides,
//! and O(1) key-chord lookup. Also retains the plugin/custom `KeyBindingManager`
//! for dynamic bindings (SendText, RunCommand, PluginCommand).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use winit::keyboard::{Key, ModifiersState, NamedKey};

// ── BindableAction ──────────────────────────────────────────────────────────

/// Every operation that can be bound to a keyboard shortcut.
///
/// Parameterized actions are expanded to concrete variants so each has
/// exactly one binding slot (e.g. `SwitchToTab1` through `SwitchToTab9`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindableAction {
    // General
    Copy,
    Paste,
    ToggleSearch,
    NewAgent,
    ToggleAgentView,
    ToggleCopyMode,
    QuickSelect,
    OpenUnicodePicker,
    ToggleCommandPalette,
    ToggleSettings,
    ToggleFullscreen,
    ClearSelection,

    // Font
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,

    // Tabs
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    SwitchToTab1,
    SwitchToTab2,
    SwitchToTab3,
    SwitchToTab4,
    SwitchToTab5,
    SwitchToTab6,
    SwitchToTab7,
    SwitchToTab8,
    SwitchToTab9,

    // Panes
    SplitVertical,
    SplitHorizontal,
    NavigatePaneUp,
    NavigatePaneDown,
    NavigatePaneLeft,
    NavigatePaneRight,
    ClosePane,
    RenameTab,
    MoveTabLeft,
    MoveTabRight,
    MoveTabToNextWindow,
    TogglePinTab,
    TogglePaneZoom,
    TogglePaneReadOnly,
    SwapPaneNext,
    BalancePanes,
    MovePaneToNextTab,
    MovePaneToNextWindow,
    LayoutColumns,
    LayoutRows,
    LayoutGrid,
    LayoutTall,
    LayoutFat,
    CyclePaneCwdMode,
    ToggleBroadcast,

    // Scrolling
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,

    // Navigation
    PreviousPrompt,
    NextPrompt,
    PreviousFailedCommand,
    NextFailedCommand,

    // Terminal
    ClearScrollback,
    ResetTerminal,
}

impl BindableAction {
    /// Every variant, in display order.
    pub fn all() -> &'static [BindableAction] {
        use BindableAction::*;
        static ALL: [BindableAction; 63] = [
            // General
            Copy,
            Paste,
            ToggleSearch,
            NewAgent,
            ToggleAgentView,
            ToggleCopyMode,
            QuickSelect,
            OpenUnicodePicker,
            ToggleCommandPalette,
            ToggleSettings,
            ToggleFullscreen,
            ClearSelection,
            // Font
            IncreaseFontSize,
            DecreaseFontSize,
            ResetFontSize,
            // Tabs
            NewTab,
            CloseTab,
            NextTab,
            PrevTab,
            SwitchToTab1,
            SwitchToTab2,
            SwitchToTab3,
            SwitchToTab4,
            SwitchToTab5,
            SwitchToTab6,
            SwitchToTab7,
            SwitchToTab8,
            SwitchToTab9,
            // Panes
            SplitVertical,
            SplitHorizontal,
            NavigatePaneUp,
            NavigatePaneDown,
            NavigatePaneLeft,
            NavigatePaneRight,
            ClosePane,
            RenameTab,
            MoveTabLeft,
            MoveTabRight,
            MoveTabToNextWindow,
            TogglePinTab,
            TogglePaneZoom,
            TogglePaneReadOnly,
            SwapPaneNext,
            BalancePanes,
            MovePaneToNextTab,
            MovePaneToNextWindow,
            LayoutColumns,
            LayoutRows,
            LayoutGrid,
            LayoutTall,
            LayoutFat,
            CyclePaneCwdMode,
            ToggleBroadcast,
            // Scrolling
            ScrollPageUp,
            ScrollPageDown,
            ScrollToTop,
            ScrollToBottom,
            // Navigation
            PreviousPrompt,
            NextPrompt,
            PreviousFailedCommand,
            NextFailedCommand,
            // Terminal
            ClearScrollback,
            ResetTerminal,
        ];
        ALL.as_slice()
    }

    /// Metadata: string id, display label, category, and default binding.
    pub fn meta(self) -> ActionMeta {
        use BindableAction::*;
        use ShortcutCategory::*;
        match self {
            // ── General ──
            Copy => ActionMeta {
                id: "copy",
                label: "Copy Selection",
                category: General,
                default_binding: Some(KeyChord::new("c", true, false, true, false)),
            },
            Paste => ActionMeta {
                id: "paste",
                label: "Paste",
                category: General,
                default_binding: Some(KeyChord::new("v", true, false, true, false)),
            },
            NewAgent => ActionMeta {
                id: "new_agent",
                label: "New Agent",
                category: General,
                default_binding: Some(KeyChord::new("i", true, false, true, false)),
            },
            ToggleAgentView => ActionMeta {
                id: "toggle_agent_view",
                label: "Agent View",
                category: General,
                default_binding: Some(KeyChord::new("a", true, false, true, false)),
            },
            ToggleSearch => ActionMeta {
                id: "toggle_search",
                label: "Find",
                category: General,
                default_binding: Some(KeyChord::new("f", true, false, true, false)),
            },
            ToggleCopyMode => ActionMeta {
                id: "toggle_copy_mode",
                label: "Keyboard Copy Mode",
                category: Navigation,
                default_binding: Some(KeyChord::new("x", true, false, true, false)),
            },
            QuickSelect => ActionMeta {
                id: "quick_select",
                label: "Quick Select Links and Matches",
                category: Navigation,
                default_binding: Some(KeyChord::new("u", true, false, true, false)),
            },
            OpenUnicodePicker => {
                action_meta("open_unicode_picker", "Insert Unicode Character", General)
            }
            ToggleCommandPalette => ActionMeta {
                id: "toggle_command_palette",
                label: "Command Palette",
                category: General,
                default_binding: Some(KeyChord::new("p", true, false, true, false)),
            },
            ToggleSettings => ActionMeta {
                id: "toggle_settings",
                label: "Open Settings",
                category: General,
                default_binding: Some(KeyChord::new(",", true, false, false, false)),
            },
            ToggleFullscreen => ActionMeta {
                id: "toggle_fullscreen",
                label: "Toggle Fullscreen",
                category: General,
                default_binding: Some(KeyChord::new("F11", false, false, false, false)),
            },
            ClearSelection => ActionMeta {
                id: "clear_selection",
                label: "Clear Selection",
                category: General,
                default_binding: Some(KeyChord::new("Escape", false, false, false, false)),
            },

            // ── Font ──
            IncreaseFontSize => ActionMeta {
                id: "increase_font_size",
                label: "Increase Font Size",
                category: Font,
                default_binding: Some(KeyChord::new("=", true, false, false, false)),
            },
            DecreaseFontSize => ActionMeta {
                id: "decrease_font_size",
                label: "Decrease Font Size",
                category: Font,
                default_binding: Some(KeyChord::new("-", true, false, false, false)),
            },
            ResetFontSize => ActionMeta {
                id: "reset_font_size",
                label: "Reset Font Size",
                category: Font,
                default_binding: Some(KeyChord::new("0", true, false, false, false)),
            },

            // ── Tabs ──
            NewTab => ActionMeta {
                id: "new_tab",
                label: "New Tab",
                category: Tabs,
                default_binding: Some(KeyChord::new("t", true, false, true, false)),
            },
            CloseTab => ActionMeta {
                id: "close_tab",
                label: "Close Tab",
                category: Tabs,
                default_binding: Some(KeyChord::new("w", true, false, true, false)),
            },
            NextTab => ActionMeta {
                id: "next_tab",
                label: "Next Tab",
                category: Tabs,
                default_binding: Some(KeyChord::new("Tab", true, false, false, false)),
            },
            PrevTab => ActionMeta {
                id: "prev_tab",
                label: "Previous Tab",
                category: Tabs,
                default_binding: Some(KeyChord::new("Tab", true, false, true, false)),
            },
            SwitchToTab1 => ActionMeta {
                id: "switch_to_tab_1",
                label: "Switch to Tab 1",
                category: Tabs,
                default_binding: Some(KeyChord::new("1", true, false, false, false)),
            },
            SwitchToTab2 => ActionMeta {
                id: "switch_to_tab_2",
                label: "Switch to Tab 2",
                category: Tabs,
                default_binding: Some(KeyChord::new("2", true, false, false, false)),
            },
            SwitchToTab3 => ActionMeta {
                id: "switch_to_tab_3",
                label: "Switch to Tab 3",
                category: Tabs,
                default_binding: Some(KeyChord::new("3", true, false, false, false)),
            },
            SwitchToTab4 => ActionMeta {
                id: "switch_to_tab_4",
                label: "Switch to Tab 4",
                category: Tabs,
                default_binding: Some(KeyChord::new("4", true, false, false, false)),
            },
            SwitchToTab5 => ActionMeta {
                id: "switch_to_tab_5",
                label: "Switch to Tab 5",
                category: Tabs,
                default_binding: Some(KeyChord::new("5", true, false, false, false)),
            },
            SwitchToTab6 => ActionMeta {
                id: "switch_to_tab_6",
                label: "Switch to Tab 6",
                category: Tabs,
                default_binding: Some(KeyChord::new("6", true, false, false, false)),
            },
            SwitchToTab7 => ActionMeta {
                id: "switch_to_tab_7",
                label: "Switch to Tab 7",
                category: Tabs,
                default_binding: Some(KeyChord::new("7", true, false, false, false)),
            },
            SwitchToTab8 => ActionMeta {
                id: "switch_to_tab_8",
                label: "Switch to Tab 8",
                category: Tabs,
                default_binding: Some(KeyChord::new("8", true, false, false, false)),
            },
            SwitchToTab9 => ActionMeta {
                id: "switch_to_tab_9",
                label: "Switch to Tab 9",
                category: Tabs,
                default_binding: Some(KeyChord::new("9", true, false, false, false)),
            },

            // ── Panes ──
            SplitVertical => ActionMeta {
                id: "split_vertical",
                label: "Split Pane Right",
                category: Panes,
                default_binding: Some(KeyChord::new("right", false, true, true, false)),
            },
            SplitHorizontal => ActionMeta {
                id: "split_horizontal",
                label: "Split Pane Down",
                category: Panes,
                default_binding: Some(KeyChord::new("down", false, true, true, false)),
            },
            NavigatePaneUp => ActionMeta {
                id: "navigate_pane_up",
                label: "Navigate Pane Up",
                category: Panes,
                default_binding: Some(KeyChord::new("up", false, true, false, false)),
            },
            NavigatePaneDown => ActionMeta {
                id: "navigate_pane_down",
                label: "Navigate Pane Down",
                category: Panes,
                default_binding: Some(KeyChord::new("down", false, true, false, false)),
            },
            NavigatePaneLeft => ActionMeta {
                id: "navigate_pane_left",
                label: "Navigate Pane Left",
                category: Panes,
                default_binding: Some(KeyChord::new("left", false, true, false, false)),
            },
            NavigatePaneRight => ActionMeta {
                id: "navigate_pane_right",
                label: "Navigate Pane Right",
                category: Panes,
                default_binding: Some(KeyChord::new("right", false, true, false, false)),
            },
            ClosePane => ActionMeta {
                id: "close_pane",
                label: "Close Pane",
                category: Panes,
                default_binding: Some(KeyChord::new("w", false, true, true, false)),
            },
            RenameTab => action_meta("rename_tab", "Rename Tab", Tabs),
            MoveTabLeft => action_meta("move_tab_left", "Move Tab Left", Tabs),
            MoveTabRight => action_meta("move_tab_right", "Move Tab Right", Tabs),
            MoveTabToNextWindow => {
                action_meta("move_tab_to_next_window", "Move Tab to Next Window", Tabs)
            }
            TogglePinTab => action_meta("toggle_pin_tab", "Pin or Unpin Tab", Tabs),
            TogglePaneZoom => action_meta("toggle_pane_zoom", "Zoom Pane", Panes),
            TogglePaneReadOnly => {
                action_meta("toggle_pane_read_only", "Toggle Pane Read-only", Panes)
            }
            SwapPaneNext => action_meta("swap_pane_next", "Swap Pane with Next", Panes),
            BalancePanes => action_meta("balance_panes", "Balance Panes", Panes),
            MovePaneToNextTab => {
                action_meta("move_pane_to_next_tab", "Move Pane to Next Tab", Panes)
            }
            MovePaneToNextWindow => action_meta(
                "move_pane_to_next_window",
                "Move Pane to Next Window",
                Panes,
            ),
            LayoutColumns => action_meta("layout_columns", "Layout: Columns", Panes),
            LayoutRows => action_meta("layout_rows", "Layout: Rows", Panes),
            LayoutGrid => action_meta("layout_grid", "Layout: Grid", Panes),
            LayoutTall => action_meta("layout_tall", "Layout: Tall", Panes),
            LayoutFat => action_meta("layout_fat", "Layout: Fat", Panes),
            CyclePaneCwdMode => {
                action_meta("cycle_pane_cwd_mode", "Cycle New Pane Directory", Panes)
            }
            ToggleBroadcast => action_meta("toggle_broadcast", "Toggle Broadcast Input", Panes),

            // ── Scrolling ──
            ScrollPageUp => ActionMeta {
                id: "scroll_page_up",
                label: "Scroll Page Up",
                category: Scrolling,
                default_binding: Some(KeyChord::new("PageUp", false, false, true, false)),
            },
            ScrollPageDown => ActionMeta {
                id: "scroll_page_down",
                label: "Scroll Page Down",
                category: Scrolling,
                default_binding: Some(KeyChord::new("PageDown", false, false, true, false)),
            },
            ScrollToTop => ActionMeta {
                id: "scroll_to_top",
                label: "Scroll to Top",
                category: Scrolling,
                default_binding: Some(KeyChord::new("Home", true, false, true, false)),
            },
            ScrollToBottom => ActionMeta {
                id: "scroll_to_bottom",
                label: "Scroll to Bottom",
                category: Scrolling,
                default_binding: Some(KeyChord::new("End", true, false, true, false)),
            },

            // ── Navigation ──
            PreviousPrompt => ActionMeta {
                id: "previous_prompt",
                label: "Previous Prompt",
                category: Navigation,
                default_binding: Some(KeyChord::new("Up", true, false, true, false)),
            },
            NextPrompt => ActionMeta {
                id: "next_prompt",
                label: "Next Prompt",
                category: Navigation,
                default_binding: Some(KeyChord::new("Down", true, false, true, false)),
            },
            PreviousFailedCommand => ActionMeta {
                id: "previous_failed_command",
                label: "Jump to Previous Failed Command",
                category: Navigation,
                default_binding: None,
            },
            NextFailedCommand => ActionMeta {
                id: "next_failed_command",
                label: "Jump to Next Failed Command",
                category: Navigation,
                default_binding: None,
            },

            // ── Terminal ──
            ClearScrollback => ActionMeta {
                id: "clear_scrollback",
                label: "Clear Scrollback",
                category: Terminal,
                default_binding: None,
            },
            ResetTerminal => ActionMeta {
                id: "reset_terminal",
                label: "Reset Terminal",
                category: Terminal,
                default_binding: None,
            },
        }
    }

    /// Convert to the internal `Action` enum used by app dispatch.
    pub fn to_action(self) -> crate::input::Action {
        use crate::input::Action;
        match self {
            Self::Copy => Action::Copy,
            Self::Paste => Action::Paste,
            Self::ToggleSearch => Action::ToggleSearch,
            Self::NewAgent => Action::NewAgent,
            Self::ToggleAgentView => Action::ToggleAgentView,
            Self::ToggleCopyMode => Action::ToggleCopyMode,
            Self::QuickSelect => Action::QuickSelect,
            Self::OpenUnicodePicker => Action::OpenUnicodePicker,
            Self::ToggleCommandPalette => Action::ToggleCommandPalette,
            Self::ToggleSettings => Action::ToggleSettings,
            Self::ToggleFullscreen => Action::ToggleFullscreen,
            Self::ClearSelection => Action::ClearSelection,
            Self::IncreaseFontSize => Action::IncreaseFontSize,
            Self::DecreaseFontSize => Action::DecreaseFontSize,
            Self::ResetFontSize => Action::ResetFontSize,
            Self::NewTab => Action::NewTab,
            Self::CloseTab => Action::CloseTab,
            Self::NextTab => Action::NextTab,
            Self::PrevTab => Action::PrevTab,
            Self::SwitchToTab1 => Action::SwitchToTab(0),
            Self::SwitchToTab2 => Action::SwitchToTab(1),
            Self::SwitchToTab3 => Action::SwitchToTab(2),
            Self::SwitchToTab4 => Action::SwitchToTab(3),
            Self::SwitchToTab5 => Action::SwitchToTab(4),
            Self::SwitchToTab6 => Action::SwitchToTab(5),
            Self::SwitchToTab7 => Action::SwitchToTab(6),
            Self::SwitchToTab8 => Action::SwitchToTab(7),
            Self::SwitchToTab9 => Action::SwitchToTab(8),
            Self::SplitVertical => Action::SplitVertical,
            Self::SplitHorizontal => Action::SplitHorizontal,
            Self::NavigatePaneUp => Action::NavigatePane("up".into()),
            Self::NavigatePaneDown => Action::NavigatePane("down".into()),
            Self::NavigatePaneLeft => Action::NavigatePane("left".into()),
            Self::NavigatePaneRight => Action::NavigatePane("right".into()),
            Self::ClosePane => Action::ClosePane,
            Self::RenameTab => Action::RenameTab,
            Self::MoveTabLeft => Action::MoveTabLeft,
            Self::MoveTabRight => Action::MoveTabRight,
            Self::MoveTabToNextWindow => Action::MoveTabToNextWindow,
            Self::TogglePinTab => Action::TogglePinTab,
            Self::TogglePaneZoom => Action::TogglePaneZoom,
            Self::TogglePaneReadOnly => Action::TogglePaneReadOnly,
            Self::SwapPaneNext => Action::SwapPaneNext,
            Self::BalancePanes => Action::BalancePanes,
            Self::MovePaneToNextTab => Action::MovePaneToNextTab,
            Self::MovePaneToNextWindow => Action::MovePaneToNextWindow,
            Self::LayoutColumns => Action::LayoutColumns,
            Self::LayoutRows => Action::LayoutRows,
            Self::LayoutGrid => Action::LayoutGrid,
            Self::LayoutTall => Action::LayoutTall,
            Self::LayoutFat => Action::LayoutFat,
            Self::CyclePaneCwdMode => Action::CyclePaneCwdMode,
            Self::ToggleBroadcast => Action::ToggleBroadcast,
            Self::ScrollPageUp => Action::ScrollPageUp,
            Self::ScrollPageDown => Action::ScrollPageDown,
            Self::ScrollToTop => Action::ScrollToTop,
            Self::ScrollToBottom => Action::ScrollToBottom,
            Self::PreviousPrompt => Action::PreviousPrompt,
            Self::NextPrompt => Action::NextPrompt,
            Self::PreviousFailedCommand => Action::PreviousFailedCommand,
            Self::NextFailedCommand => Action::NextFailedCommand,
            Self::ClearScrollback => Action::ClearScrollback,
            Self::ResetTerminal => Action::ResetTerminal,
        }
    }

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

fn action_meta(id: &'static str, label: &'static str, category: ShortcutCategory) -> ActionMeta {
    ActionMeta {
        id,
        label,
        category,
        default_binding: None,
    }
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
    /// configured, and pressed keys normalize the same way: lowercase, and
    /// `+` as `=`, which share a physical key on most layouts.
    pub fn new(key: &str, ctrl: bool, alt: bool, shift: bool, super_key: bool) -> Self {
        let key = if key == "+" { "=" } else { key };
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

    /// Apply user overrides from config on top of defaults.
    pub fn apply_overrides(&mut self, overrides: &[ShortcutOverride]) {
        for ovr in overrides {
            if let Some((action, chord)) = ovr.to_binding() {
                // Remove old chord from reverse map
                if let Some(Some(old)) = self.bindings.get(&action) {
                    self.key_map.remove(old);
                }
                // If new chord conflicts, clear the conflicting action
                if let Some(new_chord) = &chord {
                    if let Some(&conflicting) = self.key_map.get(new_chord) {
                        if conflicting != action {
                            self.bindings.insert(conflicting, None);
                        }
                    }
                    self.key_map.insert(new_chord.clone(), action);
                }
                self.bindings.insert(action, chord.clone());
                self.user_overrides.insert(action, chord);
            }
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

// ── Plugin / custom bindings (retained from original) ───────────────────────

/// A key binding definition for plugin/custom actions (SendText, RunCommand, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: String,
    #[serde(default)]
    pub modifiers: KeyModifiers,
    pub action: KeyAction,
}

/// Modifier keys for a binding
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

impl KeyModifiers {
    pub fn matches(&self, state: ModifiersState) -> bool {
        self.ctrl == state.control_key()
            && self.alt == state.alt_key()
            && self.shift == state.shift_key()
            && self.super_key == state.super_key()
    }
}

/// Action to perform when a plugin/custom key binding is triggered.
/// These are dynamic/parameterized and cannot be part of BindableAction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KeyAction {
    SendText { text: String },
    RunCommand { command: String },
    PluginCommand { command_id: String },
}

impl KeyAction {
    pub fn to_action(&self) -> Option<crate::input::Action> {
        match self {
            KeyAction::PluginCommand { command_id } => {
                Some(crate::input::Action::PluginCommand(command_id.clone()))
            }
            _ => None,
        }
    }
}

/// Manager for plugin/custom keybindings (SendText, RunCommand, PluginCommand).
#[derive(Debug, Default)]
pub struct KeyBindingManager {
    bindings: Vec<KeyBinding>,
    key_map: HashMap<String, Vec<usize>>,
}

impl KeyBindingManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(&mut self, bindings: Vec<KeyBinding>) {
        self.bindings = bindings;
        self.rebuild_key_map();
    }

    fn rebuild_key_map(&mut self) {
        self.key_map.clear();
        for (idx, binding) in self.bindings.iter().enumerate() {
            self.key_map
                .entry(binding.key.to_lowercase())
                .or_default()
                .push(idx);
        }
    }

    pub fn check(&self, key: &Key, modifiers: ModifiersState) -> Option<&KeyAction> {
        let key_name = key_to_string(key)?;
        if let Some(indices) = self.key_map.get(&key_name.to_lowercase()) {
            for &idx in indices {
                let binding = &self.bindings[idx];
                if binding.modifiers.matches(modifiers) {
                    return Some(&binding.action);
                }
            }
        }
        None
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
