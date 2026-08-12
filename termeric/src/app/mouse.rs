//! Mouse routing for modal layers, chrome, panes, and terminal content.

use super::*;

impl App {
    /// Handle mouse input
    pub(super) fn handle_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
        position: (f64, f64),
    ) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let chrome_bar_height = renderer.chrome_bar_height();
        let padding = renderer.padding();
        // Account for search bar height if visible
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 8.0 * 2.0 // Same calculation as renderer
        } else {
            0.0
        };
        let content_top = tab_bar_height + chrome_bar_height + search_bar_height;
        let scroll_offset = self
            .tabs
            .as_ref()
            .and_then(|tabs| {
                tabs.active_tab()
                    .map(|tab| tab.terminal.grid().scroll_offset())
            })
            .and_then(|offset| i32::try_from(offset).ok())
            .unwrap_or(0);
        let grid_pos = screen_to_grid(
            position.0,
            position.1,
            cell_width,
            cell_height,
            content_top, // Use content_top instead of just tab_bar_height
            padding,
            scroll_offset,
        );

        match state {
            ElementState::Pressed => {
                if button == MouseButton::Left {
                    // Check if click is on the scrollbar
                    if self.is_click_on_scrollbar(position.0, position.1) {
                        self.scrollbar_dragging = true;
                        self.scroll_to_scrollbar_position(position.1);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }

                    // Check if click is on a pane divider (for drag-to-resize)
                    if let Some(tabs) = &self.tabs {
                        if let Some(tab) = tabs.active_tab() {
                            if let Some(ref pm) = tab.pane_manager {
                                let local_x = position.0 as f32 - padding;
                                let local_y = position.1 as f32 - content_top;
                                if pm.hit_test_divider(local_x, local_y).is_some() {
                                    self.divider_dragging = true;
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                    return;
                                }
                            }
                        }
                    }

                    // Ctrl+Click to open hyperlinks
                    if self.modifiers.control_key() {
                        // Check if there's a hyperlink at this position
                        if let Some(tabs) = &self.tabs {
                            if let Some(tab) = tabs.active_tab() {
                                let grid = tab.terminal.grid();
                                // grid_pos.row is already scroll-adjusted
                                let row = grid_pos.row;
                                let col = grid_pos.col;

                                // For visible grid, row should be non-negative
                                if row >= 0 {
                                    if let Some(cell) = grid.cell(row as usize, col) {
                                        if let Some(hyperlink_id) = cell.hyperlink_id {
                                            if let Some(url) =
                                                tab.terminal.get_hyperlink_url(hyperlink_id)
                                            {
                                                log::info!("Opening hyperlink: {}", url);
                                                if let Err(e) =
                                                    crate::terminal::url::UrlDetector::open_url(url)
                                                {
                                                    log::error!("Failed to open URL: {}", e);
                                                }
                                                return;
                                            }
                                        }
                                    }

                                    // Fallback: check auto-detected URLs via UrlCache
                                    if row >= 0 {
                                        if let Some(detected) =
                                            self.url_cache.url_at(row as usize, col)
                                        {
                                            let url = &detected.url;
                                            log::info!("Opening auto-detected URL: {}", url);
                                            if let Err(e) =
                                                crate::terminal::url::UrlDetector::open_url(url)
                                            {
                                                log::error!("Failed to open URL: {}", e);
                                            }
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Check for tab bar clicks (tabs, close buttons, + button, gear)
                    if let Some(renderer) = &self.renderer {
                        let px = position.0 as f32;
                        let py = position.1 as f32;

                        // If commands panel is open, intercept all clicks
                        if self.commands_panel.is_open() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::ui::commands::panel::CommandsPanelAction;
                                let info = self.commands_panel.build_info(&self.config.commands);
                                let action = renderer.commands_panel_hit_test(
                                    px,
                                    py,
                                    &self.config.commands,
                                    size.width as f32,
                                    size.height as f32,
                                    info.animation,
                                    &info.edit_mode,
                                );
                                match action {
                                    CommandsPanelAction::Run(idx) => {
                                        if let Some(cmd) = self.config.commands.get(idx) {
                                            let command = cmd.command.clone();
                                            self.commands_panel.close();
                                            self.run_custom_command(&command);
                                        }
                                    }
                                    CommandsPanelAction::Delete(idx) => {
                                        if idx < self.config.commands.len() {
                                            let name = self.config.commands[idx].name.clone();
                                            self.config.commands.remove(idx);
                                            log::info!("Deleted custom command '{}'", name);
                                            match self.save_config() {
                                                Ok(msg) => {
                                                    log::info!("{}", msg);
                                                    self.toasts.success(format!(
                                                        "Deleted command '{}'",
                                                        name
                                                    ));
                                                }
                                                Err(e) => {
                                                    log::error!(
                                                        "Deleted '{}' but save failed: {}",
                                                        name,
                                                        e
                                                    );
                                                    self.toasts
                                                        .error(format!("Save failed: {}", e));
                                                }
                                            }
                                        }
                                    }
                                    CommandsPanelAction::AddSubmit { name, command } => {
                                        if name == "SAVE" && command == "SAVE" {
                                            // Save button clicked
                                            if let Some(cmd) = self.commands_panel.try_submit() {
                                                let cmd_name = cmd.name.clone();
                                                self.config.commands.push(cmd);
                                                match self.save_config() {
                                                    Ok(msg) => {
                                                        log::info!("{}", msg);
                                                        self.toasts.success(format!(
                                                            "Saved command '{}'",
                                                            cmd_name
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        log::error!(
                                                            "Failed to save custom command: {}",
                                                            e
                                                        );
                                                        self.toasts
                                                            .error(format!("Save failed: {}", e));
                                                    }
                                                }
                                            } else {
                                                // Form was incomplete — give feedback.
                                                self.toasts
                                                    .warning("Both name and command are required");
                                            }
                                        } else if name.is_empty() && command.is_empty() {
                                            // Click on "+ New Command" button => start editing
                                            self.commands_panel.start_add();
                                        } else {
                                            // Actually add the command
                                            self.config.commands.push(
                                                crate::config::CustomCommand {
                                                    name,
                                                    command,
                                                    alias: None,
                                                },
                                            );
                                            if let Ok(msg) = self.save_config() {
                                                log::info!("{}", msg);
                                            }
                                        }
                                    }
                                    CommandsPanelAction::ClickName => {
                                        self.commands_panel.focus_name();
                                    }
                                    CommandsPanelAction::ClickCommand => {
                                        self.commands_panel.focus_command();
                                    }
                                    CommandsPanelAction::Close => {
                                        self.commands_panel.close();
                                    }
                                    CommandsPanelAction::None => {}
                                }
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }

                        // If session management panel is open, intercept all clicks
                        if self.session_panel.is_open() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::session::panel::SessionPanelAction;
                                let action = renderer.session_panel_hit_test(
                                    px,
                                    py,
                                    self.session_panel.sessions(),
                                    size.width as f32,
                                    size.height as f32,
                                    self.session_panel.build_info().animation,
                                );
                                match action {
                                    SessionPanelAction::Load(name) => {
                                        use crate::session::restore::restore_named_session;
                                        match restore_named_session(&name) {
                                            Ok(Some(session)) => {
                                                if let Some(tabs) = &mut self.tabs {
                                                    match tabs.new_tab_from_session(&session) {
                                                        Ok(_) => {
                                                            log::info!(
                                                                "Loaded session '{}' from panel",
                                                                name
                                                            );
                                                            self.toasts.success(format!(
                                                                "Restored session '{}'",
                                                                name
                                                            ));
                                                        }
                                                        Err(e) => {
                                                            log::error!(
                                                                "Failed to restore session: {}",
                                                                e
                                                            );
                                                            self.toasts.error(format!(
                                                                "Restore failed: {e:#}"
                                                            ));
                                                        }
                                                    }
                                                }
                                            }
                                            Ok(None) => {
                                                log::warn!("Session '{}' not found", name);
                                                self.toasts.warning(format!(
                                                    "Session '{}' not found",
                                                    name
                                                ));
                                            }
                                            Err(e) => {
                                                log::error!("Failed to load session: {}", e);
                                                self.toasts.error(format!("Load failed: {}", e));
                                            }
                                        }
                                        self.session_panel.close();
                                    }
                                    SessionPanelAction::Delete(name) => {
                                        match crate::session::delete_session(&name) {
                                            Ok(()) => {
                                                log::info!("Deleted session '{}'", name);
                                                self.toasts
                                                    .success(format!("Deleted session '{}'", name));
                                            }
                                            Err(e) => {
                                                log::error!(
                                                    "Failed to delete session '{}': {}",
                                                    name,
                                                    e
                                                );
                                                self.toasts.error(format!("Delete failed: {}", e));
                                            }
                                        }
                                        // Refresh the list
                                        self.session_panel.refresh_sessions();
                                    }
                                    SessionPanelAction::Close => {
                                        self.session_panel.close();
                                    }
                                    SessionPanelAction::None => {}
                                }
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }

                        // If session dropdown is open, intercept clicks anywhere
                        // (dropdown extends below tab bar)
                        if self.session_dropdown_open {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::renderer::SessionDropdownClick;
                                let click = renderer.session_dropdown_hit_test(
                                    px,
                                    py,
                                    &self.session_dropdown_items,
                                    size.width as f32,
                                );
                                match click {
                                    SessionDropdownClick::LoadSession(name) => {
                                        use crate::session::restore::restore_named_session;
                                        match restore_named_session(&name) {
                                            Ok(Some(session)) => {
                                                if let Some(tabs) = &mut self.tabs {
                                                    match tabs.new_tab_from_session(&session) {
                                                        Ok(_) => {
                                                            log::info!("Loaded session '{}'", name);
                                                            self.toasts.success(format!(
                                                                "Restored session '{}'",
                                                                name
                                                            ));
                                                        }
                                                        Err(e) => {
                                                            log::error!(
                                                                "Failed to restore session: {}",
                                                                e
                                                            );
                                                            self.toasts.error(format!(
                                                                "Restore failed: {e:#}"
                                                            ));
                                                        }
                                                    }
                                                }
                                            }
                                            Ok(None) => {
                                                log::warn!("Session '{}' not found", name);
                                                self.toasts.warning(format!(
                                                    "Session '{}' not found",
                                                    name
                                                ));
                                            }
                                            Err(e) => {
                                                log::error!("Failed to load session: {}", e);
                                                self.toasts.error(format!("Load failed: {e:#}"));
                                            }
                                        }
                                        self.session_dropdown_open = false;
                                    }

                                    SessionDropdownClick::ManageSessions => {
                                        self.session_dropdown_open = false;
                                        self.session_panel.open();
                                    }
                                    SessionDropdownClick::Dismiss => {
                                        self.session_dropdown_open = false;
                                    }
                                    SessionDropdownClick::None => {}
                                }
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }

                        // Settings modal intercepts all clicks (inside: handle;
                        // outside: dismiss). Must come before tab bar / search bar
                        // so the modal can't be undercut by chrome under the scrim.
                        if self.settings_panel.is_active() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                let action = self.settings_panel.handle_click(
                                    px,
                                    py,
                                    &self.config,
                                    &self.shortcut_registry,
                                    size.width as f32,
                                    size.height as f32,
                                    cell_width,
                                    cell_height,
                                );
                                self.apply_settings_action(action);
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }

                        if py < renderer.tab_bar_height() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::renderer::TabBarClick;
                                let tab_infos =
                                    self.tabs.as_ref().map(|t| t.tab_info()).unwrap_or_default();
                                let click = renderer.tab_bar_hit_test(
                                    px,
                                    py,
                                    &tab_infos,
                                    size.width as f32,
                                );
                                match click {
                                    TabBarClick::Gear => {
                                        self.settings_panel.toggle();
                                    }
                                    TabBarClick::NewTab => {
                                        if let Some(tabs) = &mut self.tabs {
                                            match tabs.new_tab() {
                                                Ok(id) => {
                                                    log::info!("Created new tab {} via mouse", id);
                                                    self.maybe_show_splash(true);
                                                    self.dispatch_plugin_hook(
                                                        crate::plugin::Hook::TabCreated {
                                                            tab_id: id,
                                                        },
                                                    );
                                                }
                                                Err(e) => {
                                                    log::error!("Failed to create new tab: {}", e)
                                                }
                                            }
                                        }
                                    }
                                    TabBarClick::CloseTab(idx) => {
                                        let tab_id = self.tabs.as_ref().and_then(|t| {
                                            t.iter().find(|(i, _)| *i == idx).map(|(_, tab)| tab.id)
                                        });
                                        if let Some(tabs) = &mut self.tabs {
                                            tabs.close_tab(idx);
                                            if tabs.is_empty() {
                                                log::info!("Last tab closed, exiting");
                                                self.should_exit = true;
                                            }
                                        }
                                        if let Some(id) = tab_id {
                                            self.dispatch_plugin_hook(
                                                crate::plugin::Hook::TabClosed { tab_id: id },
                                            );
                                        }
                                    }
                                    TabBarClick::Tab(idx) => {
                                        let from = self
                                            .tabs
                                            .as_ref()
                                            .map(|t| t.active_index())
                                            .unwrap_or(0);
                                        if let Some(tabs) = &mut self.tabs {
                                            tabs.switch_to(idx);
                                        }
                                        if from != idx {
                                            self.dispatch_plugin_hook(
                                                crate::plugin::Hook::TabSwitched { from, to: idx },
                                            );
                                        }
                                    }
                                    TabBarClick::SessionDropdown => {
                                        self.session_dropdown_open = !self.session_dropdown_open;
                                        if self.session_dropdown_open {
                                            self.session_dropdown_items =
                                                crate::session::list_sessions_with_titles();
                                        }
                                    }
                                    TabBarClick::CommandsButton => {
                                        self.commands_panel.toggle();
                                    }
                                    TabBarClick::None => {}
                                }
                                if click != TabBarClick::None {
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                    return;
                                }
                            }
                        }
                    }

                    // Intercept clicks for search bar
                    if self.search.is_active() || self.search.has_results() {
                        if let Some(renderer) = &self.renderer {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::renderer::SearchBarClick;
                                let click = renderer.search_bar_hit_test(
                                    position.0 as f32,
                                    position.1 as f32,
                                    size.width as f32,
                                    self.search.match_count(),
                                    self.search.current_match_index(),
                                    self.search.query().is_empty(),
                                    self.search.is_filter_mode(),
                                    self.search.matching_line_count(),
                                );
                                let grid_ref = self.tabs.as_ref().and_then(|tabs| {
                                    tabs.active_tab().map(|tab| tab.terminal.grid())
                                });

                                match click {
                                    SearchBarClick::ToggleCaseSensitive => {
                                        if let Some(grid) = grid_ref {
                                            self.search.toggle_case_sensitive(grid);
                                        }
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::ToggleRegex => {
                                        if let Some(grid) = grid_ref {
                                            self.search.toggle_regex(grid);
                                        }
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::ToggleFilter => {
                                        self.search.toggle_filter();
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::PrevMatch => {
                                        self.search.prev();
                                        self.scroll_to_current_match();
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::NextMatch => {
                                        self.search.next();
                                        self.scroll_to_current_match();
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::None => {}
                                }
                            }
                        }
                    }
                }
                // Focus pane under click in multi-pane mode
                if let Some(tabs) = &mut self.tabs {
                    if let Some(tab) = tabs.active_tab_mut() {
                        if let Some(ref mut pm) = tab.pane_manager {
                            if let Some(renderer) = &self.renderer {
                                let px = position.0 as f32;
                                let py = position.1 as f32;
                                let content_top =
                                    renderer.tab_bar_height() + renderer.chrome_bar_height();
                                let padding = renderer.padding();
                                let rel_x = px - padding;
                                let rel_y = py - content_top;
                                let layout = pm.calculate_layout();
                                for (id, rect) in &layout {
                                    if rel_x >= rect.x
                                        && rel_x < rect.x + rect.width
                                        && rel_y >= rect.y
                                        && rel_y < rect.y + rect.height
                                    {
                                        pm.focus(*id);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(mut selection) =
                    self.mouse_state
                        .handle_press(button, grid_pos, self.modifiers)
                {
                    self.mouse_pressed = true;
                    // Double-click expands to the word under the cursor;
                    // triple-click expands to the full line. Simple/Block
                    // selections pass through unchanged.
                    if let Some(terminal) = self.active_terminal_mut() {
                        match selection.ty {
                            termeric_core::SelectionType::Semantic => {
                                let (start, end) = terminal.word_at(grid_pos);
                                selection.start = start;
                                selection.end = end;
                            }
                            termeric_core::SelectionType::Lines => {
                                let (start, end) = terminal.line_at(grid_pos);
                                selection.start = start;
                                selection.end = end;
                            }
                            _ => {}
                        }
                        terminal.set_selection(Some(selection.clone()));
                    }
                    // Keep mouse_state in sync so drags extend the expanded
                    // selection. Written after the terminal borrow ends.
                    self.mouse_state.selection = Some(selection);
                }
            }
            ElementState::Released => {
                if button == MouseButton::Left {
                    self.mouse_pressed = false;
                    self.scrollbar_dragging = false;
                    self.divider_dragging = false;
                }
                // Finalize selection on release
                let _ = self.mouse_state.handle_release(button);
                // Clear active selection state (but keep the selection visible)
                // The selection in terminal remains, but mouse_state no longer tracks it
            }
        }

        // Request redraw to show selection changes
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Handle mouse movement
    pub(super) fn handle_mouse_move(&mut self, position: (f64, f64)) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let chrome_bar_height = renderer.chrome_bar_height();
        let padding = renderer.padding();
        // Account for search bar height if visible
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 8.0 * 2.0 // Same calculation as renderer
        } else {
            0.0
        };
        let content_top = tab_bar_height + chrome_bar_height + search_bar_height;
        let scroll_offset = self
            .tabs
            .as_ref()
            .and_then(|tabs| {
                tabs.active_tab()
                    .map(|tab| tab.terminal.grid().scroll_offset())
            })
            .and_then(|offset| i32::try_from(offset).ok())
            .unwrap_or(0);
        let grid_pos = screen_to_grid(
            position.0,
            position.1,
            cell_width,
            cell_height,
            content_top,
            padding,
            scroll_offset,
        );

        // ── Tab bar hover detection for icon animations ──
        {
            let px = position.0 as f32;
            let py = position.1 as f32;
            let window_size = self.window.as_ref().map(|w| w.inner_size());
            let tab_infos = self.tabs.as_ref().map(|t| t.tab_info()).unwrap_or_default();
            let new_hover = if py < tab_bar_height {
                if let Some(renderer) = &self.renderer {
                    if let Some(size) = window_size {
                        let hit = renderer.tab_bar_hit_test(px, py, &tab_infos, size.width as f32);
                        if hit != crate::renderer::TabBarClick::None {
                            Some(hit)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if new_hover != self.tab_bar_hover {
                self.tab_bar_hover = new_hover;
                self.tab_bar_hover_progress = 0.0;
            }
        }

        // Handle scrollbar dragging
        if self.scrollbar_dragging {
            self.scroll_to_scrollbar_position(position.1);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Handle pane divider dragging
        if self.divider_dragging {
            if let Some(tabs) = &mut self.tabs {
                if let Some(tab) = tabs.active_tab_mut() {
                    if let Some(ref mut pm) = tab.pane_manager {
                        let local_x = position.0 as f32 - padding;
                        let local_y = position.1 as f32 - content_top;
                        if pm.resize_divider_at(local_x, local_y, f32::MAX) {
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                    }
                }
            }
            return;
        }

        // Set resize cursor when hovering over pane dividers
        if let Some(tabs) = &self.tabs {
            if let Some(tab) = tabs.active_tab() {
                if let Some(ref pm) = tab.pane_manager {
                    let local_x = position.0 as f32 - padding;
                    let local_y = position.1 as f32 - content_top;
                    if let Some(is_vertical) = pm.hit_test_divider(local_x, local_y) {
                        if let Some(window) = &self.window {
                            let cursor = if is_vertical {
                                winit::window::CursorIcon::EwResize
                            } else {
                                winit::window::CursorIcon::NsResize
                            };
                            window.set_cursor(cursor);
                        }
                        return;
                    } else if let Some(window) = &self.window {
                        window.set_cursor(winit::window::CursorIcon::Default);
                    }
                }
            }
        }

        // Don't update selection when an overlay panel is open,
        // but DO track hover state for visual highlights.
        if self.settings_panel.is_active() {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let (cell_width, cell_height) = renderer.cell_size();
                if let Some(size) = self.window.as_ref().map(|w| w.inner_size()) {
                    let entries = self
                        .settings_panel
                        .build_entries(&self.config, &self.shortcut_registry);
                    let layout = crate::ui::settings::SettingsModalLayout::calculate(
                        size.width as f32,
                        size.height as f32,
                        cell_width,
                        cell_height,
                        self.settings_panel.animation_progress(),
                        entries.len(),
                    );

                    // Row hover (suppressed when the dropdown overlay is open
                    // so we don't hover-highlight rows behind it).
                    let hovered = if self.settings_panel.is_dropdown_open() {
                        None
                    } else {
                        layout.visible_row_at(py)
                    };
                    self.settings_panel.set_hover(hovered);

                    // Dropdown option hover (only when the overlay is open).
                    if self.settings_panel.is_dropdown_open() {
                        let dd_hover = self
                            .settings_panel
                            .dropdown_layout(&layout, &entries)
                            .filter(|(rect, _)| rect.contains(px, py))
                            .and_then(|(rect, option_count)| {
                                rect.option_at(py).filter(|&i| i < option_count)
                            });
                        self.settings_panel.set_dropdown_hover(dd_hover);
                    } else {
                        self.settings_panel.set_dropdown_hover(None);
                    }
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.commands_panel.is_open() {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let (ui_cw, cell_height) = renderer.cell_size();
                let window_size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = window_size {
                    let window_width = size.width as f32;
                    let anim = self.commands_panel.animation_progress();
                    let panel_width = (ui_cw * 36.0).max(340.0).min(window_width * 0.45);
                    let panel_x = window_width - panel_width * anim;
                    let padding = ui_cw * 1.8;

                    if px >= panel_x {
                        let header_height = cell_height * 2.8;
                        let btn_y = header_height + cell_height * 0.8;
                        let btn_height = cell_height * 2.0;

                        // Check "+ New Command" button hover
                        let hovered_add = py >= btn_y && py <= btn_y + btn_height;

                        // Calculate list start (skip form if editing)
                        let mut list_start_y = btn_y + btn_height + cell_height * 0.8;
                        if self.commands_panel.is_editing() {
                            let field_height = cell_height * 1.8;
                            let form_total_h = field_height * 2.0 + cell_height * 3.2;
                            list_start_y += form_total_h + cell_height * 0.5;
                        }
                        if !self.config.commands.is_empty() {
                            list_start_y += cell_height * 1.5;
                        }

                        let row_height = cell_height * 3.0;
                        let mut hovered_index = None;
                        let mut hovered_delete = None;
                        for i in 0..self.config.commands.len() {
                            let item_y = list_start_y + i as f32 * row_height;
                            if py >= item_y && py < item_y + row_height {
                                hovered_index = Some(i);
                                // Check delete button
                                let del_x = panel_x + panel_width - padding - ui_cw * 1.5;
                                if px >= del_x - ui_cw * 0.5 && px <= del_x + ui_cw * 1.5 {
                                    hovered_delete = Some(i);
                                }
                                break;
                            }
                        }
                        self.commands_panel
                            .set_hover(hovered_index, hovered_add, hovered_delete);
                    } else {
                        self.commands_panel.set_hover(None, false, None);
                    }
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.session_panel.is_open() {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let (ui_cw, cell_height) = renderer.cell_size();
                let window_size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = window_size {
                    let window_width = size.width as f32;
                    let anim = self.session_panel.animation_progress();
                    let panel_width = (ui_cw * 35.0).max(320.0).min(window_width * 0.45);
                    let panel_x = window_width - panel_width * anim;
                    let padding_y = cell_height * 0.5;
                    let row_height = (cell_height * 3.5).max(52.0);
                    let title_y = cell_height * 1.5;
                    let sep_y = title_y + cell_height * 2.0;
                    let list_start_y = sep_y + padding_y + 4.0;

                    let hovered = if px >= panel_x {
                        let sessions = self.session_panel.sessions();
                        let mut found = None;
                        for i in 0..sessions.len() {
                            let item_y = list_start_y + i as f32 * row_height;
                            if py >= item_y && py < item_y + row_height {
                                found = Some(i);
                                break;
                            }
                        }
                        found
                    } else {
                        None
                    };
                    self.session_panel.set_hover(hovered);
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.session_dropdown_open {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let window_size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = window_size {
                    let (cell_width, cell_height) = renderer.cell_size();
                    let tab_bar_height = renderer.tab_bar_height();
                    let window_width = size.width as f32;

                    let gear_button_width = tab_bar_height;
                    let session_button_width = tab_bar_height;
                    let dropdown_anchor_x = window_width - gear_button_width - session_button_width;

                    let row_height = (cell_height * 2.0).max(28.0);
                    let dropdown_width = (cell_width * 22.0).max(200.0);
                    let dropdown_x =
                        (dropdown_anchor_x + session_button_width - dropdown_width).max(0.0);
                    let dropdown_y = tab_bar_height;

                    let session_count = self.session_dropdown_items.len().min(5);
                    let display_rows = session_count.max(1);
                    let total_items = display_rows + 1; // + "Manage Sessions"
                    let dropdown_height = total_items as f32 * row_height + 4.0;

                    self.session_dropdown_hovered = if px >= dropdown_x
                        && px <= dropdown_x + dropdown_width
                        && py >= dropdown_y
                        && py <= dropdown_y + dropdown_height
                    {
                        let relative_y = py - dropdown_y - 2.0;
                        let row = (relative_y / row_height) as usize;
                        if row < total_items { Some(row) } else { None }
                    } else {
                        None
                    };
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Only update selection when mouse is pressed (dragging)
        if self.mouse_pressed {
            if let Some(selection) = self.mouse_state.handle_move(grid_pos) {
                if let Some(terminal) = self.active_terminal_mut() {
                    terminal.set_selection(Some(selection));
                }
                // Request redraw for selection update
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        } else {
            // Just update position tracking when not dragging
            self.mouse_state.last_position = Some(grid_pos);
        }
    }

    /// Handle scroll wheel
    pub(super) fn handle_scroll(&mut self, delta: f64) {
        // Filter out near-zero deltas (release artifacts from trackpad/mouse)
        // These appear as -0.000 or very small values on scroll release
        if delta.abs() < 0.001 {
            return;
        }

        if let Some(terminal) = self.active_terminal_mut() {
            let lines = delta.abs().max(1.0) as usize;
            let offset = terminal.grid().scroll_offset();

            let new_offset = if delta > 0.0 {
                offset + lines
            } else {
                offset.saturating_sub(lines)
            };

            terminal.grid_mut().set_scroll_offset(new_offset);

            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// Check if a click position is within the scrollbar area
    pub(super) fn is_click_on_scrollbar(&self, x: f64, _y: f64) -> bool {
        let Some(renderer) = &self.renderer else {
            return false;
        };
        let (window_width, _) = renderer.window_size();

        // Scrollbar is 6px wide + 2px margin from right edge, with an expanded hit area
        let scrollbar_hit_width = 16.0_f32;
        let scrollbar_x = window_width as f32 - scrollbar_hit_width;

        // Check if there's actually scrollback content
        let has_scrollback = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .map(|tab| tab.terminal.grid().scrollback_len() > 0)
            .unwrap_or(false);

        has_scrollback && x as f32 >= scrollbar_x
    }

    /// Map a vertical mouse position to a scroll offset and apply it
    pub(super) fn scroll_to_scrollbar_position(&mut self, y: f64) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let tab_bar_height = renderer.tab_bar_height();
        let padding = renderer.padding();
        let (_, window_height) = renderer.window_size();

        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            let (_, cell_height) = renderer.cell_size();
            cell_height + 8.0 * 2.0
        } else {
            0.0
        };
        let content_top = tab_bar_height + search_bar_height;
        let scrollbar_top = content_top + padding;
        let scrollbar_bottom = window_height as f32 - padding;
        let scrollbar_height = scrollbar_bottom - scrollbar_top;

        if scrollbar_height <= 0.0 {
            return;
        }

        // Fraction from top (0.0=top, 1.0=bottom)
        let fraction = ((y as f32 - scrollbar_top) / scrollbar_height).clamp(0.0, 1.0);

        // Invert: top of scrollbar = max scroll, bottom = 0 scroll
        let scroll_fraction = 1.0 - fraction;

        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                let max_offset = tab.terminal.grid().scrollback_len();
                let new_offset = (scroll_fraction * max_offset as f32) as usize;
                tab.terminal.grid_mut().set_scroll_offset(new_offset);
            }
        }
    }
}
