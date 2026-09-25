//! The agent view: a docked panel that drives a coding agent through its
//! structured protocol instead of its terminal interface.
//!
//! The terminal integration in `crate::agents` watches agents that run in a
//! pane and learns what they do from hooks. The agent view instead owns the
//! agent process and talks to it over pipes: the agent streams structured
//! events (text, tool calls, permission requests, turn results) and the panel
//! answers approvals, questions, and plans directly. The two coexist, and the
//! agent view is experimental: removing it leaves terminal agents untouched.
//!
//! Layers, each usable without the next:
//! - [`event`]: agent-neutral events and commands.
//! - [`claude`]: Claude Code's stream-json protocol, mapped onto those events.
//! - [`process`]: owning the agent process and its pipes.
//! - [`transcript`]: the conversation the panel shows.
//! - [`markdown`]: the Markdown agents write, as styled blocks.
//! - [`present`]: how tool calls read: summaries, grouping, and diffs.
//! - [`session`]: one conversation: process, protocol, and transcript.
//! - [`history`]: earlier conversations Claude Code logged, to continue.

pub mod claude;
pub mod event;
pub mod history;
pub mod markdown;
pub mod present;
pub mod process;
pub mod session;
pub mod transcript;
