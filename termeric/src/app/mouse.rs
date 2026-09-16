//! Mouse routing for modal layers, chrome, panes, and terminal content.

use super::*;

impl DividerDrag {
    fn owns_tab(&self, tab: &crate::tabs::Tab) -> bool {
        tab.id == self.tab_id
            && !tab.closed
            && !tab.is_closing()
            && match (&self.domain, tab.live_domain_tab_route(), self.target) {
                (None, None, crate::panes::DividerTarget::Local(_)) => true,
                (
                    Some((domain, remote_tab)),
                    Some((current_domain, current_tab)),
                    crate::panes::DividerTarget::Domain(_),
                ) => domain == current_domain && *remote_tab == current_tab,
                _ => false,
            }
    }
}

impl App {
    /// A divider belongs to a tab, not its focused pane. Switching panes in
    /// that tab may continue the drag; hiding/closing its tab must finish it.
    pub(super) fn reconcile_divider_drag(&mut self) {
        let Some(drag) = self.divider_dragging.as_ref() else {
            return;
        };
        let valid = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .is_some_and(|tab| {
                drag.owns_tab(tab)
                    && tab.live_domain_tab_route().is_none_or(|(domain, _)| {
                        !self
                            .pending_live_domain_controls
                            .iter()
                            .any(|pending| pending.domain_id() == domain)
                    })
                    && tab.pane_manager.as_ref().is_some_and(|panes| {
                        panes
                            .get_dividers()
                            .iter()
                            .any(|divider| divider.target == drag.target)
                    })
            });
        if !valid {
            // Do not drop a live-domain preview: finalize it once through
            // the existing authoritative acknowledgment/rollback path.
            self.finish_native_pointer_gesture();
        }
    }

    pub(super) fn apply_divider_drag_position(&mut self, x: f32, y: f32) {
        self.reconcile_divider_drag();
        let Some(drag) = self.divider_dragging.as_ref() else {
            return;
        };
        let ratio = self
            .tabs
            .as_mut()
            .and_then(|tabs| tabs.tab_mut_by_id(drag.tab_id))
            .and_then(|tab| tab.pane_manager.as_mut())
            .and_then(|panes| panes.resize_divider_target(drag.target, x, y));
        if let Some(ratio_millis) = ratio {
            if let Some(active_drag) = &mut self.divider_dragging {
                active_drag.last_ratio_millis = ratio_millis;
            }
        } else {
            self.finish_native_pointer_gesture();
        }
    }

    fn focused_pointer_terminal(&self) -> Option<crate::tabs::TerminalIdentity> {
        let tabs = self.tabs.as_ref()?;
        let target = tabs.active_tab()?.focused_terminal_identity();
        tabs.origin_for_pane(target.tab_id, target.pane_id)?;
        Some(target)
    }

    pub(super) fn begin_scrollbar_drag(&mut self) {
        self.scrollbar_dragging = self.focused_pointer_terminal();
    }

    fn reconcile_terminal_mouse_capture(&mut self) {
        if self
            .terminal_mouse_capture
            .is_some_and(|capture| self.focused_pointer_terminal() != Some(capture.owner))
        {
            self.terminal_mouse_capture = None;
        }
    }

    /// A retained pointer gesture may never migrate to a different terminal,
    /// including when a caller changes focus without refreshing UI surfaces.
    fn reconcile_scrollbar_drag(&mut self) -> bool {
        if self.scrollbar_dragging.is_some()
            && self.scrollbar_dragging == self.focused_pointer_terminal()
        {
            true
        } else {
            self.scrollbar_dragging = None;
            false
        }
    }

    pub(super) fn apply_scrollbar_drag_fraction(&mut self, fraction: f32) {
        if !self.reconcile_scrollbar_drag() {
            return;
        }
        if let Some(terminal) = self.active_terminal_mut() {
            let max_offset = terminal.grid().scrollback_len();
            let new_offset = (fraction.clamp(0.0, 1.0) * max_offset as f32) as usize;
            terminal.grid_mut().set_scroll_offset(new_offset);
        }
    }

    /// These surfaces are dispatched here, below the event-loop overlays but
    /// above terminal scrollbars/dividers. Keep their ownership independent
    /// of where the pointer lands (including outside-click dismissal).
    pub(super) fn pointer_panel_owns_input(&self) -> bool {
        self.settings_panel.is_active()
            || self.commands_panel.is_open()
            || self.session_panel.is_open()
            || self.session_dropdown_open
    }

    /// A modal takes native pointer ownership even if the physical button
    /// remains held. Do this before dispatch/dismissal and animation cleanup,
    /// not only on release (which the modal may consume).
    pub(super) fn finish_obscured_native_pointer_gesture(&mut self) {
        if self.guardrail.is_active()
            || self.info_overlay.is_active()
            || self.diff_review.is_active()
            || self.context_picker.is_active()
            || self.domain_picker.is_active()
            || self.unicode_picker.is_active()
            || self.command_palette.is_active()
            || self.shortcuts_panel.is_active()
            || self.pointer_panel_owns_input()
            || self.harvest.as_ref().is_some_and(|harvest| {
                harvest.sprout_menu.is_some() || harvest.cellar.is_expanded()
            })
        {
            self.finish_native_pointer_gesture();
        }
    }

    /// End native gestures at their last preview, even if no release reaches
    /// this window. Live-domain changes use the normal acknowledgment/rollback
    /// path; dropping their drag state alone would orphan a local preview.
    pub(super) fn finish_native_pointer_gesture(&mut self) {
        for control in self.take_native_drag_controls() {
            self.queue_live_domain_control(control);
        }
    }

    pub(super) fn take_native_drag_controls(&mut self) -> Vec<LiveDomainControl> {
        self.mouse_pressed = false;
        self.scrollbar_dragging = None;
        let divider_drag = self.divider_dragging.take();
        let tab_drag = self.tab_dragging.take();
        self.tab_drag_scroll = None;
        let _ = self.mouse_state.handle_release(MouseButton::Left);
        let mut controls = Vec::new();
        if let Some(drag) = divider_drag {
            // Never commit a captured split ID through a replacement route.
            // Hidden geometry may still finalize, but its owner must be intact.
            let owner_valid = self
                .tabs
                .as_ref()
                .is_some_and(|tabs| tabs.iter().any(|(_, tab)| drag.owns_tab(tab)));
            if let (
                true,
                Some((domain_id, _)),
                crate::panes::DividerTarget::Domain(remote_split_id),
            ) = (owner_valid, drag.domain, drag.target)
            {
                controls.push(LiveDomainControl::ResizeSplit {
                    domain_id,
                    remote_split_id,
                    ratio_millis: drag.last_ratio_millis,
                });
            }
        }
        if let Some(TabDrag {
            tab_id,
            live:
                Some(LiveDomainTabDrag {
                    domain_id,
                    remote_tab_id,
                    initial_index,
                }),
        }) = tab_drag
        {
            let index = self.tabs.as_ref().and_then(|tabs| {
                tabs.iter()
                    .filter(|(_, tab)| {
                        tab.live_domain_tab_route()
                            .is_some_and(|(domain, _)| domain == domain_id)
                    })
                    .position(|(_, tab)| {
                        tab.id == tab_id
                            && !tab.closed
                            && !tab.is_closing()
                            && tab.live_domain_tab_route()
                                == Some((domain_id.as_str(), remote_tab_id))
                    })
                    .and_then(|index| u16::try_from(index).ok())
            });
            if let Some(index) = index
                && index != initial_index
            {
                controls.push(LiveDomainControl::MoveTab {
                    domain_id,
                    remote_tab_id,
                    index,
                });
            }
        }
        controls
    }

    /// Handle mouse input
    pub(super) fn handle_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
        position: (f64, f64),
    ) {
        self.finish_obscured_native_pointer_gesture();
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let chrome_bar_height = renderer.chrome_bar_height();
        let padding = renderer.padding();
        let content_left = renderer.tab_bar_width();
        // Account for search bar height if visible
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 8.0 * 2.0 // Same calculation as renderer
        } else {
            0.0
        };
        let content_top = tab_bar_height + chrome_bar_height + search_bar_height;
        if state == ElementState::Pressed
            && button != MouseButton::Left
            && self.pointer_panel_owns_input()
        {
            return;
        }
        match state {
            ElementState::Pressed => {
                if button == MouseButton::Left && !self.pointer_panel_owns_input() {
                    // Check if click is on the scrollbar
                    if self.is_click_on_scrollbar(position.0, position.1) {
                        if self.focus_pane_at(position, content_top, padding) {
                            self.refresh_focused_terminal_surfaces();
                        }
                        self.begin_scrollbar_drag();
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
                                let local_x = position.0 as f32 - content_left - padding;
                                let local_y = position.1 as f32 - content_top;
                                if let Some(divider) = pm.divider_at(
                                    local_x,
                                    local_y,
                                    crate::panes::PANE_DIVIDER_HIT_MARGIN,
                                ) {
                                    let pending_domain = tab
                                        .live_domain_tab_route()
                                        .map(|(domain, _)| domain)
                                        .filter(|domain| {
                                            self.pending_live_domain_controls
                                                .iter()
                                                .any(|pending| pending.domain_id() == *domain)
                                        });
                                    if pending_domain.is_some() {
                                        self.toasts.info(
                                            "Wait for the pending live-domain change before resizing",
                                        );
                                        return;
                                    }
                                    if tab.live_domain.is_some()
                                        && !matches!(
                                            divider.target,
                                            crate::panes::DividerTarget::Domain(_)
                                        )
                                    {
                                        self.toasts.error(
                                            "Live-domain divider is missing its daemon identity",
                                        );
                                    } else {
                                        self.divider_dragging = Some(DividerDrag {
                                            tab_id: tab.id,
                                            domain: tab.live_domain_tab_route().map(
                                                |(domain, remote_tab)| {
                                                    (domain.to_owned(), remote_tab)
                                                },
                                            ),
                                            target: divider.target,
                                            last_ratio_millis: divider.ratio_millis,
                                        });
                                    }
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                    return;
                                }
                            }
                        }
                    }
                }
                if button == MouseButton::Left {
                    // Panel actions (and the global Settings button) own the
                    // click before background tab/search/terminal handling.
                    if let Some(renderer) = &self.renderer {
                        let px = position.0 as f32;
                        let py = position.1 as f32;

                        // The Settings button is a global tab-bar control. Let
                        // it switch directly from Commands/Sessions to Settings
                        // instead of making the first click close the old panel
                        // and requiring another click to open the new one.
                        if py < renderer.tab_bar_height() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                let tab_infos =
                                    self.tabs.as_ref().map(|t| t.tab_info()).unwrap_or_default();
                                if renderer.tab_bar_hit_test(
                                    px,
                                    py,
                                    &tab_infos,
                                    size.width as f32,
                                    size.height as f32,
                                ) == crate::renderer::TabBarClick::Gear
                                {
                                    self.toggle_settings_surface();
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                    return;
                                }
                            }
                        }

                        // If commands panel is open, intercept all clicks
                        if self.commands_panel.is_open() {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
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
                                self.apply_commands_panel_action(action);
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
                                let info = self.session_panel.build_info();
                                let action = renderer.session_panel_hit_test(
                                    px,
                                    py,
                                    self.session_panel.sessions(),
                                    info.workspaces,
                                    info.rename_text.is_some(),
                                    info.scroll_offset,
                                    size.width as f32,
                                    size.height as f32,
                                    info.animation,
                                );
                                self.apply_session_panel_action(action);
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
                                        self.restore_saved_session(&name);
                                        self.session_dropdown_open = false;
                                    }

                                    SessionDropdownClick::ManageSessions => {
                                        self.session_dropdown_open = false;
                                        self.session_panel.open_sessions();
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

                        if py < renderer.tab_bar_height() || px < renderer.tab_bar_width() {
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
                                    size.height as f32,
                                );
                                match click {
                                    TabBarClick::Gear => {
                                        self.toggle_settings_surface();
                                    }
                                    TabBarClick::NewTab => {
                                        self.request_new_tab();
                                    }
                                    TabBarClick::PreviousTab => {
                                        self.handle_action(crate::input::Action::PrevTab);
                                    }
                                    TabBarClick::NextTab => {
                                        self.handle_action(crate::input::Action::NextTab);
                                    }
                                    TabBarClick::CloseTab(idx) => {
                                        self.request_close_tab(idx);
                                    }
                                    TabBarClick::Tab(idx) => {
                                        let from = self
                                            .tabs
                                            .as_ref()
                                            .map(|t| t.active_index())
                                            .unwrap_or(0);
                                        if let Some(tabs) = &mut self.tabs {
                                            tabs.switch_to(idx);
                                            self.tab_dragging = tabs.active_tab().map(|tab| {
                                                let live =
                                                    tab.live_domain_tab_route().map(
                                                        |(domain_id, remote_tab_id)| {
                                                            let initial_index =
                                                                tabs.iter()
                                                                    .filter(|(_, candidate)| {
                                                                        candidate
                                                                            .live_domain_tab_route()
                                                                            .is_some_and(
                                                                                |(candidate, _)| {
                                                                                    candidate
                                                                                        == domain_id
                                                                                },
                                                                            )
                                                                    })
                                                                    .position(|(_, candidate)| {
                                                                        candidate
                                                                    .live_domain_tab_route()
                                                                    .is_some_and(|(_, candidate)| {
                                                                        candidate == remote_tab_id
                                                                    })
                                                                    })
                                                                    .unwrap_or(0)
                                                                    as u16;
                                                            LiveDomainTabDrag {
                                                                domain_id: domain_id.to_string(),
                                                                remote_tab_id,
                                                                initial_index,
                                                            }
                                                        },
                                                    );
                                                TabDrag {
                                                    tab_id: tab.id,
                                                    live,
                                                }
                                            });
                                        }
                                        let live_drag_blocked = self
                                            .tab_dragging
                                            .as_ref()
                                            .and_then(|drag| drag.live.as_ref())
                                            .is_some_and(|live| {
                                                self.pending_live_domain_controls.iter().any(
                                                    |pending| pending.domain_id() == live.domain_id,
                                                )
                                            });
                                        if live_drag_blocked {
                                            self.tab_dragging = None;
                                            self.toasts.info(
                                                "Wait for the pending live-domain change before reordering",
                                            );
                                        }
                                        if from != idx {
                                            self.refresh_focused_terminal_surfaces();
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
                    if self.search.is_active()
                        || self.search.has_results()
                        || self.search.is_searching()
                    {
                        if let Some(renderer) = &self.renderer {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                use crate::renderer::SearchBarClick;
                                let info = self.search.build_info(self.search_cursor_pos);
                                let click = renderer.search_bar_hit_test(
                                    position.0 as f32,
                                    position.1 as f32,
                                    size.width as f32,
                                    &info,
                                );
                                match click {
                                    SearchBarClick::ToggleCaseSensitive => {
                                        self.with_search_and_active_grid(|search, grid| {
                                            search.toggle_case_sensitive(grid);
                                        });
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::ToggleRegex => {
                                        self.with_search_and_active_grid(|search, grid| {
                                            search.toggle_regex(grid);
                                        });
                                        if let Some(window) = &self.window {
                                            window.request_redraw();
                                        }
                                        return;
                                    }
                                    SearchBarClick::ToggleFilter => {
                                        if self.copy_mode.is_active() {
                                            self.toasts.warning(
                                                "Filter mode is unavailable inside copy mode; matches remain highlighted",
                                            );
                                        } else {
                                            self.search.toggle_filter();
                                        }
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
                                    SearchBarClick::Close => {
                                        self.search.cancel();
                                        self.search_cursor_pos = 0;
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
                if button == MouseButton::Left
                    && self.toggle_armed_broadcast_target_at(position, content_top, padding)
                {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if self.focus_pane_at(position, content_top, padding) {
                    self.refresh_focused_terminal_surfaces();
                }

                let grid_pos = self.focused_grid_position(
                    position,
                    cell_width,
                    cell_height,
                    content_top,
                    padding,
                );

                // Ctrl+Click opens links from the pane selected by this click.
                if self.modifiers.control_key() {
                    let col = grid_pos.col;
                    let semantic_url = self.active_terminal().and_then(|terminal| {
                        terminal
                            .grid()
                            .row_at_offset(grid_pos.row)
                            .and_then(|row| row.get(col))
                            .and_then(|cell| cell.hyperlink_id)
                            .and_then(|id| terminal.get_hyperlink_url(id))
                    });
                    let scroll_offset = self
                        .active_terminal()
                        .and_then(|terminal| i32::try_from(terminal.grid().scroll_offset()).ok())
                        .unwrap_or(0);
                    let visual_row = grid_pos.row.saturating_add(scroll_offset);
                    let detected_url = usize::try_from(visual_row).ok().and_then(|row| {
                        self.url_cache
                            .url_at(row, col)
                            .map(|detected| detected.url.as_str())
                    });
                    let url = semantic_url.or(detected_url);
                    if let Some(url) = url {
                        log::info!("Opening hyperlink: {}", url);
                        if let Err(error) = crate::terminal::url::UrlDetector::open_url(url) {
                            log::error!("Failed to open URL: {}", error);
                        }
                        return;
                    }
                }

                if self.try_report_mouse_press(
                    button,
                    position,
                    cell_width,
                    cell_height,
                    content_top,
                    padding,
                ) {
                    return;
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

                if button != MouseButton::Left {
                    if self.focus_pane_at(position, content_top, padding) {
                        self.refresh_focused_terminal_surfaces();
                    }
                    if self.try_report_mouse_press(
                        button,
                        position,
                        cell_width,
                        cell_height,
                        content_top,
                        padding,
                    ) {
                        return;
                    }
                }
            }
            ElementState::Released => {
                if self.try_report_mouse_release(
                    button,
                    position,
                    cell_width,
                    cell_height,
                    content_top,
                    padding,
                ) {
                    self.refresh_terminal_pointer_cursor();
                    return;
                }
                if button == MouseButton::Left {
                    self.finish_native_pointer_gesture();
                }
                // Primary-gesture cleanup finishes native selection while
                // retaining the selected text. Other buttons do not own it.
            }
        }

        self.refresh_terminal_pointer_cursor();

        // Request redraw to show selection changes
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Handle mouse movement
    pub(super) fn handle_mouse_move(&mut self, position: (f64, f64)) {
        self.finish_obscured_native_pointer_gesture();
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let chrome_bar_height = renderer.chrome_bar_height();
        let padding = renderer.padding();
        let content_left = renderer.tab_bar_width();
        // Account for search bar height if visible
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 8.0 * 2.0 // Same calculation as renderer
        } else {
            0.0
        };
        let content_top = tab_bar_height + chrome_bar_height + search_bar_height;
        let grid_pos =
            self.focused_grid_position(position, cell_width, cell_height, content_top, padding);

        // ── Tab bar hover detection for icon animations ──
        {
            let px = position.0 as f32;
            let py = position.1 as f32;
            let window_size = self.window.as_ref().map(|w| w.inner_size());
            let tab_infos = self.tabs.as_ref().map(|t| t.tab_info()).unwrap_or_default();
            let new_hover = if py < tab_bar_height || px < renderer.tab_bar_width() {
                if let Some(renderer) = &self.renderer {
                    if let Some(size) = window_size {
                        let hit = renderer.tab_bar_hit_test(
                            px,
                            py,
                            &tab_infos,
                            size.width as f32,
                            size.height as f32,
                        );
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
            if let Some(crate::renderer::TabBarClick::Tab(target)) = new_hover {
                self.apply_tab_drag_target(target);
            }
        }

        // Handle scrollbar dragging
        self.update_tab_drag_scroll(self.tab_bar_hover, Instant::now());
        if self.scrollbar_dragging.is_some() {
            self.scroll_to_scrollbar_position(position.1);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Handle pane divider dragging
        if self.divider_dragging.is_some() {
            let local_x = position.0 as f32 - content_left - padding;
            let local_y = position.1 as f32 - content_top;
            self.apply_divider_drag_position(local_x, local_y);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.info_overlay.is_active()
            || self.diff_review.is_active()
            || self.context_picker.is_active()
            || self.domain_picker.is_active()
            || self.unicode_picker.is_active()
            || self.guardrail.is_active()
            || self.command_palette.is_active()
            || self.shortcuts_panel.is_active()
            || self.harvest.as_ref().is_some_and(|harvest| {
                harvest.sprout_menu.is_some() || harvest.cellar.is_expanded()
            })
        {
            return;
        }

        // The native drag-and-drop cursor remains authoritative until Winit
        // reports DroppedFile or HoveredFileCancelled.
        if self.file_drop_hovered {
            return;
        }

        // Set resize cursor when hovering over pane dividers
        if !self.pointer_panel_owns_input()
            && let Some(tabs) = &self.tabs
        {
            if let Some(tab) = tabs.active_tab() {
                if let Some(ref pm) = tab.pane_manager {
                    let local_x = position.0 as f32 - content_left - padding;
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
                    let info = self.session_panel.build_info();
                    let layout = crate::session::panel::SessionPanelLayout::calculate(
                        size.width as f32,
                        size.height as f32,
                        ui_cw,
                        cell_height,
                        info.animation,
                        info.rename_text.is_some(),
                        info.workspaces,
                        info.sessions.len(),
                    );
                    let hovered = if px >= layout.panel.x {
                        layout
                            .visible_row_at(py)
                            .map(|row| info.scroll_offset + row)
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

        self.refresh_terminal_pointer_cursor();

        if self.try_report_mouse_motion(position, cell_width, cell_height, content_top, padding) {
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

        if self.search.is_filter_mode() && self.search.has_results() {
            let viewport_rows = self
                .active_terminal()
                .map(|terminal| terminal.grid().rows())
                .unwrap_or(0);
            self.search.scroll_filter(delta, viewport_rows);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        let direction = if delta > 0.0 {
            crate::input::WheelDirection::Up
        } else {
            crate::input::WheelDirection::Down
        };
        if self.try_report_wheel(direction) {
            return;
        }

        let mode = self.active_keyboard_mode();
        if !self.modifiers.shift_key()
            && self.pointer_is_over_focused_terminal()
            && let Some(bytes) = crate::input::encode_wheel_as_cursor_key(mode, direction)
        {
            self.write_terminal_protocol_input(bytes);
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

    /// Horizontal wheel events are reportable to terminal applications but do
    /// not have a native scrollback action.
    pub(super) fn handle_horizontal_scroll(&mut self, delta: f64) {
        if delta.abs() < 0.001 {
            return;
        }
        let direction = if delta > 0.0 {
            crate::input::WheelDirection::Left
        } else {
            crate::input::WheelDirection::Right
        };
        self.try_report_wheel(direction);
    }

    fn try_report_mouse_press(
        &mut self,
        button: MouseButton,
        position: (f64, f64),
        cell_width: f32,
        cell_height: f32,
        content_top: f32,
        padding: f32,
    ) -> bool {
        if self.modifiers.shift_key() {
            return false;
        }
        let Some(button) = report_button(button) else {
            return false;
        };
        let mode = self.active_keyboard_mode();
        if !mode.has_mouse_mode() {
            return false;
        }
        let Some((col, row)) =
            self.protocol_grid_position(position, cell_width, cell_height, content_top, padding)
        else {
            return false;
        };
        let Some(bytes) = crate::input::encode_mouse_report(
            mode,
            crate::input::MouseReportKind::Press(button),
            col,
            row,
            self.modifiers,
        ) else {
            return false;
        };
        let Some(owner) = self.focused_pointer_terminal() else {
            return false;
        };
        if !self.write_terminal_protocol_input(&bytes) {
            return false;
        }
        self.terminal_mouse_capture = Some(TerminalMouseCapture { owner, button });
        self.mouse_pressed = false;
        self.mouse_state.clear_selection();
        if let Some(terminal) = self.active_terminal_mut() {
            terminal.set_selection(None);
        }
        true
    }

    fn try_report_mouse_release(
        &mut self,
        button: MouseButton,
        position: (f64, f64),
        cell_width: f32,
        cell_height: f32,
        content_top: f32,
        padding: f32,
    ) -> bool {
        let Some(button) = report_button(button) else {
            return false;
        };
        let Some(capture) = self
            .terminal_mouse_capture
            .filter(|capture| capture.button == button)
        else {
            return false;
        };
        self.terminal_mouse_capture = None;
        // A release must never migrate to a newly focused or replacement PTY.
        // Do not invent coordinates or deliver input to an unfocused owner.
        if self.focused_pointer_terminal() != Some(capture.owner) {
            return true;
        }
        let mode = self.active_keyboard_mode();
        let Some((col, row)) =
            self.protocol_grid_position(position, cell_width, cell_height, content_top, padding)
        else {
            return true;
        };
        if let Some(bytes) = crate::input::encode_mouse_report(
            mode,
            crate::input::MouseReportKind::Release(button),
            col,
            row,
            self.modifiers,
        ) {
            self.write_terminal_protocol_input(&bytes);
        }
        true
    }

    fn try_report_mouse_motion(
        &mut self,
        position: (f64, f64),
        cell_width: f32,
        cell_height: f32,
        content_top: f32,
        padding: f32,
    ) -> bool {
        self.reconcile_terminal_mouse_capture();
        let mode = self.active_keyboard_mode();
        if !mode.has_mouse_mode() {
            self.terminal_mouse_capture = None;
            return false;
        }
        if self.modifiers.shift_key() && self.terminal_mouse_capture.is_none() {
            return false;
        }
        let Some((col, row)) =
            self.protocol_grid_position(position, cell_width, cell_height, content_top, padding)
        else {
            return true;
        };
        if let Some(bytes) = crate::input::encode_mouse_report(
            mode,
            crate::input::MouseReportKind::Motion(
                self.terminal_mouse_capture.map(|capture| capture.button),
            ),
            col,
            row,
            self.modifiers,
        ) {
            self.write_terminal_protocol_input(&bytes);
        }
        true
    }

    fn try_report_wheel(&mut self, direction: crate::input::WheelDirection) -> bool {
        if self.modifiers.shift_key() {
            return false;
        }
        let mode = self.active_keyboard_mode();
        if !mode.has_mouse_mode() {
            return false;
        }
        let Some(position) = self.last_cursor_pos else {
            return false;
        };
        let Some(renderer) = &self.renderer else {
            return false;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 16.0
        } else {
            0.0
        };
        let content_top =
            renderer.tab_bar_height() + renderer.chrome_bar_height() + search_bar_height;
        let padding = renderer.padding();
        let Some((col, row)) =
            self.protocol_grid_position(position, cell_width, cell_height, content_top, padding)
        else {
            return false;
        };
        let Some(bytes) = crate::input::encode_mouse_report(
            mode,
            crate::input::MouseReportKind::Wheel(direction),
            col,
            row,
            self.modifiers,
        ) else {
            return false;
        };
        self.write_terminal_protocol_input(&bytes)
    }

    fn pointer_is_over_focused_terminal(&self) -> bool {
        let Some(position) = self.last_cursor_pos else {
            return false;
        };
        let Some(renderer) = &self.renderer else {
            return false;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            cell_height + 16.0
        } else {
            0.0
        };
        self.protocol_grid_position(
            position,
            cell_width,
            cell_height,
            renderer.tab_bar_height() + renderer.chrome_bar_height() + search_bar_height,
            renderer.padding(),
        )
        .is_some()
    }

    /// Restore the protocol-selected cursor when no Termeric-owned pointer
    /// interaction currently has precedence.
    pub(super) fn refresh_terminal_pointer_cursor(&self) {
        if self.file_drop_hovered
            || self.divider_dragging.is_some()
            || self.scrollbar_dragging.is_some()
            || self.tab_dragging.is_some()
            || self.info_overlay.is_active()
            || self.diff_review.is_active()
            || self.context_picker.is_active()
            || self.domain_picker.is_active()
            || self.unicode_picker.is_active()
            || self.guardrail.is_active()
            || self.command_palette.is_active()
            || self.shortcuts_panel.is_active()
            || self.settings_panel.is_active()
            || self.commands_panel.is_open()
            || self.session_panel.is_open()
            || self.session_dropdown_open
        {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };
        let icon = if !self.pointer_is_over_focused_terminal() {
            winit::window::CursorIcon::Default
        } else if self.mouse_pressed {
            winit::window::CursorIcon::Text
        } else if let Some(terminal) = self.active_terminal() {
            native_pointer_icon(terminal.effective_pointer_shape())
        } else {
            winit::window::CursorIcon::Default
        };
        window.set_cursor(icon);
    }

    /// Convert a physical cursor position to a visible, zero-based cell in
    /// the focused terminal. Protocol coordinates never include scrollback.
    fn protocol_grid_position(
        &self,
        position: (f64, f64),
        cell_width: f32,
        cell_height: f32,
        content_top: f32,
        padding: f32,
    ) -> Option<(usize, usize)> {
        if !position.0.is_finite()
            || !position.1.is_finite()
            || cell_width <= 0.0
            || cell_height <= 0.0
        {
            return None;
        }
        let terminal = self.active_terminal()?;
        let content_left = self
            .renderer
            .as_ref()
            .map_or(0.0, crate::renderer::Renderer::tab_bar_width);
        let (origin_x, origin_y, viewport_width, viewport_height) = if let Some(tab) =
            self.tabs.as_ref().and_then(TabManager::active_tab)
            && let Some(panes) = &tab.pane_manager
            && let Some(rect) = panes.calculate_layout().get(&panes.focused_id())
        {
            (
                content_left + padding + rect.x,
                content_top + rect.y,
                rect.width,
                rect.height,
            )
        } else {
            (
                content_left + padding,
                content_top + padding,
                terminal.cols() as f32 * cell_width,
                terminal.rows() as f32 * cell_height,
            )
        };
        protocol_cell_from_viewport(
            position,
            (origin_x, origin_y),
            (viewport_width, viewport_height),
            (cell_width, cell_height),
            (terminal.cols(), terminal.rows()),
        )
    }

    /// Convert a screen position through the focused pane's viewport. Pane
    /// rectangles are relative to `content_top`; unlike single-grid mode they
    /// already include their own edge spacing.
    fn focused_grid_position(
        &self,
        position: (f64, f64),
        cell_width: f32,
        cell_height: f32,
        content_top: f32,
        padding: f32,
    ) -> termeric_core::Point {
        if let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab)
            && let Some(panes) = &tab.pane_manager
            && let Some(pane) = panes.focused_pane()
            && let Some(rect) = panes.calculate_layout().get(&pane.id)
        {
            return screen_to_pane_grid(
                position,
                cell_width,
                cell_height,
                self.renderer
                    .as_ref()
                    .map_or(0.0, crate::renderer::Renderer::tab_bar_width),
                content_top,
                padding,
                *rect,
                pane.terminal.grid().scroll_offset(),
            );
        }

        let scroll_offset = self
            .active_terminal()
            .and_then(|terminal| i32::try_from(terminal.grid().scroll_offset()).ok())
            .unwrap_or(0);
        screen_to_grid(
            position.0
                - f64::from(
                    self.renderer
                        .as_ref()
                        .map_or(0.0, crate::renderer::Renderer::tab_bar_width),
                ),
            position.1,
            cell_width,
            cell_height,
            content_top,
            padding,
            scroll_offset,
        )
    }

    /// Toggle the armed broadcast membership of the pane under a content click.
    fn toggle_armed_broadcast_target_at(
        &mut self,
        position: (f64, f64),
        content_top: f32,
        padding: f32,
    ) -> bool {
        if !matches!(self.broadcast, BroadcastState::Armed { .. }) {
            return false;
        }
        let content_left = self
            .renderer
            .as_ref()
            .map_or(0.0, crate::renderer::Renderer::tab_bar_width);
        let local_x = position.0 as f32 - content_left - padding;
        let local_y = position.1 as f32 - content_top;
        let target = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.pane_manager.as_ref())
            .and_then(|panes| {
                panes.calculate_layout().into_iter().find_map(|(id, rect)| {
                    (local_x >= rect.x
                        && local_x < rect.x + rect.width
                        && local_y >= rect.y
                        && local_y < rect.y + rect.height)
                        .then_some(id)
                })
            });
        target.is_some_and(|target| self.toggle_armed_broadcast_target(target))
    }

    /// Focus the pane under a content click. Returns whether focus moved.
    fn focus_pane_at(&mut self, position: (f64, f64), content_top: f32, padding: f32) -> bool {
        let content_left = self
            .renderer
            .as_ref()
            .map_or(0.0, crate::renderer::Renderer::tab_bar_width);
        let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) else {
            return false;
        };
        let Some(panes) = &mut tab.pane_manager else {
            return false;
        };
        let local_x = position.0 as f32 - content_left - padding;
        let local_y = position.1 as f32 - content_top;
        let target = panes.calculate_layout().into_iter().find_map(|(id, rect)| {
            (local_x >= rect.x
                && local_x < rect.x + rect.width
                && local_y >= rect.y
                && local_y < rect.y + rect.height)
                .then_some(id)
        });
        let Some(target) = target else { return false };
        if target == panes.focused_id() {
            return false;
        }
        panes.focus(target);
        let route = tab
            .live_domain_focus_route()
            .map(|(domain, pane)| (domain.to_string(), pane));
        if let (Some(authority), Some((domain, pane))) = (self.live_domain_authority.clone(), route)
        {
            crate::domain::request_focus_background(authority, domain, pane);
        }
        true
    }

    /// Rebind global surfaces that intentionally describe only the focused
    /// pane. Per-pane selection remains stored with each terminal.
    pub(super) fn refresh_focused_terminal_surfaces(&mut self) {
        self.tab_drag_scroll = None;
        self.reconcile_divider_drag();
        self.reconcile_scrollbar_drag();
        self.reconcile_terminal_mouse_capture();
        self.reconcile_broadcast_state();
        let active_tab_id = self.tabs.as_ref().and_then(TabManager::active_tab_id);
        if self
            .tab_rename
            .as_ref()
            .is_some_and(|rename| Some(rename.tab_id) != active_tab_id)
            && let Some(rename) = self.tab_rename.take()
        {
            if let Some(tab) = self.tabs.as_mut().and_then(|tabs| {
                tabs.all_tabs_mut()
                    .iter_mut()
                    .find(|tab| tab.id == rename.tab_id)
            }) {
                tab.set_user_title(rename.original);
            }
            self.toasts.info("Tab rename cancelled after focus changed");
        }
        // A composition belongs to the pane that owned focus when it began.
        // Cancel it instead of displaying or committing it in another pane.
        self.ime.clear_preedit();
        self.mouse_state.clear_selection();
        self.with_search_and_active_grid(|search, grid| search.refresh(grid));
        if let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut)
            && tab
                .focused_terminal()
                .and_then(termeric_core::Terminal::reported_working_directory)
                .is_some()
        {
            tab.update_title();
        }

        if let Some(grid) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(crate::tabs::Tab::focused_terminal)
            .map(termeric_core::Terminal::grid)
        {
            self.url_cache = UrlCache::new(grid.rows());
            update_visible_url_cache(&mut self.url_cache, grid, &self.url_detector);
        }
        self.git_watcher.invalidate();
        self.reset_cursor_blink();
        self.refresh_terminal_pointer_cursor();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Check if a click position is within the scrollbar area
    pub(super) fn is_click_on_scrollbar(&self, x: f64, y: f64) -> bool {
        let Some(renderer) = &self.renderer else {
            return false;
        };
        let (window_width, _) = renderer.window_size();

        // Scrollbar is 6px wide + 2px margin from right edge, with an expanded hit area
        let scrollbar_hit_width = 16.0_f32;
        let scrollbar_x = window_width as f32 - scrollbar_hit_width;

        let search_bar_height = if self.search.is_active() || self.search.has_results() {
            renderer.cell_height() + 16.0
        } else {
            0.0
        };
        let content_top =
            renderer.tab_bar_height() + renderer.chrome_bar_height() + search_bar_height;
        let padding = renderer.padding();
        let content_left = renderer.tab_bar_width();

        if let Some(panes) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.pane_manager.as_ref())
        {
            return panes.iter_with_layout().any(|(pane, rect)| {
                let pane_right = content_left + padding + rect.x + rect.width;
                let pane_top = content_top + rect.y;
                pane.terminal.grid().scrollback_len() > 0
                    && (x as f32) >= pane_right - scrollbar_hit_width
                    && (x as f32) < pane_right
                    && (y as f32) >= pane_top
                    && (y as f32) < pane_top + rect.height
            });
        }

        // Check if there's actually scrollback content
        let has_scrollback = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .and_then(crate::tabs::Tab::focused_terminal)
            .is_some_and(|terminal| terminal.grid().scrollback_len() > 0);

        has_scrollback && x as f32 >= scrollbar_x
    }

    /// Map a vertical mouse position to a scroll offset and apply it
    pub(super) fn scroll_to_scrollbar_position(&mut self, y: f64) {
        if !self.reconcile_scrollbar_drag() {
            return;
        }
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
        let content_top = tab_bar_height + renderer.chrome_bar_height() + search_bar_height;
        let scrollbar_top = content_top + padding;
        let scrollbar_bottom = window_height as f32 - padding;
        let (scrollbar_top, scrollbar_bottom) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.pane_manager.as_ref())
            .and_then(|panes| panes.calculate_layout().get(&panes.focused_id()).copied())
            .map(|rect| (content_top + rect.y, content_top + rect.y + rect.height))
            .unwrap_or((scrollbar_top, scrollbar_bottom));
        let scrollbar_height = scrollbar_bottom - scrollbar_top;

        if scrollbar_height <= 0.0 {
            return;
        }

        // Fraction from top (0.0=top, 1.0=bottom)
        let fraction = ((y as f32 - scrollbar_top) / scrollbar_height).clamp(0.0, 1.0);

        // Invert: top of scrollbar = max scroll, bottom = 0 scroll
        let scroll_fraction = 1.0 - fraction;

        self.apply_scrollbar_drag_fraction(scroll_fraction);
    }
}

fn report_button(button: MouseButton) -> Option<crate::input::MouseReportButton> {
    match button {
        MouseButton::Left => Some(crate::input::MouseReportButton::Left),
        MouseButton::Middle => Some(crate::input::MouseReportButton::Middle),
        MouseButton::Right => Some(crate::input::MouseReportButton::Right),
        MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => None,
    }
}

pub(super) const fn native_pointer_icon(
    shape: termeric_core::PointerShape,
) -> winit::window::CursorIcon {
    use termeric_core::PointerShape;
    use winit::window::CursorIcon;

    match shape {
        PointerShape::Alias => CursorIcon::Alias,
        PointerShape::Cell => CursorIcon::Cell,
        PointerShape::Copy => CursorIcon::Copy,
        PointerShape::Crosshair => CursorIcon::Crosshair,
        PointerShape::Default => CursorIcon::Default,
        PointerShape::EResize => CursorIcon::EResize,
        PointerShape::EwResize => CursorIcon::EwResize,
        PointerShape::Grab => CursorIcon::Grab,
        PointerShape::Grabbing => CursorIcon::Grabbing,
        PointerShape::Help => CursorIcon::Help,
        PointerShape::Move => CursorIcon::Move,
        PointerShape::NResize => CursorIcon::NResize,
        PointerShape::NeResize => CursorIcon::NeResize,
        PointerShape::NeswResize => CursorIcon::NeswResize,
        PointerShape::NoDrop => CursorIcon::NoDrop,
        PointerShape::NotAllowed => CursorIcon::NotAllowed,
        PointerShape::NsResize => CursorIcon::NsResize,
        PointerShape::NwResize => CursorIcon::NwResize,
        PointerShape::NwseResize => CursorIcon::NwseResize,
        PointerShape::Pointer => CursorIcon::Pointer,
        PointerShape::Progress => CursorIcon::Progress,
        PointerShape::SResize => CursorIcon::SResize,
        PointerShape::SeResize => CursorIcon::SeResize,
        PointerShape::SwResize => CursorIcon::SwResize,
        PointerShape::Text => CursorIcon::Text,
        PointerShape::VerticalText => CursorIcon::VerticalText,
        PointerShape::WResize => CursorIcon::WResize,
        PointerShape::Wait => CursorIcon::Wait,
        PointerShape::ZoomIn => CursorIcon::ZoomIn,
        PointerShape::ZoomOut => CursorIcon::ZoomOut,
    }
}

#[cfg(test)]
mod pointer_tests {
    use super::native_pointer_icon;
    use termeric_core::PointerShape;
    use winit::window::CursorIcon;

    #[test]
    fn every_protocol_pointer_shape_has_an_exact_native_mapping() {
        let expected = [
            CursorIcon::Alias,
            CursorIcon::Cell,
            CursorIcon::Copy,
            CursorIcon::Crosshair,
            CursorIcon::Default,
            CursorIcon::EResize,
            CursorIcon::EwResize,
            CursorIcon::Grab,
            CursorIcon::Grabbing,
            CursorIcon::Help,
            CursorIcon::Move,
            CursorIcon::NResize,
            CursorIcon::NeResize,
            CursorIcon::NeswResize,
            CursorIcon::NoDrop,
            CursorIcon::NotAllowed,
            CursorIcon::NsResize,
            CursorIcon::NwResize,
            CursorIcon::NwseResize,
            CursorIcon::Pointer,
            CursorIcon::Progress,
            CursorIcon::SResize,
            CursorIcon::SeResize,
            CursorIcon::SwResize,
            CursorIcon::Text,
            CursorIcon::VerticalText,
            CursorIcon::WResize,
            CursorIcon::Wait,
            CursorIcon::ZoomIn,
            CursorIcon::ZoomOut,
        ];
        assert_eq!(PointerShape::ALL.map(native_pointer_icon), expected);
    }
}

fn protocol_cell_from_viewport(
    position: (f64, f64),
    origin: (f32, f32),
    viewport: (f32, f32),
    cell_size: (f32, f32),
    grid_size: (usize, usize),
) -> Option<(usize, usize)> {
    if !position.0.is_finite()
        || !position.1.is_finite()
        || cell_size.0 <= 0.0
        || cell_size.1 <= 0.0
    {
        return None;
    }
    let local_x = position.0 as f32 - origin.0;
    let local_y = position.1 as f32 - origin.1;
    if local_x < 0.0 || local_y < 0.0 || local_x >= viewport.0 || local_y >= viewport.1 {
        return None;
    }
    let col = (local_x / cell_size.0).floor() as usize;
    let row = (local_y / cell_size.1).floor() as usize;
    (col < grid_size.0 && row < grid_size.1).then_some((col, row))
}

fn screen_to_pane_grid(
    position: (f64, f64),
    cell_width: f32,
    cell_height: f32,
    content_left: f32,
    content_top: f32,
    padding: f32,
    rect: crate::panes::PaneRect,
    scroll_offset: usize,
) -> termeric_core::Point {
    let local_x = (position.0 as f32 - content_left - padding - rect.x).max(0.0);
    let local_y = (position.1 as f32 - content_top - rect.y).max(0.0);
    let col = (local_x / cell_width).floor() as usize;
    let visual_row = (local_y / cell_height).floor() as i32;
    let scroll_offset = i32::try_from(scroll_offset).unwrap_or(i32::MAX);
    termeric_core::Point::new(visual_row - scroll_offset, col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::PaneRect;

    #[cfg(unix)]
    fn mouse_report_position(app: &App) -> (f64, f64) {
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        let (x, y) = tab.pane_manager.as_ref().map_or((0.0, 0.0), |panes| {
            let rect = panes.calculate_layout()[&panes.focused_id()];
            (rect.x, rect.y)
        });
        ((x + 15.0) as f64, (y + 30.0) as f64)
    }

    #[cfg(unix)]
    #[test]
    fn application_mouse_gesture_does_not_follow_tab_or_pane_focus() {
        use crate::app::tests::{
            current_harvest_capture_routes, fenced_harvest_capture,
            harvest_rerun_capture_fixture_in,
        };
        use termeric_core::term::TerminalMode;
        for split in [false, true] {
            for motion in [false, true] {
                let (mut app, directory, _, mut captures) = harvest_rerun_capture_fixture_in(split);
                if !split {
                    app.tabs.as_mut().unwrap().new_tab().unwrap();
                    captures.extend(current_harvest_capture_routes(&mut app, directory.path()));
                    app.tabs.as_mut().unwrap().switch_to(0);
                } else {
                    app.tabs
                        .as_mut()
                        .unwrap()
                        .active_tab_mut()
                        .unwrap()
                        .pane_manager
                        .as_mut()
                        .unwrap()
                        .focus(PaneId(0));
                }
                let mode = TerminalMode::MOUSE_BUTTON_EVENT | TerminalMode::MOUSE_SGR;
                app.active_terminal_mut().unwrap().set_mode(mode, true);
                let position = mouse_report_position(&app);
                assert!(app.try_report_mouse_press(
                    MouseButton::Left,
                    position,
                    10.0,
                    20.0,
                    0.0,
                    0.0
                ));
                // Change ownership without a refresh callback: the write boundary
                // itself must prevent stale presses reaching another application.
                if split {
                    let panes = app
                        .tabs
                        .as_mut()
                        .unwrap()
                        .active_tab_mut()
                        .unwrap()
                        .pane_manager
                        .as_mut()
                        .unwrap();
                    let other = panes
                        .pane_ids()
                        .into_iter()
                        .find(|id| *id != PaneId(0))
                        .unwrap();
                    panes.focus(other);
                } else {
                    app.tabs.as_mut().unwrap().switch_to(1);
                }
                app.active_terminal_mut().unwrap().set_mode(mode, true);
                let position = mouse_report_position(&app);
                if motion {
                    app.try_report_mouse_motion(position, 10.0, 20.0, 0.0, 0.0);
                }
                app.try_report_mouse_release(MouseButton::Left, position, 10.0, 20.0, 0.0, 0.0);
                let bytes = fenced_harvest_capture(&mut app, &captures);
                assert_eq!(bytes[0], b"\x1b[<0;2;2M");
                assert!(
                    bytes[1..].iter().all(Vec::is_empty),
                    "split={split}, motion={motion}: {bytes:?}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn application_mouse_capture_preserves_valid_drag_release_and_hover() {
        use crate::app::tests::{fenced_harvest_capture, harvest_rerun_capture_fixture_in};
        use termeric_core::term::TerminalMode;
        let (mut app, _directory, _, captures) = harvest_rerun_capture_fixture_in(false);
        app.active_terminal_mut().unwrap().set_mode(
            TerminalMode::MOUSE_BUTTON_EVENT | TerminalMode::MOUSE_SGR,
            true,
        );
        let position = mouse_report_position(&app);
        assert!(app.try_report_mouse_press(MouseButton::Left, position, 10.0, 20.0, 0.0, 0.0));
        let capture = app.terminal_mouse_capture;
        app.refresh_focused_terminal_surfaces();
        assert_eq!(app.terminal_mouse_capture, capture);
        assert!(!app.try_report_mouse_release(MouseButton::Right, position, 10.0, 20.0, 0.0, 0.0));
        assert_eq!(app.terminal_mouse_capture, capture);
        // Modifier changes must not turn a captured application drag into selection.
        app.modifiers = ModifiersState::SHIFT | ModifiersState::ALT;
        assert!(app.try_report_mouse_motion(position, 10.0, 20.0, 0.0, 0.0));
        assert!(app.try_report_mouse_release(MouseButton::Left, position, 10.0, 20.0, 0.0, 0.0));
        assert!(app.terminal_mouse_capture.is_none());
        assert!(!app.try_report_mouse_release(MouseButton::Left, position, 10.0, 20.0, 0.0, 0.0));
        app.modifiers = ModifiersState::empty();
        // DECSET 1003 hover remains allowed without a captured button.
        app.active_terminal_mut()
            .unwrap()
            .set_mode(TerminalMode::MOUSE_ANY_EVENT, true);
        assert!(app.try_report_mouse_motion(position, 10.0, 20.0, 0.0, 0.0));
        assert_eq!(
            fenced_harvest_capture(&mut app, &captures)[0],
            b"\x1b[<0;2;2M\x1b[<44;2;2M\x1b[<12;2;2m\x1b[<35;2;2M"
        );
    }

    #[cfg(unix)]
    #[test]
    fn application_mouse_capture_cancellation_cannot_resume_after_focus_returns() {
        use crate::app::tests::{fenced_harvest_capture, harvest_rerun_capture_fixture_in};
        use termeric_core::term::TerminalMode;
        for closed in [false, true] {
            let (mut app, _directory, _, captures) = harvest_rerun_capture_fixture_in(true);
            let original = app
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap()
                .focused_id();
            let original_index = captures.iter().position(|(origin, _)| matches!(
                origin, crate::tabs::TerminalEventOrigin::Pane { pane_id, .. } if *pane_id == original
            )).unwrap();
            app.active_terminal_mut().unwrap().set_mode(
                TerminalMode::MOUSE_BUTTON_EVENT | TerminalMode::MOUSE_SGR,
                true,
            );
            let position = mouse_report_position(&app);
            assert!(app.try_report_mouse_press(MouseButton::Left, position, 10.0, 20.0, 0.0, 0.0));
            let panes = app
                .tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .pane_manager
                .as_mut()
                .unwrap();
            if closed {
                panes.get_pane_mut(original).unwrap().closed = true;
            } else {
                let other = panes
                    .pane_ids()
                    .into_iter()
                    .find(|id| *id != original)
                    .unwrap();
                panes.focus(other);
            }
            app.refresh_focused_terminal_surfaces();
            assert!(app.terminal_mouse_capture.is_none());
            let panes = app
                .tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .pane_manager
                .as_mut()
                .unwrap();
            panes.get_pane_mut(original).unwrap().closed = false;
            panes.focus(original);
            app.try_report_mouse_motion(position, 10.0, 20.0, 0.0, 0.0);
            app.try_report_mouse_release(MouseButton::Left, position, 10.0, 20.0, 0.0, 0.0);
            let bytes = fenced_harvest_capture(&mut app, &captures);
            for (index, bytes) in bytes.iter().enumerate() {
                assert_eq!(
                    bytes.as_slice(),
                    if index == original_index {
                        b"\x1b[<0;2;2M".as_slice()
                    } else {
                        b""
                    }
                );
            }
        }
    }

    #[test]
    fn pane_grid_coordinates_include_pane_origin_and_scrollback() {
        let point = screen_to_pane_grid(
            (196.0, 174.0),
            10.0,
            20.0,
            50.0,
            50.0,
            6.0,
            PaneRect {
                x: 100.0,
                y: 60.0,
                width: 200.0,
                height: 160.0,
            },
            3,
        );
        assert_eq!(point, termeric_core::Point::new(0, 4));
    }

    #[test]
    fn protocol_coordinates_are_visible_one_pane_local_cells() {
        let cell = protocol_cell_from_viewport(
            (146.0, 174.0),
            (106.0, 110.0),
            (200.0, 160.0),
            (10.0, 20.0),
            (20, 8),
        );
        assert_eq!(cell, Some((4, 3)));
    }

    #[test]
    fn protocol_coordinates_reject_chrome_edges_and_oversized_values() {
        let args = ((6.0, 56.0), (800.0, 480.0), (10.0, 20.0), (80, 24));
        assert_eq!(
            protocol_cell_from_viewport((5.99, 100.0), args.0, args.1, args.2, args.3),
            None
        );
        assert_eq!(
            protocol_cell_from_viewport((806.0, 100.0), args.0, args.1, args.2, args.3),
            None
        );
        assert_eq!(
            protocol_cell_from_viewport((20_000.0, 20_000.0), args.0, args.1, args.2, args.3),
            None
        );
        assert_eq!(
            protocol_cell_from_viewport((f64::NAN, 100.0), args.0, args.1, args.2, args.3),
            None
        );
    }
}
