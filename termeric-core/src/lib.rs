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

pub mod ansi;
pub mod event;
pub mod graphics;
pub mod grid;
pub mod pty;
pub mod shell;
pub mod term;
pub mod unicode;

// Re-export commonly used types
pub use event::{
    ClipboardRequest, DynamicColorOperation, DynamicColorTarget, NotificationRequest, TerminalEvent,
};
pub use graphics::{
    GraphicsManager, IMAGE_PLACEHOLDER, ImageId, ImagePlacement, PresentationImage, StoredImage,
};
pub use grid::{
    Cell, CellAttrs, CellContent, CellFlags, CellRef, Color, ColorSpec, Cursor,
    DEFAULT_SCROLLBACK_MEMORY_BYTES, Grid, GridMemoryUsage, Row, RowView,
};
pub use pty::Pty;
pub use shell::{CommandRegion, PromptMark, ShellIntegration};
pub use term::{
    DynamicColors, Point, PointerShape, ReportedWorkingDirectory, Selection, SelectionType,
    Terminal, TerminalMemoryUsage, XTERM_TITLE_STACK_DEPTH,
};
