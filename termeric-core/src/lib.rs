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
pub mod checksum;
pub mod event;
pub mod file_transfer;
pub mod graphics;
pub mod grid;
pub mod hex;
pub mod pty;
pub mod safe_text;
pub mod shell;
pub mod split;
pub mod term;
pub mod unicode;

// Re-export commonly used types
pub use event::{
    ClipboardRequest, DynamicColorOperation, DynamicColorTarget, NotificationExpiry,
    NotificationOccasion, NotificationRequest, NotificationSound, NotificationUrgency,
    TerminalEvent,
};
pub use file_transfer::{
    KittyFileTransferAction, KittyFileTransferCommand, KittyFileTransferCompression,
    KittyFileTransferReply, KittyFileTransferStatus, KittyFileTransferStatusCode,
    KittyFileTransferType, KittyFileTransmissionType, MAX_KITTY_TRANSFER_DATA_BYTES,
    MAX_KITTY_TRANSFER_FILE_ID_BYTES, MAX_KITTY_TRANSFER_PATH_BYTES,
};
pub use graphics::{IMAGE_PLACEHOLDER, ImageId, ImagePlacement, PresentationImage, StoredImage};
pub use grid::{
    Cell, CellAttrs, CellContent, CellFlags, CellRef, Color, ColorSpec, Cursor,
    DEFAULT_SCROLLBACK_MEMORY_BYTES, Grid, MultiCell, MultiCellAlignment, Row, RowView,
};
pub use pty::Pty;
pub use shell::CommandRegion;
pub use term::{
    DynamicColors, ExtraCursorColor, ExtraCursorColors, ExtraCursorShape, ExtraCursorState, Point,
    PointerShape, ReportedWorkingDirectory, Selection, SelectionType, Terminal,
};
