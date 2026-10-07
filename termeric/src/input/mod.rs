//! Input handling — keyboard, mouse, actions, and keybindings
//!
//! Consolidates all input-related code into a single module.

mod actions;
pub mod keybindings;
mod keyboard;
mod mouse;
mod terminal_protocol;

pub use actions::Action;
pub use keybindings::ShortcutRegistry;
pub use keyboard::{encode_ime_text, key_to_bytes_for_terminal};
pub use mouse::{DoubleClick, MouseState};
pub(crate) use terminal_protocol::{
    BRACKETED_PASTE_OVERHEAD, MouseReportButton, MouseReportKind, WheelDirection,
    encode_focus_report, encode_mouse_report, encode_paste, encode_wheel_as_cursor_key,
};
