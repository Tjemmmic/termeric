//! Shell integration: OSC 133 prompt and command tracking.

mod prompt;

pub(crate) use prompt::PromptType;
pub use prompt::{CommandRegion, MAX_COMMAND_TEXT_BYTES, PromptTracker};
