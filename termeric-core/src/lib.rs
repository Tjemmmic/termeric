//! Terminal emulation for Termeric: the VT/ANSI parser, terminal state and
//! scrollback grid, inline graphics, shell integration, and PTY handling.
//! Its public API serves the `termeric` application and the fuzz targets; it is
//! not a general-purpose embedding interface.

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
pub mod uri;

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
    Cell, CellAttrs, CellContent, CellFlags, Color, ColorSpec, DEFAULT_SCROLLBACK_MEMORY_BYTES,
    Grid, MultiCell, MultiCellAlignment, RowView,
};
pub use pty::Pty;
pub use shell::{CommandRegion, MAX_COMMAND_TEXT_BYTES};
pub use term::{
    DynamicColors, ExtraCursorColor, ExtraCursorColors, ExtraCursorShape, ExtraCursorState, Point,
    PointerShape, Selection, SelectionType, Terminal,
};
