//! Mouse routing for modal layers, chrome, panes, and terminal content.

use super::modal_layer::ModalLayer;
use super::*;
use crate::renderer::PanePlacement;
use crate::ui::geometry::Bounds;

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
                    && tab
                        .panes
                        .get_dividers()
                        .iter()
                        .any(|divider| divider.target == drag.target)
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
            .and_then(|tab| tab.panes.resize_divider_target(drag.target, x, y));
        if let Some(ratio_millis) = ratio {
            if let Some(active_drag) = &mut self.divider_dragging {
                active_drag.last_ratio_millis = ratio_millis;
            }
        } else {
            self.finish_native_pointer_gesture();
        }
    }

    pub(super) fn focused_pointer_terminal(&self) -> Option<crate::tabs::TerminalIdentity> {
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

    /// A second click on the same workspace row within the desktop's usual
    /// double-click interval opens it; a single click only selects.
    fn promote_session_double_click(
        &mut self,
        action: crate::ui::session_panel::SessionPanelAction,
    ) -> crate::ui::session_panel::SessionPanelAction {
        use crate::ui::session_panel::SessionPanelAction;
        const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);
        let SessionPanelAction::Select(id) = action else {
            self.session_row_click = None;
            return action;
        };
        let now = std::time::Instant::now();
        let repeated = self
            .session_row_click
            .as_ref()
            .is_some_and(|(last, at)| *last == id && now.duration_since(*at) <= DOUBLE_CLICK);
        if repeated {
            self.session_row_click = None;
            SessionPanelAction::Load(id)
        } else {
            self.session_row_click = Some((id.clone(), now));
            SessionPanelAction::Select(id)
        }
    }

    /// These surfaces are dispatched here, below the event-loop overlays but
    /// above terminal scrollbars/dividers. Keep their ownership independent
    /// of where the pointer lands (including outside-click dismissal).
    pub(super) fn pointer_panel_owns_input(&self) -> bool {
        self.top_modal_layer().is_some_and(ModalLayer::is_panel)
    }

    /// A terminal-side drag is in progress: a selection, scrollbar, pane
    /// divider, or tab. It keeps the pointer even over the docked panel.
    pub(super) fn native_pointer_gesture_active(&self) -> bool {
        self.mouse_pressed
            || self.scrollbar_dragging.is_some()
            || self.divider_dragging.is_some()
            || self.tab_dragging.is_some()
    }

    /// A modal takes native pointer ownership even if the physical button
    /// remains held. Do this before dispatch/dismissal and animation cleanup,
    /// not only on release (which the modal may consume). The agent view's
    /// divider and text drags end with them: their release may never come.
    pub(super) fn finish_obscured_native_pointer_gesture(&mut self) {
        if self.top_modal_layer().is_some() {
            self.finish_native_pointer_gesture();
            self.cancel_agent_view_gestures();
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
        self.finish_native_selection();
        self.scrollbar_dragging = None;
        let divider_drag = self.divider_dragging.take();
        let tab_drag = self.tab_dragging.take();
        if let Some(drag) = &tab_drag {
            self.settle_dragged_tab(drag, Instant::now());
        }
        self.tab_drag_scroll = None;
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
            grab: _,
            live:
                Some(LiveDomainTabDrag {
                    domain_id,
                    remote_tab_id,
                    initial_index,
                }),
        }) = tab_drag
        {
            // The dragged tab must still be open and still show that remote tab.
            let index = self.tabs.as_ref().and_then(|tabs| {
                tabs.iter()
                    .map(|(_, tab)| tab)
                    .find(|tab| tab.id == tab_id && !tab.closed && !tab.is_closing())
                    .filter(|tab| {
                        tab.live_domain_tab_route() == Some((domain_id.as_str(), remote_tab_id))
                    })?;
                u16::try_from(tabs.live_domain_tab_index(&domain_id, remote_tab_id)?).ok()
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

    /// A primary press at `(local_x, local_y)` in the active tab's pane area.
    /// On a divider, within its hit margin, it starts a drag of that exact
    /// divider, or says why it cannot, and returns true. A zoomed tab shows
    /// no dividers.
    pub(super) fn press_pane_divider(&mut self, local_x: f32, local_y: f32) -> bool {
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            return false;
        };
        let Some(divider) = tab.split_panes().and_then(|panes| {
            panes.divider_at(local_x, local_y, crate::panes::PANE_DIVIDER_HIT_MARGIN)
        }) else {
            return false;
        };
        let route = tab.live_domain_tab_route();
        let pending = route.is_some_and(|(domain, _)| {
            self.pending_live_domain_controls
                .iter()
                .any(|pending| pending.domain_id() == domain)
        });
        if pending {
            self.toasts
                .info("Wait for the pending live-domain change before resizing");
            return true;
        }
        if tab.live_domain.is_some()
            && !matches!(divider.target, crate::panes::DividerTarget::Domain(_))
        {
            self.toasts
                .error("Live-domain divider is missing its daemon identity");
        } else {
            self.divider_dragging = Some(DividerDrag {
                tab_id: tab.id,
                domain: route.map(|(domain, remote_tab)| (domain.to_owned(), remote_tab)),
                target: divider.target,
                last_ratio_millis: divider.ratio_millis,
            });
        }
        self.request_redraw(RedrawReason::PointerInput);
        true
    }

    /// Handle mouse input
    pub(super) fn handle_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
        position: (f64, f64),
    ) {
        self.finish_obscured_native_pointer_gesture();
        // The docked agent view owns its area below the side panels, which
        // slide over it, and above terminal scrollbars and dividers.
        if !self.pointer_panel_owns_input() {
            let (x, y) = (position.0 as f32, position.1 as f32);
            let handled = match state {
                ElementState::Pressed => self.handle_agent_view_press(x, y, button),
                ElementState::Released => self.handle_agent_view_release(),
            };
            if handled {
                return;
            }
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let padding = renderer.padding();
        let content_left = renderer.tab_bar_width();
        let content_top = renderer.content_top();
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
                        self.request_redraw(RedrawReason::PointerInput);
                        return;
                    }

                    // A press on a pane divider starts a drag-to-resize.
                    if self.press_pane_divider(
                        position.0 as f32 - content_left - padding,
                        position.1 as f32 - content_top,
                    ) {
                        return;
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
                                    self.request_redraw(RedrawReason::PointerInput);
                                    return;
                                }
                            }
                        }

                        // If commands panel is open, intercept all clicks
                        if self.modal_layer_on_top(ModalLayer::CommandsPanel) {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                let info = self.commands_panel.build_info(&self.config.commands);
                                let action = renderer.commands_panel_hit_test(
                                    px,
                                    py,
                                    &info,
                                    size.width as f32,
                                    size.height as f32,
                                );
                                self.apply_commands_panel_action(action);
                                self.request_redraw(RedrawReason::PointerInput);
                                return;
                            }
                        }

                        // The agents panel owns every click while open,
                        // including the one beside it that closes it.
                        if self.modal_layer_on_top(ModalLayer::AgentsPanel) {
                            self.handle_agents_panel_click(px, py);
                            return;
                        }

                        // If session management panel is open, intercept all clicks
                        if self.modal_layer_on_top(ModalLayer::SessionPanel) {
                            let window_size = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = window_size {
                                let info = self.session_panel.build_info();
                                let action = renderer.session_panel_hit_test(
                                    px,
                                    py,
                                    &info,
                                    size.width as f32,
                                    size.height as f32,
                                );
                                let action = self.promote_session_double_click(action);
                                self.apply_session_panel_action(action);
                                self.request_redraw(RedrawReason::PointerInput);
                                return;
                            }
                        }

                        // An open menu owns every click: rows activate and
                        // anything outside dismisses it.
                        if self.modal_layer_on_top(ModalLayer::WorkspaceMenu) {
                            self.click_workspace_menu(px, py);
                            return;
                        }

                        // Settings modal intercepts all clicks (inside: handle;
                        // outside: dismiss). Must come before tab bar / search bar
                        // so the modal can't be undercut by chrome under the scrim.
                        if self.modal_layer_on_top(ModalLayer::Settings) {
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
                                self.request_redraw(RedrawReason::PointerInput);
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
                                            self.tab_dragging = tabs.active_tab().and_then(|tab| {
                                                let live = match tab.live_domain_tab_route() {
                                                    Some((domain_id, remote_tab_id)) => {
                                                        Some(LiveDomainTabDrag {
                                                            domain_id: domain_id.to_string(),
                                                            remote_tab_id,
                                                            // A live tab with no index of its
                                                            // own is not dragged at all.
                                                            initial_index: u16::try_from(
                                                                tabs.live_domain_tab_index(
                                                                    domain_id,
                                                                    remote_tab_id,
                                                                )?,
                                                            )
                                                            .ok()?,
                                                        })
                                                    }
                                                    None => None,
                                                };
                                                Some(TabDrag {
                                                    tab_id: tab.id,
                                                    live,
                                                    grab: None,
                                                })
                                            });
                                        }
                                        self.begin_tab_grab();
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
                                        }
                                    }
                                    TabBarClick::SessionDropdown => {
                                        self.toggle_workspace_menu(false);
                                    }
                                    TabBarClick::CommandsButton => {
                                        self.commands_panel.toggle();
                                    }
                                    TabBarClick::None => {}
                                }
                                if click != TabBarClick::None {
                                    self.request_redraw(RedrawReason::PointerInput);
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
                                    SearchBarClick::Query(cursor) => {
                                        self.focus_search_query_at(
                                            cursor,
                                            redraw_scheduler::RedrawReason::PointerInput,
                                        );
                                        return;
                                    }
                                    SearchBarClick::Background => return,
                                    SearchBarClick::ToggleCaseSensitive => {
                                        self.with_search_and_active_grid(|search, grid| {
                                            search.toggle_case_sensitive(grid);
                                        });
                                        self.request_redraw(RedrawReason::PointerInput);
                                        return;
                                    }
                                    SearchBarClick::ToggleRegex => {
                                        self.with_search_and_active_grid(|search, grid| {
                                            search.toggle_regex(grid);
                                        });
                                        self.request_redraw(RedrawReason::PointerInput);
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
                                        self.request_redraw(RedrawReason::PointerInput);
                                        return;
                                    }
                                    SearchBarClick::PrevMatch => {
                                        self.search.prev();
                                        self.scroll_to_current_match();
                                        self.request_redraw(RedrawReason::PointerInput);
                                        return;
                                    }
                                    SearchBarClick::NextMatch => {
                                        self.search.next();
                                        self.scroll_to_current_match();
                                        self.request_redraw(RedrawReason::PointerInput);
                                        return;
                                    }
                                    SearchBarClick::Close => {
                                        self.search.cancel();
                                        self.search_cursor_pos = 0;
                                        self.request_redraw(RedrawReason::PointerInput);
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
                    self.request_redraw(RedrawReason::PointerInput);
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
                    if let Some((url, directory)) = self.focused_hyperlink_open_context(grid_pos) {
                        log::info!("Opening hyperlink: {}", url);
                        self.activate_hyperlink(
                            self.active_terminal_origin(),
                            &url,
                            directory.as_deref(),
                        );
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

                self.begin_native_selection(button, grid_pos);

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
                    self.tab_dropped_outside = self.tab_released_outside(position);
                    self.finish_native_pointer_gesture();
                }
                // Primary-gesture cleanup finishes native selection while
                // retaining the selected text. Other buttons do not own it.
            }
        }

        self.refresh_terminal_pointer_cursor();

        // Request redraw to show selection changes
        self.request_redraw(RedrawReason::PointerInput);
    }

    /// Handle mouse movement
    pub(super) fn handle_mouse_move(&mut self, position: (f64, f64)) {
        self.finish_obscured_native_pointer_gesture();
        // Any modal layer, not only a side panel, hides the agent view from
        // the pointer: no hover, no drag.
        if self.top_modal_layer().is_none()
            && !self.native_pointer_gesture_active()
            && self.handle_agent_view_motion(position.0 as f32, position.1 as f32)
        {
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        let tab_bar_height = renderer.tab_bar_height();
        let padding = renderer.padding();
        let content_left = renderer.tab_bar_width();
        let content_top = renderer.content_top();
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
                self.tab_bar_hover_since = Instant::now();
            }
            self.drag_tab_to_pointer(position);
        }

        // Handle scrollbar dragging
        self.update_tab_drag_scroll(self.tab_bar_hover, Instant::now());
        if self.scrollbar_dragging.is_some() {
            self.scroll_to_scrollbar_position(position.1);
            self.request_redraw(RedrawReason::PointerInput);
            return;
        }

        // Handle pane divider dragging
        if self.divider_dragging.is_some() {
            let local_x = position.0 as f32 - content_left - padding;
            let local_y = position.1 as f32 - content_top;
            self.apply_divider_drag_position(local_x, local_y);
            self.request_redraw(RedrawReason::PointerInput);
            return;
        }

        // Overlays above the panels take no hover of their own here.
        if self
            .top_modal_layer()
            .is_some_and(|layer| !layer.is_panel())
        {
            return;
        }

        // The native drag-and-drop cursor remains authoritative until Winit
        // reports DroppedFile or HoveredFileCancelled.
        if self.file_drop_hovered {
            return;
        }

        // Set resize cursor when hovering over pane dividers. A dragged tab
        // owns the pointer's shape until it is released.
        if !self.pointer_panel_owns_input()
            && self.tab_dragging.is_none()
            && let Some(tabs) = &self.tabs
        {
            if let Some(tab) = tabs.active_tab() {
                if let Some(pm) = tab.split_panes() {
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
        if self.modal_layer_on_top(ModalLayer::Settings) {
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

                    // Row and control hover (suppressed while the dropdown
                    // overlay is open so nothing behind it lights up).
                    let open = !self.settings_panel.is_dropdown_open();
                    let hovered = open.then(|| layout.visible_row_at(px, py)).flatten();
                    self.settings_panel.set_hover(hovered);
                    let control = open
                        .then(|| {
                            use crate::ui::settings::SettingsControl;
                            if layout.close_button.contains(px, py) {
                                Some(SettingsControl::Close)
                            } else if layout.save_button.contains(px, py) {
                                Some(SettingsControl::Save)
                            } else {
                                layout.page_at(px, py).map(SettingsControl::Page)
                            }
                        })
                        .flatten();
                    self.settings_panel.set_control_hover(control);

                    // Dropdown option hover (only when the overlay is open).
                    if self.settings_panel.is_dropdown_open() {
                        let dd_hover = self
                            .settings_panel
                            .dropdown_layout(&layout, &entries)
                            .filter(|dropdown| dropdown.rect.bounds.contains(px, py))
                            .and_then(|dropdown| {
                                dropdown
                                    .rect
                                    .option_at(py)
                                    .filter(|&i| i < dropdown.options.len())
                            });
                        self.settings_panel.set_dropdown_hover(dd_hover);
                    } else {
                        self.settings_panel.set_dropdown_hover(None);
                    }
                }
            }
            self.request_redraw(RedrawReason::PointerInput);
            return;
        }

        if self.modal_layer_on_top(ModalLayer::CommandsPanel) {
            let px = position.0 as f32;
            let py = position.1 as f32;
            let layout = self.commands_panel_layout();
            self.commands_panel.set_hover(layout.control_at(px, py));
            self.request_redraw(RedrawReason::PointerInput);
            return;
        }
        if self.modal_layer_on_top(ModalLayer::AgentsPanel) {
            self.update_agents_panel_hover(position.0 as f32, position.1 as f32);
            return;
        }

        if self.modal_layer_on_top(ModalLayer::SessionPanel) {
            let px = position.0 as f32;
            let py = position.1 as f32;
            if let Some(renderer) = &self.renderer {
                let (ui_cw, cell_height) = renderer.cell_size();
                let window_size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = window_size {
                    let info = self.session_panel.build_info();
                    let layout = crate::ui::session_panel::SessionPanelLayout::calculate(
                        size.width as f32,
                        size.height as f32,
                        renderer.tab_bar_height(),
                        ui_cw,
                        cell_height,
                        info.animation,
                        info.rename_text.is_some(),
                        info.workspaces,
                        info.sessions.len(),
                    );
                    let control = layout.control_at(px, py, info.snapshot_count);
                    let hovered = if px >= layout.panel.x && control.is_none() {
                        layout
                            .visible_row_at(py)
                            .map(|row| info.scroll_offset + row)
                    } else {
                        None
                    };
                    self.session_panel.set_hover(hovered, control);
                }
            }
            self.request_redraw(RedrawReason::PointerInput);
            return;
        }

        if self.modal_layer_on_top(ModalLayer::WorkspaceMenu) {
            if self.hover_workspace_menu(position.0 as f32, position.1 as f32) {
                self.request_redraw(RedrawReason::PointerInput);
            }
            return;
        }

        self.refresh_terminal_pointer_cursor();

        if self.try_report_mouse_motion(position, cell_width, cell_height, content_top, padding) {
            return;
        }

        // Only update selection when mouse is pressed (dragging)
        if self.mouse_pressed {
            self.extend_native_selection(grid_pos);
            self.request_redraw(RedrawReason::PointerInput);
        }
    }

    /// Handle scroll wheel. The wheel goes to the pane under the pointer,
    /// which it never focuses, or to the focused pane when the pointer is
    /// over no pane.
    pub(super) fn handle_scroll(&mut self, delta: f64) {
        // Filter out near-zero deltas (release artifacts from trackpad/mouse)
        // These appear as -0.000 or very small values on scroll release
        if delta.abs() < 0.001 {
            return;
        }
        let target = self.wheel_target();
        self.scroll_terminal(delta, target);
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
        if let Some(target) = self.wheel_target()
            && let Some(cell) = target.cell
        {
            self.report_wheel(target.identity, cell, direction);
        }
    }

    fn wheel_target(&self) -> Option<WheelTarget> {
        let position = self.last_cursor_pos?;
        let renderer = self.renderer.as_ref()?;
        let (cell_width, cell_height) = renderer.cell_size();
        self.wheel_target_at(
            position,
            cell_width,
            cell_height,
            renderer.content_top(),
            renderer.padding(),
        )
    }

    fn wheel_target_at(
        &self,
        position: (f64, f64),
        cell_width: f32,
        cell_height: f32,
        content_top: f32,
        padding: f32,
    ) -> Option<WheelTarget> {
        let pane = self.pane_at_pointer(position, content_top, padding)?;
        Some(WheelTarget {
            identity: pane.identity,
            cell: self.pane_protocol_cell(pane, position, cell_width, cell_height),
        })
    }

    /// One wheel step for `target`, or for the focused pane when the pointer
    /// is over no pane: a report when its program asks for mouse input, arrow
    /// keys when its alternate screen asks for them, otherwise its scrollback.
    /// Modes come from the same terminal that receives the bytes.
    fn scroll_terminal(&mut self, delta: f64, target: Option<WheelTarget>) {
        let focused = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .map(crate::tabs::Tab::focused_terminal_identity);
        let Some(identity) = target.map(|target| target.identity).or(focused) else {
            return;
        };

        // The filtered view belongs to the focused pane's search.
        if Some(identity) == focused && self.search.is_filter_mode() && self.search.has_results() {
            let viewport_rows = self
                .active_terminal()
                .map(|terminal| terminal.grid().rows())
                .unwrap_or(0);
            self.search.scroll_filter(delta, viewport_rows);
            self.request_redraw(RedrawReason::PointerInput);
            return;
        }

        let direction = if delta > 0.0 {
            crate::input::WheelDirection::Up
        } else {
            crate::input::WheelDirection::Down
        };
        if let Some(cell) = target.and_then(|target| target.cell) {
            if self.report_wheel(identity, cell, direction) {
                return;
            }
            let mode = self.pointer_terminal_mode(identity);
            if !self.modifiers.shift_key()
                && let Some(bytes) = crate::input::encode_wheel_as_cursor_key(mode, direction)
                && self.write_pointer_protocol_input(identity, bytes)
            {
                return;
            }
        }

        if let Some(terminal) = self.pointer_terminal_mut(identity) {
            let lines = delta.abs().max(1.0) as usize;
            let offset = terminal.grid().scroll_offset();

            let new_offset = if delta > 0.0 {
                offset + lines
            } else {
                offset.saturating_sub(lines)
            };

            terminal.grid_mut().set_scroll_offset(new_offset);

            self.request_redraw(RedrawReason::PointerInput);
        }
    }

    /// Report a wheel step at `cell` to `identity`'s program when it asks for
    /// mouse input. Shift keeps the wheel for Termeric.
    fn report_wheel(
        &mut self,
        identity: crate::tabs::TerminalIdentity,
        cell: (usize, usize),
        direction: crate::input::WheelDirection,
    ) -> bool {
        if self.modifiers.shift_key() {
            return false;
        }
        let mode = self.pointer_terminal_mode(identity);
        if !mode.has_mouse_mode() {
            return false;
        }
        let Some(bytes) = crate::input::encode_mouse_report(
            mode,
            crate::input::MouseReportKind::Wheel(direction),
            cell.0,
            cell.1,
            self.modifiers,
        ) else {
            return false;
        };
        self.write_pointer_protocol_input(identity, &bytes)
    }

    /// The terminal a pointer gesture addresses. A pane whose program has
    /// exited still shows, and scrolls, its output; only writes need a live
    /// program, and their route checks that.
    fn pointer_terminal(
        &self,
        identity: crate::tabs::TerminalIdentity,
    ) -> Option<&termeric_core::Terminal> {
        self.tabs
            .as_ref()?
            .terminal_for_origin(pointer_route(identity))
    }

    fn pointer_terminal_mut(
        &mut self,
        identity: crate::tabs::TerminalIdentity,
    ) -> Option<&mut termeric_core::Terminal> {
        self.tabs
            .as_mut()?
            .terminal_for_origin_mut(pointer_route(identity))
    }

    fn pointer_terminal_mode(
        &self,
        identity: crate::tabs::TerminalIdentity,
    ) -> termeric_core::term::TerminalMode {
        self.pointer_terminal(identity)
            .map(termeric_core::Terminal::mode)
            .unwrap_or_default()
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

    fn pointer_is_over_focused_terminal(&self) -> bool {
        let Some(position) = self.last_cursor_pos else {
            return false;
        };
        let Some(renderer) = &self.renderer else {
            return false;
        };
        let (cell_width, cell_height) = renderer.cell_size();
        self.protocol_grid_position(
            position,
            cell_width,
            cell_height,
            renderer.content_top(),
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
            || self.top_modal_layer().is_some()
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
        let terminal = self.active_terminal()?;
        let (bounds, lone) = self.focused_pane_bounds(content_top, padding)?;
        terminal_protocol_cell(terminal, bounds, lone, position, (cell_width, cell_height))
    }

    /// The visible, zero-based cell of `pane`'s terminal under `position`.
    fn pane_protocol_cell(
        &self,
        pane: PointerPane,
        position: (f64, f64),
        cell_width: f32,
        cell_height: f32,
    ) -> Option<(usize, usize)> {
        let terminal = self.pointer_terminal(pane.identity)?;
        terminal_protocol_cell(
            terminal,
            pane.bounds,
            pane.lone,
            position,
            (cell_width, cell_height),
        )
    }

    /// The active tab's pane drawn under `position`, focused or not.
    pub(super) fn pane_at_pointer(
        &self,
        position: (f64, f64),
        content_top: f32,
        padding: f32,
    ) -> Option<PointerPane> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        let lone = tab.panes.pane_count() == 1;
        let placement = self.pointer_pane_placement(content_top, padding);
        let (pane_id, bounds) = pane_at(placement, tab.panes.calculate_layout(), lone, position)?;
        Some(PointerPane {
            identity: crate::tabs::TerminalIdentity {
                tab_id: tab.id,
                pane_id,
            },
            bounds,
            lone,
        })
    }

    /// Convert a screen position to a cell of the focused pane's grid, in
    /// live scrollback coordinates.
    fn focused_grid_position(
        &self,
        position: (f64, f64),
        cell_width: f32,
        cell_height: f32,
        content_top: f32,
        padding: f32,
    ) -> termeric_core::Point {
        let (origin, scroll_offset) = self
            .focused_pane_bounds(content_top, padding)
            .zip(self.active_terminal())
            .map_or_else(
                // With no tab, the area's top-left cell.
                || {
                    let placement = self.pointer_pane_placement(content_top, padding);
                    let bounds = placement.pane_bounds(Bounds::default(), true);
                    ((bounds.x, bounds.y), 0)
                },
                |((bounds, _), terminal)| ((bounds.x, bounds.y), terminal.grid().scroll_offset()),
            );
        screen_to_pane_grid(position, cell_width, cell_height, origin, scroll_offset)
    }

    /// The focused pane's window rectangle, as drawn, and whether it is the
    /// tab's only pane. `content_top` and `padding` are as the caller
    /// measured them.
    fn focused_pane_bounds(&self, content_top: f32, padding: f32) -> Option<(Bounds, bool)> {
        let panes = &self.tabs.as_ref()?.active_tab()?.panes;
        let rect = *panes.calculate_layout().get(&panes.focused_id())?;
        let lone = panes.pane_count() == 1;
        let placement = self.pointer_pane_placement(content_top, padding);
        Some((placement.pane_bounds(rect, lone), lone))
    }

    fn pointer_pane_placement(&self, content_top: f32, padding: f32) -> PanePlacement {
        let renderer = self.renderer.as_ref();
        PanePlacement {
            left: renderer.map_or(0.0, crate::renderer::Renderer::tab_bar_width),
            top: content_top,
            right: renderer.map_or(0.0, crate::renderer::Renderer::content_right),
            bottom: renderer.map_or(0.0, |renderer| renderer.window_size().1 as f32),
            padding,
        }
    }

    pub(super) fn focused_hyperlink_open_context(
        &mut self,
        point: termeric_core::Point,
    ) -> Option<(String, Option<String>)> {
        let url = self.focused_hyperlink_at(point)?.to_owned();
        let directory = self
            .active_terminal_origin()
            .and_then(|origin| self.hyperlink_open_directory(origin));
        Some((url, directory))
    }

    /// Input points use live scrollback coordinates. Convert back to a visual
    /// row before consulting the presentation and its filter projection.
    /// Semantic links retain priority over detected URL text. Refresh at the
    /// action boundary so scrolling/toggling a filter cannot reuse stale targets.
    pub(super) fn focused_hyperlink_at(&mut self, point: termeric_core::Point) -> Option<&str> {
        if self.reconcile_search_presentation() {
            return None;
        }
        self.refresh_focused_url_cache();
        let terminal = self.active_terminal()?;
        let offset = i32::try_from(terminal.grid().scroll_offset()).ok()?;
        let row = usize::try_from(point.row.checked_add(offset)?).ok()?;
        let grid = terminal.presentation_grid();
        if row >= grid.rows() || point.col >= grid.cols() {
            return None;
        }
        let projected = self.search.filtered_rows(grid.rows());
        let source_row = if projected.is_empty() {
            (row as i32).saturating_sub(i32::try_from(grid.scroll_offset()).ok()?)
        } else {
            *projected.get(row)?
        };
        grid.row_at_offset(source_row)
            .and_then(|row| row.get(point.col).and_then(|cell| cell.hyperlink_id))
            .and_then(|id| terminal.get_hyperlink_url(id))
            .or_else(|| {
                self.url_cache
                    .url_at(row, point.col)
                    .map(|url| url.url.as_str())
            })
    }

    pub(super) fn refresh_focused_url_cache(&mut self) {
        let Some(terminal) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(crate::tabs::Tab::focused_terminal)
        else {
            self.url_cache = UrlCache::new(0);
            return;
        };
        let grid = terminal.presentation_grid();
        update_visible_url_cache(
            &mut self.url_cache,
            grid,
            &self.url_detector,
            self.search.filtered_rows(grid.rows()),
        );
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
        let target = self
            .pane_at_pointer(position, content_top, padding)
            .filter(|pane| !pane.lone);
        target.is_some_and(|pane| self.toggle_armed_broadcast_target(pane.identity.pane_id))
    }

    /// Focus the pane under a content click. Returns whether focus moved.
    fn focus_pane_at(&mut self, position: (f64, f64), content_top: f32, padding: f32) -> bool {
        let Some(target) = self.pane_at_pointer(position, content_top, padding) else {
            return false;
        };
        let target = target.identity.pane_id;
        let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) else {
            return false;
        };
        let Some(panes) = tab.split_panes_mut() else {
            return false;
        };
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

        self.refresh_focused_url_cache();
        self.git_watcher.invalidate();
        // The agent view follows the tab: each tab has its own.
        self.bind_agent_view_to_active_tab();
        self.reset_cursor_blink();
        self.refresh_terminal_pointer_cursor();
        self.request_redraw(RedrawReason::Focus);
    }

    /// Check if a click position is within the scrollbar area
    pub(super) fn is_click_on_scrollbar(&self, x: f64, y: f64) -> bool {
        let Some(renderer) = &self.renderer else {
            return false;
        };
        // Scrollbar is 6px wide + 2px margin from the terminal area's right
        // edge, with an expanded hit area
        let scrollbar_hit_width = 16.0_f32;
        let scrollbar_x = renderer.content_right() - scrollbar_hit_width;

        let content_top = renderer.content_top();
        let padding = renderer.padding();
        let content_left = renderer.tab_bar_width();

        if let Some(panes) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.split_panes())
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
        let padding = renderer.padding();
        let (_, window_height) = renderer.window_size();
        let content_top = renderer.content_top();
        let scrollbar_top = content_top + padding;
        let scrollbar_bottom = window_height as f32 - padding;
        let (scrollbar_top, scrollbar_bottom) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.split_panes())
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

/// A pane of the active tab under the pointer: its terminal, its window
/// rectangle as drawn, and whether it is the tab's only pane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PointerPane {
    pub(super) identity: crate::tabs::TerminalIdentity,
    pub(super) bounds: Bounds,
    pub(super) lone: bool,
}

/// Where a wheel step lands: the pane under the pointer and, when the
/// pointer is over one of its cells, that visible cell.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WheelTarget {
    identity: crate::tabs::TerminalIdentity,
    cell: Option<(usize, usize)>,
}

fn pointer_route(identity: crate::tabs::TerminalIdentity) -> crate::tabs::TerminalEventOrigin {
    crate::tabs::TerminalEventOrigin {
        tab_id: identity.tab_id,
        pane_id: identity.pane_id,
    }
}

/// The pane whose rectangle, placed as drawn, contains `position`.
fn pane_at(
    placement: PanePlacement,
    layout: impl IntoIterator<Item = (PaneId, Bounds)>,
    lone: bool,
    position: (f64, f64),
) -> Option<(PaneId, Bounds)> {
    let (x, y) = (position.0 as f32, position.1 as f32);
    layout
        .into_iter()
        .map(|(id, rect)| (id, placement.pane_bounds(rect, lone)))
        .find(|(_, bounds)| bounds.contains(x, y))
}

/// The visible cell of `terminal` under `position` for a pane drawn at
/// `bounds`. A lone pane reports over its terminal's own cells, as an
/// unsplit terminal always has; a split pane over its whole rectangle.
fn terminal_protocol_cell(
    terminal: &termeric_core::Terminal,
    bounds: Bounds,
    lone: bool,
    position: (f64, f64),
    cell_size: (f32, f32),
) -> Option<(usize, usize)> {
    let viewport = if lone {
        (
            terminal.cols() as f32 * cell_size.0,
            terminal.rows() as f32 * cell_size.1,
        )
    } else {
        (bounds.width, bounds.height)
    };
    protocol_cell_from_viewport(
        position,
        (bounds.x, bounds.y),
        viewport,
        cell_size,
        (terminal.cols(), terminal.rows()),
    )
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

/// The cell under `position` of a grid drawn from `origin`, in live
/// scrollback coordinates.
fn screen_to_pane_grid(
    position: (f64, f64),
    cell_width: f32,
    cell_height: f32,
    origin: (f32, f32),
    scroll_offset: usize,
) -> termeric_core::Point {
    let local_x = (position.0 as f32 - origin.0).max(0.0);
    let local_y = (position.1 as f32 - origin.1).max(0.0);
    let col = (local_x / cell_width).floor() as usize;
    let visual_row = (local_y / cell_height).floor() as i32;
    let scroll_offset = i32::try_from(scroll_offset).unwrap_or(i32::MAX);
    termeric_core::Point::new(visual_row - scroll_offset, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_protocol_pointer_shape_has_an_exact_native_mapping() {
        use termeric_core::PointerShape;
        use winit::window::CursorIcon;

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

    fn mouse_report_position(app: &App) -> (f64, f64) {
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        let (x, y) = tab.split_panes().map_or((0.0, 0.0), |panes| {
            let rect = panes.calculate_layout()[&panes.focused_id()];
            (rect.x, rect.y)
        });
        ((x + 15.0) as f64, (y + 30.0) as f64)
    }

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
                        .panes
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
                    let panes = &mut app.tabs.as_mut().unwrap().active_tab_mut().unwrap().panes;
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
                .panes
                .focused_id();
            let original_index = captures
                .iter()
                .position(|(origin, _)| origin.pane_id == original)
                .unwrap();
            app.active_terminal_mut().unwrap().set_mode(
                TerminalMode::MOUSE_BUTTON_EVENT | TerminalMode::MOUSE_SGR,
                true,
            );
            let position = mouse_report_position(&app);
            assert!(app.try_report_mouse_press(MouseButton::Left, position, 10.0, 20.0, 0.0, 0.0));
            let panes = &mut app.tabs.as_mut().unwrap().active_tab_mut().unwrap().panes;
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
            let panes = &mut app.tabs.as_mut().unwrap().active_tab_mut().unwrap().panes;
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
        let placement = PanePlacement {
            left: 50.0,
            top: 50.0,
            right: 900.0,
            bottom: 700.0,
            padding: 6.0,
        };
        let rect = Bounds {
            x: 100.0,
            y: 60.0,
            width: 200.0,
            height: 160.0,
        };
        let split = placement.pane_bounds(rect, false);
        let point = screen_to_pane_grid((196.0, 174.0), 10.0, 20.0, (split.x, split.y), 3);
        assert_eq!(point, termeric_core::Point::new(0, 4));
        // A lone pane's grid starts one padding below the area's top.
        let lone = placement.pane_bounds(rect, true);
        let point = screen_to_pane_grid((96.0, 96.0), 10.0, 20.0, (lone.x, lone.y), 0);
        assert_eq!(point, termeric_core::Point::new(2, 4));
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

    #[test]
    fn the_pane_under_the_pointer_is_found_where_it_is_drawn() {
        let placement = PanePlacement {
            left: 40.0,
            top: 30.0,
            right: 840.0,
            bottom: 530.0,
            padding: 6.0,
        };
        let left = Bounds::new(0.0, 0.0, 400.0, 500.0);
        let right = Bounds::new(400.0, 0.0, 394.0, 500.0);
        let split = [(PaneId(0), left), (PaneId(1), right)];
        let id_at = |position| pane_at(placement, split, false, position).map(|(id, _)| id);
        // Split panes sit at their rectangles from the area's left padding
        // and its top edge; edges are half-open.
        assert_eq!(id_at((46.0, 30.0)), Some(PaneId(0)));
        assert_eq!(id_at((445.9, 529.9)), Some(PaneId(0)));
        assert_eq!(id_at((446.0, 100.0)), Some(PaneId(1)));
        assert_eq!(id_at((839.9, 100.0)), Some(PaneId(1)));
        // The tab rail, the left padding, the tab bar, and the window below.
        assert_eq!(id_at((45.9, 100.0)), None);
        assert_eq!(id_at((100.0, 29.9)), None);
        assert_eq!(id_at((100.0, 530.0)), None);
        assert_eq!(id_at((f64::NAN, 100.0)), None);
        // A lone pane fills the area inside the padding on every side.
        let lone = [(PaneId(0), left)];
        assert_eq!(
            pane_at(placement, lone, true, (46.0, 36.0)),
            Some((PaneId(0), Bounds::new(46.0, 36.0, 788.0, 488.0)))
        );
        assert_eq!(pane_at(placement, lone, true, (46.0, 35.9)), None);
        assert_eq!(pane_at(placement, lone, true, (834.0, 100.0)), None);
    }

    /// A point inside `pane`'s rectangle, one cell in from its corner, in a
    /// window with no tab bar or padding.
    fn pane_position(app: &App, pane: PaneId) -> (f64, f64) {
        let rect = app.active_panes().calculate_layout()[&pane];
        ((rect.x + 15.0) as f64, (rect.y + 30.0) as f64)
    }

    fn pane_scroll_offset(app: &App, pane: PaneId) -> usize {
        app.active_panes()
            .get_pane(pane)
            .unwrap()
            .terminal
            .grid()
            .scroll_offset()
    }

    #[test]
    fn the_wheel_scrolls_the_pane_under_the_pointer_without_focusing_it() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let mut tabs = TabManager::for_test(crate::test_support::sleeping_shell());
        tabs.new_tab().unwrap();
        app.tabs = Some(tabs);
        let panes = app
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .unwrap()
            .panes_for_test(
                crate::test_support::sleeping_shell(),
                800.0,
                480.0,
                10.0,
                20.0,
            );
        let left = panes.focused_id();
        let right = panes.split(SplitDirection::Vertical).unwrap();
        for pane in [left, right] {
            panes
                .get_pane_mut(pane)
                .unwrap()
                .terminal
                .process(b"history\r\n".repeat(80).as_slice());
        }
        panes.focus(left);

        let target = app.wheel_target_at(pane_position(&app, right), 10.0, 20.0, 0.0, 0.0);
        assert_eq!(target.map(|target| target.identity.pane_id), Some(right));
        app.scroll_terminal(3.0, target);
        assert_eq!(pane_scroll_offset(&app, right), 3);
        assert_eq!(pane_scroll_offset(&app, left), 0);
        assert_eq!(app.active_panes().focused_id(), left);

        // Over no pane, the wheel stays with the focused pane.
        app.scroll_terminal(3.0, None);
        assert_eq!(pane_scroll_offset(&app, left), 3);
        assert_eq!(pane_scroll_offset(&app, right), 3);
    }

    #[test]
    fn wheel_reports_and_alternate_scroll_keys_go_to_the_pane_under_the_pointer() {
        use crate::app::tests::{fenced_harvest_capture, harvest_rerun_capture_fixture_in};
        use termeric_core::term::TerminalMode;
        let (mut app, _directory, _, captures) = harvest_rerun_capture_fixture_in(true);
        let focused = app.active_panes().focused_id();
        let others: Vec<_> = app
            .active_panes()
            .pane_ids()
            .into_iter()
            .filter(|pane| *pane != focused)
            .collect();
        let (reporting, scrolling) = (others[0], others[1]);
        let mouse = TerminalMode::MOUSE_BUTTON_EVENT | TerminalMode::MOUSE_SGR;
        let panes = app.active_panes_mut();
        // The focused pane asks for mouse input too: only the hovered pane's
        // own modes decide what it receives.
        panes
            .get_pane_mut(focused)
            .unwrap()
            .terminal
            .set_mode(mouse, true);
        panes
            .get_pane_mut(reporting)
            .unwrap()
            .terminal
            .set_mode(mouse, true);
        panes.get_pane_mut(scrolling).unwrap().terminal.set_mode(
            TerminalMode::ALT_SCREEN | TerminalMode::ALTERNATE_SCROLL,
            true,
        );

        for (pane, delta) in [(reporting, 1.0), (scrolling, -1.0)] {
            let target = app.wheel_target_at(pane_position(&app, pane), 10.0, 20.0, 0.0, 0.0);
            app.scroll_terminal(delta, target);
        }

        assert_eq!(app.active_panes().focused_id(), focused);
        let bytes = fenced_harvest_capture(&mut app, &captures);
        for ((origin, _), bytes) in captures.iter().zip(bytes) {
            let expected: &[u8] = if origin.pane_id == reporting {
                b"\x1b[<64;2;2M"
            } else if origin.pane_id == scrolling {
                b"\x1b[B"
            } else {
                b""
            };
            assert_eq!(bytes, expected, "{:?}", origin.pane_id);
        }
    }

    #[test]
    fn wheel_input_never_reaches_a_read_only_pane_or_any_pane_while_broadcast_is_armed() {
        use crate::app::tests::{fenced_harvest_capture, harvest_rerun_capture_fixture_in};
        use termeric_core::term::TerminalMode;
        let (mut app, _directory, _, captures) = harvest_rerun_capture_fixture_in(true);
        let focused = app.active_panes().focused_id();
        let hovered = app
            .active_panes()
            .pane_ids()
            .into_iter()
            .find(|pane| *pane != focused)
            .unwrap();
        let pane = app.active_panes_mut().get_pane_mut(hovered).unwrap();
        pane.terminal.set_mode(
            TerminalMode::MOUSE_BUTTON_EVENT | TerminalMode::MOUSE_SGR,
            true,
        );
        pane.read_only = true;
        let target = app.wheel_target_at(pane_position(&app, hovered), 10.0, 20.0, 0.0, 0.0);
        app.scroll_terminal(1.0, target);

        app.active_panes_mut()
            .get_pane_mut(hovered)
            .unwrap()
            .read_only = false;
        app.broadcast = BroadcastState::Armed {
            tab_id: app.tabs.as_ref().unwrap().active_tab_id().unwrap(),
            expires: Instant::now() + Duration::from_secs(10),
            targets: app.active_panes().pane_ids(),
        };
        app.scroll_terminal(1.0, target);

        let bytes = fenced_harvest_capture(&mut app, &captures);
        assert!(bytes.iter().all(Vec::is_empty), "{bytes:?}");
    }

    #[test]
    fn a_second_click_on_the_same_workspace_row_opens_it() {
        use crate::ui::session_panel::SessionPanelAction;
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        let select = |id: &str| SessionPanelAction::Select(id.into());

        assert_eq!(app.promote_session_double_click(select("a")), select("a"));
        assert_eq!(app.promote_session_double_click(select("b")), select("b"));
        assert_eq!(
            app.promote_session_double_click(select("b")),
            SessionPanelAction::Load("b".into())
        );
        // A third click starts a new pair rather than opening again.
        assert_eq!(app.promote_session_double_click(select("b")), select("b"));
        // Any other action in between breaks the pair.
        assert_eq!(
            app.promote_session_double_click(SessionPanelAction::None),
            SessionPanelAction::None
        );
        assert_eq!(app.promote_session_double_click(select("b")), select("b"));
    }
}
