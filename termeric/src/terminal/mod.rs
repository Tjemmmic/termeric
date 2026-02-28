//! Terminal content features
//!
//! Modules for working with terminal content:
//! - Text search with regex support
//! - URL detection and click-to-open
//! - Terminal container traits (single/split pane abstraction)
//! - Splash screen
//! - Diff detection in terminal output

pub mod container;
pub mod diff_detector;
pub mod guardrails;
pub mod search;
pub mod splash;
pub mod url;
