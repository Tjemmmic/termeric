//! UI panels and overlays
//!
//! Interactive UI components that overlay the terminal:
//! - Command palette (Ctrl+Shift+P)
//! - Settings panel (Ctrl+,)
//! - Bounded information, error, and choice overlay
//! - Diff review overlay
//! - Context picker modal

pub mod animation;
pub mod claude_chrome;
pub mod command_palette;
pub mod commands;
pub mod constants;
pub mod context_picker;
pub mod cursor_motion;
pub mod diff_review;
pub mod domain_picker;
pub mod info_overlay;
pub mod settings;
pub mod shortcuts_panel;
pub mod text_input;
pub mod text_input_viewport;
pub mod toast;
pub mod unicode_picker;
