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

mod chrome;
mod git;
mod overlays;
mod palette;
mod panels;
mod search_bar;
mod sessions;
mod settings_render;
mod tab_bar;
pub mod types;

pub use types::*;

pub use glyph_cache::GlyphCache;

use image::ImagePipeline;

use anyhow::Result;
use std::sync::Arc;
use wgpu::*;
use winit::window::Window;
use termeric_core::{Grid, Selection, Color as TermColor, CellAttrs, CellFlags};
use crate::config::theme::Theme;
use crate::ui::settings::SettingsPanelInfo;
use crate::terminal::search::{Search, SearchBarInfo};

use text::TextPipeline;
use rect::RectPipeline;

use crate::tabs::TabInfo;
use crate::ui::command_palette::PaletteInfo;

#[derive(Clone)]
pub(crate) struct UiPalette {
    pub(crate) background: wgpu::Color,
    pub(crate) selection: [f32; 4],
    pub(crate) selection_flash: [f32; 4],
    pub(crate) selection_text: Option<[f32; 4]>,
    pub(crate) cursor: [f32; 4],
    pub(crate) cursor_text: [f32; 4],
    pub(crate) tab_bar_bg: [f32; 4],
    pub(crate) tab_active: [f32; 4],
    pub(crate) tab_inactive: [f32; 4],
    pub(crate) tab_activity: [f32; 4],
    pub(crate) tab_border: [f32; 4],
    pub(crate) tab_text_active: [f32; 4],
    pub(crate) tab_text_inactive: [f32; 4],
    // Settings panel colors
    pub(crate) settings_bg: [f32; 4],
    pub(crate) settings_sidebar_bg: [f32; 4],
    pub(crate) settings_header: [f32; 4],
    pub(crate) settings_text: [f32; 4],
    pub(crate) settings_text_dim: [f32; 4],
    pub(crate) settings_selected: [f32; 4],
    pub(crate) settings_value: [f32; 4],
    pub(crate) settings_accent: [f32; 4],
    pub(crate) settings_separator: [f32; 4],
    pub(crate) scrim: [f32; 4],
    // Search colors
    pub(crate) search_bg: [f32; 4],
    pub(crate) search_text: [f32; 4],
    pub(crate) search_match: [f32; 4],
    pub(crate) search_match_current: [f32; 4],
    pub(crate) search_indicator: [f32; 4],
    pub(crate) search_error: [f32; 4],
    // URL/hyperlink color
    pub(crate) url: [f32; 4],
    // Command palette colors
    pub(crate) palette_scrim: [f32; 4],
    pub(crate) palette_bg: [f32; 4],
    pub(crate) palette_input_bg: [f32; 4],
    pub(crate) palette_accent: [f32; 4],
    pub(crate) palette_text: [f32; 4],
    pub(crate) palette_text_dim: [f32; 4],
    pub(crate) palette_match_highlight: [f32; 4],
    pub(crate) palette_keycap_bg: [f32; 4],
    pub(crate) palette_keycap_text: [f32; 4],
    pub(crate) palette_border: [f32; 4],
    pub(crate) palette_destructive: [f32; 4],
    pub(crate) palette_category: [f32; 4],
    pub(crate) palette_footer: [f32; 4],
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
        Self {
            background: bg.to_wgpu(),
            selection: selection.to_f32_array(),
            selection_flash: adjust_alpha(fg.to_f32_array(), 0.85),
            selection_text: theme.ui.selection_text.map(|c| c.to_f32_array()),
            cursor: cursor.to_f32_array(),
            cursor_text: theme.ui.cursor_text.to_f32_array(),
            tab_bar_bg: shade(tab_inactive.to_f32_array(), 0.85),
            tab_active: tab_active.to_f32_array(),
            tab_inactive: tab_inactive.to_f32_array(),
            tab_activity: theme.primary.yellow.to_f32_array(),
            tab_border: tab_border.to_f32_array(),
            tab_text_active,
            tab_text_inactive,
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
            search_match: theme.ui.search_match.with_alpha(0.35).to_f32_array(),
            search_match_current: theme.ui.search_match.with_alpha(0.65).to_f32_array(),
            search_indicator: theme.ansi_color(2).to_f32_array(), // green
            search_error: theme.ansi_color(1).to_f32_array(), // red
            // URL/hyperlink color from theme
            url: theme.ui.url.to_f32_array(),
            // Command palette — fully theme-derived
            palette_scrim: adjust_alpha(shade(bg.to_f32_array(), 0.0), 0.55),
            palette_bg: shade(bg.to_f32_array(), 0.65),
            palette_input_bg: shade(bg.to_f32_array(), 0.50),
            palette_accent: cursor.to_f32_array(),
            palette_text: fg.to_f32_array(),
            palette_text_dim: adjust_alpha(fg.to_f32_array(), 0.50),
            palette_match_highlight: cursor.to_f32_array(),
            palette_keycap_bg: adjust_alpha(shade(bg.to_f32_array(), 0.45), 0.9),
            palette_keycap_text: adjust_alpha(fg.to_f32_array(), 0.70),
            palette_border: adjust_alpha(tab_border.to_f32_array(), 0.5),
            palette_destructive: adjust_alpha(theme.primary.red.to_f32_array(), 0.08),
            palette_category: adjust_alpha(fg.to_f32_array(), 0.40),
            palette_footer: adjust_alpha(fg.to_f32_array(), 0.35),
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

pub(crate) fn adjust_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
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
    // Tab bar hover state for icon animations
    tab_bar_hover: Option<TabBarClick>,
    tab_bar_hover_progress: f32,
    // Git completion overlay state
    git_completions: Option<GitCompletionInfo>,
    // Info overlay state (About/Version)
    info_overlay: Option<InfoOverlayInfo>,
    diff_review: Option<DiffReviewInfo>,
    context_picker: Option<ContextPickerInfo>,
    guardrail: Option<GuardrailInfo>,
    chrome_bar: Option<crate::ui::claude_chrome::ChromeBarInfo>,
    /// Visual bell intensity (0.0 = off, 1.0 = full flash)
    bell_intensity: f32,
}

impl Renderer {
    /// Create a new renderer for the given window
    pub fn new(window: Arc<Window>, font_size: f32, font_family: &str, padding: f32, scale_factor: f32, theme: &Theme) -> Result<Self> {
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
        let glyph_cache = GlyphCache::new(&device, &queue, scaled_font_size, font_family)?;
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
            tab_bar_hover: None,
            tab_bar_hover_progress: 0.0,
            git_completions: None,
            info_overlay: None,
            diff_review: None,
            context_picker: None,
            guardrail: None,
            chrome_bar: None,
            bell_intensity: 0.0,
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

    /// Get cell width in logical pixels
    pub fn cell_width(&self) -> f32 {
        self.cell_width
    }

    /// Get cell height in logical pixels
    pub fn cell_height(&self) -> f32 {
        self.cell_height
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

    /// Update tab bar hover state for icon animations
    pub fn set_tab_bar_hover(&mut self, hover: Option<TabBarClick>, progress: f32) {
        self.tab_bar_hover = hover;
        self.tab_bar_hover_progress = progress;
    }

    /// Set git completion info for rendering
    pub fn set_git_completions(&mut self, info: Option<GitCompletionInfo>) {
        self.git_completions = info;
    }

    /// Set info overlay data for rendering
    pub fn set_info_overlay(&mut self, info: Option<InfoOverlayInfo>) {
        self.info_overlay = info;
    }

    pub fn set_diff_review(&mut self, info: Option<DiffReviewInfo>) {
        self.diff_review = info;
    }

    pub fn set_context_picker(&mut self, info: Option<ContextPickerInfo>) {
        self.context_picker = info;
    }

    pub fn set_guardrail(&mut self, info: Option<GuardrailInfo>) {
        self.guardrail = info;
    }

    pub fn set_chrome_bar(&mut self, info: Option<crate::ui::claude_chrome::ChromeBarInfo>) {
        self.chrome_bar = info;
    }

    pub fn set_bell_intensity(&mut self, intensity: f32) {
        self.bell_intensity = intensity;
    }

    /// Height of the chrome bar (0 if no chrome bar is active)
    pub fn chrome_bar_height(&self) -> f32 {
        if self.chrome_bar.is_some() {
            self.cell_height * 1.4
        } else {
            0.0
        }
    }

    /// Render the terminal grid with tab bar
    /// cursor_opacity: 0.0 = invisible, 1.0 = fully visible (for smooth blink animation)
    /// cursor_pos: animated cursor position (col, row) as floats for smooth movement
    /// copy_flash: 0.0 = no flash, 1.0 = full flash (for copy confirmation)
    /// focus_dim: 0.0 = fully bright (focused), higher = dimmed (unfocused)
    /// search_bar: optional search bar info when searching
    /// search: search state for match highlighting
    /// images: optional list of image placements and their data for Kitty graphics
    pub fn render(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo], copy_flash: f32, focus_dim: f32, settings_panel: Option<&SettingsPanelInfo>, search_bar: Option<&SearchBarInfo>, search: &Search, images: Option<&[(&termeric_core::ImagePlacement, &termeric_core::StoredImage)]>, session_dropdown: Option<&SessionDropdownInfo>, session_panel: Option<&crate::session::panel::SessionPanelInfo>, commands_panel: Option<&crate::ui::commands::panel::CommandsPanelInfo>, command_palette: Option<&PaletteInfo>) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        // Build render data from grid and tabs (inserts layer boundaries)
        self.build_render_data(grid, selection, cursor_opacity, cursor_pos, tabs, copy_flash, focus_dim, settings_panel, search_bar, search, session_dropdown, session_panel, commands_panel, command_palette);

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
    fn build_render_data(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo], copy_flash: f32, focus_dim: f32, settings_panel: Option<&SettingsPanelInfo>, search_bar: Option<&SearchBarInfo>, search: &Search, session_dropdown: Option<&SessionDropdownInfo>, session_panel: Option<&crate::session::panel::SessionPanelInfo>, commands_panel: Option<&crate::ui::commands::panel::CommandsPanelInfo>, command_palette: Option<&PaletteInfo>) {
        self.rect_pipeline.clear();
        self.text_pipeline.clear();

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let window_width = self.size.0 as f32;
        let window_height = self.size.1 as f32;
        let padding = self.padding;
        let tab_bar_height = self.tab_bar_height();

        // Calculate brightness multiplier for focus dimming (1.0 = full brightness, lower = dimmed)
        let brightness = 1.0 - focus_dim;

        // Calculate search bar height if visible
        let search_bar_height = if search_bar.is_some() {
            cell_height + 8.0 * 2.0 // padding * 2
        } else {
            0.0
        };

        // Content area starts below tab bar, chrome bar (if Claude tab), and search bar
        let chrome_height = self.chrome_bar_height();
        let content_top = tab_bar_height + chrome_height + search_bar_height;

        // ── LAYER 0: Base content (tab bar, grid, cursor, scrollbar) ──
        // Render tab bar first
        self.build_tab_bar(tabs, window_width, tab_bar_height);

        // Render chrome bar for Claude tabs (sits between tab bar and content)
        if let Some(ref info) = self.chrome_bar.clone() {
            self.build_chrome_bar(info, window_width, tab_bar_height, chrome_height);
        }

        // Selection highlight color - earthy yellow (inner layer) with transparency
        // When copy_flash > 0, brighten the selection to white for a flash effect
        let selection_base = self.ui.selection;
        let selection_flash = self.ui.selection_flash;
        let selection_color = apply_brightness(mix(selection_base, selection_flash, copy_flash), brightness);

        // Get scroll offset to determine if cursor should be shown
        let scroll_offset = grid.scroll_offset();
        let scroll_offset_i32 = i32::try_from(scroll_offset).unwrap_or(i32::MAX);
        let default_bg = TermColor::default_bg();

        let max_grid_y = window_height - padding;

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

                // Apply inverse attribute if set (swap fg/bg)
                let (fg, bg) = cell.apply_inverse();

                // Draw background if not default or if selected/matched
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
                        // Use selection_text color for selected cells if theme provides one
                        let text_color = if is_selected {
                            if let Some(sel_text) = self.ui.selection_text {
                                apply_brightness(sel_text, brightness)
                            } else {
                                [fg.r as f32 / 255.0 * brightness, fg.g as f32 / 255.0 * brightness, fg.b as f32 / 255.0 * brightness, 1.0]
                            }
                        } else {
                            [fg.r as f32 / 255.0 * brightness, fg.g as f32 / 255.0 * brightness, fg.b as f32 / 255.0 * brightness, 1.0]
                        };
                        self.text_pipeline.push_char(c, x, y, text_color);
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

                    // Re-render the character under cursor with cursor_text color
                    let cursor_row = cursor.row as usize;
                    let cursor_col = cursor.col as usize;
                    if let Some(cell) = grid.cell_scrolled(cursor_row, cursor_col) {
                        if let Some(c) = cell.content.char() {
                            if !c.is_whitespace() {
                                let ct = apply_brightness(self.ui.cursor_text, brightness);
                                self.text_pipeline.push_char(c, x, y,
                                    [ct[0], ct[1], ct[2], cursor_opacity]);
                            }
                        }
                    }
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
            let scrollbar_bottom = window_height - padding;
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

        // Visual bell flash — subtle screen-edge glow
        if self.bell_intensity > 0.0 {
            let alpha = self.bell_intensity * 0.15;
            let flash_color = [1.0, 1.0, 1.0, alpha];
            self.rect_pipeline.push_rect(0.0, 0.0, window_width, window_height, flash_color);
        }

        // ── LAYER 1: Search bar (overlays grid content) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(search_info) = search_bar {
            self.build_search_bar(search_info, window_width, tab_bar_height);
        }

        // ── LAYER 2: Git completion dropdown (overlays grid) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.git_completions.clone() {
            self.build_git_completions(info, cursor_pos, window_width, window_height);
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

        // ── LAYER 7: Commands panel ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(cp) = commands_panel {
            if cp.animation > 0.001 {
                self.build_commands_panel(cp, window_width, window_height);
            }
        }

        // ── LAYER 8: Command palette (topmost overlay) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(palette) = command_palette {
            if palette.animation > 0.001 {
                self.build_command_palette(palette, window_width, window_height);
            }
        }

        // ── LAYER 9: Info overlay (above everything) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.info_overlay.clone() {
            if info.animation > 0.001 {
                self.build_info_overlay(info, window_width, window_height);
            }
        }

        // ── LAYER 10: Diff review overlay (topmost) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.diff_review.clone() {
            if info.animation > 0.001 {
                self.build_diff_review(info, window_width, window_height);
            }
        }

        // ── LAYER 11: Context picker overlay ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.context_picker.clone() {
            if info.animation > 0.001 {
                self.build_context_picker(info, window_width, window_height);
            }
        }

        // ── LAYER 12: Guardrail confirmation overlay (absolute topmost) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.guardrail.clone() {
            if info.animation > 0.001 {
                self.build_guardrail_confirmation(info, window_width, window_height);
            }
        }
    }
}
