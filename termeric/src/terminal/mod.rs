//! Features built on a terminal's content and launch.
//!
//! - `copy_mode`: keyboard scrollback navigation, selection, and quick select
//! - `diff_detector`: passive unified-diff capture for agent diff review
//! - `events`: draining terminal events beside their PTY owner
//! - `geometry`: the physical grid size shared by renderer and PTY
//! - `guardrails`: confirmation for destructive automated input
//! - `policy`: the host settings every terminal runs under
//! - `process`: a terminal's working directory and foreground program
//! - `row_text`: a row's text and each cell's place in it
//! - `search`: incremental literal and regex search
//! - `shell_integration`: shell launch preparation for OSC 7 and OSC 133
//! - `splash`: the startup splash art
//! - `url`: URL detection and opening

pub mod copy_mode;
pub mod diff_detector;
pub(crate) mod events;
pub(crate) mod geometry;
pub mod guardrails;
pub(crate) mod policy;
pub(crate) mod process;
pub(crate) mod row_text;
pub mod search;
pub mod shell_integration;
pub mod splash;
pub mod url;
