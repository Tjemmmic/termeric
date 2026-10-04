//! One frame: building every layer's geometry and submitting it.

use super::*;

/// Everything one frame draws, borrowed from the window's state.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    /// The tab's terminal when it has no panes.
    pub terminal: TerminalFrame<'a>,
    /// The tab's panes; empty draws `terminal` instead.
    pub panes: &'a [PaneRenderData<'a>],
    pub pane_dividers: &'a [crate::panes::Divider],
    /// Search state, for match highlighting in whichever terminal has focus.
    pub search: &'a Search,
    pub text_blink: TextBlinkPhases,
    pub chrome: ChromeFrame<'a>,
}

/// The single terminal a tab without panes draws.
#[derive(Clone, Copy)]
pub struct TerminalFrame<'a> {
    /// Identifies the terminal across frames, for its row and image caches.
    pub key: usize,
    pub grid: &'a Grid,
    pub selection: Option<&'a Selection>,
    pub dynamic_colors: &'a termeric_core::DynamicColors,
    pub extra_cursors: &'a termeric_core::ExtraCursorState,
    /// 0.0 is invisible and 1.0 fully visible, for a smooth blink.
    pub cursor_opacity: f32,
    /// The animated cursor position as fractional (column, row).
    pub cursor_pos: (f32, f32),
    pub cursor_style: CursorStyle,
    pub images: Option<&'a [termeric_core::PresentationImage]>,
    pub url_cache: &'a UrlCache,
    pub copy_mode: Option<&'a CopyModeRenderInfo>,
}

/// The chrome drawn around and above the terminals; `None` surfaces are
/// closed.
#[derive(Clone, Copy)]
pub struct ChromeFrame<'a> {
    pub tabs: &'a [TabInfo],
    /// 0.0 is no flash and 1.0 a full copy-confirmation flash.
    pub copy_flash: f32,
    /// 0.0 is fully bright (focused); higher dims an unfocused window.
    pub focus_dim: f32,
    pub settings_panel: Option<&'a SettingsPanelInfo>,
    pub search_bar: Option<&'a SearchBarInfo>,
    pub ime_preedit: Option<&'a ImePreeditRenderInfo>,
    pub workspace_menu: Option<&'a PopupMenuInfo>,
    pub session_panel: Option<&'a crate::session::panel::SessionPanelInfo>,
    pub commands_panel: Option<&'a crate::ui::commands::panel::CommandsPanelInfo>,
    pub agents_panel: Option<&'a crate::ui::agents_panel::AgentsPanelInfo>,
    pub command_palette: Option<&'a PaletteInfo>,
    pub shortcuts_panel: Option<&'a ShortcutsPanelInfo>,
    pub harvest: Option<&'a HarvestRenderInfo>,
}

impl Renderer {
    /// Draw pane dividers from the same current layout used by hit testing.
    fn draw_pane_dividers(
        &mut self,
        dividers: &[crate::panes::Divider],
        content_left: f32,
        content_top: f32,
    ) {
        let padding = self.padding;

        let mut groove_color = self.ui.tab_border;
        groove_color[3] = 0.9;
        let mut edge_color = self.ui.cursor;
        edge_color[3] = 0.35;
        let groove_width = crate::panes::PANE_DIVIDER_THICKNESS - 1.0;

        for div in dividers {
            if div.vertical {
                let x = content_left + padding + div.position;
                let y0 = content_top + div.start;
                let y1 = content_top + div.end;
                let len = y1 - y0;
                self.rect_pipeline
                    .push_rect(x, y0, groove_width, len, groove_color);
                // Light edge on the right (1px)
                self.rect_pipeline
                    .push_rect(x + groove_width, y0, 1.0, len, edge_color);
            } else {
                let x0 = content_left + padding + div.start;
                let x1 = content_left + padding + div.end;
                let y = content_top + div.position;
                let len = x1 - x0;
                self.rect_pipeline
                    .push_rect(x0, y, len, groove_width, groove_color);
                // Light edge on bottom (1px)
                self.rect_pipeline
                    .push_rect(x0, y + groove_width, len, 1.0, edge_color);
            }
        }
    }

    /// Draw and present one frame.
    pub fn render(&mut self, frame: &Frame<'_>) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&TextureViewDescriptor {
            format: Some(self.render_format),
            ..Default::default()
        });

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        self.glyph_cache.begin_frame();
        self.terminal_row_cache.begin_frame();
        self.dragged_tab_surface = None;
        // Build render data from grid and tabs (inserts layer boundaries)
        self.build_render_data(frame);

        // Build image render data if any
        self.build_background_image_data();
        self.build_image_data(
            frame.terminal.key,
            frame.terminal.grid,
            frame.terminal.images,
            frame.panes,
        );

        // Upload vertex data
        self.rect_pipeline.upload(&self.device, &self.queue);
        self.text_pipeline
            .upload(&self.device, &self.queue, &mut self.glyph_cache);
        self.background_image_pipeline
            .upload(&self.device, &self.queue);
        self.image_pipeline.upload(&self.device, &self.queue);
        self.terminal_row_cache.finish_frame();
        log::trace!(
            "Glyph cache: atlas {:?}; {} rasterizations; shaped runs {:?}",
            self.glyph_cache.atlas_metrics(),
            self.glyph_cache.rasterizations(),
            self.glyph_cache.shaping_cache_metrics()
        );

        {
            let mut render_pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: self.ui.background.r * self.opacity as f64,
                            g: self.ui.background.g * self.opacity as f64,
                            b: self.ui.background.b * self.opacity as f64,
                            a: self.opacity as f64,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Draw layers interleaved: for each layer, draw its rects then its text.
            // This ensures overlay backgrounds (higher layers) fully occlude content
            // from lower layers, fixing the bleed-through issue.
            let layer_count = self
                .rect_pipeline
                .layer_count()
                .max(self.text_pipeline.layer_count());

            for layer in 0..layer_count {
                // Draw rectangles for this layer (backgrounds, panels, etc.)
                if layer == 0 {
                    self.background_image_pipeline
                        .draw_below_text(&mut render_pass);
                    self.terminal_row_cache
                        .draw_rects(&self.rect_pipeline, &mut render_pass);
                }
                self.rect_pipeline.draw_layer(&mut render_pass, layer);

                // Draw images only on the base layer (layer 0) — images are terminal content
                if layer == 0 {
                    self.image_pipeline.draw_below_text(&mut render_pass);
                    self.terminal_row_cache
                        .draw_text(&self.text_pipeline, &mut render_pass);
                }

                // Draw text for this layer
                self.text_pipeline.draw_layer(&mut render_pass, layer);
                if layer == 0 {
                    self.image_pipeline.draw_above_text(&mut render_pass);
                }
            }
        }

        let tab_copy = self.copy_dragged_tab(&mut encoder, &output.texture);
        let submission = self.queue.submit(std::iter::once(encoder.finish()));
        // On Wayland this asks the compositor for a frame callback, and winit
        // holds the next redraw until it arrives. A window the compositor is
        // not showing, such as one on another workspace, then is not asked to
        // present again; presenting to it would wait inside the driver for a
        // callback that never comes, stalling the whole event loop.
        self.window.pre_present_notify();
        output.present();
        if let Some(copy) = tab_copy {
            self.finish_dragged_tab_copy(copy, submission);
        }

        Ok(())
    }

    /// Build image render data from placements
    fn build_image_data(
        &mut self,
        terminal_key: usize,
        grid: &Grid,
        images: Option<&[termeric_core::PresentationImage]>,
        panes: &[PaneRenderData],
    ) {
        self.image_pipeline.clear();
        let active_keys: Vec<_> = if panes.is_empty() {
            images
                .unwrap_or_default()
                .iter()
                .map(|presentation| (terminal_key, presentation.image.id))
                .collect()
        } else {
            panes
                .iter()
                .flat_map(|pane| {
                    pane.images
                        .iter()
                        .map(move |presentation| (pane.terminal_key, presentation.image.id))
                })
                .collect()
        };
        self.image_pipeline.gc_images(&active_keys);

        let content_top = self.content_top();
        let content_left = self.tab_bar_width();
        if panes.is_empty() {
            self.queue_terminal_images(
                terminal_key,
                images.unwrap_or_default(),
                content_left + self.padding,
                content_top + self.padding,
                grid.scroll_offset(),
                Some((
                    grid.cols() as f32 * self.cell_width(),
                    grid.rows() as f32 * self.cell_height(),
                )),
            );
        } else {
            for pane in panes {
                self.queue_terminal_images(
                    pane.terminal_key,
                    &pane.images,
                    content_left + self.padding + pane.rect.x,
                    content_top + pane.rect.y,
                    pane.grid.scroll_offset(),
                    Some((pane.rect.width, pane.rect.height)),
                );
            }
        }
    }

    fn queue_terminal_images(
        &mut self,
        terminal_key: usize,
        images: &[termeric_core::PresentationImage],
        origin_x: f32,
        origin_y: f32,
        scroll_offset: usize,
        viewport: Option<(f32, f32)>,
    ) {
        for presentation in images {
            let placement = &presentation.placement;
            let image = &presentation.image;
            let key = (terminal_key, image.id);
            self.image_pipeline
                .upload_image(&self.device, &self.queue, key, image);

            if let Some(geometry) = presentation_geometry(
                presentation,
                self.cell_width(),
                self.cell_height(),
                scroll_offset,
                viewport,
            ) {
                self.image_pipeline.push_image(
                    key,
                    origin_x + geometry.x,
                    origin_y + geometry.y,
                    geometry.width,
                    geometry.height,
                    geometry.u_min,
                    geometry.v_min,
                    geometry.u_max,
                    geometry.v_max,
                    1.0,
                    placement.z_index,
                );
            }
        }
    }

    /// Build render data from the grid and tabs
    fn build_render_data(&mut self, frame: &Frame<'_>) {
        let Frame {
            terminal:
                TerminalFrame {
                    key: _,
                    grid,
                    selection,
                    dynamic_colors,
                    extra_cursors,
                    cursor_opacity,
                    cursor_pos,
                    cursor_style,
                    images: _,
                    url_cache,
                    copy_mode,
                },
            panes,
            pane_dividers,
            search,
            text_blink,
            chrome:
                ChromeFrame {
                    tabs,
                    copy_flash,
                    focus_dim,
                    settings_panel,
                    search_bar,
                    ime_preedit,
                    workspace_menu,
                    session_panel,
                    commands_panel,
                    agents_panel,
                    command_palette,
                    shortcuts_panel,
                    harvest,
                },
        } = *frame;
        self.rect_pipeline.clear();
        self.text_pipeline.clear();
        self.visible_text_blink = VisibleTextBlink::default();

        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let window_width = self.size.0 as f32;
        let window_height = self.size.1 as f32;
        let padding = self.padding;
        let tab_bar_height = self.tab_bar_height();
        let content_left = self.tab_bar_width();
        let content_width = self.content_width_for(window_width);

        // Calculate brightness multiplier for focus dimming (1.0 = full brightness, lower = dimmed)
        let brightness = 1.0 - focus_dim;

        // Content starts directly below the tab bar; the find box floats
        // over it.
        let content_top = tab_bar_height;

        // ── LAYER 0: Base content (tab bar, grid, cursor, scrollbar) ──
        // Render tab bar first
        self.build_tab_bar(tabs, window_width, window_height, tab_bar_height);
        if let Some(target) = self.tab_bar_keyboard_focus {
            self.build_tab_bar_keyboard_focus(target, tabs, window_width, window_height);
        }

        // ── Render terminal grid(s) ──
        // Multi-pane mode: render each pane's grid in its own viewport
        // Single-grid mode: render one grid filling the content area
        if !panes.is_empty() {
            for pane in panes {
                let pane_x = content_left + padding + pane.rect.x;
                let pane_y = content_top + pane.rect.y;
                let pane_cols = (pane.rect.width / cell_width).floor() as usize;
                let pane_rows = (pane.rect.height / cell_height).floor() as usize;

                // Use pre-computed cursor position (animated for focused, raw for unfocused)
                let pane_cursor_pos = pane.cursor_pos;

                self.build_pane_grid(
                    pane.grid,
                    pane.selection,
                    pane.dynamic_colors,
                    pane.extra_cursors,
                    pane_x,
                    pane_y,
                    pane_cols.max(1),
                    pane_rows.max(1),
                    pane.is_focused,
                    pane.cursor_style,
                    pane.cursor_opacity,
                    pane_cursor_pos,
                    text_blink,
                    copy_flash,
                    brightness,
                    search,
                    pane.url_cache,
                    window_height,
                    pane.rect.width,
                );
                if let Some(copy_mode) = pane.copy_mode {
                    self.build_copy_mode_overlay(
                        copy_mode,
                        pane.grid.scroll_offset(),
                        pane_x,
                        pane_y,
                        pane.rect.width,
                        pane_rows.max(1),
                        window_height,
                    );
                }

                // Broadcast is deliberately unmistakable; zoom and focus use
                // progressively quieter outlines around the exact pane bounds.
                if pane.broadcast_target.is_some()
                    || pane.read_only
                    || pane.is_zoomed
                    || (panes.len() > 1 && pane.is_focused)
                {
                    let border_color = if let Some(state) = pane.broadcast_target {
                        match state {
                            BroadcastTargetState::Armed => [0.96, 0.68, 0.16, 1.0],
                            BroadcastTargetState::Active => [0.95, 0.24, 0.12, 1.0],
                        }
                    } else if pane.read_only {
                        [0.30, 0.78, 0.72, 1.0]
                    } else if pane.is_zoomed {
                        with_alpha(self.ui.palette_accent, 0.9)
                    } else {
                        with_alpha(self.ui.cursor, 0.5)
                    };
                    let bw = if pane.broadcast_target.is_some() || pane.read_only {
                        4.0
                    } else {
                        2.0
                    };
                    self.rect_pipeline.push_rounded_outline(
                        pane_x,
                        pane_y,
                        pane.rect.width,
                        pane.rect.height,
                        border_color,
                        0.0,
                        bw,
                    );
                }
            }
        } else {
            // Single grid mode — full content area
            let grid_x = content_left + padding;
            let grid_y = content_top + padding;
            let grid_cols = grid.cols();
            let grid_rows = grid.rows();
            let grid_width = (content_width - padding * 2.0).max(0.0);

            self.build_pane_grid(
                grid,
                selection,
                dynamic_colors,
                extra_cursors,
                grid_x,
                grid_y,
                grid_cols,
                grid_rows,
                true, // always focused in single mode
                cursor_style,
                cursor_opacity,
                cursor_pos,
                text_blink,
                copy_flash,
                brightness,
                search,
                url_cache,
                window_height,
                grid_width,
            );
            if let Some(copy_mode) = copy_mode {
                self.build_copy_mode_overlay(
                    copy_mode,
                    grid.scroll_offset(),
                    grid_x,
                    grid_y,
                    grid_width,
                    grid_rows,
                    window_height,
                );
            }
        }

        if !pane_dividers.is_empty() {
            self.draw_pane_dividers(pane_dividers, content_left, content_top);
        }

        // Visual bell flash — subtle screen-edge glow
        if self.bell_intensity > 0.0 {
            let alpha = self.bell_intensity * 0.15;
            let flash_color = [1.0, 1.0, 1.0, alpha];
            self.rect_pipeline
                .push_rect(0.0, 0.0, window_width, window_height, flash_color);
        }

        // ── Agent view: the docked panel right of the terminals, then its
        // menus, which must cover its own text ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        self.build_agent_view();
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        self.build_agent_view_overlay();

        // ── Harvest indicators (in gutter, alongside grid) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(harvest_info) = harvest {
            self.build_harvest_indicators(&harvest_info.indicators, content_top + padding);
        }

        // ── Search bar, over the grid and under every modal layer ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(search_info) = search_bar {
            self.build_search_bar(search_info, content_left, content_width, content_top);
        }

        // ── Git completion dropdown, over the grid ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        // Overlay state is taken while it draws and put back afterwards, so
        // building it can borrow the renderer without copying the state.
        if let Some(info) = self.git_completions.take() {
            if let Some(anchor) = git::CompletionAnchor::for_view(
                cursor_pos,
                panes,
                (content_left, content_top),
                (cell_width, cell_height),
                padding,
            ) {
                self.build_git_completions(
                    &info,
                    anchor,
                    content_left,
                    content_top,
                    content_width,
                    window_height,
                );
            }
            self.git_completions = Some(info);
        }

        // ── Modal layers, bottom first: the one drawn on top owns input ──
        for layer in crate::ui::modal::ModalLayer::draw_order() {
            use crate::ui::modal::ModalLayer;
            self.rect_pipeline.mark_layer();
            self.text_pipeline.mark_layer();
            match layer {
                ModalLayer::AgentsPanel => {
                    if let Some(agents) = agents_panel
                        && agents.animation > 0.001
                    {
                        self.build_agents_panel(agents, window_width, window_height);
                    }
                }
                ModalLayer::CommandsPanel => {
                    if let Some(cp) = commands_panel
                        && cp.animation > 0.001
                    {
                        self.build_commands_panel(cp, window_width, window_height);
                    }
                }
                ModalLayer::WorkspaceMenu => {
                    if let Some(menu) = workspace_menu {
                        self.build_popup_menu(menu);
                    }
                }
                ModalLayer::SessionPanel => {
                    if let Some(sp) = session_panel
                        && sp.animation > 0.001
                    {
                        self.build_session_management_panel(sp, window_width, window_height);
                    }
                }
                ModalLayer::Settings => {
                    if let Some(sp) = settings_panel
                        && sp.animation > 0.001
                    {
                        self.build_settings_panel(sp, window_width, window_height);
                        if sp.dropdown_open {
                            self.rect_pipeline.mark_layer();
                            self.text_pipeline.mark_layer();
                            self.build_dropdown_overlay(sp, window_width, window_height);
                        }
                    }
                }
                ModalLayer::ShortcutsPanel => {
                    if let Some(sp) = shortcuts_panel
                        && sp.animation > 0.001
                    {
                        self.build_shortcuts_panel(sp, window_width, window_height);
                    }
                }
                // The palette draws whichever palette-style modal is open;
                // at most one is.
                ModalLayer::CommandPalette => {
                    if let Some(palette) = command_palette
                        && palette.animation > 0.001
                    {
                        self.build_command_palette(palette, window_width, window_height);
                    }
                }
                ModalLayer::UnicodePicker | ModalLayer::DomainPicker => {}
                ModalLayer::Cellar => {
                    if let Some(cellar) = harvest.and_then(|harvest| harvest.cellar.as_ref()) {
                        self.build_root_cellar(cellar, window_width, window_height);
                    }
                }
                ModalLayer::SproutMenu => {
                    if let Some(menu) = harvest.and_then(|harvest| harvest.sprout_menu.as_ref()) {
                        self.build_sprout_menu(menu);
                    }
                }
                ModalLayer::ContextPicker => {
                    if let Some(info) = self.context_picker.take() {
                        if info.snapshot.animation > 0.001 {
                            self.build_context_picker(&info, window_width, window_height);
                        }
                        self.context_picker = Some(info);
                    }
                }
                ModalLayer::DiffReview => {
                    if let Some(info) = self.diff_review.take() {
                        if info.animation > 0.001 {
                            self.build_diff_review(&info, window_width, window_height);
                        }
                        self.diff_review = Some(info);
                    }
                }
                ModalLayer::InfoOverlay => {
                    if let Some(info) = self.info_overlay.take() {
                        if info.animation > 0.001 {
                            self.build_info_overlay(&info, window_width, window_height);
                        }
                        self.info_overlay = Some(info);
                    }
                }
                ModalLayer::Guardrail => {
                    if let Some(info) = self.guardrail.take() {
                        if info.animation > 0.001 {
                            self.build_guardrail_confirmation(&info, window_width, window_height);
                        }
                        self.guardrail = Some(info);
                    }
                }
            }
        }

        // ── Toasts, above every modal layer and below only the IME preedit ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if !self.toasts.is_empty() {
            let toasts = std::mem::take(&mut self.toasts);
            // Toasts sit in the terminal area's corner, clear of the dock.
            let toast_width = self.content_right_for(window_width);
            self.build_toasts(&toasts, toast_width, window_height);
            self.toasts = toasts;
        }

        // ── Platform IME preedit (never PTY-owned) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(preedit) = ime_preedit {
            self.build_ime_preedit(preedit, window_width, window_height);
        }
    }
}
