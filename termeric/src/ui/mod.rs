//! UI panels and overlays
//!
//! Interactive UI components that overlay the terminal:
//! - Command palette (Ctrl+Shift+P)
//! - Settings panel (Ctrl+,)
//! - Info overlay (About/Version)
//! - Diff review overlay
//! - Context picker modal

pub mod command_palette;
pub mod commands;
pub mod context_picker;
pub mod diff_review;
pub mod info_overlay;
pub mod settings;
