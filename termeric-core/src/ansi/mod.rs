//! ANSI/VT escape sequence parsing and handling
//!
//! This module provides escape sequence parsing using the `vte` crate
//! and implements the handler for terminal state updates.

mod color;
mod handler;
mod processor;

pub use color::{ansi_color, color_256};
pub use handler::MAX_OSC52_DECODED_BYTES;
pub(crate) use processor::Processor;
