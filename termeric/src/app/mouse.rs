//! Mouse routing for modal layers, chrome, panes, and terminal content.

use super::modal_layer::ModalLayer;
use super::*;
use crate::renderer::{PanePlacement, SearchBarClick};
use crate::ui::geometry::Bounds;

/// How long a hovered tab bar control takes to fade in.
const TAB_BAR_HOVER_FADE_SECONDS: f32 = 0.1;

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

/// Where the terminal area sits and how large its cells are: all that maps
/// the pointer to panes, cells, dividers, and scrollbars.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PointerMetrics {
    pub(super) placement: PanePlacement,
    pub(super) cell_size: (f32, f32),
}

impl PointerMetrics {
    /// `position` in the coordinates of the tab's pane layout, where split
    /// panes and dividers sit: from the area's left padding and top edge.
    pub(super) fn pane_area_point(self, position: (f64, f64)) -> (f32, f32) {
        (
            position.0 as f32 - self.placement.left - self.placement.padding,
            position.1 as f32 - self.placement.top,
        )
    }
}

impl App {
    /// The pointer geometry of what is drawn. A headless window, like
    /// [`App::ui_metrics`]'s, has a nominal window with 8×16 cells.
    pub(super) fn pointer_metrics(&self) -> PointerMetrics {
        let metrics = self.ui_metrics();
        PointerMetrics {
            placement: self.pane_placement(),
            cell_size: (metrics.cell_width, metrics.cell_height),
        }
    }

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
                    && tab
                        .live_domain_tab_route()
                        .is_none_or(|(domain, _)| !self.live_domain_busy(domain))
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
        // The grid the pane draws, whose scrollback the thumb shows.
        if let Some(terminal) = self.active_terminal_mut() {
            let grid = terminal.presentation_grid_mut();
            let max_offset = grid.scrollback_len();
            let new_offset = (fraction.clamp(0.0, 1.0) * max_offset as f32) as usize;
            grid.set_scroll_offset(new_offset);
        }
    }

    /// A double-click on a workspace row opens it; a single click only
    /// selects.
    fn promote_workspace_double_click(
        &mut self,
        action: crate::ui::workspace_panel::WorkspacePanelAction,
    ) -> crate::ui::workspace_panel::WorkspacePanelAction {
        use crate::ui::workspace_panel::WorkspacePanelAction;
        let WorkspacePanelAction::Select(id) = action else {
            self.workspace_row_click.reset();
            return action;
        };
        if self.workspace_row_click.press(id.clone(), Instant::now()) {
            WorkspacePanelAction::Load(id)
        } else {
            WorkspacePanelAction::Select(id)
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
        let Some(divider) = tab
            .split_panes()
            .and_then(|panes| panes.divider_at(local_x, local_y))
        else {
            return false;
        };
        let route = tab.live_domain_tab_route();
        if route.is_some_and(|(domain, _)| self.live_domain_busy(domain)) {
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
        if self.renderer.is_none() {
            return;
        }
        let metrics = self.pointer_metrics();
        if state == ElementState::Pressed
            && button != MouseButton::Left
            && self.pointer_panel_owns_input()
        {
            return;
        }
        match state {
            ElementState::Pressed => {
                let chrome = self.floating_chrome_at(position);
                if button == MouseButton::Left && !self.pointer_panel_owns_input() {
                    if matches!(chrome, Some(FloatingChrome::Scrollbar(_))) {
                        if self.focus_pane_at(position, metrics) {
                            self.refresh_focused_terminal_surfaces();
                        }
                        self.begin_scrollbar_drag();
                        self.scroll_to_scrollbar_position(position.1);
                        self.request_redraw(RedrawReason::PointerInput);
                        return;
                    }

                    // A press on a pane divider starts a drag-to-resize.
                    let (area_x, area_y) = metrics.pane_area_point(position);
                    if self.press_pane_divider(area_x, area_y) {
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

                        // The panel layer on top owns every click, inside it
                        // or beside it (which dismisses it), before the tab
                        // bar or find box beneath its scrim can take it.
                        let window_size = self.window.as_ref().map(|w| w.inner_size());
                        match self.top_modal_layer() {
                            Some(ModalLayer::Settings) => {
                                if let Some(size) = window_size {
                                    let action = self.settings_panel.handle_click(
                                        px,
                                        py,
                                        &self.config,
                                        &self.shortcut_registry,
                                        size.width as f32,
                                        size.height as f32,
                                        metrics.cell_size.0,
                                        metrics.cell_size.1,
                                    );
                                    self.apply_settings_action(action);
                                    self.request_redraw(RedrawReason::PointerInput);
                                    return;
                                }
                            }
                            Some(ModalLayer::WorkspaceMenu) => {
                                self.click_workspace_menu(px, py);
                                return;
                            }
                            Some(ModalLayer::WorkspacePanel) => {
                                if let Some(size) = window_size {
                                    let info = self.workspace_panel.build_info();
                                    let action = renderer.workspace_panel_hit_test(
                                        px,
                                        py,
                                        &info,
                                        size.width as f32,
                                        size.height as f32,
                                    );
                                    let action = self.promote_workspace_double_click(action);
                                    self.apply_workspace_panel_action(action);
                                    self.request_redraw(RedrawReason::PointerInput);
                                    return;
                                }
                            }
                            Some(ModalLayer::CommandsPanel) => {
                                if let Some(size) = window_size {
                                    let info =
                                        self.commands_panel.build_info(&self.config.commands);
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
                            Some(ModalLayer::AgentsPanel) => {
                                self.handle_agents_panel_click(px, py);
                                return;
                            }
                            // The layers above the panels took the press before
                            // `handle_mouse_input`.
                            None
                            | Some(
                                ModalLayer::Guardrail
                                | ModalLayer::InfoOverlay
                                | ModalLayer::DiffReview
                                | ModalLayer::ContextPicker
                                | ModalLayer::SproutMenu
                                | ModalLayer::Cellar
                                | ModalLayer::DomainPicker
                                | ModalLayer::UnicodePicker
                                | ModalLayer::CommandPalette
                                | ModalLayer::ShortcutsPanel,
                            ) => {}
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
                                    TabBarClick::Tab(idx) => self.press_tab(idx),
                                    TabBarClick::WorkspacesMenu => {
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
                }
                if self.press_floating_chrome(button, chrome) {
                    return;
                }
                if self.press_terminal(button, position, metrics) {
                    return;
                }
            }
            ElementState::Released => {
                if self.try_report_mouse_release(button, position, metrics) {
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

    /// What of the find box lies at `position`: `None` when it is hidden or
    /// the position is beside it.
    fn search_bar_click_at(&self, position: (f64, f64)) -> SearchBarClick {
        if !self.search_is_visible() {
            return SearchBarClick::None;
        }
        let (Some(renderer), Some(window)) = (&self.renderer, &self.window) else {
            return SearchBarClick::None;
        };
        let info = self.search.build_info(self.search_cursor_pos);
        renderer.search_bar_hit_test(
            position.0 as f32,
            position.1 as f32,
            window.inner_size().width as f32,
            &info,
        )
    }

    /// A press of any button on chrome floating over the panes is that
    /// chrome's, never the pane's beneath it. The find box works it; on a
    /// scrollbar, which the primary button drags before this, it does
    /// nothing, as on a toast (whose presses are taken before any of this).
    /// Returns false when no chrome is there.
    fn press_floating_chrome(
        &mut self,
        button: MouseButton,
        chrome: Option<FloatingChrome>,
    ) -> bool {
        match chrome {
            None => false,
            Some(FloatingChrome::FindBox(click)) => self.press_search_bar(button, click),
            Some(FloatingChrome::Toast | FloatingChrome::Scrollbar(_)) => true,
        }
    }

    /// A press of any button on the find box (`click`) is the find box's,
    /// never the pane's beneath it: the primary button works its controls,
    /// the middle button pastes the primary selection into its query, and
    /// anything else does nothing. Returns false when the press missed it.
    fn press_search_bar(&mut self, button: MouseButton, click: SearchBarClick) -> bool {
        match (button, click) {
            (_, SearchBarClick::None) => return false,
            (MouseButton::Left, click) => self.apply_search_bar_click(click),
            (MouseButton::Middle, SearchBarClick::Query(cursor)) => {
                self.paste_primary_into_search(cursor);
            }
            _ => {}
        }
        true
    }

    /// A primary press on the find box's control `click`.
    fn apply_search_bar_click(&mut self, click: SearchBarClick) {
        match click {
            SearchBarClick::Query(cursor) => {
                self.focus_search_query_at(cursor, RedrawReason::PointerInput);
                return;
            }
            SearchBarClick::None | SearchBarClick::Background => return,
            SearchBarClick::ToggleCaseSensitive => {
                self.with_search_and_active_grid(|search, grid| {
                    search.toggle_case_sensitive(grid);
                });
            }
            SearchBarClick::ToggleRegex => {
                self.with_search_and_active_grid(|search, grid| {
                    search.toggle_regex(grid);
                });
            }
            SearchBarClick::ToggleFilter => {
                if self.copy_mode.is_active() {
                    self.toasts.warning(
                        "Filter mode is unavailable inside copy mode; matches remain highlighted",
                    );
                } else {
                    self.search.toggle_filter();
                }
            }
            SearchBarClick::PrevMatch => {
                self.search.prev();
                self.scroll_to_current_match();
            }
            SearchBarClick::NextMatch => {
                self.search.next();
                self.scroll_to_current_match();
            }
            SearchBarClick::Close => {
                self.search.cancel();
                self.search_cursor_pos = 0;
            }
        }
        self.request_redraw(RedrawReason::PointerInput);
    }

    /// A press on a toast dismisses it. Returns whether one was dismissed.
    pub(super) fn press_toast(&mut self, x: f32, y: f32) -> bool {
        let Some(id) = self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.toast_hit_test(x, y))
        else {
            return false;
        };
        // The toast drawn there may have expired since that frame; the press
        // then goes to whatever is beneath.
        let dismissed = self.toasts.dismiss(id);
        if dismissed {
            self.request_redraw(RedrawReason::PointerInput);
        }
        dismissed
    }

    /// A primary press on tab `idx` selects it and may start dragging it.
    /// The second press of a double-click renames it instead.
    pub(super) fn press_tab(&mut self, idx: usize) {
        self.switch_tab(|tabs| tabs.switch_to(idx));
        if let Some(tabs) = &self.tabs {
            self.tab_dragging = tabs.active_tab().and_then(|tab| {
                let live = match tab.live_domain_tab_route() {
                    Some((domain_id, remote_tab_id)) => {
                        Some(LiveDomainTabDrag {
                            domain_id: domain_id.to_string(),
                            remote_tab_id,
                            // A live tab with no index of its
                            // own is not dragged at all.
                            initial_index: u16::try_from(
                                tabs.live_domain_tab_index(domain_id, remote_tab_id)?,
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
            .is_some_and(|live| self.live_domain_busy(&live.domain_id));
        if live_drag_blocked {
            self.tab_dragging = None;
            self.toasts
                .info("Wait for the pending live-domain change before reordering");
        }
        let Some(tab_id) = self.tabs.as_ref().and_then(TabManager::active_tab_id) else {
            return;
        };
        if self.tab_click.press(tab_id, Instant::now()) {
            self.tab_dragging = None;
            if self
                .tab_rename
                .as_ref()
                .is_none_or(|rename| rename.tab_id != tab_id)
            {
                self.begin_tab_rename();
            }
        }
    }

    /// A press over the terminal area, once no chrome or panel took it: it
    /// focuses the pane under the pointer, then opens a link (Ctrl), goes
    /// to a program that asks for mouse input (unless Shift is held), or
    /// starts a selection (primary) or pastes the primary selection
    /// (middle). Returns true when the press is fully handled.
    fn press_terminal(
        &mut self,
        button: MouseButton,
        position: (f64, f64),
        metrics: PointerMetrics,
    ) -> bool {
        if button == MouseButton::Left && self.toggle_armed_broadcast_target_at(position, metrics) {
            self.request_redraw(RedrawReason::PointerInput);
            return true;
        }
        if self.focus_pane_at(position, metrics) {
            self.refresh_focused_terminal_surfaces();
        }

        let grid_pos = self.focused_grid_position(position, metrics);

        // Ctrl+Click opens links from the pane selected by this click.
        if self.modifiers.control_key()
            && let Some((url, directory)) = self.focused_hyperlink_open_context(grid_pos)
        {
            log::info!("Opening hyperlink: {}", url);
            self.activate_hyperlink(self.active_terminal_origin(), &url, directory.as_deref());
            return true;
        }

        if self.try_report_mouse_press(button, position, metrics) {
            return true;
        }

        self.begin_native_selection(button, grid_pos);
        if button == MouseButton::Middle && self.pane_at_pointer(position, metrics).is_some() {
            self.paste_primary_selection();
        }
        false
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
        let tab_bar_height = renderer.tab_bar_height();
        let metrics = self.pointer_metrics();
        let grid_pos = self.focused_grid_position(position, metrics);

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
            let (area_x, area_y) = metrics.pane_area_point(position);
            self.apply_divider_drag_position(area_x, area_y);
            self.request_redraw(RedrawReason::PointerInput);
            return;
        }

        // The native drag-and-drop cursor remains authoritative until Winit
        // reports DroppedFile or HoveredFileCancelled.
        if self.file_drop_hovered {
            return;
        }

        // A panel layer on top tracks hover for its highlights, an overlay
        // above the panels takes no hover of its own here, and the terminal
        // beneath sees no motion: no selection, no reports.
        match self.top_modal_layer() {
            Some(ModalLayer::Settings) => {
                self.hover_settings_panel(position.0 as f32, position.1 as f32);
                self.request_redraw(RedrawReason::PointerInput);
                return;
            }
            Some(ModalLayer::WorkspaceMenu) => {
                if self.hover_workspace_menu(position.0 as f32, position.1 as f32) {
                    self.request_redraw(RedrawReason::PointerInput);
                }
                return;
            }
            Some(ModalLayer::WorkspacePanel) => {
                self.hover_workspace_panel(position.0 as f32, position.1 as f32);
                self.request_redraw(RedrawReason::PointerInput);
                return;
            }
            Some(ModalLayer::CommandsPanel) => {
                let px = position.0 as f32;
                let py = position.1 as f32;
                let layout = self.commands_panel_layout();
                self.commands_panel.set_hover(layout.control_at(px, py));
                self.request_redraw(RedrawReason::PointerInput);
                return;
            }
            Some(ModalLayer::AgentsPanel) => {
                self.update_agents_panel_hover(position.0 as f32, position.1 as f32);
                return;
            }
            Some(
                ModalLayer::Guardrail
                | ModalLayer::InfoOverlay
                | ModalLayer::DiffReview
                | ModalLayer::ContextPicker
                | ModalLayer::SproutMenu
                | ModalLayer::Cellar
                | ModalLayer::DomainPicker
                | ModalLayer::UnicodePicker
                | ModalLayer::CommandPalette
                | ModalLayer::ShortcutsPanel,
            ) => return,
            None => {}
        }

        // Set resize cursor when hovering over pane dividers. A dragged tab
        // owns the pointer's shape until it is released.
        if self.tab_dragging.is_none()
            && let Some(pm) = self
                .tabs
                .as_ref()
                .and_then(|tabs| tabs.active_tab()?.split_panes())
        {
            let (area_x, area_y) = metrics.pane_area_point(position);
            if let Some(divider) = pm.divider_at(area_x, area_y) {
                if let Some(window) = &self.window {
                    let cursor = if divider.vertical {
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

        self.refresh_terminal_pointer_cursor();

        let chrome = self.floating_chrome_at(position);
        if self.try_report_mouse_motion(position, chrome, metrics) {
            return;
        }

        // Only update selection when mouse is pressed (dragging)
        if self.mouse_pressed {
            self.extend_native_selection(grid_pos);
            self.request_redraw(RedrawReason::PointerInput);
        }
    }

    /// Hover over Settings at `(px, py)`: its rows, controls, and open dropdown.
    fn hover_settings_panel(&mut self, px: f32, py: f32) {
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
    }

    /// Hover over the workspace manager at `(px, py)`: its rows and controls.
    fn hover_workspace_panel(&mut self, px: f32, py: f32) {
        if let Some(renderer) = &self.renderer {
            let (ui_cw, cell_height) = renderer.cell_size();
            let window_size = self.window.as_ref().map(|w| w.inner_size());
            if let Some(size) = window_size {
                let info = self.workspace_panel.build_info();
                let layout = crate::ui::workspace_panel::WorkspacePanelLayout::calculate(
                    size.width as f32,
                    size.height as f32,
                    renderer.tab_bar_height(),
                    ui_cw,
                    cell_height,
                    info.animation,
                    info.rename_text.is_some(),
                    info.workspaces,
                    info.entries.len(),
                );
                let control = layout.control_at(px, py, info.snapshot_count);
                let hovered = if px >= layout.panel.x && control.is_none() {
                    layout
                        .visible_row_at(py)
                        .map(|row| info.scroll_offset + row)
                } else {
                    None
                };
                self.workspace_panel.set_hover(hovered, control);
            }
        }
    }

    /// Fade-in of the hovered tab bar control, from 0 to 1. It restarts
    /// whenever the hovered control changes and ends at once when hover ends.
    pub(super) fn tab_bar_hover_progress(&self) -> f32 {
        if self.tab_bar_hover.is_none() {
            return 0.0;
        }
        if self.reduced_motion_enabled() {
            return 1.0;
        }
        (self.tab_bar_hover_since.elapsed().as_secs_f32() / TAB_BAR_HOVER_FADE_SECONDS).min(1.0)
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
        if let WheelRoute::Terminal(target) = self.wheel_route() {
            self.scroll_terminal(delta, target);
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
        if let WheelRoute::Terminal(Some(target)) = self.wheel_route()
            && let Some(cell) = target.cell
        {
            self.report_wheel(target.identity, cell, direction);
        }
    }

    fn wheel_route(&self) -> WheelRoute {
        let (Some(position), Some(_)) = (self.last_cursor_pos, &self.renderer) else {
            return WheelRoute::Terminal(None);
        };
        self.wheel_route_at(
            position,
            self.floating_chrome_at(position),
            self.pointer_metrics(),
        )
    }

    /// Where a wheel step at `position`, over `chrome` if any, goes. Over a
    /// pane's scrollbar it scrolls that pane's scrollback, never reaching
    /// its program; over other chrome nothing scrolls.
    fn wheel_route_at(
        &self,
        position: (f64, f64),
        chrome: Option<FloatingChrome>,
        metrics: PointerMetrics,
    ) -> WheelRoute {
        match chrome {
            Some(FloatingChrome::Scrollbar(identity)) => WheelRoute::Terminal(Some(WheelTarget {
                identity,
                cell: None,
            })),
            Some(FloatingChrome::Toast | FloatingChrome::FindBox(_)) => WheelRoute::Chrome,
            None => WheelRoute::Terminal(self.pane_at_pointer(position, metrics).map(|pane| {
                WheelTarget {
                    identity: pane.identity,
                    cell: self.pane_protocol_cell(pane, position, metrics),
                }
            })),
        }
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
        metrics: PointerMetrics,
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
        let Some((col, row)) = self.protocol_grid_position(position, metrics) else {
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
        metrics: PointerMetrics,
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
        let Some((col, row)) = self.protocol_grid_position(position, metrics) else {
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

    /// Report pointer motion at `position`, over `chrome` if any, to the
    /// focused pane's program when it asks for it. Hover over chrome is the
    /// chrome's, but a drag the program holds keeps reporting across it.
    fn try_report_mouse_motion(
        &mut self,
        position: (f64, f64),
        chrome: Option<FloatingChrome>,
        metrics: PointerMetrics,
    ) -> bool {
        self.reconcile_terminal_mouse_capture();
        let mode = self.active_keyboard_mode();
        if !mode.has_mouse_mode() {
            self.terminal_mouse_capture = None;
            return false;
        }
        if self.terminal_mouse_capture.is_none() && (self.modifiers.shift_key() || chrome.is_some())
        {
            return false;
        }
        let Some((col, row)) = self.protocol_grid_position(position, metrics) else {
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
        let icon = self.last_cursor_pos.zip(self.renderer.as_ref()).map_or(
            winit::window::CursorIcon::Default,
            |(position, _)| {
                if self.floating_chrome_at(position).is_some() {
                    return winit::window::CursorIcon::Default;
                }
                self.terminal_pointer_icon_at(position, self.pointer_metrics())
            },
        );
        window.set_cursor(icon);
    }

    /// The chrome drawn over the panes at `position`, frontmost first: a
    /// toast, the find box, or a pane's scrollbar. Presses, hover reports,
    /// the wheel, and the pointer's shape all ask this, so none reaches the
    /// program beneath chrome, and over it the pointer is a plain arrow.
    fn floating_chrome_at(&self, position: (f64, f64)) -> Option<FloatingChrome> {
        let (x, y) = (position.0 as f32, position.1 as f32);
        // The hit test sees the toasts of the last frame; one that has
        // expired since is gone.
        let toast = self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.toast_hit_test(x, y))
            .is_some_and(|id| self.toasts.contains(id));
        if toast {
            return Some(FloatingChrome::Toast);
        }
        match self.search_bar_click_at(position) {
            SearchBarClick::None => self.scrollbar_at(position).map(FloatingChrome::Scrollbar),
            click => Some(FloatingChrome::FindBox(click)),
        }
    }

    /// Ctrl makes the pointer a hand over links, so pressing or releasing it
    /// reshapes the pointer where it rests over a pane, through the same
    /// check of what lies there as motion, so chrome over the pane keeps its
    /// arrow. Over a divider or outside the panes the pointer belongs to
    /// whatever drew it there.
    pub(super) fn refresh_pointer_after_modifiers(&self, previous: ModifiersState) {
        if previous.control_key() == self.modifiers.control_key() {
            return;
        }
        let (Some(position), Some(_)) = (self.last_cursor_pos, &self.renderer) else {
            return;
        };
        let metrics = self.pointer_metrics();
        let (area_x, area_y) = metrics.pane_area_point(position);
        let on_divider = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.split_panes())
            .is_some_and(|panes| panes.divider_at(area_x, area_y).is_some());
        if !on_divider && self.pane_at_pointer(position, metrics).is_some() {
            self.refresh_terminal_pointer_cursor();
        }
    }

    /// The pointer over the terminal area: an I-beam while selecting, a
    /// hand over a link while Ctrl is held (Ctrl+click opens it), and
    /// otherwise the shape the program in the pane under it asked for.
    fn terminal_pointer_icon_at(
        &self,
        position: (f64, f64),
        metrics: PointerMetrics,
    ) -> winit::window::CursorIcon {
        let hovered = self.pane_at_pointer(position, metrics).and_then(|pane| {
            let terminal = self.pointer_terminal(pane.identity)?;
            let cell = terminal_protocol_cell(
                terminal,
                pane.bounds,
                pane.lone,
                position,
                metrics.cell_size,
            )?;
            Some((pane.identity, terminal, cell))
        });
        let Some((identity, terminal, (col, row))) = hovered else {
            return terminal_pointer_icon(None, self.mouse_pressed, false);
        };
        let ctrl_over_link = self.modifiers.control_key()
            && self.hovered_link_at(identity, terminal, row, col).is_some();
        terminal_pointer_icon(
            Some(terminal.effective_pointer_shape()),
            self.mouse_pressed,
            ctrl_over_link,
        )
    }

    /// The link drawn at visible `row`, `col` of the pane `identity`, from
    /// the URL caches the last frame left. They are close enough for the
    /// pointer's shape; a click refreshes them before it opens anything.
    fn hovered_link_at<'a>(
        &'a self,
        identity: crate::tabs::TerminalIdentity,
        terminal: &'a termeric_core::Terminal,
        row: usize,
        col: usize,
    ) -> Option<&'a str> {
        let focused = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .map(crate::tabs::Tab::focused_terminal_identity);
        if focused == Some(identity) {
            let rows = terminal.presentation_grid().rows();
            visible_link_at(
                terminal,
                Some(&self.url_cache),
                self.search.filtered_rows(rows),
                row,
                col,
            )
        } else {
            let cache = self
                .pane_url_caches
                .get(&(std::ptr::from_ref(terminal) as usize));
            visible_link_at(terminal, cache, &[], row, col)
        }
    }

    /// Convert a physical cursor position to a visible, zero-based cell in
    /// the focused terminal. Protocol coordinates never include scrollback.
    fn protocol_grid_position(
        &self,
        position: (f64, f64),
        metrics: PointerMetrics,
    ) -> Option<(usize, usize)> {
        let terminal = self.active_terminal()?;
        let (bounds, lone) = self.focused_pane_bounds(metrics)?;
        terminal_protocol_cell(terminal, bounds, lone, position, metrics.cell_size)
    }

    /// The visible, zero-based cell of `pane`'s terminal under `position`.
    fn pane_protocol_cell(
        &self,
        pane: PointerPane,
        position: (f64, f64),
        metrics: PointerMetrics,
    ) -> Option<(usize, usize)> {
        let terminal = self.pointer_terminal(pane.identity)?;
        terminal_protocol_cell(
            terminal,
            pane.bounds,
            pane.lone,
            position,
            metrics.cell_size,
        )
    }

    /// The active tab's pane drawn under `position`, focused or not.
    pub(super) fn pane_at_pointer(
        &self,
        position: (f64, f64),
        metrics: PointerMetrics,
    ) -> Option<PointerPane> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        let lone = tab.panes.pane_count() == 1;
        let placement = metrics.placement;
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
        metrics: PointerMetrics,
    ) -> termeric_core::Point {
        let (origin, scroll_offset) = self
            .focused_pane_bounds(metrics)
            .zip(self.active_terminal())
            .map_or_else(
                // With no tab, the area's top-left cell.
                || {
                    let placement = metrics.placement;
                    let bounds = placement.pane_bounds(Bounds::default(), true);
                    ((bounds.x, bounds.y), 0)
                },
                |((bounds, _), terminal)| ((bounds.x, bounds.y), terminal.grid().scroll_offset()),
            );
        screen_to_pane_grid(position, metrics.cell_size, origin, scroll_offset)
    }

    /// The focused pane's window rectangle, as drawn, and whether it is the
    /// tab's only pane.
    fn focused_pane_bounds(&self, metrics: PointerMetrics) -> Option<(Bounds, bool)> {
        let panes = &self.tabs.as_ref()?.active_tab()?.panes;
        let rect = *panes.calculate_layout().get(&panes.focused_id())?;
        let lone = panes.pane_count() == 1;
        let placement = metrics.placement;
        Some((placement.pane_bounds(rect, lone), lone))
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
        let projected = self
            .search
            .filtered_rows(terminal.presentation_grid().rows());
        visible_link_at(terminal, Some(&self.url_cache), projected, row, point.col)
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
        metrics: PointerMetrics,
    ) -> bool {
        if !matches!(self.broadcast, BroadcastState::Armed { .. }) {
            return false;
        }
        let target = self
            .pane_at_pointer(position, metrics)
            .filter(|pane| !pane.lone);
        target.is_some_and(|pane| self.toggle_armed_broadcast_target(pane.identity.pane_id))
    }

    /// Focus the pane under a content click. Returns whether focus moved.
    fn focus_pane_at(&mut self, position: (f64, f64), metrics: PointerMetrics) -> bool {
        let Some(target) = self.pane_at_pointer(position, metrics) else {
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
        {
            self.tab_rename = None;
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

    /// The pane of the active tab whose scrollbar takes the pointer at
    /// `position`, by the geometry the renderer draws it with.
    fn scrollbar_at(&self, position: (f64, f64)) -> Option<crate::tabs::TerminalIdentity> {
        // A window without a renderer has drawn no scrollbar to hit.
        self.renderer.as_ref()?;
        let tab = self.tabs.as_ref()?.active_tab()?;
        let lone = tab.panes.pane_count() == 1;
        let metrics = self.pointer_metrics();
        let (x, y) = (position.0 as f32, position.1 as f32);
        let layout = tab.panes.calculate_layout();
        let pane_id = layout.into_iter().find_map(|(pane_id, rect)| {
            let grid = tab.panes.get_pane(pane_id)?.terminal.presentation_grid();
            metrics
                .placement
                .pane_scrollbar(rect, lone, grid, metrics.cell_size)
                .filter(|scrollbar| scrollbar.hit.contains(x, y))
                .map(|_| pane_id)
        })?;
        Some(crate::tabs::TerminalIdentity {
            tab_id: tab.id,
            pane_id,
        })
    }

    /// Map a vertical mouse position to a scroll offset and apply it
    pub(super) fn scroll_to_scrollbar_position(&mut self, y: f64) {
        if self.renderer.is_none() {
            return;
        }
        self.drag_scrollbar_to(self.pointer_metrics(), y as f32);
    }

    /// Scroll the dragged pane to `y` along the scrollbar track it is drawn
    /// with: the track's top is the oldest scrollback, its bottom the live
    /// screen.
    fn drag_scrollbar_to(&mut self, metrics: PointerMetrics, y: f32) {
        if !self.reconcile_scrollbar_drag() {
            return;
        }
        let Some(track) = self
            .focused_pane_scrollbar(metrics)
            .map(|scrollbar| scrollbar.track)
        else {
            return;
        };
        let from_top = ((y - track.y) / track.height).clamp(0.0, 1.0);
        self.apply_scrollbar_drag_fraction(1.0 - from_top);
    }

    /// The scrollbar of the active tab's focused pane, as it is drawn.
    fn focused_pane_scrollbar(
        &self,
        metrics: PointerMetrics,
    ) -> Option<crate::renderer::PaneScrollbar> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        let rect = *tab.panes.calculate_layout().get(&tab.panes.focused_id())?;
        let lone = tab.panes.pane_count() == 1;
        let grid = tab.focused_terminal()?.presentation_grid();
        metrics
            .placement
            .pane_scrollbar(rect, lone, grid, metrics.cell_size)
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

/// Where one wheel step goes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WheelRoute {
    /// Chrome under the pointer takes it, and nothing scrolls.
    Chrome,
    /// A pane, or the focused pane when the pointer is over none.
    Terminal(Option<WheelTarget>),
}

/// Chrome drawn over the panes, which owns the pointer wherever it is
/// drawn: no press, hover, or wheel there reaches the program beneath.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FloatingChrome {
    Toast,
    /// The find box, and the part of it there.
    FindBox(SearchBarClick),
    /// The scrollbar of the pane it is drawn in.
    Scrollbar(crate::tabs::TerminalIdentity),
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

/// The link drawn at visible `row`, `col` of `terminal`'s presented grid: an
/// OSC 8 hyperlink first, then a URL detected in `urls`, which covers the
/// same visible rows. `projected` maps visible rows to source rows while a
/// filter hides the others.
fn visible_link_at<'a>(
    terminal: &'a termeric_core::Terminal,
    urls: Option<&'a UrlCache>,
    projected: &[i32],
    row: usize,
    col: usize,
) -> Option<&'a str> {
    let grid = terminal.presentation_grid();
    if row >= grid.rows() || col >= grid.cols() {
        return None;
    }
    let source_row = if projected.is_empty() {
        (row as i32).saturating_sub(i32::try_from(grid.scroll_offset()).ok()?)
    } else {
        *projected.get(row)?
    };
    grid.row_at_offset(source_row)
        .and_then(|cells| cells.get(col).and_then(|cell| cell.hyperlink_id))
        .and_then(|id| terminal.get_hyperlink_url(id))
        .or_else(|| urls?.url_at(row, col).map(|url| url.url.as_str()))
}

/// The pointer over the terminal area given the program's shape for the
/// pane under it (`None` when over no cell), whether a selection is being
/// made, and whether Ctrl is held over a link.
fn terminal_pointer_icon(
    program: Option<termeric_core::PointerShape>,
    selecting: bool,
    ctrl_over_link: bool,
) -> winit::window::CursorIcon {
    use winit::window::CursorIcon;
    match program {
        None => CursorIcon::Default,
        Some(_) if selecting => CursorIcon::Text,
        Some(_) if ctrl_over_link => CursorIcon::Pointer,
        Some(shape) => native_pointer_icon(shape),
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

/// The cell under `position` of a grid drawn from `origin`, in live
/// scrollback coordinates.
fn screen_to_pane_grid(
    position: (f64, f64),
    (cell_width, cell_height): (f32, f32),
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

    /// A window with no tab bar or padding and 10×20 cells, as the test
    /// panes are laid out.
    const TEST_METRICS: PointerMetrics = PointerMetrics {
        placement: PanePlacement {
            left: 0.0,
            top: 0.0,
            right: 800.0,
            bottom: 480.0,
            padding: 0.0,
            scale_factor: 1.0,
        },
        cell_size: (10.0, 20.0),
    };

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
                assert!(app.try_report_mouse_press(MouseButton::Left, position, TEST_METRICS));
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
                    app.try_report_mouse_motion(position, None, TEST_METRICS);
                }
                app.try_report_mouse_release(MouseButton::Left, position, TEST_METRICS);
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
        assert!(app.try_report_mouse_press(MouseButton::Left, position, TEST_METRICS));
        let capture = app.terminal_mouse_capture;
        app.refresh_focused_terminal_surfaces();
        assert_eq!(app.terminal_mouse_capture, capture);
        assert!(!app.try_report_mouse_release(MouseButton::Right, position, TEST_METRICS));
        assert_eq!(app.terminal_mouse_capture, capture);
        // Modifier changes must not turn a captured application drag into selection.
        app.modifiers = ModifiersState::SHIFT | ModifiersState::ALT;
        assert!(app.try_report_mouse_motion(position, None, TEST_METRICS));
        assert!(app.try_report_mouse_release(MouseButton::Left, position, TEST_METRICS));
        assert!(app.terminal_mouse_capture.is_none());
        assert!(!app.try_report_mouse_release(MouseButton::Left, position, TEST_METRICS));
        app.modifiers = ModifiersState::empty();
        // DECSET 1003 hover remains allowed without a captured button.
        app.active_terminal_mut()
            .unwrap()
            .set_mode(TerminalMode::MOUSE_ANY_EVENT, true);
        assert!(app.try_report_mouse_motion(position, None, TEST_METRICS));
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
            assert!(app.try_report_mouse_press(MouseButton::Left, position, TEST_METRICS));
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
            app.try_report_mouse_motion(position, None, TEST_METRICS);
            app.try_report_mouse_release(MouseButton::Left, position, TEST_METRICS);
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
            scale_factor: 1.0,
        };
        let rect = Bounds {
            x: 100.0,
            y: 60.0,
            width: 200.0,
            height: 160.0,
        };
        let split = placement.pane_bounds(rect, false);
        let point = screen_to_pane_grid((196.0, 174.0), (10.0, 20.0), (split.x, split.y), 3);
        assert_eq!(point, termeric_core::Point::new(0, 4));
        // A lone pane's grid starts one padding below the area's top.
        let lone = placement.pane_bounds(rect, true);
        let point = screen_to_pane_grid((96.0, 96.0), (10.0, 20.0), (lone.x, lone.y), 0);
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
            scale_factor: 1.0,
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

    /// Where a wheel step at `position` goes with no chrome there.
    fn terminal_wheel_target(app: &App, position: (f64, f64)) -> Option<WheelTarget> {
        match app.wheel_route_at(position, None, TEST_METRICS) {
            WheelRoute::Terminal(target) => target,
            WheelRoute::Chrome => panic!("no chrome lies at {position:?}"),
        }
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
        let right = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        for pane in [left, right] {
            panes
                .get_pane_mut(pane)
                .unwrap()
                .terminal
                .process(b"history\r\n".repeat(80).as_slice());
        }
        panes.focus(left);

        let target = terminal_wheel_target(&app, pane_position(&app, right));
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
            let target = terminal_wheel_target(&app, pane_position(&app, pane));
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
        let target = terminal_wheel_target(&app, pane_position(&app, hovered));
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
    fn chrome_over_a_pane_keeps_hover_and_the_wheel_from_its_program() {
        use crate::app::tests::{fenced_harvest_capture, harvest_rerun_capture_fixture_in};
        use termeric_core::term::TerminalMode;
        let (mut app, _directory, _, captures) = harvest_rerun_capture_fixture_in(true);
        let focused = app.active_panes().focused_id();
        let identity = app
            .tabs
            .as_ref()
            .unwrap()
            .active_tab()
            .unwrap()
            .focused_terminal_identity();
        let terminal = &mut app
            .active_panes_mut()
            .get_pane_mut(focused)
            .unwrap()
            .terminal;
        terminal.process(b"history\r\n".repeat(80).as_slice());
        terminal.set_mode(
            TerminalMode::MOUSE_ANY_EVENT | TerminalMode::MOUSE_SGR,
            true,
        );
        let position = pane_position(&app, focused);
        let chrome = [
            FloatingChrome::Toast,
            FloatingChrome::FindBox(SearchBarClick::Background),
            FloatingChrome::FindBox(SearchBarClick::Query(0)),
            FloatingChrome::Scrollbar(identity),
        ];

        // Hover over chrome is the chrome's.
        for chrome in chrome {
            assert!(!app.try_report_mouse_motion(position, Some(chrome), TEST_METRICS));
        }
        // The wheel over the find box or a toast scrolls nothing; over a
        // scrollbar it scrolls that pane's scrollback, though its program
        // asks for the wheel.
        for chrome in &chrome[..3] {
            let route = app.wheel_route_at(position, Some(*chrome), TEST_METRICS);
            assert_eq!(route, WheelRoute::Chrome, "{chrome:?}");
        }
        let route = app.wheel_route_at(position, Some(chrome[3]), TEST_METRICS);
        let WheelRoute::Terminal(target) = route else {
            panic!("a scrollbar's wheel goes to its pane: {route:?}");
        };
        app.scroll_terminal(3.0, target);
        assert_eq!(pane_scroll_offset(&app, focused), 3);
        app.scroll_terminal(-3.0, target);
        assert_eq!(pane_scroll_offset(&app, focused), 0);

        // Beside chrome, hover is reported; a drag the program holds keeps
        // reporting across chrome. These are all the program receives.
        assert!(app.try_report_mouse_motion(position, None, TEST_METRICS));
        assert!(app.try_report_mouse_press(MouseButton::Left, position, TEST_METRICS));
        assert!(app.try_report_mouse_motion(position, Some(chrome[1]), TEST_METRICS));
        assert_only_pane_received(
            &captures,
            fenced_harvest_capture(&mut app, &captures),
            focused,
            b"\x1b[<35;2;2M\x1b[<0;2;2M\x1b[<32;2;2M",
        );
    }

    #[test]
    fn dragging_a_scrollbar_follows_the_track_it_is_drawn_with() {
        use crate::app::tests::harvest_rerun_capture_fixture_in;
        let placement = PanePlacement {
            left: 0.0,
            top: 30.0,
            right: 800.0,
            bottom: 600.0,
            padding: 12.0,
            scale_factor: 1.0,
        };
        let metrics = PointerMetrics {
            placement,
            cell_size: (10.0, 20.0),
        };
        for split in [false, true] {
            let (mut app, _directory, _, _) = harvest_rerun_capture_fixture_in(split);
            let focused = app.active_panes().focused_id();
            app.active_panes_mut()
                .get_pane_mut(focused)
                .unwrap()
                .terminal
                .process(b"history\r\n".repeat(80).as_slice());
            let scrollback = app
                .active_panes()
                .get_pane(focused)
                .unwrap()
                .terminal
                .grid()
                .scrollback_len();
            assert!(scrollback > 0);
            let track = app
                .focused_pane_scrollbar(metrics)
                .expect("a pane with scrollback draws a scrollbar")
                .track;
            app.begin_scrollbar_drag();

            // The track's top is the oldest scrollback, its bottom the live
            // screen, and its middle half way.
            app.drag_scrollbar_to(metrics, track.y);
            assert_eq!(pane_scroll_offset(&app, focused), scrollback, "{split}");
            app.drag_scrollbar_to(metrics, track.bottom());
            assert_eq!(pane_scroll_offset(&app, focused), 0, "{split}");
            app.drag_scrollbar_to(metrics, track.y + track.height / 2.0);
            assert_eq!(pane_scroll_offset(&app, focused), scrollback / 2, "{split}");
        }
    }

    #[test]
    fn scrollbars_are_found_and_dragged_on_the_grid_the_pane_draws() {
        use crate::app::tests::harvest_rerun_capture_fixture_in;
        let placement = PanePlacement {
            left: 0.0,
            top: 30.0,
            right: 800.0,
            bottom: 600.0,
            padding: 12.0,
            scale_factor: 1.0,
        };
        let metrics = PointerMetrics {
            placement,
            cell_size: (10.0, 20.0),
        };
        let (mut app, _directory, _, _) = harvest_rerun_capture_fixture_in(false);
        let focused = app.active_panes().focused_id();
        let terminal = &mut app
            .active_panes_mut()
            .get_pane_mut(focused)
            .unwrap()
            .terminal;
        terminal.process(b"history\r\n".repeat(80).as_slice());
        let track = app
            .focused_pane_scrollbar(metrics)
            .expect("a pane with scrollback draws a scrollbar")
            .track;
        app.begin_scrollbar_drag();

        // Synchronized output freezes the screen without its scrollback, so
        // no scrollbar is drawn: none is found, and a drag moves nothing.
        app.active_panes_mut()
            .get_pane_mut(focused)
            .unwrap()
            .terminal
            .process(b"\x1b[?2026h");
        assert!(app.focused_pane_scrollbar(metrics).is_none());
        app.drag_scrollbar_to(metrics, track.y);
        assert_eq!(pane_scroll_offset(&app, focused), 0);

        // Once the frame is released, the drawn scrollbar takes the drag.
        app.active_panes_mut()
            .get_pane_mut(focused)
            .unwrap()
            .terminal
            .process(b"\x1b[?2026l");
        assert!(app.focused_pane_scrollbar(metrics).is_some());
        app.drag_scrollbar_to(metrics, track.y);
        assert!(pane_scroll_offset(&app, focused) > 0);
    }

    #[test]
    fn every_press_on_a_scrollbar_is_the_scrollbars() {
        use crate::app::tests::fenced_harvest_capture;
        use termeric_core::term::TerminalMode;
        let (mut app, _directory, captures, target) = middle_click_fixture();
        app.active_panes_mut()
            .get_pane_mut(target)
            .unwrap()
            .terminal
            .set_mode(
                TerminalMode::MOUSE_BUTTON_EVENT | TerminalMode::MOUSE_SGR,
                true,
            );
        let identity = crate::tabs::TerminalIdentity {
            tab_id: app.tabs.as_ref().unwrap().active_tab_id().unwrap(),
            pane_id: target,
        };
        for button in [MouseButton::Middle, MouseButton::Right] {
            assert!(app.press_floating_chrome(button, Some(FloatingChrome::Scrollbar(identity))));
        }
        assert!(!app.press_floating_chrome(MouseButton::Middle, None));
        assert_only_pane_received(
            &captures,
            fenced_harvest_capture(&mut app, &captures),
            target,
            b"",
        );
    }

    /// The capture fixture's panes with an in-memory primary selection
    /// holding `hello`, and one pane other than the focused one.
    fn middle_click_fixture() -> (
        App,
        tempfile::TempDir,
        Vec<(crate::tabs::TerminalEventOrigin, std::path::PathBuf)>,
        PaneId,
    ) {
        use crate::app::tests::harvest_rerun_capture_fixture_in;
        let (mut app, directory, _, captures) = harvest_rerun_capture_fixture_in(true);
        let (clipboard, memory) = super::super::clipboard::Clipboard::in_memory();
        memory.lock().unwrap().primary = Some("hello".into());
        app.clipboard = clipboard;
        let focused = app.active_panes().focused_id();
        let target = app
            .active_panes()
            .pane_ids()
            .into_iter()
            .find(|pane| *pane != focused)
            .unwrap();
        (app, directory, captures, target)
    }

    fn assert_only_pane_received(
        captures: &[(crate::tabs::TerminalEventOrigin, std::path::PathBuf)],
        bytes: Vec<Vec<u8>>,
        pane: PaneId,
        expected: &[u8],
    ) {
        for ((origin, _), bytes) in captures.iter().zip(bytes) {
            let wanted: &[u8] = if origin.pane_id == pane {
                expected
            } else {
                b""
            };
            assert_eq!(
                String::from_utf8_lossy(&bytes),
                String::from_utf8_lossy(wanted),
                "{:?}",
                origin.pane_id
            );
        }
    }

    #[test]
    fn a_middle_click_pastes_the_primary_selection_into_the_pane_under_it() {
        use crate::app::tests::fenced_harvest_capture;
        use termeric_core::term::TerminalMode;
        let (mut app, _directory, captures, target) = middle_click_fixture();
        app.active_panes_mut()
            .get_pane_mut(target)
            .unwrap()
            .terminal
            .set_mode(TerminalMode::BRACKETED_PASTE, true);
        let position = pane_position(&app, target);
        assert!(!app.press_terminal(MouseButton::Middle, position, TEST_METRICS));
        assert_eq!(app.active_panes().focused_id(), target);
        let bytes = fenced_harvest_capture(&mut app, &captures);
        assert_only_pane_received(&captures, bytes, target, b"\x1b[200~hello\x1b[201~");
    }

    #[test]
    fn a_program_using_the_mouse_gets_the_middle_click_and_shift_pastes_instead() {
        use crate::app::tests::fenced_harvest_capture;
        use termeric_core::term::TerminalMode;
        let (mut app, _directory, captures, target) = middle_click_fixture();
        app.active_panes_mut()
            .get_pane_mut(target)
            .unwrap()
            .terminal
            .set_mode(
                TerminalMode::MOUSE_BUTTON_EVENT | TerminalMode::MOUSE_SGR,
                true,
            );
        let position = pane_position(&app, target);
        assert!(app.press_terminal(MouseButton::Middle, position, TEST_METRICS));
        assert!(app.try_report_mouse_release(MouseButton::Middle, position, TEST_METRICS));
        app.modifiers = ModifiersState::SHIFT;
        assert!(!app.press_terminal(MouseButton::Middle, position, TEST_METRICS));
        let bytes = fenced_harvest_capture(&mut app, &captures);
        assert_only_pane_received(&captures, bytes, target, b"\x1b[<1;2;2M\x1b[<1;2;2mhello");
    }

    #[test]
    fn the_find_box_takes_every_press_on_it_and_a_middle_click_pastes_into_its_query() {
        use crate::app::tests::fenced_harvest_capture;
        use winit::keyboard::Key;
        let (mut app, _directory, captures, target) = middle_click_fixture();
        let (clipboard, memory) = super::super::clipboard::Clipboard::in_memory();
        memory.lock().unwrap().primary = Some("hello".into());
        app.clipboard = clipboard;
        app.search.start();
        assert!(app.handle_search_key(&Key::Character("ab".into())));
        app.search.confirm();

        // The middle button on the query resumes it and pastes where it
        // landed, even after the query was confirmed.
        assert!(app.press_search_bar(MouseButton::Middle, SearchBarClick::Query(1)));
        assert!(app.search.is_active());
        assert_eq!(app.search.query(), "ahellob");
        assert_eq!(app.search_cursor_pos, "ahello".len());

        // The same limits as a keyboard paste: nothing that breaks the line.
        memory.lock().unwrap().primary = Some("one\ntwo".into());
        assert!(app.press_search_bar(MouseButton::Middle, SearchBarClick::Query(0)));
        assert_eq!(app.search.query(), "ahellob");

        // Anywhere else on the box, a press other than the primary one does
        // nothing at all; beside the box it is not the box's.
        for (button, click) in [
            (MouseButton::Middle, SearchBarClick::Background),
            (MouseButton::Middle, SearchBarClick::Close),
            (MouseButton::Middle, SearchBarClick::NextMatch),
            (MouseButton::Right, SearchBarClick::Query(0)),
            (MouseButton::Right, SearchBarClick::Close),
        ] {
            assert!(app.press_search_bar(button, click), "{button:?} {click:?}");
        }
        assert!(!app.press_search_bar(MouseButton::Middle, SearchBarClick::None));
        assert!(app.search.is_active());
        assert_eq!(app.search.query(), "ahellob");

        // While the find box holds the keyboard, a middle click on a pane
        // pastes nowhere.
        memory.lock().unwrap().primary = Some("hello".into());
        let position = pane_position(&app, target);
        assert!(!app.press_terminal(MouseButton::Middle, position, TEST_METRICS));
        assert_eq!(app.search.query(), "ahellob");
        let bytes = fenced_harvest_capture(&mut app, &captures);
        assert!(bytes.iter().all(Vec::is_empty), "{bytes:?}");
    }

    #[test]
    fn the_pointer_is_a_hand_over_a_link_only_while_ctrl_is_held() {
        use termeric_core::PointerShape;
        use winit::window::CursorIcon;
        let shape = Some(PointerShape::Crosshair);
        assert_eq!(
            terminal_pointer_icon(shape, false, false),
            CursorIcon::Crosshair
        );
        assert_eq!(
            terminal_pointer_icon(shape, false, true),
            CursorIcon::Pointer
        );
        // A selection in progress keeps its I-beam, link or not.
        assert_eq!(terminal_pointer_icon(shape, true, true), CursorIcon::Text);
        // Off the cells, nothing applies.
        assert_eq!(
            terminal_pointer_icon(None, false, true),
            CursorIcon::Default
        );
    }

    #[test]
    fn ctrl_over_a_link_in_any_pane_shows_the_hand() {
        use winit::window::CursorIcon;
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
        let right = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        panes.focus(left);
        // A detected URL in the focused pane and an OSC 8 link in the other,
        // both starting at the second row's first cell.
        panes
            .get_pane_mut(left)
            .unwrap()
            .terminal
            .process(b"\r\nhttps://left.example");
        panes
            .get_pane_mut(right)
            .unwrap()
            .terminal
            .process(b"\r\n\x1b]8;;https://right.example\x1b\\link\x1b]8;;\x1b\\ plain");
        app.refresh_focused_url_cache();

        for pane in [left, right] {
            let link = pane_position(&app, pane);
            let plain = (link.0 + 250.0, link.1);
            app.modifiers = ModifiersState::empty();
            assert_eq!(
                app.terminal_pointer_icon_at(link, TEST_METRICS),
                CursorIcon::Text
            );
            app.modifiers = ModifiersState::CONTROL;
            assert_eq!(
                app.terminal_pointer_icon_at(link, TEST_METRICS),
                CursorIcon::Pointer,
                "{pane:?}"
            );
            assert_eq!(
                app.terminal_pointer_icon_at(plain, TEST_METRICS),
                CursorIcon::Text,
                "{pane:?}"
            );
        }
    }

    #[test]
    fn a_second_click_on_the_same_workspace_row_opens_it() {
        use crate::ui::workspace_panel::WorkspacePanelAction;
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        let select = |id: &str| WorkspacePanelAction::Select(id.into());

        assert_eq!(app.promote_workspace_double_click(select("a")), select("a"));
        assert_eq!(app.promote_workspace_double_click(select("b")), select("b"));
        assert_eq!(
            app.promote_workspace_double_click(select("b")),
            WorkspacePanelAction::Load("b".into())
        );
        // A third click starts a new pair rather than opening again.
        assert_eq!(app.promote_workspace_double_click(select("b")), select("b"));
        // Any other action in between breaks the pair.
        assert_eq!(
            app.promote_workspace_double_click(WorkspacePanelAction::None),
            WorkspacePanelAction::None
        );
        assert_eq!(app.promote_workspace_double_click(select("b")), select("b"));
    }
}
