//! winit event loop integration
//!
//! Implements `ApplicationHandler` for the App, handling window events.

use std::sync::Arc;
use std::time::Instant;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow},
    window::{Window, WindowId},
};

use crate::input::keybindings::KeyChord;
use crate::ui::settings::SettingsKey;

use super::redraw_scheduler::RedrawReason;
use super::{App, AppEvent, clear_pty_wake_pending, clear_transcript_wake_pending};

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
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
            .with_title(if self.launch.quick_access {
                "Termeric Quick Access"
            } else {
                "Termeric"
            })
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

        #[cfg(target_os = "linux")]
        {
            use winit::platform::startup_notify::{
                EventLoopExtStartupNotify, WindowAttributesExtStartupNotify,
            };
            let app_id = self
                .launch
                .app_id
                .as_deref()
                .unwrap_or(if self.launch.quick_access {
                    "termeric-quick-access"
                } else {
                    "termeric"
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
            if let Some(token) = event_loop.read_token_from_env() {
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
                // Winit leaves IME disabled by default. This also enables
                // native dead-key composition on platforms such as macOS.
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
                    let error = format!("Failed to initialize renderer: {e:#}");
                    if self.redact_startup_error_log {
                        log::error!("Private GUI-control window renderer initialization failed");
                    } else {
                        log::error!("{error}");
                    }
                    self.startup_error = Some(error);
                    self.should_exit = true;
                    if !self.managed_window {
                        event_loop.exit();
                    }
                    return;
                }

                // Initialize tabs with first tab
                if let Err(e) = self.init_tabs() {
                    let error = format!("Failed to initialize tabs: {e:#}");
                    if self.redact_startup_error_log {
                        log::error!("Private GUI-control window tab initialization failed");
                    } else {
                        log::error!("{error}");
                    }
                    self.startup_error = Some(error);
                    self.should_exit = true;
                    if !self.managed_window {
                        event_loop.exit();
                    }
                    return;
                }

                window.set_visible(true);
                if self.launch.quick_access {
                    window.focus_window();
                }

                // Initial render
                self.redraw_scheduler.request(RedrawReason::Startup);
                let reasons = self.redraw_scheduler.begin_frame();
                log::trace!("Rendering frame: reasons={reasons}");
                let _ = self.render();
                self.redraw_scheduler.finish_frame();
                self.native_frame_stats
                    .record(self.redraw_scheduler.rendered_frames());
            }
            Err(e) => {
                let error = format!("Failed to create window: {e:#}");
                if self.redact_startup_error_log {
                    log::error!("Private GUI-control native window creation failed");
                } else {
                    log::error!("{error}");
                }
                self.startup_error = Some(error);
                self.should_exit = true;
                if !self.managed_window {
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
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
                if !self.managed_window {
                    event_loop.exit();
                }
            }

            WindowEvent::Resized(size) => {
                let recovering_surface = self.surface_lost;
                self.handle_resize(size.width, size.height);
                self.surface_lost = false;
                self.surface_lost_at = None;
                if recovering_surface {
                    self.redraw_scheduler.request(RedrawReason::SurfaceRecovery);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
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
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
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

            WindowEvent::KeyboardInput { event, .. } => {
                if matches!(
                    event.logical_key,
                    winit::keyboard::Key::Named(winit::keyboard::NamedKey::AltGraph)
                ) {
                    self.alt_graph_active = event.state == winit::event::ElementState::Pressed;
                }

                // Handle guardrail confirmation (highest priority)
                if self.guardrail.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        let request_id = self.guardrail.request_id();
                        let action = match &event.logical_key {
                            Key::Named(NamedKey::Enter) if self.modifiers.control_key() => {
                                crate::terminal::guardrails::GuardrailAction::Confirm { request_id }
                            }
                            Key::Named(NamedKey::Enter | NamedKey::Escape) => {
                                crate::terminal::guardrails::GuardrailAction::Cancel { request_id }
                            }
                            _ => crate::terminal::guardrails::GuardrailAction::None,
                        };
                        self.apply_guardrail_action(action);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // Information overlays own all input until their close
                // animation completes. Actions are request-scoped so input
                // during exit cannot target a replacement notice.
                if self.info_overlay.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use crate::ui::info_overlay::InfoOverlayAction;
                        use winit::keyboard::{Key, NamedKey};
                        let request_id = self.info_overlay.request_id();
                        let action = if event.logical_key == Key::Named(NamedKey::Tab) {
                            self.ime.clear_preedit();
                            self.info_overlay
                                .cycle_focus(request_id, self.modifiers.shift_key());
                            InfoOverlayAction::None
                        } else if event.logical_key == Key::Named(NamedKey::Enter) {
                            if event.repeat {
                                InfoOverlayAction::None
                            } else {
                                self.info_overlay.activate_focused(request_id)
                            }
                        } else if self.info_overlay.input_focused() {
                            match &event.logical_key {
                                Key::Named(NamedKey::Escape) => {
                                    InfoOverlayAction::CancelInput { request_id }
                                }
                                Key::Named(NamedKey::Backspace) => {
                                    self.info_overlay.backspace_input(request_id);
                                    InfoOverlayAction::None
                                }
                                Key::Named(NamedKey::Delete) => {
                                    self.info_overlay.delete_input(request_id);
                                    InfoOverlayAction::None
                                }
                                Key::Named(NamedKey::ArrowLeft) => {
                                    self.info_overlay.move_input_left(request_id);
                                    InfoOverlayAction::None
                                }
                                Key::Named(NamedKey::ArrowRight) => {
                                    self.info_overlay.move_input_right(request_id);
                                    InfoOverlayAction::None
                                }
                                Key::Named(NamedKey::Home) => {
                                    self.info_overlay.move_input_home(request_id);
                                    InfoOverlayAction::None
                                }
                                Key::Named(NamedKey::End) => {
                                    self.info_overlay.move_input_end(request_id);
                                    InfoOverlayAction::None
                                }
                                Key::Named(NamedKey::ArrowUp) => InfoOverlayAction::ScrollLines {
                                    request_id,
                                    delta: -1,
                                },
                                Key::Named(NamedKey::ArrowDown) => InfoOverlayAction::ScrollLines {
                                    request_id,
                                    delta: 1,
                                },
                                Key::Named(NamedKey::PageUp) => InfoOverlayAction::ScrollLines {
                                    request_id,
                                    delta: -(self.info_overlay.viewport_rows() as isize),
                                },
                                Key::Named(NamedKey::PageDown) => InfoOverlayAction::ScrollLines {
                                    request_id,
                                    delta: self.info_overlay.viewport_rows() as isize,
                                },
                                Key::Character(character)
                                    if self.modifiers.control_key()
                                        && character.as_str().eq_ignore_ascii_case("c") =>
                                {
                                    InfoOverlayAction::Copy { request_id }
                                }
                                Key::Character(character)
                                    if self.modifiers.control_key()
                                        && character.as_str().eq_ignore_ascii_case("d") =>
                                {
                                    self.info_overlay.choice(request_id, "unset").map_or(
                                        InfoOverlayAction::None,
                                        |_| InfoOverlayAction::Choose {
                                            request_id,
                                            choice_id: "unset".into(),
                                        },
                                    )
                                }
                                Key::Character(character)
                                    if !self.modifiers.control_key()
                                        && !self.modifiers.super_key()
                                        && !self.modifiers.alt_key() =>
                                {
                                    self.info_overlay
                                        .insert_input_text(request_id, character.as_str());
                                    InfoOverlayAction::None
                                }
                                _ => InfoOverlayAction::None,
                            }
                        } else {
                            match &event.logical_key {
                                Key::Named(NamedKey::Escape) => {
                                    if self.info_overlay.has_input() {
                                        InfoOverlayAction::CancelInput { request_id }
                                    } else {
                                        InfoOverlayAction::Dismiss { request_id }
                                    }
                                }
                                Key::Named(NamedKey::ArrowUp) => InfoOverlayAction::ScrollLines {
                                    request_id,
                                    delta: -1,
                                },
                                Key::Named(NamedKey::ArrowDown) => InfoOverlayAction::ScrollLines {
                                    request_id,
                                    delta: 1,
                                },
                                Key::Named(NamedKey::PageUp) => InfoOverlayAction::ScrollLines {
                                    request_id,
                                    delta: -(self.info_overlay.viewport_rows() as isize),
                                },
                                Key::Named(NamedKey::PageDown) => InfoOverlayAction::ScrollLines {
                                    request_id,
                                    delta: self.info_overlay.viewport_rows() as isize,
                                },
                                Key::Named(NamedKey::ArrowLeft) => {
                                    InfoOverlayAction::ScrollColumns {
                                        request_id,
                                        delta: -4,
                                    }
                                }
                                Key::Named(NamedKey::ArrowRight) => {
                                    InfoOverlayAction::ScrollColumns {
                                        request_id,
                                        delta: 4,
                                    }
                                }
                                Key::Named(NamedKey::Home) if self.modifiers.control_key() => {
                                    InfoOverlayAction::ScrollToTop { request_id }
                                }
                                Key::Named(NamedKey::End) if self.modifiers.control_key() => {
                                    InfoOverlayAction::ScrollToBottom { request_id }
                                }
                                Key::Named(NamedKey::Home) => {
                                    InfoOverlayAction::ScrollToLeft { request_id }
                                }
                                Key::Named(NamedKey::End) => {
                                    InfoOverlayAction::ScrollToRight { request_id }
                                }
                                Key::Character(character)
                                    if character.as_str().eq_ignore_ascii_case("c") =>
                                {
                                    InfoOverlayAction::Copy { request_id }
                                }
                                Key::Character(character) => character
                                    .as_str()
                                    .chars()
                                    .next()
                                    .and_then(|shortcut| {
                                        self.info_overlay.choice_for_shortcut(request_id, shortcut)
                                    })
                                    .map_or(InfoOverlayAction::None, |choice| {
                                        InfoOverlayAction::Choose {
                                            request_id,
                                            choice_id: choice.id.clone(),
                                        }
                                    }),
                                _ => InfoOverlayAction::None,
                            }
                        };
                        self.apply_info_overlay_action(action);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // Handle diff review overlay input
                if self.diff_review.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use crate::ui::diff_review::DiffReviewAction;
                        use winit::keyboard::{Key, NamedKey};
                        let request_id = self.diff_review.request_id();
                        let action = match &event.logical_key {
                            Key::Named(NamedKey::Enter | NamedKey::Escape) => {
                                DiffReviewAction::Dismiss { request_id }
                            }
                            Key::Named(NamedKey::ArrowUp) => DiffReviewAction::ScrollRows {
                                request_id,
                                delta: -1,
                            },
                            Key::Named(NamedKey::ArrowDown) => DiffReviewAction::ScrollRows {
                                request_id,
                                delta: 1,
                            },
                            Key::Character(key) if key.as_str().eq_ignore_ascii_case("k") => {
                                DiffReviewAction::ScrollRows {
                                    request_id,
                                    delta: -3,
                                }
                            }
                            Key::Character(key) if key.as_str().eq_ignore_ascii_case("j") => {
                                DiffReviewAction::ScrollRows {
                                    request_id,
                                    delta: 3,
                                }
                            }
                            Key::Named(NamedKey::PageUp) => DiffReviewAction::ScrollRows {
                                request_id,
                                delta: -(self.diff_review.viewport_rows() as isize),
                            },
                            Key::Named(NamedKey::PageDown) => DiffReviewAction::ScrollRows {
                                request_id,
                                delta: self.diff_review.viewport_rows() as isize,
                            },
                            Key::Named(NamedKey::ArrowLeft) => DiffReviewAction::ScrollColumns {
                                request_id,
                                delta: -4,
                            },
                            Key::Named(NamedKey::ArrowRight) => DiffReviewAction::ScrollColumns {
                                request_id,
                                delta: 4,
                            },
                            Key::Named(NamedKey::Home) if self.modifiers.control_key() => {
                                DiffReviewAction::ScrollToTop { request_id }
                            }
                            Key::Named(NamedKey::End) if self.modifiers.control_key() => {
                                DiffReviewAction::ScrollToBottom { request_id }
                            }
                            Key::Named(NamedKey::Home) => {
                                DiffReviewAction::ScrollToLeft { request_id }
                            }
                            Key::Named(NamedKey::End) => {
                                DiffReviewAction::ScrollToRight { request_id }
                            }
                            Key::Character(key) if key.as_str().eq_ignore_ascii_case("c") => {
                                DiffReviewAction::Copy { request_id }
                            }
                            _ => DiffReviewAction::None,
                        };
                        self.apply_diff_review_action(action);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // Handle context picker input
                if self.context_picker.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use crate::ui::context_picker::{ContextPickerAction, ContextPickerPhase};
                        use winit::keyboard::{Key, NamedKey};
                        let request_id = self.context_picker.request_id();
                        let action = match (self.context_picker.phase(), &event.logical_key) {
                            (_, Key::Named(NamedKey::Escape)) => {
                                ContextPickerAction::Cancel { request_id }
                            }
                            (_, Key::Named(NamedKey::Enter)) => {
                                ContextPickerAction::Primary { request_id }
                            }
                            (ContextPickerPhase::Sources, Key::Named(NamedKey::ArrowUp)) => {
                                ContextPickerAction::Move {
                                    request_id,
                                    delta: -1,
                                }
                            }
                            (ContextPickerPhase::Sources, Key::Named(NamedKey::ArrowDown)) => {
                                ContextPickerAction::Move {
                                    request_id,
                                    delta: 1,
                                }
                            }
                            (ContextPickerPhase::Sources, Key::Named(NamedKey::PageUp)) => {
                                ContextPickerAction::Move {
                                    request_id,
                                    delta: -(self.context_picker.viewport_rows() as isize),
                                }
                            }
                            (ContextPickerPhase::Sources, Key::Named(NamedKey::PageDown)) => {
                                ContextPickerAction::Move {
                                    request_id,
                                    delta: self.context_picker.viewport_rows() as isize,
                                }
                            }
                            (ContextPickerPhase::Sources, Key::Named(NamedKey::Space)) => self
                                .context_picker
                                .selected_source()
                                .map_or(ContextPickerAction::None, |source| {
                                    ContextPickerAction::Toggle { request_id, source }
                                }),
                            (ContextPickerPhase::Sources, Key::Character(key))
                                if key.as_str().eq_ignore_ascii_case("e") =>
                            {
                                ContextPickerAction::EditFiles { request_id }
                            }
                            (ContextPickerPhase::FilePaths, Key::Named(NamedKey::Backspace)) => {
                                ContextPickerAction::BackspaceFileInput { request_id }
                            }
                            (ContextPickerPhase::FilePaths, Key::Character(text)) => text
                                .chars()
                                .next()
                                .map_or(ContextPickerAction::None, |character| {
                                    ContextPickerAction::AppendFileInput {
                                        request_id,
                                        character,
                                    }
                                }),
                            _ => ContextPickerAction::None,
                        };
                        self.apply_context_picker_action(action);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // Harvest menus own keyboard input while logically open.
                if let Some(request_id) = self
                    .harvest
                    .as_ref()
                    .and_then(|harvest| harvest.sprout_menu.as_ref().map(|menu| menu.request_id))
                {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                self.dismiss_sprout_request(request_id);
                            }
                            Key::Named(NamedKey::ArrowUp) => {
                                if let Some(menu) = self
                                    .harvest
                                    .as_mut()
                                    .and_then(|harvest| harvest.sprout_menu.as_mut())
                                {
                                    menu.select_up();
                                }
                            }
                            Key::Named(NamedKey::ArrowDown) => {
                                if let Some(menu) = self
                                    .harvest
                                    .as_mut()
                                    .and_then(|harvest| harvest.sprout_menu.as_mut())
                                {
                                    menu.select_down();
                                }
                            }
                            Key::Named(NamedKey::Home) => {
                                if let Some(menu) = self
                                    .harvest
                                    .as_mut()
                                    .and_then(|harvest| harvest.sprout_menu.as_mut())
                                {
                                    menu.select_first();
                                }
                            }
                            Key::Named(NamedKey::End) => {
                                if let Some(menu) = self
                                    .harvest
                                    .as_mut()
                                    .and_then(|harvest| harvest.sprout_menu.as_mut())
                                {
                                    menu.select_last();
                                }
                            }
                            Key::Named(NamedKey::Enter) => {
                                let action = self
                                    .harvest
                                    .as_ref()
                                    .and_then(|harvest| harvest.sprout_menu.as_ref())
                                    .and_then(|menu| menu.selected_action().copied());
                                if let Some(action) = action {
                                    self.execute_sprout_request(request_id, action);
                                }
                            }
                            _ => {}
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                if self
                    .harvest
                    .as_ref()
                    .is_some_and(|harvest| harvest.cellar.is_expanded())
                {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        let activate = match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                if let Some(harvest) = &mut self.harvest {
                                    harvest.cellar.close();
                                }
                                None
                            }
                            Key::Named(NamedKey::ArrowUp) => {
                                if let Some(harvest) = &mut self.harvest {
                                    harvest.cellar.select_up();
                                }
                                None
                            }
                            Key::Named(NamedKey::ArrowDown) => {
                                if let Some(harvest) = &mut self.harvest {
                                    harvest.cellar.select_down();
                                }
                                None
                            }
                            Key::Named(NamedKey::PageUp) => {
                                if let Some(harvest) = &mut self.harvest {
                                    harvest.cellar.select_page(-1);
                                }
                                None
                            }
                            Key::Named(NamedKey::PageDown) => {
                                if let Some(harvest) = &mut self.harvest {
                                    harvest.cellar.select_page(1);
                                }
                                None
                            }
                            Key::Named(NamedKey::Home) => {
                                if let Some(harvest) = &mut self.harvest {
                                    harvest.cellar.select_first();
                                }
                                None
                            }
                            Key::Named(NamedKey::End) => {
                                if let Some(harvest) = &mut self.harvest {
                                    harvest.cellar.select_last();
                                }
                                None
                            }
                            Key::Named(NamedKey::Enter) => self
                                .harvest
                                .as_ref()
                                .and_then(|harvest| harvest.cellar.selected_id()),
                            _ => None,
                        };
                        if let Some(id) = activate {
                            self.apply_cellar_action(crate::harvest::CellarAction::Activate(id));
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // The live-domain picker reuses palette interaction and
                // geometry but owns a separate staged discovery state.
                if self.domain_picker.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        let command = match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                self.domain_picker.back_or_close();
                                None
                            }
                            Key::Named(NamedKey::ArrowUp) => {
                                self.domain_picker.select_prev();
                                None
                            }
                            Key::Named(NamedKey::ArrowDown) => {
                                self.domain_picker.select_next();
                                None
                            }
                            Key::Named(NamedKey::Enter) => self.domain_picker.activate(),
                            Key::Named(NamedKey::Backspace) => {
                                self.domain_picker.handle_backspace();
                                None
                            }
                            Key::Named(NamedKey::Space) => {
                                self.domain_picker.handle_char(' ');
                                None
                            }
                            Key::Character(c)
                                if self.modifiers.control_key()
                                    && self.modifiers.shift_key()
                                    && (c.as_str() == "p" || c.as_str() == "P") =>
                            {
                                self.domain_picker.close();
                                None
                            }
                            Key::Character(c)
                                if !self.modifiers.control_key() && !self.modifiers.alt_key() =>
                            {
                                for character in c.chars() {
                                    self.domain_picker.handle_char(character);
                                }
                                None
                            }
                            _ => None,
                        };
                        if let Some(command) = command {
                            self.handle_domain_picker_command(command);
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                if self.unicode_picker.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        let selection = match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                self.close_unicode_picker();
                                None
                            }
                            Key::Named(NamedKey::ArrowUp) => {
                                self.unicode_picker.select_prev();
                                None
                            }
                            Key::Named(NamedKey::ArrowDown) => {
                                self.unicode_picker.select_next();
                                None
                            }
                            Key::Named(NamedKey::Enter) => self.unicode_picker.activate(),
                            Key::Named(NamedKey::Backspace) => {
                                self.unicode_picker.handle_backspace();
                                None
                            }
                            Key::Named(NamedKey::Space) => {
                                self.unicode_picker.handle_char(' ');
                                None
                            }
                            Key::Character(value)
                                if !self.modifiers.control_key() && !self.modifiers.alt_key() =>
                            {
                                for character in value.chars() {
                                    self.unicode_picker.handle_char(character);
                                }
                                None
                            }
                            _ => None,
                        };
                        if let Some(selection) = selection {
                            self.apply_unicode_picker_selection(selection);
                        } else if !self.unicode_picker.is_active() {
                            self.pending_unicode_picker = None;
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // Handle command palette input first (highest priority when open)
                if self.command_palette.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                self.command_palette.close();
                            }
                            Key::Named(NamedKey::ArrowUp) => {
                                self.command_palette.select_prev();
                            }
                            Key::Named(NamedKey::ArrowDown) => {
                                self.command_palette.select_next();
                            }
                            Key::Named(NamedKey::Enter) => {
                                let result = self.command_palette.execute();
                                self.handle_palette_result(result);
                            }
                            Key::Named(NamedKey::Backspace) => {
                                self.command_palette.handle_backspace();
                            }
                            Key::Named(NamedKey::Space) => {
                                self.command_palette.handle_char(' ');
                            }
                            Key::Character(c) => {
                                // Ctrl+Shift+P to close (toggle)
                                if self.modifiers.control_key()
                                    && self.modifiers.shift_key()
                                    && (c.as_str() == "p" || c.as_str() == "P")
                                {
                                    self.command_palette.close();
                                } else if !self.modifiers.control_key() && !self.modifiers.alt_key()
                                {
                                    for ch in c.chars() {
                                        self.command_palette.handle_char(ch);
                                    }
                                }
                            }
                            _ => {}
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // Handle shortcuts panel input (centered modal)
                if self.shortcuts_panel.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use crate::ui::shortcuts_panel::ShortcutsKey;
                        use winit::keyboard::{Key, NamedKey};

                        // In recording mode, capture the raw key
                        if self.shortcuts_panel.is_recording() {
                            let shortcuts_key = match &event.logical_key {
                                Key::Named(NamedKey::Escape) => Some(ShortcutsKey::Escape),
                                Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
                                    Some(ShortcutsKey::Delete)
                                }
                                _ => match &event.logical_key {
                                    Key::Named(
                                        NamedKey::Control
                                        | NamedKey::Shift
                                        | NamedKey::Alt
                                        | NamedKey::Super,
                                    ) => None,
                                    _ => {
                                        KeyChord::from_key_event(&event.logical_key, self.modifiers)
                                            .map(ShortcutsKey::RecordedChord)
                                    }
                                },
                            };
                            if let Some(key) = shortcuts_key {
                                let action = self
                                    .shortcuts_panel
                                    .handle_key(key, &self.shortcut_registry);
                                self.apply_shortcuts_action(action);
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                            }
                            return;
                        }

                        let shortcuts_key = match &event.logical_key {
                            Key::Named(NamedKey::Escape) => Some(ShortcutsKey::Escape),
                            Key::Named(NamedKey::ArrowUp) => Some(ShortcutsKey::Up),
                            Key::Named(NamedKey::ArrowDown) => Some(ShortcutsKey::Down),
                            Key::Named(NamedKey::Enter) => Some(ShortcutsKey::Enter),
                            Key::Named(NamedKey::Backspace) => Some(ShortcutsKey::Backspace),
                            Key::Named(NamedKey::Delete) => Some(ShortcutsKey::Delete),
                            Key::Character(c)
                                if !self.modifiers.control_key() && !self.modifiers.alt_key() =>
                            {
                                c.chars().next().map(ShortcutsKey::Char)
                            }
                            _ => None,
                        };

                        if let Some(key) = shortcuts_key {
                            let action = self
                                .shortcuts_panel
                                .handle_key(key, &self.shortcut_registry);
                            self.apply_shortcuts_action(action);
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // Handle settings panel input (highest priority when open)
                if self.settings_panel.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};

                        // The same shortcut opens and closes Settings. Check it
                        // before treating character keys as filter input.
                        if self
                            .shortcut_registry
                            .check(&event.logical_key, self.modifiers)
                            == Some(crate::input::keybindings::BindableAction::ToggleSettings)
                        {
                            self.toggle_settings_surface();
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }

                        let settings_key = match &event.logical_key {
                            Key::Named(NamedKey::Escape) => Some(SettingsKey::Escape),
                            Key::Named(NamedKey::ArrowUp) => Some(SettingsKey::Up),
                            Key::Named(NamedKey::ArrowDown) => Some(SettingsKey::Down),
                            Key::Named(NamedKey::ArrowLeft) => Some(SettingsKey::Left),
                            Key::Named(NamedKey::ArrowRight) => Some(SettingsKey::Right),
                            Key::Named(NamedKey::Enter) => Some(SettingsKey::Enter),
                            Key::Named(NamedKey::Tab) => Some(SettingsKey::Tab),
                            Key::Named(NamedKey::Backspace) => Some(SettingsKey::FilterBackspace),
                            Key::Named(NamedKey::Delete) => Some(SettingsKey::Delete),
                            Key::Character(c) if self.modifiers.control_key() => match c.as_str() {
                                "s" | "S" => Some(SettingsKey::Save),
                                "r" | "R" => Some(SettingsKey::Reset),
                                _ => None,
                            },
                            Key::Character(c)
                                if !self.modifiers.control_key() && !self.modifiers.alt_key() =>
                            {
                                c.chars().next().map(SettingsKey::FilterChar)
                            }
                            _ => None,
                        };

                        if let Some(key) = settings_key {
                            let action = self.settings_panel.handle_key(
                                key,
                                &self.config,
                                &self.shortcut_registry,
                            );
                            self.apply_settings_action(action);
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                    }
                    return;
                }

                // The workspace manager is a modal side panel. Rename input
                // and dismissal are handled here, and no keystroke may leak
                // through to the focused PTY while it is open.
                if self.session_panel.is_open() {
                    if event.state == winit::event::ElementState::Pressed {
                        use crate::session::panel::SessionPanelKey;
                        use winit::keyboard::{Key, NamedKey};
                        match &event.logical_key {
                            Key::Named(NamedKey::Backspace) if self.session_panel.is_renaming() => {
                                self.session_panel.rename_backspace();
                            }
                            Key::Named(NamedKey::Space) if self.session_panel.is_renaming() => {
                                self.session_panel.rename_char(' ');
                            }
                            Key::Character(text)
                                if self.session_panel.is_renaming()
                                    && !self.modifiers.control_key()
                                    && !self.modifiers.alt_key() =>
                            {
                                for character in text.chars() {
                                    self.session_panel.rename_char(character);
                                }
                            }
                            key => {
                                let panel_key = match key {
                                    Key::Named(NamedKey::Escape) => Some(SessionPanelKey::Escape),
                                    Key::Named(NamedKey::ArrowUp) => Some(SessionPanelKey::Up),
                                    Key::Named(NamedKey::ArrowDown) => Some(SessionPanelKey::Down),
                                    Key::Named(NamedKey::Enter) => Some(SessionPanelKey::Enter),
                                    Key::Named(NamedKey::Delete) => Some(SessionPanelKey::Delete),
                                    Key::Named(NamedKey::F2) => Some(SessionPanelKey::Rename),
                                    _ => None,
                                };
                                if let Some(key) = panel_key {
                                    let action = self.session_panel.handle_key(key);
                                    self.apply_session_panel_action(action);
                                }
                            }
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // Handle commands panel inline form input
                if self.commands_panel.is_open() && self.commands_panel.is_editing() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                self.commands_panel.cancel_edit();
                            }
                            Key::Named(NamedKey::Tab) => {
                                self.commands_panel.next_field();
                            }
                            Key::Named(NamedKey::Enter) => {
                                self.apply_commands_panel_action(
                                    crate::ui::commands::panel::CommandsPanelAction::Submit,
                                );
                            }
                            Key::Named(NamedKey::Backspace) => {
                                self.commands_panel.form_backspace();
                            }
                            Key::Named(NamedKey::Space) => {
                                self.commands_panel.form_char(' ');
                            }
                            Key::Character(c) => {
                                for ch in c.chars() {
                                    self.commands_panel.form_char(ch);
                                }
                            }
                            _ => {}
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // Handle commands panel Escape to close
                if self.commands_panel.is_open() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        if let Key::Named(NamedKey::Escape) = &event.logical_key {
                            self.commands_panel.close();
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }
                    }
                }

                if self.handle_tab_rename_input(&event) {
                    return;
                }

                // Handle search mode input
                if self.handle_search_mode_input(&event) {
                    return;
                }

                if self.handle_copy_mode_input(&event) {
                    return;
                }

                if self.cancel_broadcast_on_escape(&event) {
                    return;
                }

                // F6 enters the tab-bar focus ring. While it is active, all
                // navigation and activation keys stay in application chrome.
                if self.handle_chrome_keyboard(&event) {
                    return;
                }

                // Handle git completion navigation (Tab to accept, Up/Down to navigate, Escape to dismiss)
                if self.git_completions_visible
                    && event.state == winit::event::ElementState::Pressed
                {
                    if self.focused_pane_read_only() {
                        self.clear_terminal_input_tracking();
                        return;
                    }
                    use winit::keyboard::Key;
                    match event.logical_key.as_ref() {
                        Key::Named(winit::keyboard::NamedKey::Tab) => {
                            // Let Shift+Tab pass through to the PTY
                            // (e.g., for Claude Code mode switching)
                            if !self.modifiers.shift_key() {
                                // Accept the selected completion
                                if let Some(idx) = self.git_completion_index {
                                    if let Some(branch) = self.git_completions.get(idx).cloned() {
                                        if let Some(partial) = crate::git::detect_git_completion(
                                            &self.input_line_buffer,
                                        ) {
                                            let partial_len = partial.len();
                                            if let Some(tabs) = &mut self.tabs {
                                                if let Some(tab) = tabs.active_tab_mut() {
                                                    // Erase partial text with backspaces
                                                    for _ in 0..partial_len {
                                                        let _ = tab.write(b"\x7f");
                                                    }
                                                    // Insert branch name
                                                    let _ = tab.write(branch.as_bytes());
                                                }
                                            }
                                            // Update input buffer to reflect the accepted completion
                                            let buf = self.input_line_buffer.clone();
                                            if let Some(last_space) = buf.rfind(' ') {
                                                self.input_line_buffer.truncate(last_space + 1);
                                                self.input_line_buffer.push_str(&branch);
                                            }
                                        }
                                    }
                                }
                                self.git_completions_visible = false;
                                self.git_completions.clear();
                                self.git_completion_index = None;
                                return;
                            }
                        }
                        Key::Named(winit::keyboard::NamedKey::ArrowDown) => {
                            if let Some(ref mut idx) = self.git_completion_index {
                                let max = self.git_completions.len().saturating_sub(1);
                                *idx = (*idx + 1).min(max);
                            }
                            return;
                        }
                        Key::Named(winit::keyboard::NamedKey::ArrowUp) => {
                            if let Some(ref mut idx) = self.git_completion_index {
                                *idx = idx.saturating_sub(1);
                            }
                            return;
                        }
                        Key::Named(winit::keyboard::NamedKey::Escape) => {
                            self.git_completions_visible = false;
                            self.git_completions.clear();
                            self.git_completion_index = None;
                            return;
                        }
                        _ => {} // Other keys fall through normally
                    }
                }

                // Arming is a safe application mode: only pane navigation,
                // target selection, confirmation, and cancellation are
                // accepted. No other key may leak into a PTY.
                if self.handle_broadcast_arming_key(&event) {
                    return;
                }

                // Check unified shortcut registry (replaces hardcoded check_keybinding)
                if event.state == winit::event::ElementState::Pressed {
                    if let Some(bindable) = self
                        .shortcut_registry
                        .check(&event.logical_key, self.modifiers)
                    {
                        let action = bindable.to_action();
                        if self.handle_action(action) {
                            return;
                        }
                    }
                }

                // Check plugin/custom keybindings (SendText, RunCommand, PluginCommand)
                if event.state == winit::event::ElementState::Pressed {
                    if let Some(key_action) = self
                        .keybinding_manager
                        .check(&event.logical_key, self.modifiers)
                    {
                        if self.handle_key_action(key_action.clone()) {
                            return;
                        }
                    }
                }

                // Convert key event to terminal input
                if self.write_broadcast_key(&event) {
                    return;
                }
                let modifiers = self.modifiers;
                let alt_graph_active = self.alt_graph_active;
                let data = self.active_terminal().and_then(|terminal| {
                    crate::input::key_to_bytes_for_terminal(
                        &event,
                        modifiers,
                        alt_graph_active,
                        terminal,
                    )
                });
                if let Some(data) = data {
                    if event.state == winit::event::ElementState::Pressed {
                        self.native_presentation_latency.begin();
                    }
                    self.write_to_pty(&data);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
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
                if self.guardrail.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                    {
                        let request_id = self.guardrail.request_id();
                        let action = if let (Some(pos), Some(renderer)) =
                            (self.last_cursor_pos, &self.renderer)
                        {
                            let (width, height) = renderer.window_size();
                            let layout = crate::terminal::guardrails::GuardrailLayout::calculate(
                                width as f32,
                                height as f32,
                                renderer.cell_width(),
                                renderer.cell_height(),
                                self.guardrail.animation(),
                            );
                            layout.hit_test(pos.0 as f32, pos.1 as f32, request_id)
                        } else {
                            crate::terminal::guardrails::GuardrailAction::Cancel { request_id }
                        };
                        self.apply_guardrail_action(action);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // Information overlay pointer actions use the same request and
                // geometry as keyboard, rendering, and accessibility.
                if self.info_overlay.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                    {
                        if let (Some(pos), Some(renderer)) = (self.last_cursor_pos, &self.renderer)
                        {
                            use crate::ui::info_overlay::InfoOverlayLayout;
                            let (sw, sh) = renderer.window_size();
                            let layout = InfoOverlayLayout::calculate(
                                sw as f32,
                                sh as f32,
                                renderer.cell_width(),
                                renderer.cell_height(),
                                self.info_overlay.animation(),
                                self.info_overlay.title(),
                                self.info_overlay.lines().len(),
                                self.info_overlay.max_line_chars(),
                            );

                            let (mx, my) = (pos.0 as f32, pos.1 as f32);
                            let request_id = self.info_overlay.request_id();
                            let action = if layout.copy_button.contains(mx, my) {
                                self.info_overlay.set_focus(
                                    request_id,
                                    Some(crate::ui::info_overlay::InfoOverlayFocus::Copy),
                                );
                                crate::ui::info_overlay::InfoOverlayAction::Copy { request_id }
                            } else if layout.continue_button.contains(mx, my) {
                                if self.info_overlay.has_input() {
                                    crate::ui::info_overlay::InfoOverlayAction::SubmitInput {
                                        request_id,
                                    }
                                } else {
                                    crate::ui::info_overlay::InfoOverlayAction::Dismiss {
                                        request_id,
                                    }
                                }
                            } else if let Some(line) = layout.content_row_at(
                                mx,
                                my,
                                self.info_overlay.scroll_offset(),
                                self.info_overlay.lines().len(),
                            ) {
                                if self
                                    .info_overlay
                                    .input_info(request_id)
                                    .is_some_and(|input| input.source_line == line)
                                {
                                    self.info_overlay.set_focus(
                                        request_id,
                                        Some(crate::ui::info_overlay::InfoOverlayFocus::Input),
                                    );
                                    let column =
                                        self.info_overlay.horizontal_offset().saturating_add(
                                            ((mx - layout.content.x) / layout.char_scale)
                                                .floor()
                                                .max(0.0)
                                                as usize,
                                        );
                                    self.info_overlay
                                        .set_input_cursor_from_column(request_id, column);
                                    crate::ui::info_overlay::InfoOverlayAction::None
                                } else {
                                    self.info_overlay.choice_at_line(request_id, line).map_or(
                                        crate::ui::info_overlay::InfoOverlayAction::None,
                                        |choice| {
                                            crate::ui::info_overlay::InfoOverlayAction::Choose {
                                                request_id,
                                                choice_id: choice.id.clone(),
                                            }
                                        },
                                    )
                                }
                            } else {
                                crate::ui::info_overlay::InfoOverlayAction::None
                            };
                            self.apply_info_overlay_action(action);
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                    }
                    return;
                }
                if self.diff_review.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                        && let (Some(pos), Some(renderer)) = (self.last_cursor_pos, &self.renderer)
                    {
                        let (width, height) = renderer.window_size();
                        let layout = crate::ui::diff_review::DiffReviewLayout::calculate(
                            width as f32,
                            height as f32,
                            renderer.cell_width(),
                            renderer.cell_height(),
                            self.diff_review.animation(),
                            self.diff_review.filename(),
                            self.diff_review.rows().len(),
                            self.diff_review.max_line_chars(),
                        );
                        let action = layout.hit_test(
                            pos.0 as f32,
                            pos.1 as f32,
                            self.diff_review.request_id(),
                        );
                        self.apply_diff_review_action(action);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                if self.context_picker.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                        && let (Some(pos), Some(renderer)) = (self.last_cursor_pos, &self.renderer)
                    {
                        let (width, height) = renderer.window_size();
                        let snapshot = self.context_picker.snapshot();
                        let layout = crate::ui::context_picker::ContextPickerLayout::calculate(
                            width as f32,
                            height as f32,
                            renderer.cell_width(),
                            renderer.cell_height(),
                            snapshot.animation,
                            snapshot.phase,
                            snapshot.sources.len(),
                        );
                        let action = layout.hit_test(pos.0 as f32, pos.1 as f32, &snapshot);
                        self.apply_context_picker_action(action);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // Sprout menu is an input-blocking context menu. Click an
                // action to execute it, or click outside to dismiss.
                if self
                    .harvest
                    .as_ref()
                    .is_some_and(|harvest| harvest.sprout_menu.is_some())
                {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                    {
                        let hit = self.last_cursor_pos.and_then(|pos| {
                            let renderer = self.renderer.as_ref()?;
                            let menu = self.harvest.as_ref()?.sprout_menu.as_ref()?;
                            let (window_width, window_height) = renderer.window_size();
                            let info = super::sprout_menu_render_info(
                                menu,
                                renderer.padding(),
                                renderer.cell_width(),
                                renderer.cell_height(),
                                renderer.tab_bar_width(),
                                renderer.tab_bar_height() + renderer.chrome_bar_height(),
                                window_width as f32,
                                window_height as f32,
                            );
                            crate::renderer::sprout_menu_hit_test(&info, pos.0 as f32, pos.1 as f32)
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
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                if self
                    .harvest
                    .as_ref()
                    .is_some_and(|harvest| harvest.cellar.is_expanded())
                {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                        && let Some(pos) = self.last_cursor_pos
                        && let Some(layout) = self.harvest_cellar_layout()
                    {
                        let action = layout.hit_test(pos.0 as f32, pos.1 as f32, true);
                        self.apply_cellar_action(action);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                if self.domain_picker.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                    {
                        let command = if let (Some(pos), Some(renderer)) =
                            (self.last_cursor_pos, &self.renderer)
                        {
                            use crate::ui::command_palette::PaletteLayout;
                            let (sw, sh) = renderer.window_size();
                            let info = self.domain_picker.build_info();
                            let layout = PaletteLayout::calculate(
                                sw as f32,
                                sh as f32,
                                renderer.cell_width(),
                                renderer.cell_height(),
                                info.animation,
                                info.entries.len(),
                            );
                            self.domain_picker
                                .handle_click(pos.0 as f32, pos.1 as f32, &layout)
                        } else {
                            self.domain_picker.close();
                            None
                        };
                        if let Some(command) = command {
                            self.handle_domain_picker_command(command);
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                if self.unicode_picker.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                    {
                        let selection = if let (Some(pos), Some(renderer)) =
                            (self.last_cursor_pos, &self.renderer)
                        {
                            use crate::ui::command_palette::PaletteLayout;
                            let (sw, sh) = renderer.window_size();
                            let info = self.unicode_picker.build_info();
                            let layout = PaletteLayout::calculate(
                                sw as f32,
                                sh as f32,
                                renderer.cell_width(),
                                renderer.cell_height(),
                                info.animation,
                                info.entries.len(),
                            );
                            self.unicode_picker
                                .handle_click(pos.0 as f32, pos.1 as f32, &layout)
                        } else {
                            self.close_unicode_picker();
                            None
                        };
                        if let Some(selection) = selection {
                            self.apply_unicode_picker_selection(selection);
                        } else if !self.unicode_picker.is_active() {
                            self.pending_unicode_picker = None;
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // When palette is open, clicks route through the palette:
                // inside an entry → select+execute, inside but not on an entry
                // → absorb, outside → close.
                if self.command_palette.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                    {
                        if let (Some(pos), Some(renderer)) = (self.last_cursor_pos, &self.renderer)
                        {
                            use crate::ui::command_palette::PaletteLayout;
                            let (sw, sh) = renderer.window_size();
                            let info = self.command_palette.build_info();
                            let layout = PaletteLayout::calculate(
                                sw as f32,
                                sh as f32,
                                renderer.cell_width(),
                                renderer.cell_height(),
                                info.animation,
                                info.entries.len(),
                            );
                            let result = self.command_palette.handle_click(
                                pos.0 as f32,
                                pos.1 as f32,
                                &layout,
                            );
                            if let Some(res) = result {
                                self.handle_palette_result(res);
                            }
                        } else {
                            self.command_palette.close();
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // When shortcuts panel is open, dismiss on outside clicks
                if self.shortcuts_panel.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                    {
                        if let (Some(pos), Some(renderer)) = (self.last_cursor_pos, &self.renderer)
                        {
                            use crate::ui::shortcuts_panel::ShortcutsPanelLayout;
                            let (sw, sh) = renderer.window_size();
                            let info = self.shortcuts_panel.build_info(&self.shortcut_registry);
                            let layout = ShortcutsPanelLayout::calculate(
                                sw as f32,
                                sh as f32,
                                renderer.cell_width(),
                                renderer.cell_height(),
                                info.animation,
                                info.entries.len(),
                            );
                            let (mx, my) = (pos.0 as f32, pos.1 as f32);
                            if !layout.contains(mx, my) {
                                self.shortcuts_panel.close();
                            } else {
                                let action = self.shortcuts_panel.handle_click(
                                    mx,
                                    my,
                                    &layout,
                                    &self.shortcut_registry,
                                );
                                self.apply_shortcuts_action(action);
                            }
                        } else {
                            self.shortcuts_panel.close();
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // Use tracked cursor position — settings_panel handle_click
                // (called from handle_mouse_input) does outside-click dismiss.
                if let Some(pos) = self.last_cursor_pos {
                    self.handle_mouse_input(button, state, pos);
                    self.finish_pointer_chrome_focus(button, state);
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.last_cursor_pos = Some((position.x, position.y));
                // Redraw for hover effects on overlays
                if self.info_overlay.is_active() || self.diff_review.is_active() {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
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
                if self.info_overlay.is_active() {
                    let (mut horizontal, mut vertical) = match delta {
                        winit::event::MouseScrollDelta::LineDelta(x, y) => (x, y),
                        winit::event::MouseScrollDelta::PixelDelta(pos) => {
                            ((pos.x / 20.0) as f32, (pos.y / 20.0) as f32)
                        }
                    };
                    if self.modifiers.shift_key() && horizontal == 0.0 {
                        horizontal = vertical;
                        vertical = 0.0;
                    }
                    let request_id = self.info_overlay.request_id();
                    let action = if horizontal.abs() > vertical.abs() {
                        crate::ui::info_overlay::InfoOverlayAction::ScrollColumns {
                            request_id,
                            delta: -(horizontal.signum() * horizontal.abs().ceil().max(1.0))
                                as isize,
                        }
                    } else {
                        crate::ui::info_overlay::InfoOverlayAction::ScrollLines {
                            request_id,
                            delta: -(vertical.signum() * vertical.abs().ceil().max(1.0)) as isize,
                        }
                    };
                    self.apply_info_overlay_action(action);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if self.diff_review.is_active() {
                    let (mut horizontal, mut vertical) = match delta {
                        winit::event::MouseScrollDelta::LineDelta(x, y) => (x, y),
                        winit::event::MouseScrollDelta::PixelDelta(pos) => {
                            ((pos.x / 20.0) as f32, (pos.y / 20.0) as f32)
                        }
                    };
                    if self.modifiers.shift_key() && horizontal == 0.0 {
                        horizontal = vertical;
                        vertical = 0.0;
                    }
                    let request_id = self.diff_review.request_id();
                    let action = if horizontal.abs() > vertical.abs() {
                        crate::ui::diff_review::DiffReviewAction::ScrollColumns {
                            request_id,
                            delta: -(horizontal.signum() * horizontal.abs().ceil().max(1.0))
                                as isize,
                        }
                    } else {
                        crate::ui::diff_review::DiffReviewAction::ScrollRows {
                            request_id,
                            delta: -(vertical.signum() * vertical.abs().ceil().max(1.0)) as isize,
                        }
                    };
                    self.apply_diff_review_action(action);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if self.context_picker.is_active() {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.y / 20.0) as f32,
                    };
                    let delta = if lines > 0.0 {
                        -(lines.ceil() as isize)
                    } else if lines < 0.0 {
                        (-lines).ceil() as isize
                    } else {
                        0
                    };
                    self.apply_context_picker_action(
                        crate::ui::context_picker::ContextPickerAction::Scroll {
                            request_id: self.context_picker.request_id(),
                            delta,
                        },
                    );
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if self.session_panel.is_open() {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.y / 20.0) as f32,
                    };
                    if lines > 0.0 {
                        self.session_panel.scroll_up(lines.ceil() as usize);
                    } else if lines < 0.0 {
                        self.session_panel.scroll_down((-lines).ceil() as usize);
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                // Scroll settings panel when active
                if self.settings_panel.is_active() {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.y / 20.0) as f32,
                    };
                    if lines > 0.0 {
                        self.settings_panel.scroll_up(lines.ceil() as usize);
                    } else if lines < 0.0 {
                        let total = self
                            .settings_panel
                            .build_entries(&self.config, &self.shortcut_registry)
                            .len();
                        self.settings_panel
                            .scroll_down((-lines).ceil() as usize, total);
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                // Scroll shortcuts panel when active
                if self.shortcuts_panel.is_active() {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.y / 20.0) as f32,
                    };
                    if lines > 0.0 {
                        self.shortcuts_panel.scroll_up(lines.ceil() as usize);
                    } else if lines < 0.0 {
                        self.shortcuts_panel
                            .scroll_down((-lines).ceil() as usize, &self.shortcut_registry);
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if self.domain_picker.is_active() {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.y / 20.0) as f32,
                    };
                    if lines > 0.0 {
                        self.domain_picker.scroll_up(lines.ceil() as usize);
                    } else if lines < 0.0 {
                        self.domain_picker.scroll_down((-lines).ceil() as usize);
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if self.unicode_picker.is_active() {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.y / 20.0) as f32,
                    };
                    if lines > 0.0 {
                        self.unicode_picker.scroll_up(lines.ceil() as usize);
                    } else if lines < 0.0 {
                        self.unicode_picker.scroll_down((-lines).ceil() as usize);
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                // Scroll palette list when palette is active
                if self.command_palette.is_active() {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.y / 20.0) as f32,
                    };
                    if lines > 0.0 {
                        self.command_palette.scroll_up(lines.ceil() as usize);
                    } else if lines < 0.0 {
                        self.command_palette.scroll_down((-lines).ceil() as usize);
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                // Root Cellar owns the wheel while open; positive deltas move
                // toward older commands, matching terminal scroll direction.
                if self
                    .harvest
                    .as_ref()
                    .is_some_and(|harvest| harvest.cellar.is_expanded())
                {
                    let lines = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                        winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.y / 20.0) as f32,
                    };
                    if let Some(harvest) = &mut self.harvest {
                        if lines > 0.0 {
                            harvest.cellar.scroll_by(-(lines.ceil() as isize));
                        } else if lines < 0.0 {
                            harvest.cellar.scroll_by((-lines).ceil() as isize);
                        }
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                // Every remaining modal/context surface owns the wheel while
                // visible, even when it has no scroll action of its own.
                if self.commands_panel.is_open()
                    || self.session_panel.is_open()
                    || self.session_dropdown_open
                    || self.info_overlay.is_active()
                    || self.diff_review.is_active()
                    || self.context_picker.is_active()
                    || self.domain_picker.is_active()
                    || self.unicode_picker.is_active()
                    || self.guardrail.is_active()
                    || self
                        .harvest
                        .as_ref()
                        .is_some_and(|harvest| harvest.sprout_menu.is_some())
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

            WindowEvent::Focused(focused) => {
                if !focused {
                    self.ime.clear_preedit();
                }
                self.handle_window_focus_change(focused);
                self.redraw_scheduler.request(RedrawReason::Focus);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            WindowEvent::Occluded(occluded) => {
                self.window_occluded = occluded;
                if !occluded {
                    self.redraw_scheduler.request(RedrawReason::WindowEvent);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                // Process any pending PTY events
                if self.process_pty_events() {
                    self.redraw_scheduler.request(RedrawReason::TerminalOutput);
                }

                if self.should_exit {
                    if !self.managed_window {
                        event_loop.exit();
                    }
                    return;
                }
                if self.window_occluded {
                    return;
                }

                let reasons = self.redraw_scheduler.begin_frame();
                log::trace!("Rendering frame: reasons={reasons}");
                let presented = self.render();
                self.redraw_scheduler.finish_frame();
                if presented {
                    self.native_presentation_latency
                        .finish_after_terminal_present(&reasons);
                }
                self.native_frame_stats
                    .record(self.redraw_scheduler.rendered_frames());
            }

            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        self.finish_obscured_native_pointer_gesture();
        match event {
            AppEvent::SystemAccessibilityChanged(preferences) => {
                self.redraw_scheduler.request(RedrawReason::Accessibility);
                self.set_system_accessibility_preferences(preferences);
            }
            AppEvent::SystemAccessibilityUnavailable(error) => {
                log::debug!("Desktop accessibility preferences are unavailable: {error}");
            }
            AppEvent::ControlRequest {
                request,
                respond_to,
            } => {
                let result = match request {
                    crate::control::protocol::Request::Inventory => {
                        let mut budget = super::ControlInventoryBudget::default();
                        Ok(crate::control::protocol::Response::Inventory(
                            crate::control::protocol::ProcessInventory {
                                process_id: std::process::id(),
                                windows: vec![self.control_inventory(1, &mut budget)],
                                truncated: false,
                            },
                        ))
                    }
                    crate::control::protocol::Request::ReadText {
                        window_id,
                        tab_id,
                        pane_id,
                        history_lines,
                        max_bytes,
                    } => {
                        if window_id != 1 {
                            Err(format!("GUI window {window_id} does not exist"))
                        } else {
                            self.read_control_text(tab_id, pane_id, history_lines, max_bytes)
                                .map(|(screen, history_lines, text, truncated)| {
                                    crate::control::protocol::Response::Text {
                                        process_id: std::process::id(),
                                        window_id,
                                        tab_id,
                                        pane_id,
                                        screen,
                                        history_lines,
                                        text,
                                        truncated,
                                    }
                                })
                        }
                    }
                    crate::control::protocol::Request::LaunchStatus { .. } => Err(
                        "Launch status requires the process-level application controller".into(),
                    ),
                    crate::control::protocol::Request::CreateWindow { .. }
                    | crate::control::protocol::Request::LaunchWindow { .. }
                    | crate::control::protocol::Request::CreateTab { .. }
                    | crate::control::protocol::Request::LaunchTab { .. }
                    | crate::control::protocol::Request::SplitPane { .. }
                    | crate::control::protocol::Request::FocusWindow { .. }
                    | crate::control::protocol::Request::SetQuickAccessVisibility { .. }
                    | crate::control::protocol::Request::SelectTab { .. }
                    | crate::control::protocol::Request::FocusPane { .. }
                    | crate::control::protocol::Request::CloseWindow { .. }
                    | crate::control::protocol::Request::CloseTab { .. }
                    | crate::control::protocol::Request::ClosePane { .. }
                    | crate::control::protocol::Request::MoveTab { .. }
                    | crate::control::protocol::Request::MovePane { .. }
                    | crate::control::protocol::Request::ResizeWindow { .. }
                    | crate::control::protocol::Request::ResizeDivider { .. }
                    | crate::control::protocol::Request::SendText { .. } => {
                        Err("GUI mutations require the process-level application controller".into())
                    }
                };
                let _ = respond_to.send(result);
            }
            AppEvent::Pty => {
                clear_pty_wake_pending();
                if self.process_pty_events() {
                    self.redraw_scheduler.request(RedrawReason::TerminalOutput);
                }
            }
            AppEvent::Git => {
                // The immutable result is drained in `about_to_wait`, where
                // cwd selection and redraw scheduling stay centralized.
            }
            AppEvent::Transcript => {
                clear_transcript_wake_pending();
                if self.process_pty_events() {
                    self.redraw_scheduler.request(RedrawReason::WindowEvent);
                }
            }
            AppEvent::AccessKit(event) => {
                self.redraw_scheduler.request(RedrawReason::Accessibility);
                self.handle_accesskit_event(event);
            }
            AppEvent::RendererDeviceLost {
                window_id,
                generation,
                failure,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::SurfaceRecovery);
                    self.handle_renderer_device_lost(generation, failure);
                }
            }
            AppEvent::BackgroundImageLoaded {
                window_id,
                generation,
                path,
                result,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_background_image_loaded(generation, path, result);
                }
            }
            AppEvent::LiveDomainControlCompleted {
                window_id,
                control,
                result,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_live_domain_control_completed(control, result);
                }
            }
            AppEvent::LiveDomainTopology {
                window_id,
                domain_id,
                result,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_live_domain_topology(domain_id, result);
                }
            }
            AppEvent::DomainPickerLoaded {
                window_id,
                generation,
                result,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_domain_picker_loaded(generation, result);
                }
            }
            AppEvent::DomainPickerPrepared {
                window_id,
                generation,
                result,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_domain_picker_prepared(generation, result);
                }
            }
            AppEvent::ContextGathered {
                window_id,
                request_id,
                result,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_context_gathered(request_id, result);
                }
            }
            AppEvent::FileUploadCompleted {
                window_id,
                request_id,
                result,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_file_upload_completed(request_id, result);
                }
            }
            AppEvent::FileUploadProgress {
                window_id,
                request_id,
                bytes_sent,
                total_bytes,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_file_upload_progress(request_id, bytes_sent, total_bytes);
                }
            }
            AppEvent::KittyFileTransferCompleted {
                window_id,
                token,
                result,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_kitty_file_transfer_completed(token, result);
                }
            }
            AppEvent::KittyFileUploadCompleted {
                window_id,
                token,
                result,
            } => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_kitty_file_upload_completed(token, result);
                }
            }
            AppEvent::StructuredPluginCompleted(completion) => {
                if self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.id() == completion.window_id)
                {
                    self.redraw_scheduler.request(RedrawReason::BackgroundTask);
                    self.handle_structured_plugin_completed(completion);
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.should_exit {
            if !self.managed_window {
                event_loop.exit();
            }
            return;
        }
        self.poll_idle_sources(Instant::now());
        // Process PTY events before waiting
        if self.process_pty_events() {
            self.redraw_scheduler.request(RedrawReason::TerminalOutput);
        }
        if self.should_exit {
            if !self.managed_window {
                event_loop.exit();
            }
            return;
        }

        // Native redraw requests are coalesced here. PTY reader threads wake
        // this loop through a user event, so no polling deadline is required.
        self.finish_obscured_native_pointer_gesture();
        if self.redraw_scheduler.has_pending()
            && !self.surface_lost
            && !self.window_occluded
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }

        if let Some(deadline) = self.next_idle_deadline(Instant::now()) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
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
