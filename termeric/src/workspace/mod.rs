//! Complete terminal-workspace snapshots.
//!
//! Workspaces are deliberately separate from single-tab [`crate::session`]
//! snapshots. They preserve organization and relaunch metadata, but never file
//! descriptors or live processes.

mod model;
mod storage;

pub use model::*;
#[allow(unused_imports)]
pub use storage::*;
