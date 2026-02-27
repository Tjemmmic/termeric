//! App-internal action enum and built-in keybinding checks

use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Action that can be triggered by a key binding
#[derive(Clone, Debug)]
pub enum Action {
    /// Copy selected text to clipboard
    Copy,
    /// Paste from clipboard
    Paste,
    /// Scroll up by N lines
    ScrollUp(usize),
    /// Scroll down by N lines
    ScrollDown(usize),
    /// Scroll to top of scrollback
    ScrollToTop,
    /// Scroll to bottom (current output)
    ScrollToBottom,
    /// Increase font size
    IncreaseFontSize,
    /// Decrease font size
    DecreaseFontSize,
    /// Reset font size to default
    ResetFontSize,
    /// Clear selection
    ClearSelection,
    /// Create a new tab
    NewTab,
    /// Close current tab
    CloseTab,
    /// Switch to next tab
    NextTab,
    /// Switch to previous tab
    PrevTab,
    /// Switch to tab by number (0-8)
    SwitchToTab(usize),
    /// Toggle search mode
    ToggleSearch,
    /// Toggle settings panel
    ToggleSettings,
    /// Navigate to previous prompt (shell integration)
    PreviousPrompt,
    /// Navigate to next prompt (shell integration)
    NextPrompt,
    /// Execute a plugin command by its ID
    PluginCommand(String),
    /// Toggle command palette
    ToggleCommandPalette,
    /// No action (pass through to PTY)
    None,
}

/// Check if a key event triggers a bound action
pub fn check_keybinding(event: &KeyEvent, modifiers: ModifiersState) -> Option<Action> {
    if event.state != ElementState::Pressed {
        return None;
    }

    let ctrl = modifiers.control_key();
    let shift = modifiers.shift_key();
    let _alt = modifiers.alt_key();

    match &event.logical_key {
        Key::Character(c) => {
            let c_str = c.as_str();
            let c_lower = c.to_lowercase();

            // Font size - check original string since + and = don't change with to_lowercase
            if ctrl && (c_str == "+" || c_str == "=") {
                return Some(Action::IncreaseFontSize);
            }
            if ctrl && c_str == "-" {
                return Some(Action::DecreaseFontSize);
            }

            match c_lower.as_str() {
                // Clipboard
                "c" if ctrl && shift => Some(Action::Copy),
                "v" if ctrl && shift => Some(Action::Paste),
                // Tabs
                "t" if ctrl && shift => Some(Action::NewTab),
                "w" if ctrl && shift => Some(Action::CloseTab),
                "f" if ctrl && shift => Some(Action::ToggleSearch),
                "p" if ctrl && shift => Some(Action::ToggleCommandPalette),
                // Tab switching by number (Ctrl+1-9)
                "1" if ctrl => Some(Action::SwitchToTab(0)),
                "2" if ctrl => Some(Action::SwitchToTab(1)),
                "3" if ctrl => Some(Action::SwitchToTab(2)),
                "4" if ctrl => Some(Action::SwitchToTab(3)),
                "5" if ctrl => Some(Action::SwitchToTab(4)),
                "6" if ctrl => Some(Action::SwitchToTab(5)),
                "7" if ctrl => Some(Action::SwitchToTab(6)),
                "8" if ctrl => Some(Action::SwitchToTab(7)),
                "9" if ctrl => Some(Action::SwitchToTab(8)),
                "0" if ctrl => Some(Action::ResetFontSize),
                "," if ctrl => Some(Action::ToggleSettings),
                _ => None,
            }
        }
        Key::Named(named) => {
            match named {
                // Scrolling
                NamedKey::PageUp if shift => Some(Action::ScrollUp(10)),
                NamedKey::PageDown if shift => Some(Action::ScrollDown(10)),
                NamedKey::Home if ctrl && shift => Some(Action::ScrollToTop),
                NamedKey::End if ctrl && shift => Some(Action::ScrollToBottom),
                // Tab navigation
                NamedKey::Tab if ctrl && shift => Some(Action::PrevTab),
                NamedKey::Tab if ctrl => Some(Action::NextTab),
                // Shell integration prompt navigation
                NamedKey::ArrowUp if ctrl && shift => Some(Action::PreviousPrompt),
                NamedKey::ArrowDown if ctrl && shift => Some(Action::NextPrompt),
                // Selection
                NamedKey::Escape => Some(Action::ClearSelection),
                _ => None,
            }
        }
        _ => None,
    }
}
