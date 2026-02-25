//! GPU rendering pipeline using wgpu
//!
//! This module handles all GPU-accelerated rendering including:
//! - Text glyph rendering with texture atlas
//! - Background rectangle rendering
//! - Cursor rendering
//! - Tab bar rendering

mod glyph_cache;
mod text;
mod rect;
mod image;

pub use glyph_cache::GlyphCache;

use image::ImagePipeline;

use anyhow::Result;
use std::sync::Arc;
use wgpu::*;
use winit::window::Window;
use termeric_core::{Grid, Selection, Color as TermColor, CellAttrs, CellFlags};
use crate::config::theme::Theme;
use crate::ui::settings::{SettingsPanelInfo, SettingsPanelLayout};
use crate::terminal::search::{Search, SearchBarInfo};

use text::TextPipeline;
use rect::RectPipeline;

use crate::tabs::TabInfo;
use crate::ui::command_palette::CommandDef;
use crate::panes::{PaneRect, Divider};

/// Render information for a single pane in a multi-pane layout
pub struct PaneRenderInfo<'a> {
    /// The grid to render
    pub grid: &'a Grid,
    /// Selection in this pane (if any)
    pub selection: Option<&'a Selection>,
    /// Pane layout rectangle (in pixels)
    pub rect: PaneRect,
    /// Whether this pane is focused
    pub is_focused: bool,
    /// Cursor position for this pane (relative to pane)
    pub cursor_pos: (f32, f32),
}

/// Command bar display information
#[derive(Debug, Clone)]
pub struct CommandBarInfo {
    /// The current command input text
    pub input: String,
    /// Cursor position in the input
    pub cursor_pos: usize,
    /// Message to display (success or error)
    pub message: Option<String>,
    /// Whether the message is an error
    pub is_error: bool,
    /// Autocomplete suggestions to display
    pub suggestions: Vec<CommandDef>,
    /// Index of selected suggestion
    pub selected_suggestion: usize,
    /// Current input phase
    pub phase: crate::ui::command_palette::CommandInputPhase,
    /// Scroll offset for message display (in lines)
    pub message_scroll_offset: usize,
}

#[derive(Clone)]
struct UiPalette {
    background: wgpu::Color,
    foreground: [f32; 4],
    selection: [f32; 4],
    selection_flash: [f32; 4],
    cursor: [f32; 4],
    tab_bar_bg: [f32; 4],
    tab_active: [f32; 4],
    tab_inactive: [f32; 4],
    tab_activity: [f32; 4],
    tab_border: [f32; 4],
    tab_text_active: [f32; 4],
    tab_text_inactive: [f32; 4],
    command_bg: [f32; 4],
    command_border: [f32; 4],
    command_text: [f32; 4],
    command_error: [f32; 4],
    command_success: [f32; 4],
    command_hint: [f32; 4],
    prompt_selecting: [f32; 4],
    prompt_args: [f32; 4],
    prompt_history: [f32; 4],
    dropdown_bg: [f32; 4],
    dropdown_border: [f32; 4],
    dropdown_shadow: [f32; 4],
    // Settings panel colors
    settings_bg: [f32; 4],
    settings_sidebar_bg: [f32; 4],
    settings_header: [f32; 4],
    settings_text: [f32; 4],
    settings_text_dim: [f32; 4],
    settings_selected: [f32; 4],
    settings_value: [f32; 4],
    settings_accent: [f32; 4],
    settings_separator: [f32; 4],
    scrim: [f32; 4],
    // Search colors
    search_bg: [f32; 4],
    search_text: [f32; 4],
    search_match: [f32; 4],
    search_match_current: [f32; 4],
    search_indicator: [f32; 4],
    search_error: [f32; 4],
    // URL/hyperlink color
    url: [f32; 4],
}

impl UiPalette {
    fn from_theme(theme: &Theme) -> Self {
        let bg = theme.ui.background;
        let fg = theme.ui.foreground;
        let selection = theme.ui.selection;
        let cursor = theme.ui.cursor;
        let tab_active = theme.ui.tab_active;
        let tab_inactive = theme.ui.tab_inactive;
        let tab_border = theme.ui.border;

        let tab_text_active = contrast_text(tab_active.to_f32_array());
        let tab_text_inactive = adjust_alpha(fg.to_f32_array(), 0.9);
        let command_bg = shade(bg.to_f32_array(), 0.9);
        let command_text = fg.to_f32_array();

        Self {
            background: bg.to_wgpu(),
            foreground: fg.to_f32_array(),
            selection: selection.to_f32_array(),
            selection_flash: adjust_alpha(fg.to_f32_array(), 0.85),
            cursor: cursor.to_f32_array(),
            tab_bar_bg: shade(tab_inactive.to_f32_array(), 0.85),
            tab_active: tab_active.to_f32_array(),
            tab_inactive: tab_inactive.to_f32_array(),
            tab_activity: theme.primary.yellow.to_f32_array(),
            tab_border: tab_border.to_f32_array(),
            tab_text_active,
            tab_text_inactive,
            command_bg,
            command_border: tab_border.to_f32_array(),
            command_text,
            command_error: theme.primary.red.to_f32_array(),
            command_success: theme.primary.green.to_f32_array(),
            command_hint: adjust_alpha(fg.to_f32_array(), 0.6),
            prompt_selecting: cursor.to_f32_array(),
            prompt_args: theme.primary.yellow.to_f32_array(),
            prompt_history: theme.primary.blue.to_f32_array(),
            dropdown_bg: shade(bg.to_f32_array(), 0.75),
            dropdown_border: tab_border.to_f32_array(),
            dropdown_shadow: shade(bg.to_f32_array(), 0.0),
            // Settings panel
            settings_bg: adjust_alpha(shade(bg.to_f32_array(), 0.85), 0.97),
            settings_sidebar_bg: adjust_alpha(shade(bg.to_f32_array(), 0.7), 0.97),
            settings_header: fg.to_f32_array(),
            settings_text: fg.to_f32_array(),
            settings_text_dim: adjust_alpha(fg.to_f32_array(), 0.55),
            settings_selected: adjust_alpha(cursor.to_f32_array(), 0.25),
            settings_value: cursor.to_f32_array(),
            settings_accent: tab_active.to_f32_array(),
            settings_separator: adjust_alpha(tab_border.to_f32_array(), 0.4),
            // Warm scrim derived from theme background
            scrim: adjust_alpha(shade(bg.to_f32_array(), 0.3), 0.30),
            // Search colors - warm gold tones per style guide
            search_bg: shade(bg.to_f32_array(), 0.85),
            search_text: fg.to_f32_array(),
            search_match: adjust_alpha(theme.primary.yellow.to_f32_array(), 0.35),
            search_match_current: adjust_alpha(theme.primary.yellow.to_f32_array(), 0.65),
            search_indicator: theme.primary.green.to_f32_array(),
            search_error: theme.primary.red.to_f32_array(),
            // URL/hyperlink color from theme
            url: theme.ui.url.to_f32_array(),
        }
    }
}

fn shade(color: [f32; 4], factor: f32) -> [f32; 4] {
    [
        (color[0] * factor).clamp(0.0, 1.0),
        (color[1] * factor).clamp(0.0, 1.0),
        (color[2] * factor).clamp(0.0, 1.0),
        color[3],
    ]
}

fn adjust_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] = alpha;
    color
}

fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

fn apply_brightness(color: [f32; 4], brightness: f32) -> [f32; 4] {
    [
        (color[0] * brightness).clamp(0.0, 1.0),
        (color[1] * brightness).clamp(0.0, 1.0),
        (color[2] * brightness).clamp(0.0, 1.0),
        color[3],
    ]
}

fn contrast_text(bg: [f32; 4]) -> [f32; 4] {
    let luminance = 0.2126 * bg[0] + 0.7152 * bg[1] + 0.0722 * bg[2];
    if luminance > 0.6 {
        [0.12, 0.09, 0.07, 1.0]
    } else {
        [0.95, 0.92, 0.88, 1.0]
    }
}

/// Main renderer that manages all GPU resources
pub struct Renderer {
    #[allow(dead_code)]
    window: Arc<Window>,
    surface: Surface<'static>,
    device: Device,
    queue: Queue,
    config: SurfaceConfiguration,
    size: (u32, u32),
    // Rendering pipelines
    text_pipeline: TextPipeline,
    rect_pipeline: RectPipeline,
    image_pipeline: ImagePipeline,
    // Glyph cache
    glyph_cache: GlyphCache,
    // Cell dimensions
    cell_width: f32,
    cell_height: f32,
    // Padding around terminal content
    padding: f32,
    padding_logical: f32,
    scale_factor: f32,
    font_size: f32,
    // Theme-driven UI palette
    ui: UiPalette,
    // Window opacity (0.0 - 1.0)
    opacity: f32,
}

impl Renderer {
    /// Create a new renderer for the given window
    pub fn new(window: Arc<Window>, font_size: f32, font_family: &str, padding: f32, scale_factor: f32, theme: &Theme, ligatures: bool) -> Result<Self> {
        let size = window.inner_size();

        // Create wgpu instance
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::all(),
            ..Default::default()
        });

        // Create surface - window is Arc so it lives long enough
        let surface = instance.create_surface(window.clone())?;

        // Request adapter
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| anyhow::anyhow!("Failed to find suitable GPU adapter"))?;

        log::info!("Using GPU: {:?}", adapter.get_info().name);

        // Request device
        let (device, queue) = pollster::block_on(adapter.request_device(
            &DeviceDescriptor {
                required_features: Features::empty(),
                required_limits: Limits::default(),
                memory_hints: MemoryHints::default(),
                label: Some("Termeric Device"),
            },
            None,
        ))?;

        // Configure surface
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode: PresentMode::Fifo,
            alpha_mode: if surface_caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
                wgpu::CompositeAlphaMode::PreMultiplied
            } else {
                surface_caps.alpha_modes[0]
            },
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Create glyph cache
        let scaled_font_size = (font_size * scale_factor).max(1.0);
        let glyph_cache = GlyphCache::new(&device, &queue, scaled_font_size, font_family, ligatures)?;
        let (cell_width, cell_height) = glyph_cache.cell_size();

        // Create pipelines
        let text_pipeline = TextPipeline::new(&device, surface_format, &glyph_cache)?;
        let rect_pipeline = RectPipeline::new(&device, surface_format)?;
        let image_pipeline = ImagePipeline::new(&device, surface_format)?;

        let padding_physical = (padding * scale_factor).round().max(0.0);

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            size: (size.width, size.height),
            text_pipeline,
            rect_pipeline,
            image_pipeline,
            glyph_cache,
            cell_width,
            cell_height,
            padding: padding_physical,
            padding_logical: padding,
            scale_factor,
            font_size,
            ui: UiPalette::from_theme(theme),
            opacity: 1.0,
        })
    }

    /// Resize the renderer
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.size = (width, height);
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);

            // Update uniforms
            self.text_pipeline.resize(&self.queue, width, height);
            self.rect_pipeline.resize(&self.queue, width, height);
            self.image_pipeline.resize(&self.queue, width, height);
        }
    }

    /// Get the cell dimensions
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Calculate terminal dimensions from window size (accounting for tab bar)
    pub fn calculate_grid_size(&self, width: u32, height: u32) -> (usize, usize) {
        let tab_bar_height = self.tab_bar_height();
        let available_width = (width as f32 - self.padding * 2.0).max(0.0);
        let cols = (available_width / self.cell_width).floor() as usize;
        // Subtract tab bar height from available height
        let available_height = (height as f32 - tab_bar_height - self.padding * 2.0).max(0.0);
        let rows = (available_height / self.cell_height).floor() as usize;
        (cols.max(1), rows.max(1))
    }

    /// Get the tab bar height
    pub fn tab_bar_height(&self) -> f32 {
        (self.cell_height * 1.1).round().max(18.0) + 10.0
    }

    /// Get current font size
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Get current font family
    pub fn font_family(&self) -> &str {
        self.glyph_cache.font_family()
    }

    /// Change font size
    pub fn set_font_size(&mut self, font_size: f32) {
        self.font_size = font_size;
        let scaled = (font_size * self.scale_factor).max(1.0);
        self.glyph_cache.set_font_size(scaled, &self.queue);
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
    }

    /// Change font family
    pub fn set_font_family(&mut self, font_family: String) {
        self.glyph_cache.set_font_family(font_family, &self.queue);
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
    }

    /// Get all installed monospace font families
    pub fn get_monospace_fonts(&self) -> Vec<String> {
        self.glyph_cache.get_monospace_fonts()
    }

    /// Get current padding
    pub fn padding(&self) -> f32 {
        self.padding
    }

    /// Get current window size in physical pixels
    pub fn window_size(&self) -> (u32, u32) {
        self.size
    }

    /// Change padding
    pub fn set_padding(&mut self, padding: f32) {
        self.padding_logical = padding.max(0.0);
        self.padding = (self.padding_logical * self.scale_factor).round().max(0.0);
    }

    /// Update scale factor for HiDPI displays
    pub fn set_scale_factor(&mut self, scale_factor: f32) {
        self.scale_factor = scale_factor.max(0.1);
        let scaled = (self.font_size * self.scale_factor).max(1.0);
        self.glyph_cache.set_font_size(scaled, &self.queue);
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.padding = (self.padding_logical * self.scale_factor).round().max(0.0);
    }

    /// Set the active theme for UI rendering
    pub fn set_theme(&mut self, theme: &Theme) {
        self.ui = UiPalette::from_theme(theme);
    }

    /// Set window opacity (0.0 = fully transparent, 1.0 = fully opaque)
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.1, 1.0);
    }

    /// Add dividers between panes to the render data
    /// This should be called during build_render_data when panes are present
    pub fn add_pane_dividers(&mut self, dividers: &[Divider], tab_bar_height: f32) {
        let divider_color = self.ui.tab_border;
        let divider_thickness = 2.0;

        for divider in dividers {
            if divider.vertical {
                // Vertical divider (between left/right panes)
                self.rect_pipeline.push_rect(
                    divider.position - divider_thickness / 2.0,
                    tab_bar_height + divider.start,
                    divider_thickness,
                    divider.end - divider.start,
                    divider_color,
                );
            } else {
                // Horizontal divider (between top/bottom panes)
                self.rect_pipeline.push_rect(
                    divider.start,
                    tab_bar_height + divider.position - divider_thickness / 2.0,
                    divider.end - divider.start,
                    divider_thickness,
                    divider_color,
                );
            }
        }
    }

    fn command_bar_base_height(&self) -> f32 {
        (self.cell_height + 16.0).round().max(28.0)
    }

    /// Render the terminal grid with tab bar
    /// cursor_opacity: 0.0 = invisible, 1.0 = fully visible (for smooth blink animation)
    /// cursor_pos: animated cursor position (col, row) as floats for smooth movement
    /// copy_flash: 0.0 = no flash, 1.0 = full flash (for copy confirmation)
    /// focus_dim: 0.0 = fully bright (focused), higher = dimmed (unfocused)
    /// command_bar: optional command bar info when in command mode
    /// search_bar: optional search bar info when searching
    /// search: search state for match highlighting
    /// images: optional list of image placements and their data for Kitty graphics
    pub fn render(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo], copy_flash: f32, focus_dim: f32, command_bar: Option<&CommandBarInfo>, settings_panel: Option<&SettingsPanelInfo>, search_bar: Option<&SearchBarInfo>, search: &Search, images: Option<&[(&termeric_core::ImagePlacement, &termeric_core::StoredImage)]>, session_dropdown: Option<&SessionDropdownInfo>, session_panel: Option<&crate::session::panel::SessionPanelInfo>) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        // Build render data from grid and tabs (inserts layer boundaries)
        self.build_render_data(grid, selection, cursor_opacity, cursor_pos, tabs, copy_flash, focus_dim, command_bar, settings_panel, search_bar, search, session_dropdown, session_panel);

        // Build image render data if any
        self.build_image_data(images);

        // Upload vertex data
        self.rect_pipeline.upload(&self.device, &self.queue);
        self.text_pipeline.upload(&self.device, &self.queue, &mut self.glyph_cache);
        self.image_pipeline.upload(&self.device, &self.queue);

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
            let layer_count = self.rect_pipeline.layer_count().max(self.text_pipeline.layer_count());

            for layer in 0..layer_count {
                // Draw rectangles for this layer (backgrounds, panels, etc.)
                self.rect_pipeline.draw_layer(&mut render_pass, layer);

                // Draw images only on the base layer (layer 0) — images are terminal content
                if layer == 0 {
                    self.image_pipeline.draw(&mut render_pass);
                }

                // Draw text for this layer
                self.text_pipeline.draw_layer(&mut render_pass, &self.glyph_cache, layer);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Build image render data from placements
    fn build_image_data(&mut self, images: Option<&[(&termeric_core::ImagePlacement, &termeric_core::StoredImage)]>) {
        self.image_pipeline.clear();

        let Some(images) = images else { return };

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let padding = self.padding;
        let tab_bar_height = self.tab_bar_height();

        for (placement, image) in images {
            // Ensure image is uploaded to GPU
            self.image_pipeline.upload_image(&self.device, &self.queue, image);

            // Calculate position
            let x = padding + (placement.col as f32) * cell_width + placement.x_offset as f32;
            let y = tab_bar_height + padding + (placement.row as f32) * cell_height + placement.y_offset as f32;

            // Calculate size (use placement size or image natural size)
            let width = placement.width
                .map(|w| w as f32 * cell_width)
                .unwrap_or(image.width as f32);
            let height = placement.height
                .map(|h| h as f32 * cell_height)
                .unwrap_or(image.height as f32);

            self.image_pipeline.push_image(image.id, x, y, width, height);
        }
    }

    /// Build render data from the grid and tabs
    fn build_render_data(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo], copy_flash: f32, focus_dim: f32, command_bar: Option<&CommandBarInfo>, settings_panel: Option<&SettingsPanelInfo>, search_bar: Option<&SearchBarInfo>, search: &Search, session_dropdown: Option<&SessionDropdownInfo>, session_panel: Option<&crate::session::panel::SessionPanelInfo>) {
        self.rect_pipeline.clear();
        self.text_pipeline.clear();

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let window_width = self.size.0 as f32;
        let window_height = self.size.1 as f32;
        let padding = self.padding;
        let tab_bar_height = self.tab_bar_height();

        // Calculate command bar height to know where grid ends
        let command_bar_height = if let Some(cmd_bar) = command_bar {
            self.calculate_command_bar_height(cmd_bar, window_width, window_height)
        } else {
            0.0
        };

        // Calculate dropdown height if showing suggestions
        let dropdown_height = if let Some(cmd_bar) = command_bar {
            if !cmd_bar.suggestions.is_empty() {
                self.calculate_dropdown_height(cmd_bar, window_height - command_bar_height)
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Calculate brightness multiplier for focus dimming (1.0 = full brightness, lower = dimmed)
        let brightness = 1.0 - focus_dim;

        // Calculate search bar height if visible
        let search_bar_height = if search_bar.is_some() {
            cell_height + 8.0 * 2.0 // padding * 2
        } else {
            0.0
        };

        // Content area starts below tab bar and search bar (if visible)
        let content_top = tab_bar_height + search_bar_height;

        // ── LAYER 0: Base content (tab bar, grid, cursor, scrollbar) ──
        // Render tab bar first
        self.build_tab_bar(tabs, window_width, tab_bar_height);

        // Selection highlight color - earthy yellow (inner layer) with transparency
        // When copy_flash > 0, brighten the selection to white for a flash effect
        let selection_base = self.ui.selection;
        let selection_flash = self.ui.selection_flash;
        let selection_color = apply_brightness(mix(selection_base, selection_flash, copy_flash), brightness);

        // Get scroll offset to determine if cursor should be shown
        let scroll_offset = grid.scroll_offset();
        let scroll_offset_i32 = i32::try_from(scroll_offset).unwrap_or(i32::MAX);
        let default_bg = TermColor::default_bg();

        // Calculate maximum y position for grid content (don't overlap command bar or dropdown)
        let max_grid_y = window_height - padding - command_bar_height - dropdown_height - 8.0; // 8px gap for dropdown

        // Determine which rows to render based on filter mode
        let filter_mode = search.is_filter_mode() && !search.matching_rows().is_empty();
        let matching_rows = search.matching_rows();

        // Iterate over grid cells (offset by tab bar height and search bar if visible)
        // In filter mode, only show rows that contain matches
        // rows_to_render: Vec of (visual_row_index, grid_row_offset)
        // grid_row_offset: 0+ for visible grid, negative for scrollback
        let rows_to_render: Vec<(usize, i32)> = if filter_mode {
            // In filter mode, render only matching rows (both visible and scrollback)
            // Each matching row is rendered at a visual position starting from 0
            matching_rows.iter()
                .enumerate()
                .map(|(visual_row, &grid_row)| (visual_row, grid_row))
                .collect()
        } else {
            // Normal mode: render all visible rows
            (0..grid.rows()).map(|row| (row, row as i32 - scroll_offset_i32)).collect()
        };

        for (visual_row, grid_row) in rows_to_render {
            // Get the row data - use row_at_offset which handles both visible and scrollback
            let row_data = if filter_mode {
                // In filter mode, use row_at_offset to access any row (visible or scrollback)
                grid.row_at_offset(grid_row)
            } else {
                // In normal mode, use cell_scrolled access pattern
                // For this we need to handle row-by-row below
                None
            };

            for col in 0..grid.cols() {
                // Get cell based on mode
                let cell = if filter_mode {
                    // Access cell from the row_data we fetched
                    row_data.and_then(|r| r.get(col))
                } else {
                    grid.cell_scrolled(visual_row, col)
                };
                let Some(cell) = cell else { continue };

                let x = padding + col as f32 * cell_width;
                let y = content_top + padding + visual_row as f32 * cell_height;

                // Skip rendering if this row would overlap with command bar
                if y + cell_height > max_grid_y {
                    break;
                }

                // Check if cell is selected (use grid_row for selection check)
                let is_selected = selection
                    .map(|s| s.contains(grid_row, col))
                    .unwrap_or(false);

                // Check if cell is part of a search match
                // The row index in search is relative to the grid: negative for scrollback, 0+ for visible
                let is_search_match = search.is_match(grid_row, col);
                let is_current_match = search.is_current_match(grid_row, col);

                // Draw background if not default or if selected/matched
                let bg = cell.bg;
                if is_selected {
                    self.rect_pipeline.push_rect(
                        x, y,
                        cell_width, cell_height,
                        selection_color,
                    );
                } else if is_current_match {
                    // Current search match - brighter gold
                    self.rect_pipeline.push_rect(
                        x, y,
                        cell_width, cell_height,
                        apply_brightness(self.ui.search_match_current, brightness),
                    );
                } else if is_search_match {
                    // Other search matches - subtle gold
                    self.rect_pipeline.push_rect(
                        x, y,
                        cell_width, cell_height,
                        apply_brightness(self.ui.search_match, brightness),
                    );
                } else if bg != default_bg {
                    self.rect_pipeline.push_rect(
                        x, y,
                        cell_width, cell_height,
                        [
                            bg.r as f32 / 255.0 * brightness,
                            bg.g as f32 / 255.0 * brightness,
                            bg.b as f32 / 255.0 * brightness,
                            1.0
                        ],
                    );
                }

                // Draw character with focus dimming applied
                if let Some(c) = cell.content.char() {
                    if !c.is_whitespace() {
                        let fg = cell.fg;
                        self.text_pipeline.push_char(
                            c,
                            x, y,
                            [
                                fg.r as f32 / 255.0 * brightness,
                                fg.g as f32 / 255.0 * brightness,
                                fg.b as f32 / 255.0 * brightness,
                                1.0
                            ],
                        );
                    }
                }

                // Draw underline if any underline style is active
                if cell.attrs.has_underline() {
                    // Get underline color (use custom color or fg)
                    let underline_color = cell.underline_color.unwrap_or(cell.fg);
                    let ul_color = apply_brightness(
                        [
                            underline_color.r as f32 / 255.0,
                            underline_color.g as f32 / 255.0,
                            underline_color.b as f32 / 255.0,
                            1.0
                        ],
                        brightness
                    );

                    // Position underline at bottom of cell
                    let underline_y = y + cell_height - 2.0;
                    let underline_thickness = 1.0;

                    if cell.attrs.contains(CellAttrs::CURLY_UNDERLINE) {
                        // Curly/wavy underline - draw as a series of small arcs
                        // For simplicity, we approximate with a thicker line
                        // A proper implementation would use a shader
                        self.draw_wavy_underline(x, underline_y, cell_width, ul_color);
                    } else if cell.attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
                        // Double underline - two lines
                        self.rect_pipeline.push_rect(x, underline_y - 2.0, cell_width, underline_thickness, ul_color);
                        self.rect_pipeline.push_rect(x, underline_y, cell_width, underline_thickness, ul_color);
                    } else if cell.attrs.contains(CellAttrs::DOTTED_UNDERLINE) {
                        // Dotted underline - series of small dots
                        self.draw_dotted_underline(x, underline_y, cell_width, ul_color);
                    } else if cell.attrs.contains(CellAttrs::DASHED_UNDERLINE) {
                        // Dashed underline - series of dashes
                        self.draw_dashed_underline(x, underline_y, cell_width, ul_color);
                    } else {
                        // Single underline (default)
                        self.rect_pipeline.push_rect(x, underline_y, cell_width, underline_thickness, ul_color);
                    }
                }

                // Draw hyperlink underline (only if no other underline is already drawn)
                if cell.flags.contains(CellFlags::HYPERLINK) && !cell.attrs.has_underline() {
                    // Use URL color from theme for hyperlinks
                    let link_color = apply_brightness(self.ui.url, brightness);
                    let underline_y = y + cell_height - 2.0;
                    // Draw a dotted underline for hyperlinks
                    self.draw_dotted_underline(x, underline_y, cell_width, link_color);
                }
            }
        }

        // Draw cursor (only when not scrolled back into history and not in filter mode)
        if cursor_opacity > 0.0 && scroll_offset == 0 && !filter_mode {
            let cursor = grid.cursor();
            if cursor.visible {
                // Use animated cursor position for smooth movement
                let x = padding + cursor_pos.0 * cell_width;
                let y = content_top + padding + cursor_pos.1 * cell_height;

                // Only draw cursor if it won't overlap with command bar
                if y + cell_height <= max_grid_y {
                    // Bright orange cursor (turmeric core) with animated opacity and focus dimming
                    let cursor_color = apply_brightness(self.ui.cursor, brightness);
                    self.rect_pipeline.push_rounded_rect(
                        x, y,
                        cell_width, cell_height,
                        [cursor_color[0], cursor_color[1], cursor_color[2], cursor_opacity],
                        2.0,
                    );
                }
            }
        }

        // Draw scrollbar (only when there is scrollback content)
        let scrollback_len = grid.scrollback_len();
        if scrollback_len > 0 {
            let visible_rows = grid.rows();
            let total_rows = scrollback_len + visible_rows;

            // Scrollbar geometry
            let scrollbar_width = 6.0_f32;
            let scrollbar_margin = 2.0_f32;
            let scrollbar_x = window_width - scrollbar_width - scrollbar_margin;
            let scrollbar_top = content_top + padding;
            let scrollbar_bottom = window_height - padding - command_bar_height;
            let scrollbar_height = scrollbar_bottom - scrollbar_top;

            if scrollbar_height > 20.0 {
                // Draw track (subtle background)
                let track_color = [1.0_f32, 1.0, 1.0, 0.05];
                self.rect_pipeline.push_rounded_rect(
                    scrollbar_x, scrollbar_top,
                    scrollbar_width, scrollbar_height,
                    track_color,
                    3.0,
                );

                // Calculate thumb size and position
                let thumb_ratio = (visible_rows as f32) / (total_rows as f32);
                let thumb_height = (scrollbar_height * thumb_ratio).max(20.0).min(scrollbar_height);

                // scroll_offset=0 means at bottom (current), max means scrolled to top
                let max_offset = scrollback_len;
                let scroll_fraction = if max_offset > 0 {
                    scroll_offset as f32 / max_offset as f32
                } else {
                    0.0
                };

                // When scroll_offset is 0 (at bottom), thumb should be at the bottom
                // When scroll_offset is max (at top), thumb should be at the top
                let thumb_travel = scrollbar_height - thumb_height;
                let thumb_y = scrollbar_top + thumb_travel * (1.0 - scroll_fraction);

                // Draw thumb
                let thumb_color = [1.0_f32, 1.0, 1.0, 0.25];
                self.rect_pipeline.push_rounded_rect(
                    scrollbar_x, thumb_y,
                    scrollbar_width, thumb_height,
                    thumb_color,
                    3.0,
                );
            }
        }

        // ── LAYER 1: Search bar (overlays grid content) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(search_info) = search_bar {
            self.build_search_bar(search_info, window_width, tab_bar_height);
        }

        // ── LAYER 2: Command bar + autocomplete dropdown (overlays grid and search) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(cmd_bar) = command_bar {
            self.build_command_bar(cmd_bar, window_width, window_height, command_bar_height);
        }

        // ── LAYER 3: Settings panel + settings dropdown (overlays everything below) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(sp) = settings_panel {
            if sp.animation > 0.001 {
                self.build_settings_panel(sp, window_width, window_height);
            }
        }

        // ── LAYER 4: Settings dropdown overlay (on top of settings panel) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(sp) = settings_panel {
            if sp.animation > 0.001 && sp.dropdown_open {
                self.build_dropdown_overlay(sp, window_width, window_height);
            }
        }

        // ── LAYER 5: Session dropdown (on top of everything) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(sd) = session_dropdown {
            self.build_session_dropdown(sd, window_width);
        }

        // ── LAYER 6: Session management panel (topmost overlay) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(sp) = session_panel {
            if sp.animation > 0.001 {
                self.build_session_management_panel(sp, window_width, window_height);
            }
        }
    }

    /// Build tab bar render data
    fn build_tab_bar(&mut self, tabs: &[TabInfo], window_width: f32, tab_bar_height: f32) {
        let tab_bar_bg = self.ui.tab_bar_bg;
        self.rect_pipeline.push_rect(0.0, 0.0, window_width, tab_bar_height, tab_bar_bg);

        let active_tab_bg = self.ui.tab_active;
        let inactive_tab_bg = self.ui.tab_inactive;
        let activity_indicator = self.ui.tab_activity;
        let active_text_color = self.ui.tab_text_active;
        let inactive_text_color = self.ui.tab_text_inactive;

        // Calculate tab width - minimum 80px, maximum 200px, or divide space evenly
        // Reserve space for the + button, session dropdown, and gear icon on the right
        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let add_button_width = tab_bar_height;
        let available_tab_width = window_width - gear_button_width - session_button_width - add_button_width;
        let tab_count = tabs.len().max(1);
        let full_tab_width = ((available_tab_width / tab_count as f32).min(200.0)).max(80.0);
        let tab_padding = 8.0;
        let tab_height = tab_bar_height - 4.0; // Leave some margin
        let show_close_buttons = true;
        // Reserve space for close button inside tab
        let close_btn_size = self.cell_height;
        let close_btn_margin = 4.0;

        // Track cumulative x position (accounting for animated widths)
        let mut current_x = 0.0;

        for (i, tab) in tabs.iter().enumerate() {
            // Calculate animation multiplier based on slide-in and close animations
            let anim_multiplier = if tab.close_progress > 0.0 {
                // Closing: shrink from full to 0 with ease-in curve (accelerate)
                let t = tab.close_progress;
                let eased = t * t; // ease-in: starts slow, speeds up
                1.0 - eased
            } else {
                // Opening: grow from 0 to full with ease-out curve (decelerate)
                let t = tab.slide_progress;
                1.0 - (1.0 - t) * (1.0 - t) // ease-out
            };

            // Animated tab width
            let tab_width = full_tab_width * anim_multiplier;

            let x = current_x;
            let y = 2.0; // Small top margin

            // Only render if the tab has some visible width
            if tab_width > 2.0 {
                // Tab background with animated alpha for fade effect
                let base_bg_color = if tab.is_active {
                    active_tab_bg
                } else if tab.has_activity {
                    activity_indicator
                } else {
                    inactive_tab_bg
                };

                // Fade the background based on animation progress
                let bg_color = [
                    base_bg_color[0],
                    base_bg_color[1],
                    base_bg_color[2],
                    base_bg_color[3] * anim_multiplier,
                ];

                self.rect_pipeline.push_rounded_rect(x + 1.0, y, tab_width - 2.0, tab_height, bg_color, 4.0);

                // Tab text - only render when animation progress is significant
                if anim_multiplier > 0.3 {
                    let text_alpha = ((anim_multiplier - 0.3) / 0.7).min(1.0); // Fade text

                    let base_text_color = if tab.is_active {
                        active_text_color
                    } else {
                        inactive_text_color
                    };

                    let text_color = [
                        base_text_color[0],
                        base_text_color[1],
                        base_text_color[2],
                        base_text_color[3] * text_alpha,
                    ];

                    // Render tab title (truncate if needed)
                    // Reserve space for close button when visible
                    let right_reserve = if show_close_buttons {
                        close_btn_size + close_btn_margin * 2.0
                    } else {
                        tab_padding
                    };
                    let title = if tab.title.len() > 15 {
                        format!("{}...", &tab.title[..12])
                    } else {
                        tab.title.clone()
                    };

                    // Add tab number prefix
                    let label = format!("{}: {}", i + 1, title);

                    // Render each character of the label
                    let text_y = y + (tab_height - self.cell_height) / 2.0; // Center vertically
                    let mut text_x = x + tab_padding;

                    for c in label.chars() {
                        if text_x + self.cell_width > x + tab_width - right_reserve {
                            break; // Don't overflow into close button area
                        }
                        if !c.is_whitespace() {
                            self.text_pipeline.push_char(c, text_x, text_y, text_color);
                        }
                        text_x += self.cell_width * 0.85;
                    }

                    // ── Close (×) button ──
                    if show_close_buttons {
                        let close_x = x + tab_width - close_btn_size - close_btn_margin - 2.0;
                        let close_y = y + (tab_height - close_btn_size) / 2.0;

                        // Subtle rounded background for close button
                        let close_bg = adjust_alpha(inactive_tab_bg, 0.4 * text_alpha);
                        self.rect_pipeline.push_rounded_rect(
                            close_x, close_y,
                            close_btn_size, close_btn_size,
                            close_bg, 3.0,
                        );

                        // × character centered in the button
                        let close_char_x = close_x + (close_btn_size - self.cell_width) / 2.0;
                        let close_char_y = close_y + (close_btn_size - self.cell_height) / 2.0;
                        let close_text_color = [
                            text_color[0],
                            text_color[1],
                            text_color[2],
                            text_color[3] * 0.7,
                        ];
                        self.text_pipeline.push_char('×', close_char_x, close_char_y, close_text_color);
                    }
                }
            }

            current_x += tab_width;
        }

        let border_color = adjust_alpha(self.ui.tab_border, 0.7);
        self.rect_pipeline.push_rect(0.0, tab_bar_height - 1.0, window_width, 1.0, border_color);

        // ── Add (+) tab button ──
        let add_size = tab_bar_height;
        let add_x = window_width - gear_button_width - session_button_width - add_size;
        let add_y = 0.0;
        let add_bg = adjust_alpha(self.ui.tab_inactive, 0.35);
        self.rect_pipeline.push_rounded_rect(
            add_x + 2.0, add_y + 2.0,
            add_size - 4.0, tab_bar_height - 6.0,
            add_bg, 4.0,
        );
        let add_char_x = add_x + (add_size - self.cell_width) / 2.0;
        let add_char_y = add_y + (tab_bar_height - self.cell_height) / 2.0;
        self.text_pipeline.push_char('+', add_char_x, add_char_y, self.ui.tab_text_inactive);

        // ── Session dropdown (▾) button ──
        let session_size = tab_bar_height;
        let session_x = window_width - gear_button_width - session_size;
        let session_y = 0.0;
        let session_bg = adjust_alpha(self.ui.tab_inactive, 0.35);
        self.rect_pipeline.push_rounded_rect(
            session_x + 2.0, session_y + 2.0,
            session_size - 4.0, tab_bar_height - 6.0,
            session_bg, 4.0,
        );
        let session_char_x = session_x + (session_size - self.cell_width) / 2.0;
        let session_char_y = session_y + (tab_bar_height - self.cell_height) / 2.0;
        self.text_pipeline.push_char('▾', session_char_x, session_char_y, self.ui.tab_text_inactive);

        // ── Gear icon button (right side of tab bar) ──
        let gear_size = tab_bar_height;
        let gear_x = window_width - gear_size;
        let gear_y = 0.0;
        // Subtle background for the gear button
        let gear_bg = adjust_alpha(self.ui.tab_inactive, 0.5);
        self.rect_pipeline.push_rounded_rect(
            gear_x + 2.0, gear_y + 2.0,
            gear_size - 4.0, tab_bar_height - 6.0,
            gear_bg, 4.0,
        );
        // Render the gear icon character centered
        let gear_char_x = gear_x + (gear_size - self.cell_width) / 2.0;
        let gear_char_y = gear_y + (tab_bar_height - self.cell_height) / 2.0;
        self.text_pipeline.push_char('⚙', gear_char_x, gear_char_y, self.ui.tab_text_inactive);
    }

    /// Calculate command bar height based on content
    fn calculate_command_bar_height(&self, info: &CommandBarInfo, window_width: f32, window_height: f32) -> f32 {
        let padding = 8.0;

        if let Some(msg) = &info.message {
            // Count lines in message (considering newlines and wrapping)
            let max_chars_per_line = ((window_width - padding * 2.0) / self.cell_width) as usize;
            let mut line_count = 0;
            let mut current_line_len = 0;

            for c in msg.chars() {
                if c == '\n' {
                    line_count += 1;
                    current_line_len = 0;
                } else {
                    current_line_len += 1;
                    if current_line_len >= max_chars_per_line {
                        line_count += 1;
                        current_line_len = 0;
                    }
                }
            }
            if current_line_len > 0 {
                line_count += 1;
            }

            // Use dynamic height for messages, capped at 60% of window height
            let needed_height = (line_count as f32 * self.cell_height) + padding * 2.0;
            needed_height.min(window_height * 0.6).max(self.command_bar_base_height())
        } else {
            self.command_bar_base_height()
        }
    }

    /// Calculate dropdown height based on number of suggestions
    fn calculate_dropdown_height(&self, info: &CommandBarInfo, available_height: f32) -> f32 {
        const SUGGESTION_HEIGHT: f32 = 28.0;
        const PADDING: f32 = 12.0;
        const SPACING: f32 = 1.0;
        const BORDER_WIDTH: f32 = 1.0;
        let actual_available_height = available_height - self.tab_bar_height() - 8.0;
        let max_suggestions_that_fit = ((actual_available_height - PADDING * 2.0 - BORDER_WIDTH * 2.0) / (SUGGESTION_HEIGHT + SPACING)).floor() as usize;
        let num_suggestions = info.suggestions.len().min(6).min(max_suggestions_that_fit.max(1));

        if num_suggestions == 0 {
            return 0.0;
        }

        num_suggestions as f32 * (SUGGESTION_HEIGHT + SPACING) - SPACING + PADDING * 2.0 + BORDER_WIDTH * 2.0
    }

    /// Build command bar render data
    fn build_command_bar(&mut self, info: &CommandBarInfo, window_width: f32, window_height: f32, bar_height: f32) {
        let padding = self.padding.max(8.0);
        let bar_y = window_height - bar_height;

        let bar_bg = self.ui.command_bg;
        self.rect_pipeline.push_rect(0.0, bar_y, window_width, bar_height, bar_bg);

        let border_color = adjust_alpha(self.ui.command_border, 0.8);
        self.rect_pipeline.push_rect(0.0, bar_y, window_width, 1.0, border_color);

        // Render message and/or input
        let text_y = bar_y + padding;
        let mut text_x = padding;

        // Reserve space for input line if we're typing
        let has_input = !info.input.is_empty();
        let input_line_height = if has_input { self.cell_height + padding } else { 0.0 };

        if let Some(msg) = &info.message {
            // Show message (supports multi-line and scrolling)
            let text_color = if info.is_error {
                self.ui.command_error
            } else {
                self.ui.command_success
            };

            let mut current_x = text_x;
            let max_y = bar_y + bar_height - padding - input_line_height; // Leave room for input if typing
            let mut line_number = 0; // Track which line we're on (accounting for wraps)
            let scroll_offset = info.message_scroll_offset;
            let mut visible_line_y = text_y; // Y position for rendering (after applying scroll)

            for c in msg.chars() {
                if c == '\n' {
                    // Move to next line
                    line_number += 1;
                    current_x = text_x;
                    // Update visible Y if we're past scroll offset
                    if line_number > scroll_offset {
                        visible_line_y += self.cell_height;
                        if visible_line_y + self.cell_height > max_y {
                            break;
                        }
                    }
                    continue;
                }

                // Word wrap if line is too long
                if current_x + self.cell_width > window_width - padding {
                    line_number += 1;
                    current_x = text_x;
                    if line_number > scroll_offset {
                        visible_line_y += self.cell_height;
                        if visible_line_y + self.cell_height > max_y {
                            break;
                        }
                    }
                }

                // Only render if we're past the scroll offset
                if line_number >= scroll_offset {
                    self.text_pipeline.push_char(c, current_x, visible_line_y, text_color);
                }
                current_x += self.cell_width; // Use same spacing as terminal for readability
            }
        }

        // Show command input (always show if there's input, positioned at bottom when message is present)
        if has_input || info.message.is_none() {
            let input_text_y = if info.message.is_some() && has_input {
                // Position at bottom when message is showing
                bar_y + bar_height - self.cell_height - padding
            } else {
                // Center text vertically when no message
                bar_y + (bar_height - self.cell_height) / 2.0
            };

            text_x = padding; // Reset x position

            let (prompt_text, prompt_color) = match info.phase {
                crate::ui::command_palette::CommandInputPhase::SelectingCommand => {
                    ("> ", self.ui.prompt_selecting)
                }
                crate::ui::command_palette::CommandInputPhase::EnteringArguments => {
                    (": ", self.ui.prompt_args)
                }
                crate::ui::command_palette::CommandInputPhase::BrowsingHistory => {
                    ("⏮ ", self.ui.prompt_history)
                }
            };
            let text_color = self.ui.command_text;

            // Draw prompt
            for c in prompt_text.chars() {
                self.text_pipeline.push_char(c, text_x, input_text_y, prompt_color);
                text_x += self.cell_width;
            }
            text_x += self.cell_width * 0.2; // Small space after prompt

            // Draw input text
            let mut char_index = 0;
            for c in info.input.chars() {
                if text_x + self.cell_width > window_width - padding {
                    break;
                }

                // Draw cursor at current position
                if char_index == info.cursor_pos {
                    let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
                    self.rect_pipeline.push_rect(
                        text_x, input_text_y,
                        2.0, self.cell_height,
                        cursor_color,
                    );
                }

                self.text_pipeline.push_char(c, text_x, input_text_y, text_color);
                text_x += self.cell_width;
                char_index += c.len_utf8();
            }

            // Draw cursor at end if at end of input
            if info.cursor_pos >= info.input.len() {
                let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
                self.rect_pipeline.push_rect(
                    text_x, input_text_y,
                    2.0, self.cell_height,
                    cursor_color,
                );
            }

            // Phase-aware hint text on the right (only when not showing message)
            if info.message.is_none() {
                let hint = match info.phase {
                    crate::ui::command_palette::CommandInputPhase::SelectingCommand => {
                        "↑↓ Navigate  ⭾ Complete  Ctrl+↑↓ History  Esc Cancel"
                    }
                    crate::ui::command_palette::CommandInputPhase::EnteringArguments => {
                        "↑↓ Navigate  ⭾ Complete  ⏎ Execute  Esc Cancel"
                    }
                    crate::ui::command_palette::CommandInputPhase::BrowsingHistory => {
                        "Ctrl+↑↓ Browse  ⏎ Select  Esc Cancel"
                    }
                };
                let hint_color = self.ui.command_hint;
                let hint_width = hint.chars().count() as f32 * self.cell_width * 0.6;
                let mut hint_x = window_width - hint_width - padding;

                for c in hint.chars() {
                    self.text_pipeline.push_char(c, hint_x, input_text_y, hint_color);
                    hint_x += self.cell_width * 0.6;
                }
            }
        }

        // Render autocomplete dropdown above command bar
        if !info.suggestions.is_empty() {
            self.build_autocomplete_dropdown(info, bar_y, window_width, self.tab_bar_height());
        }
    }

    /// Build autocomplete dropdown above command bar
    fn build_autocomplete_dropdown(&mut self, info: &CommandBarInfo, command_bar_y: f32, window_width: f32, tab_bar_height: f32) {
        const SUGGESTION_HEIGHT: f32 = 28.0; // Slightly more compact
        const PADDING: f32 = 12.0; // More padding for breathing room
        const SPACING: f32 = 1.0; // Tighter spacing between items
        const MARGIN: f32 = 16.0; // Margin from window edges
        const BORDER_WIDTH: f32 = 1.0;

        // Calculate how many suggestions can fit above the command bar
        let available_height = command_bar_y - tab_bar_height - 8.0; // 8px gap from tab bar
        let max_suggestions_that_fit = ((available_height - PADDING * 2.0 - BORDER_WIDTH * 2.0) / (SUGGESTION_HEIGHT + SPACING)).floor() as usize;
        let num_suggestions = info.suggestions.len().min(6).min(max_suggestions_that_fit.max(1));

        // Dropdown dimensions - centered with margins
        let dropdown_width = window_width - MARGIN * 2.0;
        let dropdown_height = num_suggestions as f32 * (SUGGESTION_HEIGHT + SPACING) - SPACING + PADDING * 2.0 + BORDER_WIDTH * 2.0;
        let dropdown_x = MARGIN;
        let dropdown_y = command_bar_y - dropdown_height - 8.0; // 8px gap above command bar

        // Shadow effect - draw multiple slightly offset darker rects
        let shadow_color = self.ui.dropdown_shadow;
        for i in 1..=3 {
            let offset = i as f32 * 2.0;
            self.rect_pipeline.push_rounded_rect(
                dropdown_x + offset,
                dropdown_y + offset,
                dropdown_width,
                dropdown_height,
                shadow_color,
                8.0,
            );
        }

        // Main dropdown background - solid dark background
        let dropdown_bg = self.ui.dropdown_bg;
        self.rect_pipeline.push_rounded_rect(dropdown_x, dropdown_y, dropdown_width, dropdown_height, dropdown_bg, 8.0);

        let border_color = adjust_alpha(self.ui.dropdown_border, 0.6);
        // Top border
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y, dropdown_width, BORDER_WIDTH, border_color);
        // Bottom border
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y + dropdown_height - BORDER_WIDTH, dropdown_width, BORDER_WIDTH, border_color);
        // Left border
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y, BORDER_WIDTH, dropdown_height, border_color);
        // Right border
        self.rect_pipeline.push_rect(dropdown_x + dropdown_width - BORDER_WIDTH, dropdown_y, BORDER_WIDTH, dropdown_height, border_color);

        // Render each suggestion (only those that fit)
        for (idx, suggestion) in info.suggestions.iter().take(num_suggestions).enumerate() {
            let suggestion_y = dropdown_y + BORDER_WIDTH + PADDING + idx as f32 * (SUGGESTION_HEIGHT + SPACING);
            let is_selected = idx == info.selected_suggestion;

            // Selection highlight with rounded effect
            if is_selected {
                let selection_color = adjust_alpha(self.ui.cursor, 0.2);
                let highlight_margin = 4.0;
                self.rect_pipeline.push_rounded_rect(
                    dropdown_x + highlight_margin,
                    suggestion_y,
                    dropdown_width - highlight_margin * 2.0,
                    SUGGESTION_HEIGHT,
                    selection_color,
                    4.0,
                );

                // Left accent bar for selected item
                let accent_color = self.ui.cursor;
                let accent_width = 3.0;
                self.rect_pipeline.push_rounded_rect(
                    dropdown_x + highlight_margin,
                    suggestion_y,
                    accent_width,
                    SUGGESTION_HEIGHT,
                    accent_color,
                    2.0,
                );
            }

            let text_y = suggestion_y + (SUGGESTION_HEIGHT - self.cell_height) / 2.0;
            let mut text_x = dropdown_x + PADDING + (if is_selected { 10.0 } else { 6.0 });

            // Category icon/prefix
            let category_color = if is_selected {
                self.ui.cursor
            } else {
                adjust_alpha(self.ui.prompt_args, 0.7)
            };

            let category_icon = match suggestion.category {
                "Theme" => "◆",
                "Font" => "A",
                "Config" => "⚙",
                "Help" => "?",
                _ => "▸",
            };

            for c in category_icon.chars() {
                self.text_pipeline.push_char(c, text_x, text_y, category_color);
                text_x += self.cell_width; // Standard character width
            }
            text_x += self.cell_width * 0.5; // Space after icon

            // Command name
            let name_color = if is_selected {
                self.ui.foreground
            } else {
                self.ui.command_text
            };

            for c in suggestion.name.chars() {
                self.text_pipeline.push_char(c, text_x, text_y, name_color);
                text_x += self.cell_width;
            }

            // Alias (if present)
            if let Some(alias) = suggestion.alias {
                text_x += self.cell_width * 0.5;
                let alias_color = adjust_alpha(self.ui.command_text, 0.7);

                self.text_pipeline.push_char('(', text_x, text_y, alias_color);
                text_x += self.cell_width;

                for c in alias.chars() {
                    self.text_pipeline.push_char(c, text_x, text_y, alias_color);
                    text_x += self.cell_width;
                }

                self.text_pipeline.push_char(')', text_x, text_y, alias_color);
                text_x += self.cell_width;
            }

            // Description (right side) - add some spacing
            text_x += self.cell_width * 2.0; // Add spacing before description
            let max_desc_x = dropdown_x + dropdown_width - PADDING - 4.0; // Leave margin on right

            let desc_color = if is_selected {
                adjust_alpha(self.ui.command_text, 0.95)
            } else {
                adjust_alpha(self.ui.command_text, 0.75)
            };

            // Add separator before description
            let separator = "—";
            for c in separator.chars() {
                if text_x + self.cell_width < max_desc_x {
                    self.text_pipeline.push_char(c, text_x, text_y, adjust_alpha(self.ui.command_text, 0.5));
                    text_x += self.cell_width * 0.8;
                }
            }
            text_x += self.cell_width * 0.5; // Small gap after separator

            for c in suggestion.description.chars() {
                if text_x + self.cell_width * 3.0 > max_desc_x {
                    // Add ellipsis if we're running out of space
                    if text_x + self.cell_width * 1.5 <= max_desc_x {
                        self.text_pipeline.push_char('.', text_x, text_y, desc_color);
                        text_x += self.cell_width * 0.4;
                        self.text_pipeline.push_char('.', text_x, text_y, desc_color);
                        text_x += self.cell_width * 0.4;
                        self.text_pipeline.push_char('.', text_x, text_y, desc_color);
                    }
                    break;
                }

                self.text_pipeline.push_char(c, text_x, text_y, desc_color);
                text_x += self.cell_width * 0.95; // Slightly compressed for descriptions
            }
        }
    }

    /// Build settings panel render data
    fn build_settings_panel(&mut self, info: &SettingsPanelInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        // UI character width — use actual cell width from font metrics.
        // This ensures proper character spacing regardless of font.
        let ui_cw = self.cell_width;

        // Panel dimensions — all derived from font metrics so layout adapts
        // Sidebar: padding + icon(3 cells) + longest label("Scrollback"=10) + padding = ~16.5 cells
        let padding_x = ui_cw * 1.5;
        let sidebar_width = padding_x + ui_cw * 3.0 + ui_cw * 10.0 + padding_x;
        // Content: label area + gap + value area + padding
        let min_content_width = ui_cw * 30.0; // needs room for labels + values
        let panel_width = ((sidebar_width + min_content_width).max(window_width * 0.35)).min(window_width * 0.55);
        let panel_x = window_width - panel_width * anim; // slide in from right
        let panel_y = 0.0;
        let panel_h = window_height;

        // Content area is whatever remains after sidebar
        let content_x = panel_x + sidebar_width;
        let content_width = panel_width - sidebar_width;

        let row_height = (self.cell_height * 2.2).max(32.0);
        let padding_y = self.cell_height * 0.5;

        // ── Scrim (warm-tinted dimmed overlay on terminal behind panel) ──
        let scrim = self.ui.scrim;
        self.rect_pipeline.push_rect(
            0.0, 0.0,
            panel_x, window_height,
            [scrim[0], scrim[1], scrim[2], scrim[3] * anim],
        );

        // ── Sidebar background ──
        self.rect_pipeline.push_rect(
            panel_x, panel_y,
            sidebar_width, panel_h,
            self.ui.settings_sidebar_bg,
        );

        // ── Main panel background ──
        self.rect_pipeline.push_rect(
            content_x, panel_y,
            content_width, panel_h,
            self.ui.settings_bg,
        );

        // ── Sidebar border (thin line between sidebar and content) ──
        self.rect_pipeline.push_rect(
            content_x - 1.0, panel_y,
            1.0, panel_h,
            self.ui.settings_separator,
        );

        // ── Panel title in sidebar ──
        let title = "Settings";
        let title_y = panel_y + self.cell_height * 1.5;
        let title_x = panel_x + padding_x;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                title_x + i as f32 * ui_cw,
                title_y,
                self.ui.settings_accent,
            );
        }

        // ── Section buttons in sidebar ──
        let section_start_y = title_y + self.cell_height * 2.5;
        for (idx, (section, is_active)) in info.sections.iter().enumerate() {
            let sy = section_start_y + idx as f32 * row_height;

            // Active section highlight bar
            if *is_active {
                // Left accent bar
                self.rect_pipeline.push_rounded_rect(
                    panel_x, sy,
                    3.0, row_height,
                    self.ui.settings_accent,
                    2.0,
                );
                // Highlight background
                self.rect_pipeline.push_rounded_rect(
                    panel_x + 3.0, sy,
                    sidebar_width - 3.0, row_height,
                    self.ui.settings_selected,
                    4.0,
                );
            }

            // Section label
            let label_x = panel_x + padding_x;
            let text_y = sy + (row_height - self.cell_height) / 2.0;
            let color = if *is_active { self.ui.settings_text } else { self.ui.settings_text_dim };
            for (i, c) in section.label().chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    label_x + i as f32 * ui_cw,
                    text_y,
                    color,
                );
            }
        }

        // ── Keyboard hints at bottom of sidebar ──
        let hints = ["↑↓ Navigate", "←→ Adjust", "Tab Section", "Esc Close"];
        let hint_start_y = window_height - (hints.len() as f32 * self.cell_height * 1.6) - self.cell_height;
        for (idx, hint) in hints.iter().enumerate() {
            let hy = hint_start_y + idx as f32 * self.cell_height * 1.6;
            let hx = panel_x + padding_x;
            for (i, c) in hint.chars().enumerate() {
                if hx + i as f32 * ui_cw * 0.85 < content_x - padding_x {
                    self.text_pipeline.push_char(
                        c,
                        hx + i as f32 * ui_cw * 0.85,
                        hy,
                        self.ui.settings_text_dim,
                    );
                }
            }
        }

        // ── Content area: section header ──
        let header_y = panel_y + self.cell_height * 1.5;
        let header_x = content_x + padding_x;
        let section_title = info.section.label();
        for (i, c) in section_title.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                header_x + i as f32 * ui_cw * 1.1,
                header_y,
                self.ui.settings_header,
            );
        }

        // Separator under header
        let sep_y = header_y + self.cell_height * 1.8;
        self.rect_pipeline.push_rect(
            content_x + padding_x, sep_y,
            content_width - padding_x * 2.0, 1.0,
            self.ui.settings_separator,
        );

        // ── Content area: setting items ──
        let items_start_y = sep_y + padding_y + self.cell_height * 0.5;

        // Dynamic value-area X: positioned after the longest label + a gap,
        // clamped so it never exceeds 65% of content width.
        let max_label_len = info.items.iter()
            .map(|item| item.label.len())
            .max()
            .unwrap_or(0);
        let label_end = padding_x + max_label_len as f32 * ui_cw;
        let min_gap = ui_cw * 2.0;
        let max_value_area = content_width * 0.65;
        let value_area_offset = (label_end + min_gap).min(max_value_area);
        let value_area_x = content_x + value_area_offset;

        // Calculate dropdown occlusion region (if dropdown is open, items below
        // the selected item may be hidden behind the dropdown overlay).
        // Since all text renders on top of all rects, we must skip rendering
        // text for occluded items to avoid text-on-text confusion.
        // (dd_x, dd_top, dd_bottom) — X is the left edge of the dropdown
        let dropdown_occludes: Option<(f32, f32, f32)> = if info.dropdown_open {
            if let Some(sel_item) = info.items.get(info.selected_item) {
                let (options_text, option_count) = match &sel_item.kind {
                    crate::ui::settings::SettingKind::Dropdown { options, .. } => (options.clone(), options.len()),
                    crate::ui::settings::SettingKind::SteppedList { values, .. } => {
                        (values.iter().map(|v| v.to_string()).collect::<Vec<_>>(), values.len())
                    }
                    _ => (vec![], 0),
                };
                if option_count > 0 {
                    // Match the exact dropdown position calculation from build_dropdown_overlay
                    let max_option_len = options_text.iter().map(|s| s.len()).max().unwrap_or(10);
                    let min_dd_width = (max_option_len as f32 + 4.0) * ui_cw;
                    let remaining_space = content_width - value_area_offset;
                    let dd_width = min_dd_width.max(remaining_space).min(content_width - ui_cw * 2.0);

                    // Shift dropdown left if it would exceed panel bounds
                    let dd_value_area_x = content_x + value_area_offset;
                    let dd_x = if dd_value_area_x + dd_width > panel_x + panel_width - ui_cw {
                        (panel_x + panel_width - dd_width - ui_cw).max(content_x + ui_cw)
                    } else {
                        dd_value_area_x
                    };

                    let visible_count = option_count.min(8);
                    let option_h = row_height * 0.85;
                    let dd_top = items_start_y + info.selected_item as f32 * row_height + row_height;
                    let dd_bottom = dd_top + visible_count as f32 * option_h + 8.0;
                    Some((dd_x, dd_top, dd_bottom))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        for (idx, item) in info.items.iter().enumerate() {
            let iy = items_start_y + idx as f32 * row_height;
            let is_selected = idx == info.selected_item;

            // Check if this item's text would be occluded by the dropdown
            let is_occluded = if let Some((_dd_x, dd_top, dd_bottom)) = dropdown_occludes {
                let item_top = iy;
                let item_bottom = iy + row_height;
                // Item is occluded if it overlaps the dropdown region
                item_top < dd_bottom && item_bottom > dd_top
            } else {
                false
            };

            // Selection highlight
            if is_selected {
                self.rect_pipeline.push_rounded_rect(
                    content_x + padding_x * 0.5, iy,
                    content_width - padding_x, row_height,
                    self.ui.settings_selected,
                    4.0,
                );
                // Left accent bar for selected item
                self.rect_pipeline.push_rounded_rect(
                    content_x, iy,
                    3.0, row_height,
                    self.ui.settings_accent,
                    2.0,
                );
            }

            // If occluded by dropdown, still draw labels but skip value text below

            let text_y = iy + (row_height - self.cell_height) / 2.0;

            // Label — clip at dropdown boundary if occluded
            let label_x = content_x + padding_x;
            let label_color = if is_selected { self.ui.settings_text } else { self.ui.settings_text_dim };
            let label_clip_x = if is_occluded {
                if let Some((dd_x, _, _)) = dropdown_occludes { dd_x } else { f32::MAX }
            } else {
                f32::MAX
            };
            for (i, c) in item.label.chars().enumerate() {
                let cx = label_x + i as f32 * ui_cw;
                if cx + ui_cw > label_clip_x {
                    break; // Don't render characters that extend into dropdown
                }
                self.text_pipeline.push_char(c, cx, text_y, label_color);
            }

            // Value (right-aligned area) — skip if occluded by dropdown
            if !is_occluded {
                // value_area_x already computed above from longest label
                let value_color = if is_selected { self.ui.settings_value } else { self.ui.settings_text };
                let max_x = content_x + content_width - padding_x;

                match &item.kind {
                    crate::ui::settings::SettingKind::Numeric { .. } => {
                        // Draw ◀ value ▶ controls for numeric
                        if is_selected {
                            self.text_pipeline.push_char(
                                '◀',
                                value_area_x,
                                text_y,
                                self.ui.settings_accent,
                            );
                        }

                        let val_x = value_area_x + ui_cw * 2.0;
                        for (i, c) in item.value.chars().enumerate() {
                            let cx = val_x + i as f32 * ui_cw;
                            if cx + ui_cw > max_x { break; }
                            self.text_pipeline.push_char(c, cx, text_y, value_color);
                        }

                        if is_selected {
                            let arrow_x = val_x + item.value.len() as f32 * ui_cw + ui_cw;
                            if arrow_x + ui_cw <= max_x {
                                self.text_pipeline.push_char('▶', arrow_x, text_y, self.ui.settings_accent);
                            }
                        }
                    }
                    crate::ui::settings::SettingKind::Dropdown { .. } | crate::ui::settings::SettingKind::SteppedList { .. } => {
                        // Draw value with ▼ chevron (click or Enter to open dropdown)
                        let val_x = value_area_x + ui_cw * 2.0;
                        for (i, c) in item.value.chars().enumerate() {
                            let cx = val_x + i as f32 * ui_cw;
                            if cx + ui_cw > max_x { break; }
                            self.text_pipeline.push_char(c, cx, text_y, value_color);
                        }

                        // Down chevron — only if it fits
                        let chevron_x = val_x + item.value.len() as f32 * ui_cw + ui_cw;
                        if chevron_x + ui_cw <= max_x {
                            let chevron_color = if is_selected { self.ui.settings_accent } else { self.ui.settings_text_dim };
                            self.text_pipeline.push_char('▼', chevron_x, text_y, chevron_color);
                        }
                    }
                    crate::ui::settings::SettingKind::ReadOnly => {
                        // Just display the value, truncated with '.'
                        let val_x = value_area_x + ui_cw * 2.0;
                        for (i, c) in item.value.chars().enumerate() {
                            let cx = val_x + i as f32 * ui_cw;
                            if cx + ui_cw > max_x {
                                self.text_pipeline.push_char('.', cx, text_y, self.ui.settings_text_dim);
                                break;
                            }
                            self.text_pipeline.push_char(
                                c, cx, text_y,
                                self.ui.settings_text_dim,
                            );
                        }
                    }
                }
            }

            // Thin separator between items
            if idx + 1 < info.items.len() {
                self.rect_pipeline.push_rect(
                    content_x + padding_x, iy + row_height - 0.5,
                    content_width - padding_x * 2.0, 0.5,
                    self.ui.settings_separator,
                );
            }
        }

        // ── Footer: Save hint ──
        let footer_y = window_height - self.cell_height * 2.0;
        let footer_text = "Ctrl+S Save  |  Ctrl+R Reset";
        let footer_x = content_x + padding_x;
        // Separator above footer
        self.rect_pipeline.push_rect(
            content_x + padding_x, footer_y - padding_y,
            content_width - padding_x * 2.0, 1.0,
            self.ui.settings_separator,
        );

        // Skip footer text if occluded by dropdown
        let footer_occluded = if let Some((_, _dd_top, dd_bottom)) = dropdown_occludes {
            footer_y < dd_bottom
        } else {
            false
        };
        if !footer_occluded {
            let footer_max_x = content_x + content_width - padding_x;
            for (i, c) in footer_text.chars().enumerate() {
                let cx = footer_x + i as f32 * ui_cw * 0.9;
                if cx + ui_cw > footer_max_x { break; }
                self.text_pipeline.push_char(c, cx, footer_y, self.ui.settings_text_dim);
            }
        }

        // ── Dropdown overlay rendered separately in build_dropdown_overlay ──
    }

    /// Build dropdown overlay render data — rendered last so it appears on top of everything
    fn build_dropdown_overlay(&mut self, info: &SettingsPanelInfo, window_width: f32, _window_height: f32) {
        // ALL coordinates MUST match build_settings_panel exactly
        let ui_cw = self.cell_width;
        let padding_x = ui_cw * 1.5;
        let sidebar_width = padding_x + ui_cw * 3.0 + ui_cw * 10.0 + padding_x;
        let min_content_width = ui_cw * 30.0;
        let panel_width = ((sidebar_width + min_content_width).max(window_width * 0.35)).min(window_width * 0.55);
        let anim = info.animation;
        let panel_x = window_width - panel_width * anim;
        let panel_y = 0.0_f32;

        let content_x = panel_x + sidebar_width;
        let content_width = panel_width - sidebar_width;

        let row_height = (self.cell_height * 2.2).max(32.0);
        let padding_y = self.cell_height * 0.5;

        let header_y = panel_y + self.cell_height * 1.5;
        let sep_y = header_y + self.cell_height * 1.8;
        let items_start_y = sep_y + padding_y + self.cell_height * 0.5;

        if let Some(item) = info.items.get(info.selected_item) {
            let (options_text, option_count): (Vec<String>, usize) = match &item.kind {
                crate::ui::settings::SettingKind::Dropdown { options, .. } => {
                    (options.clone(), options.len())
                }
                crate::ui::settings::SettingKind::SteppedList { values, .. } => {
                    (values.iter().map(|v| v.to_string()).collect(), values.len())
                }
                _ => (vec![], 0),
            };

            if option_count > 0 {
                // Dynamic value-area: match build_settings_panel
                let max_label_len = info.items.iter()
                    .map(|item| item.label.len())
                    .max()
                    .unwrap_or(0);
                let padding_x = ui_cw * 1.5;
                let label_end = padding_x + max_label_len as f32 * ui_cw;
                let min_gap = ui_cw * 2.0;
                let max_value_area = content_width * 0.65;
                let value_area_offset = (label_end + min_gap).min(max_value_area);
                let dd_value_area_x = content_x + value_area_offset;
                let anchor_y = items_start_y + info.selected_item as f32 * row_height + row_height;

                // Calculate dropdown width based on longest option text
                let max_option_len = options_text.iter()
                    .map(|s| s.len())
                    .max()
                    .unwrap_or(10);
                // Minimum width: longest option + padding on both sides
                let min_dd_width = (max_option_len as f32 + 4.0) * ui_cw;
                // Maximum width: remaining space to right edge
                let remaining_space = content_width - value_area_offset;
                // Use larger of minimum needed or remaining space, but cap at content width
                let dd_width = min_dd_width.max(remaining_space).min(content_width - ui_cw * 2.0);

                // Shift dropdown left if it would exceed panel bounds
                let dd_x = if dd_value_area_x + dd_width > panel_x + panel_width - ui_cw {
                    // Shift left to fit within panel
                    (panel_x + panel_width - dd_width - ui_cw).max(content_x + ui_cw)
                } else {
                    dd_value_area_x
                };

                let option_h = row_height * 0.85;
                let max_visible = 8usize;
                let visible_count = option_count.min(max_visible);
                let dd_height = visible_count as f32 * option_h + 8.0;

                // Shadow
                let shadow_color = [0.0, 0.0, 0.0, 0.3];
                self.rect_pipeline.push_rounded_rect(
                    dd_x + 2.0, anchor_y + 2.0,
                    dd_width, dd_height,
                    shadow_color, 6.0,
                );

                // Background
                // Use fully opaque background so content behind doesn't bleed through
                let dd_bg = [
                    self.ui.settings_bg[0],
                    self.ui.settings_bg[1],
                    self.ui.settings_bg[2],
                    1.0,
                ];
                self.rect_pipeline.push_rounded_rect(
                    dd_x, anchor_y,
                    dd_width, dd_height,
                    dd_bg, 6.0,
                );

                // Border
                let border_color = self.ui.settings_separator;
                self.rect_pipeline.push_rounded_rect(
                    dd_x, anchor_y,
                    dd_width, dd_height,
                    adjust_alpha(border_color, 0.5), 6.0,
                );
                // Inner background (slightly inset to simulate border)
                self.rect_pipeline.push_rounded_rect(
                    dd_x + 1.0, anchor_y + 1.0,
                    dd_width - 2.0, dd_height - 2.0,
                    dd_bg, 5.0,
                );

                // Render options
                for (oidx, opt_text) in options_text.iter().enumerate().take(visible_count) {
                    let oy = anchor_y + 4.0 + oidx as f32 * option_h;
                    let is_highlighted = oidx == info.dropdown_highlight;

                    if is_highlighted {
                        self.rect_pipeline.push_rounded_rect(
                            dd_x + 4.0, oy,
                            dd_width - 8.0, option_h,
                            self.ui.settings_selected, 4.0,
                        );
                        // Accent bar
                        self.rect_pipeline.push_rounded_rect(
                            dd_x + 2.0, oy,
                            3.0, option_h,
                            self.ui.settings_accent, 2.0,
                        );
                    }

                    let opt_text_y = oy + (option_h - self.cell_height) / 2.0;
                    let opt_color = if is_highlighted {
                        self.ui.settings_value
                    } else {
                        self.ui.settings_text
                    };

                    for (ci, c) in opt_text.chars().enumerate() {
                        let cx = dd_x + ui_cw * 1.5 + ci as f32 * ui_cw;
                        if cx + ui_cw > dd_x + dd_width - 8.0 {
                            self.text_pipeline.push_char('.', cx, opt_text_y, self.ui.settings_text_dim);
                            break;
                        }
                        self.text_pipeline.push_char(c, cx, opt_text_y, opt_color);
                    }
                }
            }
        }
    }

    /// Draw a wavy/curly underline (approximated with small segments)
    fn draw_wavy_underline(&mut self, x: f32, y: f32, width: f32, color: [f32; 4]) {
        // Approximate wavy line with a series of small rectangles at varying heights
        // Using a visible wave pattern similar to LSP error underlines
        let wave_period: f32 = 6.0; // Pixels per wave cycle (wider = more visible waves)
        let wave_amplitude: f32 = 2.0; // Pixels of vertical displacement
        let segment_width: f32 = 1.5; // Smaller segments = smoother curve
        let thickness: f32 = 1.5; // Slightly thicker for visibility

        let mut current_x = x;
        let mut phase: f32 = 0.0;

        while current_x < x + width {
            // Sine wave approximation - center the wave vertically
            let offset_y = (phase * std::f32::consts::TAU).sin() * wave_amplitude;
            self.rect_pipeline.push_rect(
                current_x,
                y + offset_y - wave_amplitude, // Offset so wave is centered around y
                segment_width.min(x + width - current_x),
                thickness,
                color,
            );

            current_x += segment_width;
            phase += segment_width / wave_period;
        }
    }

    /// Draw a dotted underline
    fn draw_dotted_underline(&mut self, x: f32, y: f32, width: f32, color: [f32; 4]) {
        let dot_size: f32 = 1.5;
        let gap: f32 = 2.0;
        let step = dot_size + gap;

        let mut current_x = x;
        while current_x < x + width {
            self.rect_pipeline.push_rect(current_x, y, dot_size.min(x + width - current_x), 1.0, color);
            current_x += step;
        }
    }

    /// Draw a dashed underline
    fn draw_dashed_underline(&mut self, x: f32, y: f32, width: f32, color: [f32; 4]) {
        let dash_length: f32 = 4.0;
        let gap: f32 = 2.0;
        let step = dash_length + gap;

        let mut current_x = x;
        while current_x < x + width {
            self.rect_pipeline.push_rect(current_x, y, dash_length.min(x + width - current_x), 1.0, color);
            current_x += step;
        }
    }

    /// Build search bar render data
    /// Renders at the top of the terminal content area, below the tab bar
    fn build_search_bar(&mut self, info: &SearchBarInfo, window_width: f32, tab_bar_height: f32) {
        let padding = 8.0;
        let bar_height = self.cell_height + padding * 2.0;
        let bar_y = tab_bar_height;

        // Background - warm dark tone
        let bar_bg = self.ui.search_bg;
        self.rect_pipeline.push_rect(0.0, bar_y, window_width, bar_height, bar_bg);

        // Bottom border
        let border_color = adjust_alpha(self.ui.command_border, 0.5);
        self.rect_pipeline.push_rect(0.0, bar_y + bar_height - 1.0, window_width, 1.0, border_color);

        let text_y = bar_y + padding;
        let mut text_x = padding;

        // Search icon/prompt
        let prompt = "/";
        let prompt_color = self.ui.search_indicator;
        for c in prompt.chars() {
            self.text_pipeline.push_char(c, text_x, text_y, prompt_color);
            text_x += self.cell_width;
        }
        text_x += self.cell_width * 0.3;

        // Query text
        let text_color = self.ui.search_text;
        let mut char_index = 0;
        for c in info.query.chars() {
            // Draw cursor at current position
            if char_index == info.cursor_pos {
                let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
                self.rect_pipeline.push_rect(
                    text_x, text_y,
                    2.0, self.cell_height,
                    cursor_color,
                );
            }

            self.text_pipeline.push_char(c, text_x, text_y, text_color);
            text_x += self.cell_width;
            char_index += c.len_utf8();
        }

        // Draw cursor at end if at end of query
        if info.cursor_pos >= info.query.len() {
            let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
            self.rect_pipeline.push_rect(
                text_x, text_y,
                2.0, self.cell_height,
                cursor_color,
            );
        }
        text_x += self.cell_width;

        // Mode indicators on the right side
        let mut right_x = window_width - padding;

        // Navigation arrows ▲▼ (only show if there are matches)
        if info.match_count > 0 {
            // Down arrow ▼
            let arrow_color = self.ui.search_indicator;
            right_x -= self.cell_width;
            self.text_pipeline.push_char('▼', right_x, text_y, arrow_color);

            // Up arrow ▲
            right_x -= self.cell_width;
            self.text_pipeline.push_char('▲', right_x, text_y, arrow_color);

            right_x -= self.cell_width * 0.5; // Gap before count
        }

        // Match count indicator
        let count_text = if info.match_count > 0 {
            format!("{}/{}", info.current_match.unwrap_or(0), info.match_count)
        } else if !info.query.is_empty() {
            "No matches".to_string()
        } else {
            String::new()
        };

        if !count_text.is_empty() {
            let count_width = count_text.len() as f32 * self.cell_width;
            right_x -= count_width;

            let count_color = if info.match_count > 0 {
                self.ui.search_indicator
            } else {
                self.ui.search_error
            };

            for c in count_text.chars() {
                self.text_pipeline.push_char(c, right_x, text_y, count_color);
                right_x += self.cell_width;
            }
            right_x -= count_width + self.cell_width; // Reset and add gap
        }

        // Regex indicator [.*] - always show, dimmed when inactive
        {
            let indicator = "[.*]";
            let indicator_width = indicator.len() as f32 * self.cell_width;
            right_x -= indicator_width + self.cell_width * 0.5;

            let regex_color = if info.regex_error.is_some() {
                self.ui.search_error
            } else if info.use_regex {
                self.ui.search_indicator
            } else {
                adjust_alpha(self.ui.search_indicator, 0.35)
            };

            for c in indicator.chars() {
                self.text_pipeline.push_char(c, right_x, text_y, regex_color);
                right_x += self.cell_width;
            }
            right_x -= indicator_width + self.cell_width;
        }

        // Case sensitivity indicator [Aa] - always show, dimmed when inactive
        {
            let indicator = "[Aa]";
            let indicator_width = indicator.len() as f32 * self.cell_width;
            right_x -= indicator_width + self.cell_width * 0.5;

            // Bright when active, dimmed when inactive
            let indicator_color = if info.case_sensitive {
                self.ui.search_indicator
            } else {
                adjust_alpha(self.ui.search_indicator, 0.35)
            };

            for c in indicator.chars() {
                self.text_pipeline.push_char(c, right_x, text_y, indicator_color);
                right_x += self.cell_width;
            }
            right_x -= indicator_width + self.cell_width;
        }

        // Filter mode indicator [⊜] - always show, dimmed when inactive
        // Shows line count when active
        {
            let indicator = if info.filter_mode {
                format!("[⊜{}]", info.matching_line_count)
            } else {
                "[⊜]".to_string()
            };
            let indicator_width = indicator.chars().count() as f32 * self.cell_width;
            right_x -= indicator_width + self.cell_width * 0.5;

            // Bright when active, dimmed when inactive
            let indicator_color = if info.filter_mode {
                self.ui.search_indicator
            } else {
                adjust_alpha(self.ui.search_indicator, 0.35)
            };

            for c in indicator.chars() {
                self.text_pipeline.push_char(c, right_x, text_y, indicator_color);
                right_x += self.cell_width;
            }
            let _ = right_x; // Silence unused warning
        }

        // Show regex error if any
        if let Some(error) = &info.regex_error {
            let error_x = text_x + self.cell_width;
            let error_text = format!("Error: {}", error);
            let error_color = self.ui.search_error;

            let max_chars = ((window_width - error_x - padding * 2.0) / self.cell_width) as usize;
            for (i, c) in error_text.chars().take(max_chars.saturating_sub(3)).enumerate() {
                self.text_pipeline.push_char(c, error_x + i as f32 * self.cell_width, text_y, error_color);
            }
        }
    }

    /// Check what element was clicked in the search bar
    /// Returns a SearchBarClick indicating which element (if any) was clicked
    ///
    /// Parameters must match the SearchBarInfo used for rendering:
    /// - match_count: number of search matches
    /// - current_match: current match index (1-based)
    /// - query_is_empty: whether the search query is empty
    /// - filter_mode: whether filter mode is active
    /// - matching_line_count: number of lines with matches (for filter button width)
    pub fn search_bar_hit_test(
        &self,
        x: f32,
        y: f32,
        window_width: f32,
        match_count: usize,
        current_match: Option<usize>,
        query_is_empty: bool,
        filter_mode: bool,
        matching_line_count: usize,
    ) -> SearchBarClick {
        let padding = 8.0;
        let bar_height = self.cell_height + padding * 2.0;
        let tab_bar_height = self.tab_bar_height();

        // Check if click is within search bar bounds
        if y < tab_bar_height || y > tab_bar_height + bar_height {
            return SearchBarClick::None;
        }

        // Calculate button positions from the right edge
        // This MUST exactly match the rendering logic in build_search_bar
        let mut right_x = window_width - padding;

        // Navigation arrows ▲▼ (only show if there are matches)
        if match_count > 0 {
            // Down arrow ▼
            right_x -= self.cell_width;
            let down_start = right_x;
            let down_end = right_x + self.cell_width;
            if x >= down_start && x < down_end {
                return SearchBarClick::NextMatch;
            }

            // Up arrow ▲
            right_x -= self.cell_width;
            let up_start = right_x;
            let up_end = right_x + self.cell_width;
            if x >= up_start && x < up_end {
                return SearchBarClick::PrevMatch;
            }

            right_x -= self.cell_width * 0.5; // Gap before count
        }

        // Match count indicator - calculate exact width like rendering does
        let count_text = if match_count > 0 {
            format!("{}/{}", current_match.unwrap_or(0), match_count)
        } else if !query_is_empty {
            "No matches".to_string()
        } else {
            String::new()
        };

        if !count_text.is_empty() {
            let count_width = count_text.len() as f32 * self.cell_width;
            right_x -= count_width;
            // Skip drawing the count (we're at the start of count now)
            // After rendering, right_x is reset: right_x -= count_width + self.cell_width
            right_x -= self.cell_width; // Gap after count
        }

        // Regex indicator [.*] - 4 chars
        let regex_width = 4.0 * self.cell_width;
        right_x -= regex_width + self.cell_width * 0.5;
        let regex_start = right_x;
        let regex_end = right_x + regex_width;
        if x >= regex_start && x < regex_end {
            return SearchBarClick::ToggleRegex;
        }
        // After drawing: right_x -= indicator_width + self.cell_width
        right_x -= self.cell_width;

        // Case sensitivity indicator [Aa] - 4 chars
        let case_width = 4.0 * self.cell_width;
        right_x -= case_width + self.cell_width * 0.5;
        let case_start = right_x;
        let case_end = right_x + case_width;
        if x >= case_start && x < case_end {
            return SearchBarClick::ToggleCaseSensitive;
        }
        right_x -= self.cell_width;

        // Filter mode indicator [⊜] or [⊜N] - variable width
        let filter_indicator = if filter_mode {
            format!("[⊜{}]", matching_line_count)
        } else {
            "[⊜]".to_string()
        };
        let filter_width = filter_indicator.chars().count() as f32 * self.cell_width;
        right_x -= filter_width + self.cell_width * 0.5;
        let filter_start = right_x;
        let filter_end = right_x + filter_width;
        if x >= filter_start && x < filter_end {
            return SearchBarClick::ToggleFilter;
        }

        SearchBarClick::None
    }
    /// Hit-test the tab bar to determine what was clicked
    ///
    /// The layout logic here MUST exactly mirror `build_tab_bar`.
    pub fn tab_bar_hit_test(
        &self,
        x: f32,
        y: f32,
        tabs: &[TabInfo],
        window_width: f32,
    ) -> TabBarClick {
        let tab_bar_height = self.tab_bar_height();

        // Only handle clicks within the tab bar vertical bounds
        if y >= tab_bar_height {
            return TabBarClick::None;
        }

        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let add_button_width = tab_bar_height;

        // ── Gear icon (rightmost) ──
        let gear_x = window_width - gear_button_width;
        if x >= gear_x {
            return TabBarClick::Gear;
        }

        // ── Session dropdown (▾) button (left of gear) ──
        let session_x = window_width - gear_button_width - session_button_width;
        if x >= session_x {
            return TabBarClick::SessionDropdown;
        }

        // ── Add (+) button (left of session) ──
        let add_x = window_width - gear_button_width - session_button_width - add_button_width;
        if x >= add_x {
            return TabBarClick::NewTab;
        }

        // ── Individual tabs ──
        let available_tab_width = window_width - gear_button_width - session_button_width - add_button_width;
        let tab_count = tabs.len().max(1);
        let full_tab_width = ((available_tab_width / tab_count as f32).min(200.0)).max(80.0);
        let tab_height = tab_bar_height - 4.0;
        let show_close_buttons = true;
        let close_btn_size = self.cell_height;
        let close_btn_margin = 4.0;

        let mut current_x = 0.0f32;

        for (i, tab) in tabs.iter().enumerate() {
            let anim_multiplier = if tab.close_progress > 0.0 {
                let t = tab.close_progress;
                1.0 - t * t
            } else {
                let t = tab.slide_progress;
                1.0 - (1.0 - t) * (1.0 - t)
            };
            let tab_width = full_tab_width * anim_multiplier;

            if x >= current_x && x < current_x + tab_width {
                // Click is within this tab — check if it hit the close button
                if show_close_buttons && tab_width > 2.0 && anim_multiplier > 0.3 {
                    let tab_y = 2.0;
                    let close_x = current_x + tab_width - close_btn_size - close_btn_margin - 2.0;
                    let close_y = tab_y + (tab_height - close_btn_size) / 2.0;

                    if x >= close_x && x <= close_x + close_btn_size
                        && y >= close_y && y <= close_y + close_btn_size
                    {
                        return TabBarClick::CloseTab(i);
                    }
                }
                return TabBarClick::Tab(i);
            }

            current_x += tab_width;
        }

        TabBarClick::None
    }
}

/// Result of clicking in the tab bar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBarClick {
    /// Clicked on tab body at given index
    Tab(usize),
    /// Clicked close (×) button on tab at given index
    CloseTab(usize),
    /// Clicked the + (new tab) button
    NewTab,
    /// Clicked the session dropdown (▾) button
    SessionDropdown,
    /// Clicked the gear icon
    Gear,
    /// No element was clicked
    None,
}

/// Result of clicking in the search bar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBarClick {
    /// No element was clicked
    None,
    /// Toggle case sensitivity [Aa]
    ToggleCaseSensitive,
    /// Toggle regex mode [.*]
    ToggleRegex,
    /// Toggle filter mode [⊜]
    ToggleFilter,
    /// Previous match ▲
    PrevMatch,
    /// Next match ▼
    NextMatch,
}

/// Info for rendering the session dropdown
#[derive(Debug, Clone)]
pub struct SessionDropdownInfo {
    /// List of sessions as (id, display_title) pairs
    pub sessions: Vec<(String, String)>,
}

/// Result of clicking in the session dropdown
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionDropdownClick {
    /// Clicked on a session by name
    LoadSession(String),
    /// Clicked "Manage Sessions…"
    ManageSessions,
    /// Clicked outside the dropdown
    Dismiss,
    /// No click in dropdown area
    None,
}

const SESSION_DROPDOWN_MAX_ITEMS: usize = 5;

impl Renderer {
    /// Build session dropdown render data
    fn build_session_dropdown(&mut self, info: &SessionDropdownInfo, window_width: f32) {
        let tab_bar_height = self.tab_bar_height();
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        // Position dropdown below the ▾ button (left of gear)
        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let dropdown_anchor_x = window_width - gear_button_width - session_button_width;

        // Dropdown dimensions
        let row_height = (cell_height * 2.0).max(28.0);
        let padding_x = cell_width * 1.5;
        let dropdown_width = (cell_width * 22.0).max(200.0);
        let dropdown_x = (dropdown_anchor_x + session_button_width - dropdown_width).max(0.0);
        let dropdown_y = tab_bar_height;

        // Items: at least 1 row (placeholder if empty) + "Manage Sessions…"
        let session_count = info.sessions.len().min(SESSION_DROPDOWN_MAX_ITEMS);
        let display_rows = session_count.max(1); // at least 1 row for placeholder
        let extra_items = 1; // Manage Sessions
        let total_items = display_rows + extra_items;
        let dropdown_height = total_items as f32 * row_height + 4.0; // 4px padding

        // Background
        let bg_color = self.ui.settings_bg;
        self.rect_pipeline.push_rounded_rect(
            dropdown_x, dropdown_y,
            dropdown_width, dropdown_height,
            bg_color, 8.0,
        );

        // Border
        let border_color = self.ui.settings_separator;
        self.rect_pipeline.push_rounded_rect(
            dropdown_x - 1.0, dropdown_y - 1.0,
            dropdown_width + 2.0, dropdown_height + 2.0,
            border_color, 8.0,
        );
        // Re-draw bg on top of border to create border effect
        self.rect_pipeline.push_rounded_rect(
            dropdown_x, dropdown_y,
            dropdown_width, dropdown_height,
            bg_color, 8.0,
        );

        let text_color = self.ui.settings_text;
        let dim_text_color = self.ui.settings_text_dim;
        let accent_color = self.ui.settings_accent;

        // Render session items (or placeholder if none)
        if info.sessions.is_empty() {
            let item_y = dropdown_y + 2.0;
            let text_y = item_y + (row_height - cell_height) / 2.0;
            let mut text_x = dropdown_x + padding_x;
            for c in "No saved sessions".chars() {
                if text_x + cell_width > dropdown_x + dropdown_width - padding_x {
                    break;
                }
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, text_x, text_y, dim_text_color);
                }
                text_x += cell_width * 0.85;
            }
        }

        for (i, (_, display_title)) in info.sessions.iter().take(SESSION_DROPDOWN_MAX_ITEMS).enumerate() {
            let item_y = dropdown_y + 2.0 + i as f32 * row_height;
            let text_y = item_y + (row_height - cell_height) / 2.0;
            let mut text_x = dropdown_x + padding_x;

            // Session icon
            self.text_pipeline.push_char('◆', text_x - cell_width * 0.3, text_y, accent_color);
            text_x += cell_width;

            // Display session title, truncate if needed
            let display_name: String = if display_title.chars().count() > 18 {
                let truncated: String = display_title.chars().take(17).collect();
                format!("{}…", truncated)
            } else {
                display_title.clone()
            };
            for c in display_name.chars() {
                if text_x + cell_width > dropdown_x + dropdown_width - padding_x {
                    break;
                }
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, text_x, text_y, text_color);
                }
                text_x += cell_width * 0.85;
            }
        }

        // Separator before action items
        let display_rows = session_count.max(1); // match build logic
        let sep_y = dropdown_y + 2.0 + display_rows as f32 * row_height;
        self.rect_pipeline.push_rect(
            dropdown_x + padding_x, sep_y,
            dropdown_width - padding_x * 2.0, 1.0,
            self.ui.settings_separator,
        );

        // "Manage Sessions…" item
        let manage_y = sep_y + 1.0;
        let manage_text_y = manage_y + (row_height - cell_height) / 2.0;
        let mut manage_x = dropdown_x + padding_x;
        for c in "Manage Sessions…".chars() {
            if !c.is_whitespace() {
                self.text_pipeline.push_char(c, manage_x, manage_text_y, dim_text_color);
            }
            manage_x += cell_width * 0.85;
        }
    }

    /// Hit-test the session dropdown to determine what was clicked
    pub fn session_dropdown_hit_test(
        &self,
        x: f32,
        y: f32,
        sessions: &[(String, String)],
        window_width: f32,
    ) -> SessionDropdownClick {
        let tab_bar_height = self.tab_bar_height();
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        let gear_button_width = tab_bar_height;
        let session_button_width = tab_bar_height;
        let dropdown_anchor_x = window_width - gear_button_width - session_button_width;

        let row_height = (cell_height * 2.0).max(28.0);
        let dropdown_width = (cell_width * 22.0).max(200.0);
        let dropdown_x = (dropdown_anchor_x + session_button_width - dropdown_width).max(0.0);
        let dropdown_y = tab_bar_height;

        let session_count = sessions.len().min(SESSION_DROPDOWN_MAX_ITEMS);
        let display_rows = session_count.max(1); // match build logic
        let extra_items = 1;
        let total_items = display_rows + extra_items;
        let dropdown_height = total_items as f32 * row_height + 4.0;

        // Check if click is within dropdown bounds
        if x < dropdown_x || x > dropdown_x + dropdown_width
            || y < dropdown_y || y > dropdown_y + dropdown_height
        {
            return SessionDropdownClick::Dismiss;
        }

        // Determine which row was clicked
        let relative_y = y - dropdown_y - 2.0;
        let row = (relative_y / row_height) as usize;

        if row < display_rows {
            // Clicked on a session row (or placeholder if empty)
            if let Some((session_id, _)) = sessions.get(row) {
                return SessionDropdownClick::LoadSession(session_id.clone());
            }
            // Placeholder row — do nothing
            return SessionDropdownClick::None;
        } else if row == display_rows {
            return SessionDropdownClick::ManageSessions;
        }

        SessionDropdownClick::None
    }

    /// Build session management panel render data
    fn build_session_management_panel(&mut self, info: &crate::session::panel::SessionPanelInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        // Panel dimensions
        let panel_width = (ui_cw * 35.0).max(320.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * anim;
        let panel_h = window_height;
        let row_height = (cell_height * 3.5).max(52.0);
        let padding_x = ui_cw * 1.5;
        let padding_y = cell_height * 0.5;

        // ── Scrim ──
        let scrim = self.ui.scrim;
        self.rect_pipeline.push_rect(
            0.0, 0.0,
            panel_x, window_height,
            [scrim[0], scrim[1], scrim[2], scrim[3] * anim],
        );

        // ── Panel background ──
        self.rect_pipeline.push_rect(
            panel_x, 0.0,
            panel_width, panel_h,
            self.ui.settings_bg,
        );

        // ── Left border ──
        self.rect_pipeline.push_rect(
            panel_x, 0.0,
            2.0, panel_h,
            self.ui.settings_accent,
        );

        // ── Title ──
        let title = "Sessions";
        let title_y = cell_height * 1.5;
        let title_x = panel_x + padding_x;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                title_x + i as f32 * ui_cw,
                title_y,
                self.ui.settings_accent,
            );
        }

        // ── Close button (×) ──
        let close_x = panel_x + panel_width - padding_x - ui_cw;
        self.text_pipeline.push_char(
            '×',
            close_x,
            title_y,
            self.ui.settings_text,
        );

        // ── Separator under title ──
        let sep_y = title_y + cell_height * 2.0;
        self.rect_pipeline.push_rect(
            panel_x + padding_x, sep_y,
            panel_width - padding_x * 2.0, 1.0,
            self.ui.settings_separator,
        );

        // ── Session list ──
        let list_start_y = sep_y + padding_y + 4.0;
        let text_color = self.ui.settings_text;
        let dim_color = self.ui.settings_text_dim;
        let accent_color = self.ui.settings_accent;
        let btn_color = self.ui.settings_separator;

        if info.sessions.is_empty() {
            // Show "No saved sessions" placeholder
            let placeholder = "No saved sessions";
            let ph_y = list_start_y + row_height * 0.5;
            let mut px = panel_x + padding_x;
            for c in placeholder.chars() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, px, ph_y, dim_color);
                }
                px += ui_cw * 0.85;
            }
        }

        for (i, session) in info.sessions.iter().enumerate() {
            let item_y = list_start_y + i as f32 * row_height;

            // Session icon
            let icon_y = item_y + padding_y;
            self.text_pipeline.push_char('◆', panel_x + padding_x, icon_y, accent_color);

            // Session display name
            let name_x = panel_x + padding_x + ui_cw * 2.0;
            let name: String = if session.display_name.chars().count() > 22 {
                let truncated: String = session.display_name.chars().take(21).collect();
                format!("{}…", truncated)
            } else {
                session.display_name.clone()
            };
            for (j, c) in name.chars().enumerate() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(
                        c,
                        name_x + j as f32 * ui_cw * 0.85,
                        icon_y,
                        text_color,
                    );
                }
            }

            // Metadata line: "N tabs · Xm ago"
            let meta = format!("{} tab{} · {}", session.tab_count,
                if session.tab_count == 1 { "" } else { "s" },
                session.last_modified);
            let meta_y = icon_y + cell_height * 1.3;
            let mut mx = name_x;
            for c in meta.chars() {
                if !c.is_whitespace() {
                    self.text_pipeline.push_char(c, mx, meta_y, dim_color);
                }
                mx += ui_cw * 0.8;
            }

            // ── Action buttons: [Load] [Delete] ──
            let btn_height = cell_height * 1.5;
            let btn_y = icon_y + cell_height * 0.1;

            // Delete button (right-aligned)
            let delete_text = "Delete";
            let delete_w = ui_cw * (delete_text.len() as f32 + 2.0);
            let delete_x = panel_x + panel_width - padding_x - delete_w;
            self.rect_pipeline.push_rect(
                delete_x, btn_y,
                delete_w, btn_height,
                [0.6, 0.2, 0.2, 0.5],
            );
            for (j, c) in delete_text.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    delete_x + ui_cw * 1.0 + j as f32 * ui_cw * 0.85,
                    btn_y + (btn_height - cell_height) * 0.5,
                    text_color,
                );
            }

            // Load button (left of delete)
            let load_text = "Load";
            let load_w = ui_cw * (load_text.len() as f32 + 2.0);
            let load_x = delete_x - load_w - ui_cw * 0.5;
            self.rect_pipeline.push_rect(
                load_x, btn_y,
                load_w, btn_height,
                btn_color,
            );
            for (j, c) in load_text.chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    load_x + ui_cw * 1.0 + j as f32 * ui_cw * 0.85,
                    btn_y + (btn_height - cell_height) * 0.5,
                    text_color,
                );
            }

            // Row separator
            let row_sep_y = item_y + row_height - 1.0;
            self.rect_pipeline.push_rect(
                panel_x + padding_x, row_sep_y,
                panel_width - padding_x * 2.0, 1.0,
                self.ui.settings_separator,
            );
        }
    }

    /// Hit-test the session management panel
    pub fn session_panel_hit_test(
        &self,
        x: f32,
        y: f32,
        sessions: &[crate::session::panel::SessionEntry],
        window_width: f32,
        window_height: f32,
        animation: f32,
    ) -> crate::session::panel::SessionPanelAction {
        use crate::session::panel::SessionPanelAction;

        let ui_cw = self.cell_width;
        let cell_height = self.cell_height;

        let panel_width = (ui_cw * 35.0).max(320.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * animation;
        let padding_x = ui_cw * 1.5;
        let padding_y = cell_height * 0.5;
        let row_height = (cell_height * 3.5).max(52.0);

        // Click outside panel → close
        if x < panel_x {
            return SessionPanelAction::Close;
        }

        // Close button (× in top-right)
        let title_y = cell_height * 1.5;
        let close_x = panel_x + panel_width - padding_x - ui_cw;
        if x >= close_x && x <= close_x + ui_cw * 1.5
            && y >= title_y && y <= title_y + cell_height * 1.5
        {
            return SessionPanelAction::Close;
        }

        // Session list area
        let sep_y = title_y + cell_height * 2.0;
        let list_start_y = sep_y + padding_y + 4.0;

        for (i, session) in sessions.iter().enumerate() {
            let item_y = list_start_y + i as f32 * row_height;
            if y < item_y || y > item_y + row_height {
                continue;
            }

            let btn_height = cell_height * 1.5;
            let btn_y = item_y + padding_y + cell_height * 0.1;

            // Check if click is in the button row area
            if y >= btn_y && y <= btn_y + btn_height {
                // Delete button
                let delete_text = "Delete";
                let delete_w = ui_cw * (delete_text.len() as f32 + 2.0);
                let delete_x = panel_x + panel_width - padding_x - delete_w;
                if x >= delete_x && x <= delete_x + delete_w {
                    return SessionPanelAction::Delete(session.id.clone());
                }

                // Load button
                let load_text = "Load";
                let load_w = ui_cw * (load_text.len() as f32 + 2.0);
                let load_x = delete_x - load_w - ui_cw * 0.5;
                if x >= load_x && x <= load_x + load_w {
                    return SessionPanelAction::Load(session.id.clone());
                }
            }

            // Clicking anywhere else on the session row → load it
            return SessionPanelAction::Load(session.id.clone());
        }

        SessionPanelAction::None
    }
}
