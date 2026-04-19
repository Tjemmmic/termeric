//! Input handling — keyboard, mouse, actions, and keybindings
//!
//! Consolidates all input-related code into a single module.

mod actions;
pub mod keybindings;
mod keyboard;
mod mouse;

pub use actions::Action;
pub use keybindings::ShortcutRegistry;
pub use keyboard::key_to_bytes_with_modifiers;
pub use mouse::{MouseState, screen_to_grid};
