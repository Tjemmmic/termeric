//! The audible bell on macOS: the system alert sound the person chose in
//! System Settings → Sound, played by AppKit.

pub(super) struct BellSound;

impl BellSound {
    pub(super) fn new() -> Self {
        Self
    }

    /// Plays the alert sound. AppKit plays it on its own and reports nothing,
    /// so there is never a failure to show.
    pub(super) fn play(&mut self) -> Option<String> {
        objc2_app_kit::NSBeep();
        None
    }

    /// Nothing to reset: the alert sound has no failures to show again.
    pub(super) fn reenabled(&mut self) {}
}
