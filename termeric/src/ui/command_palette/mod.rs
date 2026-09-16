//! Command palette for built-in terminal commands
//!
//! Provides a Ctrl+Shift+P command palette for configuring the terminal.

mod input_viewport;
pub mod palette;
pub use crate::ui::text_input_viewport::TextInputViewport as PaletteInputViewport;
pub use palette::{CommandPalette, PaletteInfo, PaletteLayout, PaletteResult};
