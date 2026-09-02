//! Terminal content features
//!
//! Modules for working with terminal content:
//! - Text search with regex support
//! - Splash screen
//! - Diff detection in terminal output

pub mod container;
pub mod copy_mode;
pub mod diff_detector;
pub(crate) mod events;
pub(crate) mod geometry;
pub mod guardrails;
pub mod search;
pub mod shell_integration;
pub mod splash;
pub mod url;
