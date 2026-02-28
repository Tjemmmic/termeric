//! winit event loop integration
//!
//! Implements `ApplicationHandler` for the App, handling window events.

use std::sync::Arc;
use std::time::Instant;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};

use crate::input::{check_keybinding, key_to_bytes_with_modifiers};
use crate::ui::settings::SettingsKey;

use super::App;

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // Create window
        let mut window_attrs = Window::default_attributes()
            .with_title("Termeric")
            .with_inner_size(LogicalSize::new(
                self.config.window.width,
                self.config.window.height,
            ));

        // Load and set window icon if available
        if let Some(icon) = Self::load_icon() {
            window_attrs = window_attrs.with_window_icon(Some(icon));
        }

        match event_loop.create_window(window_attrs) {
            Ok(window) => {
                log::info!("Window created");
                let window = Arc::new(window);

                // Initialize renderer
                if let Err(e) = self.init_renderer(window.clone()) {
                    log::error!("Failed to initialize renderer: {}", e);
                    event_loop.exit();
                    return;
                }

                // Initialize tabs with first tab
                if let Err(e) = self.init_tabs() {
                    log::error!("Failed to initialize tabs: {}", e);
                    event_loop.exit();
                    return;
                }

                // Initial render
                self.render();
            }
            Err(e) => {
                log::error!("Failed to create window: {}", e);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                log::info!("Close requested");
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                self.handle_resize(size.width, size.height);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = self.window.as_ref().map(|window| window.inner_size());
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

            WindowEvent::KeyboardInput { event, .. } => {
                // Handle guardrail confirmation (highest priority)
                if self.guardrail.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        match &event.logical_key {
                            Key::Named(NamedKey::Enter) => {
                                // Confirm — send the pending data
                                let data = self.guardrail.confirm();
                                if let Some(tabs) = &mut self.tabs {
                                    if let Some(tab) = tabs.active_tab_mut() {
                                        let _ = tab.write(&data);
                                    }
                                }
                            }
                            Key::Named(NamedKey::Escape) => {
                                self.guardrail.dismiss();
                            }
                            _ => {}
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // Dismiss info overlay on any keypress (highest priority)
                if self.info_overlay.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        self.info_overlay.dismiss();
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }

                // Handle diff review overlay input
                if self.diff_review.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                // Mark diffs as reviewed before dismissing
                                if let (Some(tab_idx), Some(tabs)) = (self.diff_review.diff_tab_index, &mut self.tabs) {
                                    if let Some((_, tab)) = tabs.iter().enumerate().find(|(i, _)| *i == tab_idx) {
                                        // We can't mutate through iter(), use the index directly
                                    }
                                    // Mark via active tab's diff detector
                                    if let Some(tab) = tabs.active_tab_mut() {
                                        tab.diff_detector.mark_all_reviewed();
                                    }
                                }
                                self.diff_review.dismiss();
                            }
                            Key::Named(NamedKey::ArrowUp) => {
                                self.diff_review.scroll_up(3);
                            }
                            Key::Character(k) if k == "k" => {
                                self.diff_review.scroll_up(3);
                            }
                            Key::Named(NamedKey::ArrowDown) => {
                                self.diff_review.scroll_down(3);
                            }
                            Key::Character(k) if k == "j" => {
                                self.diff_review.scroll_down(3);
                            }
                            Key::Character(c) if c == "c" => {
                                // Copy raw diff to clipboard
                                if let Some(raw) = self.diff_review.raw_text() {
                                    if let Some(clipboard) = &mut self.clipboard {
                                        let _ = clipboard.set_text(raw.to_string());
                                        log::info!("Copied diff to clipboard");
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

                // Handle context picker input
                if self.context_picker.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                self.context_picker.dismiss();
                            }
                            Key::Named(NamedKey::ArrowUp) => {
                                self.context_picker.move_up();
                            }
                            Key::Named(NamedKey::ArrowDown) => {
                                self.context_picker.move_down();
                            }
                            Key::Named(NamedKey::Space) => {
                                self.context_picker.toggle_selected();
                            }
                            Key::Named(NamedKey::Enter) => {
                                if self.context_picker.has_selection() {
                                    self.inject_context();
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
                                if self.modifiers.control_key() && self.modifiers.shift_key()
                                    && (c.as_str() == "p" || c.as_str() == "P")
                                {
                                    self.command_palette.close();
                                } else if !self.modifiers.control_key() && !self.modifiers.alt_key() {
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

                // Handle settings panel input first (highest priority when open)
                if self.settings_panel.is_active() {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey};
                        let settings_key = match &event.logical_key {
                            Key::Named(NamedKey::Escape) => Some(SettingsKey::Escape),
                            Key::Named(NamedKey::ArrowUp) => Some(SettingsKey::Up),
                            Key::Named(NamedKey::ArrowDown) => Some(SettingsKey::Down),
                            Key::Named(NamedKey::ArrowLeft) => Some(SettingsKey::Left),
                            Key::Named(NamedKey::ArrowRight) => Some(SettingsKey::Right),
                            Key::Named(NamedKey::Enter) => Some(SettingsKey::Enter),
                            Key::Named(NamedKey::Tab) => Some(SettingsKey::Tab),
                            Key::Character(c) if self.modifiers.control_key() => {
                                match c.as_str() {
                                    "s" | "S" => Some(SettingsKey::Save),
                                    "r" | "R" => Some(SettingsKey::Reset),
                                    _ => None,
                                }
                            }
                            _ => None,
                        };

                        if let Some(key) = settings_key {
                            let action = self.settings_panel.handle_key(key, &self.config);
                            self.apply_settings_action(action);
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
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
                                if let Some(cmd) = self.commands_panel.try_submit() {
                                    self.config.commands.push(cmd);
                                    if let Ok(msg) = self.save_config() {
                                        log::info!("{}", msg);
                                    }
                                }
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

                // Handle search mode input
                if self.handle_search_mode_input(&event) {
                    return;
                }

                // Handle git completion navigation (Tab to accept, Up/Down to navigate, Escape to dismiss)
                if self.git_completions_visible && event.state == winit::event::ElementState::Pressed {
                    use winit::keyboard::Key;
                    match event.logical_key.as_ref() {
                        Key::Named(winit::keyboard::NamedKey::Tab) => {
                            // Accept the selected completion
                            if let Some(idx) = self.git_completion_index {
                                if let Some(branch) = self.git_completions.get(idx).cloned() {
                                    if let Some(partial) = crate::git::detect_git_completion(&self.input_line_buffer) {
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

                // Check custom keybindings first (highest priority after modal UI)
                if event.state == winit::event::ElementState::Pressed {
                    if let Some(key_action) = self.keybinding_manager.check(&event.logical_key, self.modifiers) {
                        if self.handle_key_action(key_action.clone()) {
                            return;
                        }
                    }
                }

                // Check for default keybindings
                if let Some(action) = check_keybinding(&event, self.modifiers) {
                    self.handle_action(action);
                    return;
                }

                // Convert key event to terminal input
                if let Some(data) = key_to_bytes_with_modifiers(&event, self.modifiers) {
                    self.write_to_pty(&data);
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                // Dismiss info overlay on any click
                if self.info_overlay.is_active() {
                    if state == winit::event::ElementState::Pressed {
                        self.info_overlay.dismiss();
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // When palette is open, only dismiss on clicks outside the panel
                if self.command_palette.is_active() {
                    if state == winit::event::ElementState::Pressed
                        && button == winit::event::MouseButton::Left
                    {
                        if let (Some(pos), Some(renderer)) = (self.last_cursor_pos, &self.renderer) {
                            let cw = renderer.cell_width();
                            let (sw, sh) = renderer.window_size();
                            let (ww, wh) = (sw as f32, sh as f32);
                            // Approximate panel bounds (must match build_command_palette geometry)
                            let panel_width = (cw * 55.0_f32).max(450.0).min(ww * 0.65);
                            let panel_x = (ww - panel_width) / 2.0;
                            let panel_y = wh * 0.12; // top position
                            let panel_height = wh * 0.75; // max height
                            let (mx, my) = (pos.0 as f32, pos.1 as f32);
                            // Only close if click is outside the panel
                            if mx < panel_x || mx > panel_x + panel_width
                                || my < panel_y || my > panel_y + panel_height
                            {
                                self.command_palette.close();
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
                // Use tracked cursor position
                if let Some(pos) = self.last_cursor_pos {
                    self.handle_mouse_input(button, state, pos);
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.last_cursor_pos = Some((position.x, position.y));
                self.handle_mouse_move((position.x, position.y));
            }

            WindowEvent::MouseWheel { delta, .. } => {
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
                let scroll_speed = self.config.scrollback.scroll_speed;
                let y_delta = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y as f64 * scroll_speed,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y / 10.0,
                };
                self.handle_scroll(y_delta);
            }

            WindowEvent::Focused(focused) => {
                if self.window_focused != focused {
                    self.window_focused = focused;
                    self.focus_change_time = Instant::now();
                    log::debug!("Window focus changed: {}", focused);
                }
            }

            WindowEvent::RedrawRequested => {
                // Process any pending PTY events
                self.process_pty_events();

                if self.should_exit {
                    event_loop.exit();
                    return;
                }

                // Render
                self.render();
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Process PTY events before waiting
        self.process_pty_events();

        // Request redraw if we have new content
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Save session before cleanup
        self.save_current_session();

        // Drop order is critical: renderer holds a wgpu Surface that references
        // the window, so we must drop resources in the correct order:
        // 1. Tabs (PTY processes) first
        // 2. Renderer (GPU resources / Surface) second
        // 3. Window last (implicit, after this Drop returns)
        self.tabs.take();
        self.renderer.take();
        log::debug!("App resources cleaned up");
    }
}
