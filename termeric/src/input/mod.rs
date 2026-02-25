//! Input handling — keyboard, mouse, actions, and keybindings
//!
//! Consolidates all input-related code into a single module.

mod keyboard;
mod mouse;
mod actions;
pub mod keybindings;

pub use keyboard::{key_to_bytes, key_to_bytes_with_modifiers};
pub use mouse::{MouseState, screen_to_grid};
pub use actions::{Action, check_keybinding};
pub use keybindings::{KeyAction, KeyBinding, KeyBindingManager};
