//! Showing and hiding the quick-access terminal.
//!
//! Winit can't hide a window on Wayland (`set_visible` does nothing there),
//! so hiding closes the native window and keeps everything else (tabs,
//! shells, panels); showing opens a new native window for the same `App`,
//! with the activation token of the launch that asked, so the desktop gives
//! it focus. X11 takes the same path, so both behave alike.
//!
//! A native window's ID changes with it, and on Wayland a closed window's ID
//! can be the next window's, so a hidden terminal is filed under an ID no
//! native window has (`hidden_window_id`) until it is shown again.

use super::{App, Application, RedrawReason};
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

/// The ID a hidden quick-access terminal is filed under: counted down from
/// the top of the range, where neither a Wayland surface's address nor an
/// X11 window ID ever lies.
fn hidden_window_id(control_window_id: u64) -> WindowId {
    WindowId::from(u64::MAX - control_window_id)
}

impl App {
    /// Close the native window, keeping the terminals running. The window
    /// stops having focus and pointer gestures end, as when it is hidden.
    fn close_quick_access_window(&mut self) {
        self.ime.clear_preedit();
        self.handle_window_focus_change(false);
        self.handle_window_occlusion_change(true);
        self.quick_access_visible = false;
        // A device-loss report from the closing renderer is stale.
        self.renderer_generation = self.renderer_generation.wrapping_add(1);
        self.surface_lost = false;
        self.surface_lost_at = None;
        // The renderer's surface refers to the window, so it goes first.
        self.accessibility_adapter = None;
        self.renderer = None;
        self.window = None;
    }

    /// Open a new native window for this terminal, focused with `token`
    /// when the desktop gave one.
    fn open_quick_access_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        token: Option<String>,
    ) -> Result<WindowId, String> {
        self.activation_token = token;
        let opened = self.open_native_window(event_loop);
        self.activation_token = None;
        let window = opened.map_err(|error| format!("Could not open the window: {error}"))?;
        if let Err(error) = self.init_renderer(window.clone()) {
            self.accessibility_adapter = None;
            self.renderer = None;
            self.window = None;
            return Err(format!("Could not draw the window: {error:#}"));
        }
        let size = window.inner_size();
        self.handle_resize(size.width, size.height);
        self.quick_access_visible = true;
        self.window_occluded = false;
        window.set_visible(true);
        window.focus_window();
        self.request_redraw(RedrawReason::WindowEvent);
        Ok(window.id())
    }
}

impl Application {
    /// Show or hide the quick-access terminal filed under `native_id`, with
    /// control ID `control_id`. Showing uses the asking launch's activation
    /// `token`. Asking for the state it is in already changes nothing.
    pub(super) fn set_quick_access_visible(
        &mut self,
        event_loop: &ActiveEventLoop,
        native_id: WindowId,
        control_id: u64,
        visible: bool,
        token: Option<String>,
    ) -> Result<(), String> {
        let shown = self
            .windows
            .get(&native_id)
            .ok_or_else(|| format!("GUI window {control_id} is already closing"))?
            .window
            .is_some();
        match (visible, shown) {
            (true, false) => self.show_quick_access(event_loop, native_id, token),
            (false, true) => {
                self.hide_quick_access(native_id, control_id);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn show_quick_access(
        &mut self,
        event_loop: &ActiveEventLoop,
        native_id: WindowId,
        token: Option<String>,
    ) -> Result<(), String> {
        let Some(window) = self.windows.get_mut(&native_id) else {
            return Ok(());
        };
        let shown = window.open_quick_access_window(event_loop, token)?;
        if let (Some(listener), Some(native)) = (&self.wayland_drop, &window.window) {
            listener.register(native, window.window_key());
        }
        self.refile_window(native_id, shown);
        Ok(())
    }

    fn hide_quick_access(&mut self, native_id: WindowId, control_id: u64) {
        let Some(window) = self.windows.get_mut(&native_id) else {
            return;
        };
        if let Some(listener) = &self.wayland_drop {
            listener.unregister(window.window_key());
        }
        window.close_quick_access_window();
        self.refile_window(native_id, hidden_window_id(control_id));
    }

    /// Show every hidden quick-access terminal that asked to be shown (by a
    /// click on one of its notifications).
    pub(super) fn show_requested_quick_access(&mut self, event_loop: &ActiveEventLoop) {
        let mut requested = Vec::new();
        for (native_id, window) in &mut self.windows {
            if std::mem::take(&mut window.request_quick_access_show) && !window.should_exit {
                requested.push((*native_id, window.control_window_id));
            }
        }
        for (native_id, control_id) in requested {
            if let Err(error) =
                self.set_quick_access_visible(event_loop, native_id, control_id, true, None)
            {
                log::warn!("Could not show the quick-access terminal: {error}");
            }
        }
    }

    /// File the window under `old` as `new` instead, keeping its place in
    /// the window order and a tab drop it started.
    fn refile_window(&mut self, old: WindowId, new: WindowId) {
        let Some(window) = self.windows.remove(&old) else {
            return;
        };
        self.windows.insert(new, window);
        for id in &mut self.window_order {
            if *id == old {
                *id = new;
            }
        }
        if let Some(control_id) = self.control_window_ids.remove(&old) {
            self.control_window_ids.insert(new, control_id);
        }
        if let Some(drop) = &mut self.tab_drop
            && drop.source == old
        {
            drop.source = new;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::WindowResult;
    use crate::cli::LaunchPlan;
    use crate::config::Config;

    #[test]
    fn a_hidden_terminal_keeps_its_place_and_its_results_but_takes_no_moved_tabs() {
        let mut application =
            Application::from_config(Config::default(), None, LaunchPlan::default()).unwrap();
        for control_id in [1, 2, 3] {
            let native = WindowId::from(40 + control_id);
            let mut window = App::new_headless(Config::default()).unwrap();
            window.control_window_id = control_id;
            window.launch.quick_access = control_id == 2;
            application.windows.insert(native, window);
            application.window_order.push(native);
            application.control_window_ids.insert(native, control_id);
        }
        let (first, quick, last) = (
            WindowId::from(41_u64),
            WindowId::from(42_u64),
            WindowId::from(43_u64),
        );
        assert_eq!(application.next_live_window(first), Some(quick));

        application.hide_quick_access(quick, 2);

        let hidden = hidden_window_id(2);
        assert!(!application.windows.contains_key(&quick));
        assert_eq!(application.window_order, [first, hidden, last]);
        assert_eq!(application.control_native_window_id(2), Ok(hidden));
        let inventory = application.control_inventory();
        let listed = inventory
            .windows
            .iter()
            .find(|window| window.id == 2)
            .unwrap();
        assert!(listed.quick_access && !listed.visible && !listed.focused);
        assert_eq!(application.next_live_window(first), Some(last));

        let key = application.windows[&hidden].window_key();
        application.deliver_window_result(
            key,
            WindowResult::FilesDropped(Err("only files on this computer can be dropped".into())),
        );
        assert_eq!(application.windows[&hidden].toasts.iter().count(), 1);
    }

    #[test]
    fn a_hidden_terminal_is_filed_apart_from_every_native_window() {
        // Wayland IDs are user-space addresses (below 2^47) and X11 IDs are
        // 32-bit, so the top of the range is free.
        assert_eq!(hidden_window_id(1), WindowId::from(u64::MAX - 1));
        assert!(u64::from(hidden_window_id(1_000_000)) > 1 << 63);
    }
}
