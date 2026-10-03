//! The command palette: a centered, fuzzy-filtered list of the built-in,
//! plugin, and terminal-profile commands. It also shows quick picks (themes,
//! agents, supervisor modes), and the domain and Unicode pickers draw and
//! navigate through it (see [`PaletteOwner`]).

mod input_viewport;
mod owner;
pub mod palette;
pub use owner::PaletteOwner;
pub use palette::{CommandPalette, PaletteInfo, PaletteLayout, PaletteResult};
