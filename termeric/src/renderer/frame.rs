//! One frame: building every layer's geometry and submitting it.

use super::*;

/// Everything one frame draws, borrowed from the window's state.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    /// The active tab's visible panes: one when it is unsplit or zoomed.
    pub panes: &'a [PaneRenderData<'a>],
    /// Whether `panes` is the tab's only pane, which is drawn as an unsplit
    /// terminal (see [`PanePlacement::pane_bounds`]).
    pub lone_pane: bool,
    pub pane_dividers: &'a [crate::panes::Divider],
    /// Search state, for match highlighting in whichever terminal has focus.
    pub search: &'a Search,
    pub text_blink: TextBlinkPhases,
    pub chrome: ChromeFrame<'a>,
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
    pub session_panel: Option<&'a crate::ui::session_panel::SessionPanelInfo>,
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
        self.build_image_data(frame.panes, frame.lone_pane);

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
    fn build_image_data(&mut self, panes: &[PaneRenderData], lone_pane: bool) {
        self.image_pipeline.clear();
        let active_keys: Vec<_> = panes
            .iter()
            .flat_map(|pane| {
                pane.images
                    .iter()
                    .map(move |presentation| (pane.terminal_key, presentation.image.id))
            })
            .collect();
        self.image_pipeline.gc_images(&active_keys);

        let placement = self.pane_placement();
        for pane in panes {
            let bounds = placement.pane_bounds(pane.rect, lone_pane);
            // A lone pane's images are clipped to its grid's own cells, as an
            // unsplit terminal's always were; a split pane's to its rectangle.
            let viewport = if lone_pane {
                (
                    pane.grid.cols() as f32 * self.cell_width(),
                    pane.grid.rows() as f32 * self.cell_height(),
                )
            } else {
                (bounds.width, bounds.height)
            };
            self.queue_terminal_images(
                pane.terminal_key,
                &pane.images,
                bounds.x,
                bounds.y,
                pane.grid.scroll_offset(),
                Some(viewport),
            );
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
            panes,
            lone_pane,
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

        // ── Render terminal grids, each in its own viewport ──
        let placement = self.pane_placement();
        for pane in panes {
            let bounds = placement.pane_bounds(pane.rect, lone_pane);
            let (pane_cols, pane_rows) = PanePlacement::pane_cells(
                pane.rect,
                lone_pane,
                (pane.grid.cols(), pane.grid.rows()),
                (cell_width, cell_height),
            );
            let pane_width = bounds.width.max(0.0);
            let scrollbar = placement.pane_scrollbar(
                pane.rect,
                lone_pane,
                pane.grid,
                (cell_width, cell_height),
            );

            self.build_pane_grid(
                pane.grid,
                pane.selection,
                pane.dynamic_colors,
                pane.extra_cursors,
                bounds.x,
                bounds.y,
                pane_cols,
                pane_rows,
                pane.is_focused,
                pane.cursor_style,
                pane.cursor_opacity,
                pane.cursor_pos,
                text_blink,
                copy_flash,
                brightness,
                search,
                pane.url_cache,
                window_height,
                pane_width,
                scrollbar,
            );
            if let Some(copy_mode) = pane.copy_mode {
                self.build_copy_mode_overlay(
                    copy_mode,
                    pane.grid.scroll_offset(),
                    bounds.x,
                    bounds.y,
                    pane_width,
                    pane_rows,
                    window_height,
                );
            }

            // Broadcast is deliberately unmistakable; zoom and focus use
            // progressively quieter outlines around the exact pane bounds. A
            // lone pane has none: the tab bar shows its read-only state.
            if !lone_pane
                && (pane.broadcast_target.is_some()
                    || pane.read_only
                    || pane.is_zoomed
                    || (panes.len() > 1 && pane.is_focused))
            {
                let border_color = pane_outline_color(
                    &self.ui,
                    pane.broadcast_target,
                    pane.read_only,
                    pane.is_zoomed,
                );
                let bw = if pane.broadcast_target.is_some() || pane.read_only {
                    4.0
                } else {
                    2.0
                };
                self.rect_pipeline.push_rounded_outline(
                    bounds.x,
                    bounds.y,
                    pane.rect.width,
                    pane.rect.height,
                    border_color,
                    0.0,
                    bw,
                );
            }
        }

        if !pane_dividers.is_empty() {
            self.draw_pane_dividers(pane_dividers, content_left, content_top);
        }

        // Visual bell flash — subtle screen-edge glow
        if self.bell_intensity > 0.0 {
            let flash_color = bell_flash_color(&self.ui, self.bell_intensity);
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
            self.build_harvest_indicators(&harvest_info.indicators);
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
                panes,
                lone_pane,
                placement,
                (cell_width, cell_height),
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
            self.build_toasts(&toasts, window_width, window_height);
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

/// The outline of a split or zoomed pane. Broadcast states use the status
/// colors the tab bar shows them in, read-only the theme's cyan; zoom and
/// focus use progressively quieter accents.
fn pane_outline_color(
    ui: &UiPalette,
    broadcast_target: Option<BroadcastTargetState>,
    read_only: bool,
    zoomed: bool,
) -> [f32; 4] {
    match broadcast_target {
        Some(BroadcastTargetState::Armed) => ui.warning,
        Some(BroadcastTargetState::Active) => ui.error,
        None if read_only => ui.info,
        None if zoomed => with_alpha(ui.palette_accent, 0.9),
        None => with_alpha(ui.cursor, 0.5),
    }
}

/// The visual bell's glow over the whole window at `intensity` (0 to 1):
/// the terminal's foreground, so it brightens dark themes and darkens light
/// ones.
fn bell_flash_color(ui: &UiPalette, intensity: f32) -> [f32; 4] {
    with_alpha(ui.foreground, intensity * 0.15)
}

#[cfg(test)]
mod chrome_color_tests {
    use super::*;

    #[test]
    fn pane_outlines_and_the_bell_take_the_themes_colors() {
        let theme = Theme::load_builtin("solarized-light").unwrap();
        let ui = UiPalette::from_theme_with_contrast(&theme, false);
        let outline =
            |broadcast, read_only, zoomed| pane_outline_color(&ui, broadcast, read_only, zoomed);
        // Broadcast outranks the read-only lock, which outranks zoom.
        assert_eq!(
            outline(Some(BroadcastTargetState::Armed), true, true),
            ui.warning
        );
        assert_eq!(
            outline(Some(BroadcastTargetState::Active), true, true),
            ui.error
        );
        assert_eq!(outline(None, true, true), ui.info);
        assert_eq!(
            outline(None, false, true),
            with_alpha(ui.palette_accent, 0.9)
        );
        assert_eq!(outline(None, false, false), with_alpha(ui.cursor, 0.5));
        assert_eq!(ui.info, theme.primary.cyan.to_f32_array());

        assert_eq!(
            bell_flash_color(&ui, 1.0),
            with_alpha(theme.ui.foreground.to_f32_array(), 0.15)
        );
    }
}
