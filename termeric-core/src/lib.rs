//! Termeric Core - Terminal Emulation Library
//!
//! This crate provides the core terminal emulation functionality for Termeric.
//! It is designed to be reusable and can be embedded in other applications.
//!
//! # Modules
//!
//! - `grid` - Terminal grid and cell management
//! - `term` - Terminal state machine
//! - `ansi` - ANSI/VT escape sequence parsing
//! - `pty` - Pseudo-terminal handling
//!
//! # Example
//!
//! ```rust,ignore
//! use termeric_core::{Terminal, Pty};
//!
//! // Create a new terminal with 80 columns and 24 rows
//! let mut terminal = Terminal::new(80, 24);
//!
//! // Spawn a shell
//! let pty = Pty::new(80, 24)?;
//!
//! // Process output from shell
//! terminal.process(&output_bytes);
//!
//! // Get the grid for rendering
//! let grid = terminal.grid();
//! ```

pub mod grid;
pub mod term;
pub mod ansi;
pub mod pty;
pub mod event;
pub mod graphics;
pub mod shell;

// Re-export commonly used types
pub use grid::{Cell, CellContent, CellAttrs, CellFlags, Color, Grid, Row, Cursor};
pub use term::{Terminal, Selection, SelectionType, Point};
pub use pty::Pty;
pub use event::{TerminalEvent, NotificationRequest};
pub use graphics::{GraphicsManager, ImageId, ImagePlacement, StoredImage};
pub use shell::{ShellIntegration, PromptMark, CommandRegion};
