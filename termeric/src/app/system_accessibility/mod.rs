//! The system's accessibility preferences (reduced motion, increased
//! contrast), read at start and on every change, and sent to the event loop
//! as small immutable snapshots: from the XDG desktop portal on Linux
//! (`portal.rs`), from `NSWorkspace` on macOS (`workspace.rs`).

#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "macos")]
mod workspace;

use super::AppEvent;
use winit::event_loop::EventLoopProxy;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SystemAccessibilityPreferences {
    pub reduce_motion: Option<bool>,
    pub high_contrast: Option<bool>,
}

/// Report the preferences, then each change, through `proxy`. Called once,
/// from the event loop.
pub(super) fn start_monitor(proxy: EventLoopProxy<AppEvent>) {
    #[cfg(target_os = "linux")]
    portal::start_monitor(proxy);
    #[cfg(target_os = "macos")]
    workspace::start_monitor(proxy);
}
