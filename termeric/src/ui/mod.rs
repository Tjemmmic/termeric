//! UI panels and overlays
//!
//! Interactive UI components that overlay the terminal:
//! - Command palette (Ctrl+Shift+P)
//! - Settings panel (Ctrl+,)
//! - Info overlay (About/Version)
//! - Diff review overlay
//! - Context picker modal

pub mod animation;
pub mod command_palette;
pub mod constants;
pub mod text_input;
pub mod commands;
pub mod claude_chrome;
pub mod context_picker;
pub mod diff_review;
pub mod info_overlay;
pub mod settings;
