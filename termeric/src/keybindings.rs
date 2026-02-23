//! Custom keybinding system
//!
//! Allows users to define custom key bindings in their configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// A key binding definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBinding {
    /// Key to trigger the action
    pub key: String,
    /// Modifiers required (Ctrl, Alt, Shift, Super)
    #[serde(default)]
    pub modifiers: KeyModifiers,
    /// Action to perform
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
    /// Check if modifiers match the given state
    pub fn matches(&self, state: ModifiersState) -> bool {
        self.ctrl == state.control_key()
            && self.alt == state.alt_key()
            && self.shift == state.shift_key()
            && self.super_key == state.super_key()
    }

    /// Create from a modifier state
    pub fn from_state(state: ModifiersState) -> Self {
        Self {
            ctrl: state.control_key(),
            alt: state.alt_key(),
            shift: state.shift_key(),
            super_key: state.super_key(),
        }
    }
}

/// Action to perform when a key binding is triggered
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KeyAction {
    /// Send text/escape sequence to terminal
    SendText { text: String },
    /// Run a shell command in a new pane
    RunCommand { command: String },
    /// Open command palette
    CommandPalette,
    /// Open settings
    OpenSettings,
    /// Create new tab
    NewTab,
    /// Close current tab/pane
    CloseTab,
    /// Next tab
    NextTab,
    /// Previous tab
    PrevTab,
    /// Switch to specific tab (1-indexed)
    SwitchToTab { index: usize },
    /// Split pane vertically
    SplitVertical,
    /// Split pane horizontally
    SplitHorizontal,
    /// Navigate to pane in direction
    NavigatePane { direction: String },
    /// Close current pane
    ClosePane,
    /// Toggle fullscreen
    ToggleFullscreen,
    /// Toggle search
    ToggleSearch,
    /// Copy selection
    Copy,
    /// Paste clipboard
    Paste,
    /// Scroll up
    ScrollUp { lines: usize },
    /// Scroll down
    ScrollDown { lines: usize },
    /// Scroll to top
    ScrollToTop,
    /// Scroll to bottom
    ScrollToBottom,
    /// Clear scrollback
    ClearScrollback,
    /// Reset terminal
    ResetTerminal,
    /// Navigate to previous prompt (shell integration)
    PreviousPrompt,
    /// Navigate to next prompt (shell integration)
    NextPrompt,
}

impl KeyAction {
    /// Convert a KeyAction to an internal Action if possible.
    /// Returns None for actions that need special handling (SendText, RunCommand, etc.)
    pub fn to_action(&self) -> Option<crate::input::Action> {
        use crate::input::Action;

        match self {
            KeyAction::Copy => Some(Action::Copy),
            KeyAction::Paste => Some(Action::Paste),
            KeyAction::NewTab => Some(Action::NewTab),
            KeyAction::CloseTab => Some(Action::CloseTab),
            KeyAction::NextTab => Some(Action::NextTab),
            KeyAction::PrevTab => Some(Action::PrevTab),
            KeyAction::SwitchToTab { index } => Some(Action::SwitchToTab(*index)),
            KeyAction::ToggleFullscreen => None, // Not yet implemented
            KeyAction::ToggleSearch => Some(Action::ToggleSearch),
            KeyAction::ScrollUp { lines } => Some(Action::ScrollUp(*lines)),
            KeyAction::ScrollDown { lines } => Some(Action::ScrollDown(*lines)),
            KeyAction::ScrollToTop => Some(Action::ScrollToTop),
            KeyAction::ScrollToBottom => Some(Action::ScrollToBottom),
            KeyAction::ClearScrollback => None, // Not yet implemented
            KeyAction::ResetTerminal => None, // Not yet implemented
            KeyAction::PreviousPrompt => Some(Action::PreviousPrompt),
            KeyAction::NextPrompt => Some(Action::NextPrompt),
            // These require special handling in App
            KeyAction::SendText { .. } => None,
            KeyAction::RunCommand { .. } => None,
            KeyAction::CommandPalette => None,
            KeyAction::OpenSettings => Some(Action::ToggleSettings),
            KeyAction::SplitVertical => None, // Will be handled when split panes implemented
            KeyAction::SplitHorizontal => None,
            KeyAction::NavigatePane { .. } => None,
            KeyAction::ClosePane => None,
        }
    }

    /// Check if this action sends text directly to the PTY
    pub fn send_text(&self) -> Option<&str> {
        match self {
            KeyAction::SendText { text } => Some(text),
            _ => None,
        }
    }
}

/// Keybinding manager
#[derive(Debug, Default)]
pub struct KeyBindingManager {
    /// Custom bindings from config
    bindings: Vec<KeyBinding>,
    /// Quick lookup by key name
    key_map: HashMap<String, Vec<usize>>,
}

impl KeyBindingManager {
    /// Create a new keybinding manager
    pub fn new() -> Self {
        Self::default()
    }

    /// Load bindings from config
    pub fn load(&mut self, bindings: Vec<KeyBinding>) {
        self.bindings = bindings;
        self.rebuild_key_map();
    }

    /// Rebuild the key lookup map
    fn rebuild_key_map(&mut self) {
        self.key_map.clear();
        for (idx, binding) in self.bindings.iter().enumerate() {
            self.key_map
                .entry(binding.key.to_lowercase())
                .or_default()
                .push(idx);
        }
    }

    /// Check if a key event matches a binding and return the action
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

    /// Get all bindings
    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }
}

/// Convert a winit Key to a string representation
fn key_to_string(key: &Key) -> Option<String> {
    match key {
        Key::Named(named) => Some(named_key_to_string(named)),
        Key::Character(c) => Some(c.to_string()),
        _ => None,
    }
}

/// Convert a named key to string
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modifier_matching() {
        let mods = KeyModifiers {
            ctrl: true,
            alt: false,
            shift: true,
            super_key: false,
        };

        let mut state = ModifiersState::empty();
        state |= ModifiersState::CONTROL;
        state |= ModifiersState::SHIFT;

        assert!(mods.matches(state));
    }

    #[test]
    fn test_keybinding_lookup() {
        let mut manager = KeyBindingManager::new();
        manager.load(vec![
            KeyBinding {
                key: "t".to_string(),
                modifiers: KeyModifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                },
                action: KeyAction::NewTab,
            },
        ]);

        let mut mods = ModifiersState::empty();
        mods |= ModifiersState::CONTROL;
        mods |= ModifiersState::SHIFT;

        let key = Key::Character("t".into());
        let action = manager.check(&key, mods);
        assert!(matches!(action, Some(KeyAction::NewTab)));
    }
}
