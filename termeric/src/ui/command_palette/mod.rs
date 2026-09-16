//! Command palette for built-in terminal commands
//!
//! Provides a Ctrl+Shift+P command palette for configuring the terminal.

mod input_viewport;
pub mod palette;
pub use input_viewport::PaletteInputViewport;
pub use palette::{CommandPalette, PaletteInfo, PaletteLayout, PaletteResult};
