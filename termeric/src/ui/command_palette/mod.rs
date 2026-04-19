//! Command palette for built-in terminal commands
//!
//! Provides a Ctrl+Shift+P command palette for configuring the terminal.

pub mod palette;
pub use palette::{
    CommandPalette, CommandSource, PaletteInfo, PaletteLayout, PaletteResult, RegisteredCommand,
};
