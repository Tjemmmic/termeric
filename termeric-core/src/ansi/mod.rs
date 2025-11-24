//! ANSI/VT escape sequence parsing and handling
//!
//! This module provides escape sequence parsing using the `vte` crate
//! and implements the handler for terminal state updates.

mod handler;
mod processor;
mod color;

pub use processor::Processor;
pub use color::{AnsiColor, color_256, ansi_color, ansi_bright_color};
