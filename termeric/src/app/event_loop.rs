//! winit event loop integration
//!
//! Window events for one `App`, dispatched by `Application`.

use std::sync::Arc;
use winit::{
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::WindowEvent,
    event_loop::ActiveEventLoop,
    window::Window,
};

use crate::input::keybindings::KeyChord;
use crate::ui::settings::SettingsKey;

use super::App;
use super::redraw_scheduler::RedrawReason;
use super::text_field_keys::FieldKey;

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
                    return;
                }

                window.set_visible(true);
                if self.launch.quick_access {
                    window.focus_window();
                }
                self.warn_about_outdated_agent_hooks();

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
            }
        }
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

                // Query paste respects modal ownership and the configured
                // chord before query handlers consume its character key.
                if event.state == winit::event::ElementState::Pressed
                    && self.handle_query_paste_key(&event.logical_key)
                {
                    return;
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
                            if let Some(field_key) = self.text_edit_for_key(&event.logical_key) {
                                if let FieldKey::Edit(edit) = field_key {
                                    self.info_overlay.edit_input(request_id, edit);
                                }
                                InfoOverlayAction::None
                            } else {
                                match &event.logical_key {
                                    Key::Named(NamedKey::Escape) => {
                                        InfoOverlayAction::CancelInput { request_id }
                                    }
                                    Key::Named(NamedKey::ArrowUp) => {
                                        InfoOverlayAction::ScrollLines {
                                            request_id,
                                            delta: -1,
                                        }
                                    }
                                    Key::Named(NamedKey::ArrowDown) => {
                                        InfoOverlayAction::ScrollLines {
                                            request_id,
                                            delta: 1,
                                        }
                                    }
                                    Key::Named(NamedKey::PageUp) => {
                                        InfoOverlayAction::ScrollLines {
                                            request_id,
                                            delta: -(self.info_overlay.viewport_rows() as isize),
                                        }
                                    }
                                    Key::Named(NamedKey::PageDown) => {
                                        InfoOverlayAction::ScrollLines {
                                            request_id,
                                            delta: self.info_overlay.viewport_rows() as isize,
                                        }
                                    }
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
                                // A held key must not pick a choice it did not
                                // mean to reach.
                                Key::Character(_) if event.repeat => InfoOverlayAction::None,
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
                            Key::Character(key)
                                if key.as_str().eq_ignore_ascii_case("s")
                                    && self.diff_review.has_change_actions() =>
                            {
                                DiffReviewAction::Stage { request_id }
                            }
                            Key::Character(key)
                                if key.as_str().eq_ignore_ascii_case("r")
                                    && self.diff_review.has_change_actions() =>
                            {
                                DiffReviewAction::Revert { request_id }
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
                if event.state == winit::event::ElementState::Pressed
                    && self.handle_palette_edit_key(&event.logical_key)
                {
                    return;
                }
                if self.domain_picker.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        let command = match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                self.back_or_close_domain_picker();
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
                                self.close_domain_picker();
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

                        if self.handle_shortcuts_filter_key(&event.logical_key) {
                            return;
                        }
                        let shortcuts_key = match &event.logical_key {
                            Key::Named(NamedKey::Escape) => Some(ShortcutsKey::Escape),
                            Key::Named(NamedKey::ArrowUp) => Some(ShortcutsKey::Up),
                            Key::Named(NamedKey::ArrowDown) => Some(ShortcutsKey::Down),
                            Key::Named(NamedKey::Enter) => Some(ShortcutsKey::Enter),
                            Key::Named(NamedKey::Backspace) => Some(ShortcutsKey::Backspace),
                            Key::Named(NamedKey::Delete) => Some(ShortcutsKey::Delete),
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

                        if self.handle_settings_filter_key(&event.logical_key) {
                            return;
                        }
                        let settings_key = match &event.logical_key {
                            Key::Named(NamedKey::Escape) => Some(SettingsKey::Escape),
                            Key::Named(NamedKey::ArrowUp) => Some(SettingsKey::Up),
                            Key::Named(NamedKey::ArrowDown) => Some(SettingsKey::Down),
                            Key::Named(NamedKey::ArrowLeft) => Some(SettingsKey::Left),
                            Key::Named(NamedKey::ArrowRight) => Some(SettingsKey::Right),
                            Key::Named(NamedKey::Enter) => Some(SettingsKey::Enter),
                            Key::Named(NamedKey::Tab) if self.modifiers.shift_key() => {
                                Some(SettingsKey::BackTab)
                            }
                            Key::Named(NamedKey::Tab) => Some(SettingsKey::Tab),
                            Key::Named(NamedKey::Backspace) => Some(SettingsKey::FilterBackspace),
                            Key::Named(NamedKey::Delete) => Some(SettingsKey::Delete),
                            Key::Character(c) if self.modifiers.control_key() => match c.as_str() {
                                "s" | "S" => Some(SettingsKey::Save),
                                "r" | "R" => Some(SettingsKey::Reset),
                                _ => None,
                            },
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
                        if self.handle_session_rename_key(&event.logical_key) {
                            return;
                        }
                        let panel_key = match &event.logical_key {
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
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // An open toolbar menu owns every key, so none reaches the PTY.
                if self.workspace_menu.is_some() {
                    if event.state == winit::event::ElementState::Pressed {
                        self.handle_workspace_menu_key(&event.logical_key);
                    }
                    return;
                }

                // Both browse mode and the inline form own modal input.
                if self.commands_panel.is_open() {
                    if event.state == winit::event::ElementState::Pressed {
                        self.handle_commands_panel_key(&event.logical_key);
                    }
                    return;
                }
                if self.agents_panel.is_open() {
                    if event.state == winit::event::ElementState::Pressed {
                        self.handle_agents_panel_key(&event.logical_key);
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
                    if event.state == winit::event::ElementState::Pressed {
                        if !super::agent_view_input::composer_chord(
                            &event.logical_key,
                            self.modifiers,
                        ) && let Some(bindable) = self
                            .shortcut_registry
                            .check(&event.logical_key, self.modifiers)
                            && self.handle_action(bindable.to_action())
                        {
                            return;
                        }
                        self.handle_agent_view_key(&event.logical_key, event.text.as_deref());
                    }
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
                            self.diff_review.has_change_actions(),
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
                            let (_, window_height) = renderer.window_size();
                            let info = super::sprout_menu_render_info(
                                menu,
                                renderer.padding(),
                                renderer.cell_width(),
                                renderer.cell_height(),
                                renderer.tab_bar_width(),
                                renderer.content_top(),
                                renderer.content_right(),
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
                        self.ime.clear_preedit();
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
                            self.close_domain_picker();
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
                        self.ime.clear_preedit();
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
                // → move query caret or absorb, outside → close.
                if self.command_palette.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                    {
                        self.ime.clear_preedit();
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
                self.handle_mouse_wheel(delta);
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
                self.handle_window_occlusion_change(occluded);
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
}

impl App {
    /// Scroll whatever owns the wheel: the frontmost modal surface, a
    /// side panel, the agent view, or the terminal.
    pub(super) fn handle_mouse_wheel(&mut self, delta: winit::event::MouseScrollDelta) {
        // The guardrail draws above every surface and can open over
        // any of them, so nothing beneath it may scroll.
        if self.guardrail.is_active() {
            return;
        }
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
                    delta: -(horizontal.signum() * horizontal.abs().ceil().max(1.0)) as isize,
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
                    delta: -(horizontal.signum() * horizontal.abs().ceil().max(1.0)) as isize,
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
        if self.session_panel.is_open()
            || self.workspace_menu.is_some()
            || self.info_overlay.is_active()
            || self.diff_review.is_active()
            || self.context_picker.is_active()
            || self.domain_picker.is_active()
            || self.unicode_picker.is_active()
            || self
                .harvest
                .as_ref()
                .is_some_and(|harvest| harvest.sprout_menu.is_some())
        {
            return;
        }
        if self.handle_commands_panel_scroll(delta) || self.handle_agents_panel_scroll(delta) {
            return;
        }
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
        assert_eq!(app.info_overlay.scroll_offset(), 3);

        app.guardrail.show(
            crate::terminal::guardrails::GuardrailMatch {
                command: "rm -rf /".into(),
                reason: "fixture".into(),
                severity: crate::terminal::guardrails::Severity::Critical,
                target: "Local".into(),
                user: "tester".into(),
            },
            vec![crate::terminal::guardrails::GuardedWrite::for_pane(
                1,
                crate::panes::PaneId(0),
                b"x".to_vec(),
            )],
        );
        app.handle_mouse_wheel(MouseScrollDelta::LineDelta(0.0, -3.0));
        assert_eq!(app.info_overlay.scroll_offset(), 3);
    }
}
