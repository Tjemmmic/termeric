//! macOS keeps the display accessibility options on `NSWorkspace` and posts
//! `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification` on the
//! workspace's own notification centre when one changes.

use super::super::AppEvent;
use super::SystemAccessibilityPreferences;
use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
use objc2_foundation::{NSNotification, NSOperationQueue};
use std::ptr::NonNull;
use winit::event_loop::EventLoopProxy;

pub(super) fn start_monitor(proxy: EventLoopProxy<AppEvent>) {
    if proxy
        .send_event(AppEvent::SystemAccessibilityChanged(current_preferences()))
        .is_err()
    {
        return;
    }
    let observer = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
        let _ = proxy.send_event(AppEvent::SystemAccessibilityChanged(current_preferences()));
    });
    let workspace = NSWorkspace::sharedWorkspace();
    // SAFETY: the name is AppKit's own notification name, there is no object
    // to filter by, the main queue is where AppKit delivers it anyway, and
    // the block only reads the workspace and sends to the event loop.
    let token = unsafe {
        workspace
            .notificationCenter()
            .addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &observer,
            )
    };
    // The observation lasts as long as the process: the token is what keeps
    // it registered, and nothing ever ends it.
    std::mem::forget(token);
}

fn current_preferences() -> SystemAccessibilityPreferences {
    let workspace = NSWorkspace::sharedWorkspace();
    SystemAccessibilityPreferences {
        reduce_motion: Some(workspace.accessibilityDisplayShouldReduceMotion()),
        high_contrast: Some(workspace.accessibilityDisplayShouldIncreaseContrast()),
    }
}

#[cfg(test)]
mod tests {
    /// The preferences are readable from any thread, and always known.
    #[test]
    fn the_display_options_are_read_as_known_values() {
        let preferences = super::current_preferences();
        assert!(preferences.reduce_motion.is_some());
        assert!(preferences.high_contrast.is_some());
    }
}
