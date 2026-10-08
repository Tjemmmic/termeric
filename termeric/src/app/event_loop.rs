//! winit event loop integration
//!
//! Window events for one `App`, dispatched by `Application`.

use std::sync::Arc;

use super::modal_layer::ModalLayer;
use winit::{
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::WindowEvent,
    event_loop::ActiveEventLoop,
    window::Window,
};

use super::App;
use super::redraw_scheduler::RedrawReason;
use crate::ui::text_card::OverlayScroll;

impl App {
    pub(super) fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // Create window
        let quick_monitor = self
            .launch
            .quick_access
            .then(|| {
                event_loop
                    .primary_monitor()
                    .or_else(|| event_loop.available_monitors().next())
            })
            .flatten();
        let quick_geometry = quick_monitor.as_ref().map(|monitor| {
            quick_access_geometry(monitor.position(), monitor.size(), self.launch.pixel_size)
        });
        let mut window_attrs = Window::default_attributes()
            .with_title(self.desired_native_window_title())
            .with_visible(false)
            .with_inner_size(LogicalSize::new(
                self.config.window.width,
                self.config.window.height,
            ))
            .with_decorations(self.config.window.decorations)
            .with_window_level(
                self.config
                    .window
                    .level
                    .native_for_window(self.launch.quick_access),
            );
        if self.launch.quick_access {
            window_attrs = window_attrs.with_decorations(false);
        }
        if let Some((position, size)) = quick_geometry {
            window_attrs = window_attrs.with_position(position).with_inner_size(size);
        } else if let Some((width, height)) = self.launch.pixel_size {
            window_attrs = window_attrs.with_inner_size(PhysicalSize::new(width, height));
        }

        {
            use winit::platform::startup_notify::{
                EventLoopExtStartupNotify, WindowAttributesExtStartupNotify,
            };
            let app_id = self
                .launch
                .app_id
                .as_deref()
                .unwrap_or(if self.launch.quick_access {
                    super::QUICK_ACCESS_APP_ID
                } else {
                    super::APP_ID
                });
            window_attrs = winit::platform::x11::WindowAttributesExtX11::with_name(
                window_attrs,
                app_id,
                app_id,
            );
            window_attrs = winit::platform::wayland::WindowAttributesExtWayland::with_name(
                window_attrs,
                app_id,
                app_id,
            );
            // A window opened for a launch that joined this process takes
            // that launch's token; the first window takes this process's.
            if let Some(token) = self.activation_token.take() {
                window_attrs = window_attrs
                    .with_activation_token(winit::window::ActivationToken::from_raw(token));
            } else if let Some(token) = event_loop.read_token_from_env() {
                window_attrs = window_attrs.with_activation_token(token);
                winit::platform::startup_notify::reset_activation_token_env();
            }
        }

        // Load and set window icon if available
        if let Some(icon) = Self::load_icon() {
            window_attrs = window_attrs.with_window_icon(Some(icon));
        }

        match event_loop.create_window(window_attrs) {
            Ok(window) => {
                log::info!("Window created");
                let window = Arc::new(window);
                // Winit leaves IME disabled by default.
                window.set_ime_allowed(true);
                self.accessibility_adapter = Some(accesskit_winit::Adapter::with_event_loop_proxy(
                    event_loop,
                    &window,
                    super::EVENT_LOOP_PROXY
                        .get()
                        .cloned()
                        .expect("event-loop proxy is installed before resume"),
                ));

                // Initialize renderer
                if let Err(e) = self.init_renderer(window.clone()) {
                    self.fail_startup(
                        format!("Failed to initialize renderer: {e:#}"),
                        "window renderer initialization",
                    );
                    return;
                }

                // Initialize tabs with first tab
                if let Err(e) = self.init_tabs() {
                    self.fail_startup(
                        format!("Failed to initialize tabs: {e:#}"),
                        "window tab initialization",
                    );
                    return;
                }

                window.set_visible(true);
                if self.launch.quick_access {
                    window.focus_window();
                }
                self.warn_about_outdated_agent_hooks();

                self.redraw_scheduler.request(RedrawReason::Startup);
                self.render_frame();
            }
            Err(e) => self.fail_startup(
                format!("Failed to create window: {e:#}"),
                "native window creation",
            ),
        }
    }

    /// Draw a frame for every reason requested since the last one.
    fn render_frame(&mut self) {
        let reasons = self.redraw_scheduler.begin_frame();
        log::trace!("Rendering frame: reasons={reasons}");
        self.render();
    }

    /// End a window that could not start, keeping `error` for the requester.
    /// A private GUI-control window logs only which `stage` failed, never the
    /// error's details.
    fn fail_startup(&mut self, error: String, stage: &str) {
        if self.redact_startup_error_log {
            log::error!("Private GUI-control {stage} failed");
        } else {
            log::error!("{error}");
        }
        self.startup_error = Some(error);
        self.should_exit = true;
    }

    pub(super) fn window_event(&mut self, event: WindowEvent) {
        self.finish_obscured_native_pointer_gesture();
        if let (Some(adapter), Some(window)) = (&mut self.accessibility_adapter, &self.window) {
            adapter.process_event(window, &event);
        }
        let reason = match &event {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                Some(RedrawReason::Resize)
            }
            WindowEvent::KeyboardInput { .. }
            | WindowEvent::ModifiersChanged(_)
            | WindowEvent::Ime(_) => Some(RedrawReason::KeyboardInput),
            WindowEvent::CursorMoved { .. }
            | WindowEvent::CursorLeft { .. }
            | WindowEvent::CursorEntered { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::HoveredFile(_)
            | WindowEvent::HoveredFileCancelled
            | WindowEvent::DroppedFile(_) => Some(RedrawReason::PointerInput),
            WindowEvent::Focused(_) => Some(RedrawReason::Focus),
            _ => None,
        };
        if let Some(reason) = reason {
            self.redraw_scheduler.request(reason);
        }

        match event {
            WindowEvent::CloseRequested => {
                log::info!("Close requested");
                self.should_exit = true;
            }

            WindowEvent::Resized(size) => {
                let recovering_surface = self.surface_lost;
                self.handle_resize(size.width, size.height);
                self.surface_lost = false;
                self.surface_lost_at = None;
                if recovering_surface {
                    self.redraw_scheduler.request(RedrawReason::SurfaceRecovery);
                }
                self.request_redraw(RedrawReason::Resize);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // Use renderer-owned dimensions rather than querying the native
                // window. X11 may deliver a final scale event after WM_DELETE;
                // XGetGeometry on that already-destroyed handle panics in Winit.
                let size = self.renderer.as_ref().map(|renderer| {
                    let (width, height) = renderer.window_size();
                    winit::dpi::PhysicalSize::new(width, height)
                });
                if let Some(size) = size {
                    self.handle_scale_factor_change(scale_factor, size);
                }
                self.request_redraw(RedrawReason::Resize);
            }

            WindowEvent::ModifiersChanged(modifiers) => {
                let previous = std::mem::replace(&mut self.modifiers, modifiers.state());
                self.refresh_pointer_after_modifiers(previous);
            }

            WindowEvent::Ime(event) => {
                self.handle_ime_event(event);
            }

            WindowEvent::HoveredFile(path) => {
                self.file_drop_hovered = true;
                let allowed = self.dropped_path_allowed(&path);
                if let Some(window) = &self.window {
                    window.set_cursor(if allowed {
                        winit::window::CursorIcon::Copy
                    } else {
                        winit::window::CursorIcon::NotAllowed
                    });
                }
            }

            WindowEvent::HoveredFileCancelled => {
                self.file_drop_hovered = false;
                self.refresh_terminal_pointer_cursor();
            }

            WindowEvent::DroppedFile(path) => {
                self.file_drop_hovered = false;
                self.refresh_terminal_pointer_cursor();
                self.handle_dropped_path(&path);
            }

            WindowEvent::KeyboardInput { event, .. } => self.handle_keyboard_event(event),

            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button_event(state, button)
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.last_cursor_pos = Some((position.x, position.y));
                // Redraw for hover effects on overlays
                if self.info_overlay.is_active() || self.diff_review.is_active() {
                    self.request_redraw(RedrawReason::PointerInput);
                }
                self.handle_mouse_move((position.x, position.y));
            }

            WindowEvent::CursorLeft { .. } => {
                // Keep the last coordinates for protocol/button releases, but
                // never keep an overflow dwell alive outside the surface.
                self.tab_bar_hover = None;
                self.tab_drag_scroll = None;
            }

            WindowEvent::MouseWheel { delta, .. } => {
                self.handle_mouse_wheel(delta);
            }

            WindowEvent::Focused(focused) => {
                if !focused {
                    self.ime.clear_preedit();
                }
                self.handle_window_focus_change(focused);
                self.request_redraw(RedrawReason::Focus);
            }

            WindowEvent::Occluded(occluded) => {
                self.handle_window_occlusion_change(occluded);
                if !occluded {
                    self.request_redraw(RedrawReason::WindowEvent);
                }
            }

            WindowEvent::RedrawRequested => {
                // Process any pending PTY events
                if self.process_pty_events() {
                    self.redraw_scheduler.request(RedrawReason::TerminalOutput);
                }

                if self.should_exit {
                    return;
                }
                if self.window_occluded {
                    return;
                }

                self.render_frame();
            }

            _ => {}
        }
    }
}

impl App {
    /// One key press or release, offered to each input owner in priority
    /// order: the top modal layer, tab rename, the agent view, search and copy
    /// mode, then application shortcuts and finally the focused terminal.
    fn handle_keyboard_event(&mut self, event: winit::event::KeyEvent) {
        if matches!(
            event.logical_key,
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::AltGraph)
        ) {
            self.alt_graph_active = event.state == winit::event::ElementState::Pressed;
        }

        // Query paste respects modal ownership and the configured
        // chord before query handlers consume its character key.
        if event.state == winit::event::ElementState::Pressed
            && self.handle_query_paste_key(&event.logical_key)
        {
            return;
        }

        // The top modal layer owns every key, pressed or released, so none
        // reaches anything beneath it.
        if let Some(layer) = self.top_modal_layer() {
            if event.state == winit::event::ElementState::Pressed {
                self.handle_modal_layer_key(layer, &event);
            }
            return;
        }

        if self.handle_tab_rename_input(&event) {
            return;
        }

        // The docked agent view takes keys only while it has keyboard
        // focus. Global shortcuts still apply, except the chords its
        // composer edits with.
        if self.agent_view_has_keyboard() {
            if event.state == winit::event::ElementState::Pressed
                && (super::agent_view_input::composer_chord(
                    &event.logical_key,
                    self.modifiers,
                    self.alt_graph_active,
                ) || !self.run_app_shortcut(&event.logical_key))
            {
                self.handle_agent_view_key(&event.logical_key, event.text.as_deref());
            }
            return;
        }

        // The find box, then copy mode and quick select: each takes its own
        // chords, runs application shortcuts, and swallows everything else.
        if self.handle_search_mode_input(&event.logical_key, event.state) {
            return;
        }

        if self.handle_copy_mode_input(&event.logical_key, event.state) {
            return;
        }

        if self.cancel_broadcast_on_escape(&event) {
            return;
        }

        // F6 enters the tab-bar focus ring. While it is active, all
        // navigation and activation keys stay in application chrome.
        if self.handle_chrome_keyboard(&event.logical_key, event.state) {
            return;
        }

        if event.state == winit::event::ElementState::Pressed
            && self.handle_git_completion_key(&event.logical_key)
        {
            return;
        }

        // Arming is a safe application mode: only pane navigation,
        // target selection, confirmation, and cancellation are
        // accepted. No other key may leak into a PTY.
        if self.handle_broadcast_arming_key(&event) {
            return;
        }

        if event.state == winit::event::ElementState::Pressed
            && self.handle_bound_key(&event.logical_key)
        {
            return;
        }

        // Convert key event to terminal input
        if self.write_broadcast_key(&event) {
            return;
        }
        let modifiers = self.modifiers;
        let alt_graph_active = self.alt_graph_active;
        let data = self.active_terminal().and_then(|terminal| {
            crate::input::key_to_bytes_for_terminal(&event, modifiers, alt_graph_active, terminal)
        });
        if let Some(data) = data {
            self.write_to_pty(&data);
        }
    }

    /// A pressed key for the modal layer `layer`, which is on top.
    fn handle_modal_layer_key(&mut self, layer: ModalLayer, event: &winit::event::KeyEvent) {
        let key = &event.logical_key;
        match layer {
            ModalLayer::Guardrail => self.handle_guardrail_key(key),
            // Actions are request-scoped, so input during the overlay's
            // close animation cannot target a replacement notice.
            ModalLayer::InfoOverlay => self.handle_info_overlay_key(key, event.repeat),
            ModalLayer::DiffReview => self.handle_diff_review_key(key),
            ModalLayer::ContextPicker => self.handle_context_picker_key(key),
            ModalLayer::SproutMenu => {
                if let Some(request_id) = self
                    .harvest
                    .as_ref()
                    .and_then(|harvest| harvest.sprout_menu.as_ref().map(|menu| menu.request_id))
                {
                    self.handle_sprout_menu_key(request_id, key);
                }
            }
            ModalLayer::Cellar => self.handle_cellar_key(key),
            // The palette-style modals share the palette's query editing;
            // the live-domain picker keeps its own staged discovery state.
            ModalLayer::DomainPicker | ModalLayer::UnicodePicker | ModalLayer::CommandPalette => {
                if !self.handle_palette_edit_key(key) {
                    self.handle_palette_modal_key(key);
                }
            }
            ModalLayer::ShortcutsPanel => self.handle_shortcuts_panel_key(key),
            ModalLayer::Settings => self.handle_settings_panel_key(key),
            ModalLayer::WorkspaceMenu => self.handle_workspace_menu_key(key),
            // Rename input and dismissal; no key leaks through to the PTY.
            ModalLayer::SessionPanel => self.handle_session_panel_key(key),
            // Both browse mode and the inline form.
            ModalLayer::CommandsPanel => {
                self.handle_commands_panel_key(key);
            }
            ModalLayer::AgentsPanel => {
                self.handle_agents_panel_key(key);
            }
        }
    }

    /// A pressed key's bindings: an application shortcut first, then a
    /// plugin or custom keybinding (SendText, RunCommand, PluginCommand).
    /// Returns true when one handled the key, which then never reaches the
    /// terminal.
    pub(super) fn handle_bound_key(&mut self, key: &winit::keyboard::Key) -> bool {
        if self.run_app_shortcut(key) {
            return true;
        }
        let Some(key_action) = self.keybinding_manager.check(key, self.modifiers).cloned() else {
            return false;
        };
        self.handle_key_action(key_action);
        true
    }

    /// Runs the built-in application shortcut bound to a pressed key.
    /// Returns true when it ran and took the key.
    ///
    /// A surface holding the keyboard (the agent view, the find box, copy
    /// mode, quick select) checks its own chords first and calls this for any
    /// other key, then swallows the key if this returns false. Custom
    /// keybindings never run there: they type into, or run commands in, the
    /// terminal the surface is covering.
    pub(super) fn run_app_shortcut(&mut self, key: &winit::keyboard::Key) -> bool {
        self.shortcut_registry
            .check(key, self.modifiers)
            .is_some_and(|bindable| self.handle_action(bindable.to_action()))
    }

    /// One mouse button press or release: copy-mode exit and terminal
    /// capture first, then the top modal layer, then `handle_mouse_input`.
    fn handle_mouse_button_event(
        &mut self,
        state: winit::event::ElementState,
        button: winit::event::MouseButton,
    ) {
        // Toasts are drawn above every modal layer, so a press on one is
        // theirs alone: nothing beneath sees it, not even copy mode.
        if state == winit::event::ElementState::Pressed
            && let Some(pos) = self.last_cursor_pos
            && self.press_toast(pos.0 as f32, pos.1 as f32)
        {
            return;
        }
        if state == winit::event::ElementState::Pressed && self.copy_mode.is_active() {
            self.leave_copy_mode(true);
        }
        // A modal may open during an application-owned drag. Complete
        // that protocol lifecycle before modal click interception so
        // the child application cannot retain a stuck button.
        if state == winit::event::ElementState::Released
            && self.terminal_mouse_capture.is_some()
            && let Some(pos) = self.last_cursor_pos
        {
            self.handle_mouse_input(button, state, pos);
            if self.terminal_mouse_capture.is_none() {
                return;
            }
        }
        let pressed = state == winit::event::ElementState::Pressed;
        let left = button == winit::event::MouseButton::Left;
        let pos = self.last_cursor_pos;
        match self.top_modal_layer() {
            // Settings, the Workspaces menu, and the side panels take the
            // pointer in `handle_mouse_input`, after the global Settings
            // button, as do the agent view and the terminals with none open.
            // Settings dismisses itself on a click outside it there.
            None
            | Some(
                ModalLayer::Settings
                | ModalLayer::WorkspaceMenu
                | ModalLayer::SessionPanel
                | ModalLayer::CommandsPanel
                | ModalLayer::AgentsPanel,
            ) => {
                if let Some(pos) = pos {
                    self.handle_mouse_input(button, state, pos);
                    self.finish_pointer_chrome_focus(button, state);
                }
            }
            // Every layer above those owns the buttons while on top: it acts
            // on presses of the ones it uses, and nothing beneath sees any.
            Some(_) if !pressed => {}
            Some(ModalLayer::Guardrail) => {
                if left {
                    let request_id = self.guardrail.request_id();
                    let action = if let Some(pos) = pos {
                        self.guardrail_layout()
                            .hit_test(pos.0 as f32, pos.1 as f32, request_id)
                    } else {
                        crate::terminal::guardrails::GuardrailAction::Cancel { request_id }
                    };
                    self.apply_guardrail_action(action);
                    self.request_redraw(RedrawReason::PointerInput);
                }
            }
            // Information overlay pointer actions use the same request and
            // geometry as keyboard, rendering, and accessibility.
            Some(ModalLayer::InfoOverlay) => {
                if matches!(
                    button,
                    winit::event::MouseButton::Left | winit::event::MouseButton::Middle
                ) && let Some(pos) = pos
                {
                    let layout = self.info_overlay_layout();
                    let (mx, my) = (pos.0 as f32, pos.1 as f32);
                    let press = if layout.copy_button.contains(mx, my) {
                        super::info_overlay::InfoOverlayPress::CopyButton
                    } else if layout.continue_button.contains(mx, my) {
                        super::info_overlay::InfoOverlayPress::ContinueButton
                    } else if let Some(line) = layout.content_row_at(
                        mx,
                        my,
                        self.info_overlay.viewport().row(),
                        self.info_overlay.lines().len(),
                    ) {
                        let column = self.info_overlay.viewport().column().saturating_add(
                            ((mx - layout.content.x) / layout.char_scale)
                                .floor()
                                .max(0.0) as usize,
                        );
                        super::info_overlay::InfoOverlayPress::Line { line, column }
                    } else {
                        super::info_overlay::InfoOverlayPress::Elsewhere
                    };
                    self.press_info_overlay(button, press);
                    self.request_redraw(RedrawReason::PointerInput);
                }
            }
            Some(ModalLayer::DiffReview) => {
                if left && let Some(pos) = pos {
                    let action = self.diff_review_layout().hit_test(
                        pos.0 as f32,
                        pos.1 as f32,
                        self.diff_review.request_id(),
                    );
                    self.apply_diff_review_action(action);
                    self.request_redraw(RedrawReason::PointerInput);
                }
            }
            Some(ModalLayer::ContextPicker) => {
                if left && let Some(pos) = pos {
                    let snapshot = self.context_picker.snapshot();
                    let action = self.context_picker_layout().hit_test(
                        pos.0 as f32,
                        pos.1 as f32,
                        &snapshot,
                    );
                    self.apply_context_picker_action(action);
                    self.request_redraw(RedrawReason::PointerInput);
                }
            }
            // An input-blocking context menu: a click on an action runs it,
            // and one outside dismisses the menu.
            Some(ModalLayer::SproutMenu) => {
                if left {
                    let hit = pos.and_then(|pos| {
                        self.sprout_menu_layout()?
                            .hit_test(pos.0 as f32, pos.1 as f32)
                    });
                    let selected = hit.and_then(|index| {
                        let menu = self.harvest.as_ref()?.sprout_menu.as_ref()?;
                        let action = menu.actions.get(index).copied()?;
                        menu.action_enabled(action)
                            .then_some((menu.request_id, action))
                    });
                    if let Some((request_id, action)) = selected {
                        self.execute_sprout_request(request_id, action);
                    } else if hit.is_none()
                        && let Some(request_id) = self.harvest.as_ref().and_then(|harvest| {
                            harvest.sprout_menu.as_ref().map(|menu| menu.request_id)
                        })
                    {
                        self.dismiss_sprout_request(request_id);
                    }
                    self.request_redraw(RedrawReason::PointerInput);
                }
            }
            Some(ModalLayer::Cellar) => {
                if left
                    && let Some(pos) = pos
                    && let Some(layout) = self.cellar_layout()
                {
                    let action = layout.hit_test(pos.0 as f32, pos.1 as f32, true);
                    self.apply_cellar_action(action);
                    self.request_redraw(RedrawReason::PointerInput);
                }
            }
            // Palette-style modals take every click: an entry is chosen,
            // the query takes the caret, and a click outside closes them.
            Some(
                ModalLayer::DomainPicker | ModalLayer::UnicodePicker | ModalLayer::CommandPalette,
            ) => {
                if left {
                    self.handle_palette_modal_press();
                }
            }
            // A click outside the shortcuts panel dismisses it.
            Some(ModalLayer::ShortcutsPanel) => {
                if left {
                    self.press_shortcuts_panel(pos);
                }
            }
        }
    }

    /// A primary press at `pos` while the shortcuts panel is on top.
    fn press_shortcuts_panel(&mut self, pos: Option<(f64, f64)>) {
        if let (Some(pos), Some(renderer)) = (pos, &self.renderer) {
            use crate::ui::shortcuts_panel::ShortcutsPanelLayout;
            let (sw, sh) = renderer.window_size();
            let (cw, ch) = (renderer.cell_width(), renderer.cell_height());
            let info = self.shortcuts_panel.build_info(&self.shortcut_registry);
            let layout = ShortcutsPanelLayout::calculate(
                sw as f32,
                sh as f32,
                cw,
                ch,
                info.animation,
                info.entries.len(),
            );
            let (mx, my) = (pos.0 as f32, pos.1 as f32);
            if !layout.contains(mx, my) {
                self.shortcuts_panel.close();
            } else if !self.handle_shortcuts_filter_click(mx, my, &layout, cw, ch) {
                let action =
                    self.shortcuts_panel
                        .handle_click(mx, my, &layout, &self.shortcut_registry);
                self.apply_shortcuts_action(action);
            }
        } else {
            self.shortcuts_panel.close();
        }
        self.request_redraw(RedrawReason::PointerInput);
    }
}

/// Whole columns and rows a modal surface scrolls for one wheel event,
/// positive toward the end; a pixel delta counts 20 pixels to a line. The
/// terminal's own scrolling follows `scrollback.scroll_speed` instead.
pub(super) fn modal_wheel_steps(delta: winit::event::MouseScrollDelta) -> (isize, isize) {
    let (x, y) = match delta {
        winit::event::MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
        winit::event::MouseScrollDelta::PixelDelta(position) => {
            (position.x / 20.0, position.y / 20.0)
        }
    };
    // A turn away from the person (positive) moves toward the start.
    let steps = |lines: f64| {
        if lines.is_finite() && lines != 0.0 {
            -(lines.signum() * lines.abs().ceil()) as isize
        } else {
            0
        }
    };
    (steps(x), steps(y))
}

impl App {
    /// Scroll whatever owns the wheel: the frontmost modal surface, a
    /// side panel, the agent view, or the terminal.
    pub(super) fn handle_mouse_wheel(&mut self, delta: winit::event::MouseScrollDelta) {
        let (columns, rows) = modal_wheel_steps(delta);
        // Shift turns a vertical wheel sideways, but only in the views that
        // scroll both ways; every other surface keeps scrolling vertically.
        let (two_axis_columns, two_axis_rows) = if self.modifiers.shift_key() && columns == 0 {
            (rows, 0)
        } else {
            (columns, rows)
        };
        let sideways = two_axis_columns.abs() > two_axis_rows.abs();
        match self.top_modal_layer() {
            Some(ModalLayer::InfoOverlay) => {
                use crate::ui::info_overlay::InfoOverlayAction;
                let request_id = self.info_overlay.request_id();
                self.apply_info_overlay_action(if sideways {
                    InfoOverlayAction::Scroll {
                        request_id,
                        by: OverlayScroll::Columns(two_axis_columns),
                    }
                } else {
                    InfoOverlayAction::Scroll {
                        request_id,
                        by: OverlayScroll::Rows(two_axis_rows),
                    }
                });
            }
            Some(ModalLayer::DiffReview) => {
                use crate::ui::diff_review::DiffReviewAction;
                let request_id = self.diff_review.request_id();
                self.apply_diff_review_action(if sideways {
                    DiffReviewAction::Scroll {
                        request_id,
                        by: OverlayScroll::Columns(two_axis_columns),
                    }
                } else {
                    DiffReviewAction::Scroll {
                        request_id,
                        by: OverlayScroll::Rows(two_axis_rows),
                    }
                });
            }
            Some(ModalLayer::ContextPicker) => self.apply_context_picker_action(
                crate::ui::context_picker::ContextPickerAction::Scroll {
                    request_id: self.context_picker.request_id(),
                    delta: rows,
                },
            ),
            Some(ModalLayer::SessionPanel) if rows < 0 => {
                self.session_panel.scroll_up(rows.unsigned_abs());
            }
            Some(ModalLayer::SessionPanel) => self.session_panel.scroll_down(rows.unsigned_abs()),
            Some(ModalLayer::Settings) if rows < 0 => {
                self.settings_panel.scroll_up(rows.unsigned_abs());
            }
            Some(ModalLayer::Settings) => {
                let total = self
                    .settings_panel
                    .build_entries(&self.config, &self.shortcut_registry)
                    .len();
                self.settings_panel.scroll_down(rows.unsigned_abs(), total);
            }
            Some(ModalLayer::ShortcutsPanel) if rows < 0 => {
                self.shortcuts_panel.scroll_up(rows.unsigned_abs());
            }
            Some(ModalLayer::ShortcutsPanel) => self
                .shortcuts_panel
                .scroll_down(rows.unsigned_abs(), &self.shortcut_registry),
            Some(
                ModalLayer::DomainPicker | ModalLayer::UnicodePicker | ModalLayer::CommandPalette,
            ) => {
                if let Some(modal) = self.open_palette_modal() {
                    let palette = self.palette_owner_mut(modal).palette_mut();
                    if rows < 0 {
                        palette.scroll_up(rows.unsigned_abs());
                    } else {
                        palette.scroll_down(rows.unsigned_abs());
                    }
                }
            }
            Some(ModalLayer::Cellar) => {
                if let Some(harvest) = &mut self.harvest {
                    harvest.cellar.scroll_by(rows);
                }
            }
            Some(ModalLayer::CommandsPanel) => self.scroll_commands_panel(rows),
            Some(ModalLayer::AgentsPanel) => self.scroll_agents_panel(rows),
            // Every other layer owns the wheel while on top, even with no
            // scroll action of its own.
            Some(ModalLayer::Guardrail | ModalLayer::SproutMenu | ModalLayer::WorkspaceMenu) => {}
            None => {
                self.scroll_beneath_modal_layers(delta);
                return;
            }
        }
        self.request_redraw(RedrawReason::PointerInput);
    }

    /// The wheel over the agent view or a terminal, once no modal layer owns it.
    fn scroll_beneath_modal_layers(&mut self, delta: winit::event::MouseScrollDelta) {
        if let Some((x, y)) = self.last_cursor_pos
            && self.handle_agent_view_wheel(x as f32, y as f32, delta)
        {
            return;
        }
        let scroll_speed = self.config.scrollback.scroll_speed;
        let (x_delta, y_delta) = match delta {
            winit::event::MouseScrollDelta::LineDelta(x, y) => {
                (x as f64 * scroll_speed, y as f64 * scroll_speed)
            }
            winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x / 10.0, pos.y / 10.0),
        };
        if x_delta.abs() > y_delta.abs() {
            self.handle_horizontal_scroll(x_delta);
        } else {
            self.handle_scroll(y_delta);
        }
    }
}

fn quick_access_geometry(
    monitor_position: PhysicalPosition<i32>,
    monitor_size: PhysicalSize<u32>,
    requested: Option<(u32, u32)>,
) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    let default_width = (u64::from(monitor_size.width) * 4 / 5) as u32;
    let default_height = (u64::from(monitor_size.height) * 9 / 20) as u32;
    let (requested_width, requested_height) = requested.unwrap_or((default_width, default_height));
    let width = requested_width.clamp(1, monitor_size.width.max(1));
    let height = requested_height.clamp(1, monitor_size.height.max(1));
    let horizontal_margin = monitor_size.width.saturating_sub(width) / 2;
    let x = i64::from(monitor_position.x) + i64::from(horizontal_margin);
    (
        PhysicalPosition::new(
            x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            monitor_position.y,
        ),
        PhysicalSize::new(width, height),
    )
}

impl Drop for App {
    fn drop(&mut self) {
        self.live_domain_topology_alive
            .store(false, std::sync::atomic::Ordering::Release);
        if let Some(active) = &self.active_file_upload {
            active
                .cancellation
                .store(true, std::sync::atomic::Ordering::Release);
        }
        self.shutdown_kitty_file_transfers();
        // Save the complete workspace before PTYs and renderer are dropped.
        self.auto_save_workspace();

        // Drop order is critical: renderer holds a wgpu Surface that references
        // the window, so we must drop resources in the correct order:
        // 1. Tabs (PTY processes) first
        // 2. Renderer (GPU resources / Surface) second
        // 3. Window last (implicit, after this Drop returns)
        self.tabs.take();
        self.renderer.take();
        self.accessibility_adapter.take();
        log::debug!(
            "Redraw scheduler: {}",
            self.redraw_scheduler.diagnostic_summary()
        );
        log::debug!("App resources cleaned up");
    }
}

#[cfg(test)]
mod quick_access_tests {
    use super::*;

    #[test]
    fn geometry_is_top_centered_monitor_relative_and_clamped() {
        let (position, size) = quick_access_geometry(
            PhysicalPosition::new(-1_920, 24),
            PhysicalSize::new(1_920, 1_080),
            None,
        );
        assert_eq!(position, PhysicalPosition::new(-1_728, 24));
        assert_eq!(size, PhysicalSize::new(1_536, 486));

        let (position, size) = quick_access_geometry(
            PhysicalPosition::new(-1_920, 24),
            PhysicalSize::new(1_920, 1_080),
            Some((4_000, 2_000)),
        );
        assert_eq!(position, PhysicalPosition::new(-1_920, 24));
        assert_eq!(size, PhysicalSize::new(1_920, 1_080));
    }
}

#[cfg(test)]
mod app_id_tests {
    use crate::app::{APP_ID, QUICK_ACCESS_APP_ID};

    /// Desktops give a window the icon of the installed entry whose
    /// `StartupWMClass` (and, on Wayland, file name) is the window's app ID.
    #[test]
    fn desktop_entries_name_the_window_identities() {
        for (app_id, entry) in [
            (
                APP_ID,
                include_str!("../../../assets/org.termeric.Termeric.desktop"),
            ),
            (
                QUICK_ACCESS_APP_ID,
                include_str!("../../../assets/org.termeric.Termeric.QuickAccess.desktop"),
            ),
        ] {
            let lines: Vec<&str> = entry.lines().collect();
            assert!(lines.contains(&format!("StartupWMClass={app_id}").as_str()));
            assert!(lines.contains(&format!("Icon={APP_ID}").as_str()));
        }
    }
}

#[cfg(test)]
mod wheel_tests {
    use super::*;
    use crate::config::Config;
    use winit::event::MouseScrollDelta;

    #[test]
    fn the_guardrail_keeps_the_wheel_from_scrolling_surfaces_beneath_it() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let lines = (0..100).map(|line| format!("line {line}")).collect();
        app.info_overlay.show("Long notice", lines);
        app.info_overlay.set_viewport(5, 80);
        app.handle_mouse_wheel(MouseScrollDelta::LineDelta(0.0, -3.0));
        assert_eq!(app.info_overlay.viewport().row(), 3);

        app.guardrail.show(
            crate::terminal::guardrails::GuardrailMatch {
                command: "rm -rf /".into(),
                reason: "fixture".into(),
                severity: crate::terminal::guardrails::Severity::Critical,
            },
            "GUI tab 1, pane 0".into(),
            vec![crate::terminal::guardrails::GuardedWrite::for_pane(
                1,
                crate::panes::PaneId(0),
                b"x".to_vec(),
            )],
        );
        app.handle_mouse_wheel(MouseScrollDelta::LineDelta(0.0, -3.0));
        assert_eq!(app.info_overlay.viewport().row(), 3);
    }

    #[test]
    fn leaving_settings_for_shortcuts_hands_the_wheel_over_at_once() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        app.apply_settings_action(crate::ui::settings::SettingsAction::OpenKeyboardShortcuts);
        // Even while it fades out, Settings no longer takes input.
        assert!(!app.settings_panel.is_active());
        let offset = |app: &App| {
            app.shortcuts_panel
                .build_info(&app.shortcut_registry)
                .scroll_offset
        };
        let before = offset(&app);
        app.handle_mouse_wheel(MouseScrollDelta::LineDelta(0.0, -3.0));
        assert!(offset(&app) > before);
    }

    #[test]
    fn shift_turns_the_wheel_sideways_only_in_views_that_scroll_both_ways() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.modifiers = winit::keyboard::ModifiersState::SHIFT;

        // Settings scrolls only vertically, so Shift leaves the wheel alone.
        app.settings_panel.toggle();
        let settings_offset = |app: &mut App| {
            app.settings_panel
                .build_info(&app.config, &app.shortcut_registry)
                .scroll_offset
        };
        assert_eq!(settings_offset(&mut app), 0);
        app.handle_mouse_wheel(MouseScrollDelta::LineDelta(0.0, -3.0));
        assert!(settings_offset(&mut app) > 0);

        // The info overlay, above Settings, scrolls both ways, so Shift
        // turns the same wheel sideways there.
        let lines = (0..100)
            .map(|line| format!("line {line} {}", "wide ".repeat(60)))
            .collect();
        app.info_overlay.show("Wide notice", lines);
        app.info_overlay.set_viewport(5, 80);
        app.handle_mouse_wheel(MouseScrollDelta::LineDelta(0.0, -3.0));
        assert_eq!(app.info_overlay.viewport().row(), 0);
        assert_eq!(app.info_overlay.viewport().column(), 3);
    }
}
