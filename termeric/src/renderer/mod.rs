//! GPU rendering pipeline using wgpu
//!
//! This module handles all GPU-accelerated rendering including:
//! - Text glyph rendering with texture atlas
//! - Background rectangle rendering
//! - Cursor rendering
//! - Tab bar rendering

mod glyph_cache;
mod image;
mod rect;
mod text;

mod chrome;
pub(crate) mod draw;
mod git;
mod harvest;
mod overlays;
mod palette;
mod panels;
mod search_bar;
mod sessions;
mod settings_render;
mod shortcuts_render;
mod tab_bar;
mod toasts;
pub mod types;

pub use types::*;

pub use glyph_cache::GlyphCache;

use image::ImagePipeline;

use crate::config::CursorStyle;
use crate::config::theme::Theme;
use crate::terminal::search::{Search, SearchBarInfo};
use crate::terminal::url::UrlCache;
use crate::ui::settings::SettingsPanelInfo;
use anyhow::Result;
use std::sync::Arc;
use termeric_core::{CellAttrs, CellFlags, Color as TermColor, Grid, Selection};
use wgpu::*;
use winit::window::Window;

use rect::RectPipeline;
use text::TextPipeline;

use crate::tabs::TabInfo;
use crate::ui::command_palette::PaletteInfo;
use crate::ui::shortcuts_panel::ShortcutsPanelInfo;

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
    pub(crate) settings_header: [f32; 4],
    pub(crate) settings_text: [f32; 4],
    pub(crate) settings_text_dim: [f32; 4],
    pub(crate) settings_selected: [f32; 4],
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
            settings_header: fg.to_f32_array(),
            settings_text: fg.to_f32_array(),
            settings_text_dim: adjust_alpha(fg.to_f32_array(), 0.55),
            settings_selected: adjust_alpha(cursor.to_f32_array(), 0.25),
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
            search_error: theme.ansi_color(1).to_f32_array(),     // red
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
    /// Transient toast notifications for the current frame
    toasts: Vec<crate::ui::toast::ToastInfo>,
    /// Pane dividers for the current frame
    pane_dividers: Vec<crate::panes::Divider>,
    /// Visual bell intensity (0.0 = off, 1.0 = full flash)
    bell_intensity: f32,
    /// URL cache for detected URLs (auto-detected, not OSC 8)
    url_cache: Option<UrlCache>,
}

impl Renderer {
    /// Create a new renderer for the given window
    pub fn new(
        window: Arc<Window>,
        font_size: f32,
        font_family: &str,
        fallback_families: &[String],
        line_height_multiplier: f32,
        padding: f32,
        scale_factor: f32,
        theme: &Theme,
        ligatures: bool,
    ) -> Result<Self> {
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
            alpha_mode: if surface_caps
                .alpha_modes
                .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
            {
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
        let glyph_cache = GlyphCache::new(
            &device,
            &queue,
            scaled_font_size,
            font_family,
            fallback_families,
            line_height_multiplier,
            ligatures,
        )?;
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
            toasts: Vec::new(),
            pane_dividers: Vec::new(),
            bell_intensity: 0.0,
            url_cache: None,
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

    /// Apply a primary font and ordered configured fallbacks.
    pub fn set_font_families(&mut self, primary: String, fallbacks: &[String]) {
        self.glyph_cache
            .set_font_families(primary, fallbacks, &self.queue);
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
    }

    /// Change the configured line-height multiplier.
    pub fn set_line_height(&mut self, multiplier: f32) {
        self.glyph_cache.set_line_height(multiplier, &self.queue);
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
    }

    /// Get all installed monospace font families
    pub fn get_monospace_fonts(&self) -> Vec<String> {
        self.glyph_cache.get_monospace_fonts()
    }

    /// Get current font family from the glyph cache
    pub fn font_family(&self) -> &str {
        self.glyph_cache.font_family()
    }

    /// Get the unscaled font size from the glyph cache
    pub fn glyph_font_size(&self) -> f32 {
        self.glyph_cache.font_size()
    }

    /// Enable or disable ligature rendering
    pub fn set_ligatures(&mut self, enabled: bool) {
        self.glyph_cache.set_ligatures(enabled);
    }

    /// Clear the glyph cache (e.g., after font changes)
    pub fn clear_glyph_cache(&mut self) {
        self.glyph_cache.clear(&self.queue);
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

    /// Update the toast stack for the current frame. Empty vec clears.
    pub fn set_toasts(&mut self, toasts: Vec<crate::ui::toast::ToastInfo>) {
        self.toasts = toasts;
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

    /// Store pane dividers for rendering in the next frame.
    pub fn add_pane_dividers(&mut self, dividers: &[crate::panes::Divider]) {
        self.pane_dividers = dividers.to_vec();
    }

    /// Draw stored pane dividers. Called from within build_render_data().
    fn draw_pane_dividers(&mut self, content_top: f32) {
        let padding = self.padding;

        // Dark groove + light edge for a subtle 3D divider
        let groove_color = [0.15, 0.15, 0.18, 0.9];
        let edge_color = [0.35, 0.35, 0.40, 0.4];

        for div in self.pane_dividers.clone() {
            if div.vertical {
                let x = padding + div.position;
                let y0 = content_top + div.start;
                let y1 = content_top + div.end;
                let len = y1 - y0;
                // Main groove (3px)
                self.rect_pipeline.push_rect(x, y0, 3.0, len, groove_color);
                // Light edge on the right (1px)
                self.rect_pipeline
                    .push_rect(x + 3.0, y0, 1.0, len, edge_color);
            } else {
                let x0 = padding + div.start;
                let x1 = padding + div.end;
                let y = content_top + div.position;
                let len = x1 - x0;
                // Main groove (3px)
                self.rect_pipeline.push_rect(x0, y, len, 3.0, groove_color);
                // Light edge on bottom (1px)
                self.rect_pipeline
                    .push_rect(x0, y + 3.0, len, 1.0, edge_color);
            }
        }
    }

    pub fn set_bell_intensity(&mut self, intensity: f32) {
        self.bell_intensity = intensity;
    }

    /// Set the URL cache for auto-detected URL underlining
    pub fn set_url_cache(&mut self, cache: Option<UrlCache>) {
        self.url_cache = cache;
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
    /// panes: multi-pane render data (empty = single-grid mode)
    /// harvest: optional harvest mode render info
    pub fn render(
        &mut self,
        grid: &Grid,
        selection: Option<&Selection>,
        cursor_opacity: f32,
        cursor_pos: (f32, f32),
        tabs: &[TabInfo],
        copy_flash: f32,
        focus_dim: f32,
        settings_panel: Option<&SettingsPanelInfo>,
        search_bar: Option<&SearchBarInfo>,
        search: &Search,
        images: Option<&[(&termeric_core::ImagePlacement, &termeric_core::StoredImage)]>,
        session_dropdown: Option<&SessionDropdownInfo>,
        session_panel: Option<&crate::session::panel::SessionPanelInfo>,
        commands_panel: Option<&crate::ui::commands::panel::CommandsPanelInfo>,
        command_palette: Option<&PaletteInfo>,
        shortcuts_panel: Option<&ShortcutsPanelInfo>,
        cursor_style: CursorStyle,
        panes: &[PaneRenderData],
        harvest: Option<&HarvestRenderInfo>,
    ) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        // Build render data from grid and tabs (inserts layer boundaries)
        self.build_render_data(
            grid,
            selection,
            cursor_opacity,
            cursor_pos,
            tabs,
            copy_flash,
            focus_dim,
            settings_panel,
            search_bar,
            search,
            session_dropdown,
            session_panel,
            commands_panel,
            command_palette,
            shortcuts_panel,
            cursor_style,
            panes,
            harvest,
        );

        // Build image render data if any
        self.build_image_data(images);

        // Upload vertex data
        self.rect_pipeline.upload(&self.device, &self.queue);
        self.text_pipeline
            .upload(&self.device, &self.queue, &mut self.glyph_cache);
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
            let layer_count = self
                .rect_pipeline
                .layer_count()
                .max(self.text_pipeline.layer_count());

            for layer in 0..layer_count {
                // Draw rectangles for this layer (backgrounds, panels, etc.)
                self.rect_pipeline.draw_layer(&mut render_pass, layer);

                // Draw images only on the base layer (layer 0) — images are terminal content
                if layer == 0 {
                    self.image_pipeline.draw(&mut render_pass);
                }

                // Draw text for this layer
                self.text_pipeline
                    .draw_layer(&mut render_pass, &self.glyph_cache, layer);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Build image render data from placements
    fn build_image_data(
        &mut self,
        images: Option<&[(&termeric_core::ImagePlacement, &termeric_core::StoredImage)]>,
    ) {
        self.image_pipeline.clear();

        let Some(images) = images else {
            // No active images — garbage-collect stale GPU textures
            let empty: Vec<termeric_core::ImageId> = Vec::new();
            self.image_pipeline.gc_images(&empty);
            return;
        };

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let padding = self.padding;
        let tab_bar_height = self.tab_bar_height();

        // Collect active image IDs for GC
        let active_ids: Vec<_> = images.iter().map(|(_, img)| img.id).collect();
        self.image_pipeline.gc_images(&active_ids);

        for (placement, image) in images {
            // Skip upload if already on GPU
            if self.image_pipeline.has_image(image.id) {
                // Use cached GPU dimensions for size calculation
                if let Some((_gw, _gh)) = self.image_pipeline.image_dimensions(image.id) {
                    // dimensions available — proceed with placement below
                }
            } else {
                self.image_pipeline
                    .upload_image(&self.device, &self.queue, image);
            }

            // Calculate position
            let x = padding + (placement.col as f32) * cell_width + placement.x_offset as f32;
            let y = tab_bar_height
                + padding
                + (placement.row as f32) * cell_height
                + placement.y_offset as f32;

            // Calculate size (use placement size or image natural size)
            let width = placement
                .width
                .map(|w| w as f32 * cell_width)
                .unwrap_or(image.width as f32);
            let height = placement
                .height
                .map(|h| h as f32 * cell_height)
                .unwrap_or(image.height as f32);

            self.image_pipeline
                .push_image(image.id, x, y, width, height);
        }
    }

    /// Build render data from the grid and tabs
    fn build_render_data(
        &mut self,
        grid: &Grid,
        selection: Option<&Selection>,
        cursor_opacity: f32,
        cursor_pos: (f32, f32),
        tabs: &[TabInfo],
        copy_flash: f32,
        focus_dim: f32,
        settings_panel: Option<&SettingsPanelInfo>,
        search_bar: Option<&SearchBarInfo>,
        search: &Search,
        session_dropdown: Option<&SessionDropdownInfo>,
        session_panel: Option<&crate::session::panel::SessionPanelInfo>,
        commands_panel: Option<&crate::ui::commands::panel::CommandsPanelInfo>,
        command_palette: Option<&PaletteInfo>,
        shortcuts_panel: Option<&ShortcutsPanelInfo>,
        cursor_style: CursorStyle,
        panes: &[PaneRenderData],
        harvest: Option<&HarvestRenderInfo>,
    ) {
        self.rect_pipeline.clear();
        self.text_pipeline.clear();
        self.pane_dividers.clear();

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

        // ── Render terminal grid(s) ──
        // Multi-pane mode: render each pane's grid in its own viewport
        // Single-grid mode: render one grid filling the content area
        if !panes.is_empty() {
            for pane in panes {
                let pane_x = padding + pane.rect.x;
                let pane_y = content_top + pane.rect.y;
                let pane_cols = (pane.rect.width / cell_width).floor() as usize;
                let pane_rows = (pane.rect.height / cell_height).floor() as usize;

                // Use pre-computed cursor position (animated for focused, raw for unfocused)
                let pane_cursor_pos = pane.cursor_pos;

                self.build_pane_grid(
                    pane.grid,
                    pane.selection,
                    pane_x,
                    pane_y,
                    pane_cols.max(1),
                    pane_rows.max(1),
                    pane.is_focused,
                    cursor_style,
                    cursor_opacity,
                    pane_cursor_pos,
                    copy_flash,
                    brightness,
                    search,
                    window_height,
                    pane.rect.width,
                );

                // Draw focus indicator for the focused pane (subtle accent border)
                if panes.len() > 1 && pane.is_focused {
                    let border_color = adjust_alpha(self.ui.cursor, 0.5);
                    let bw = 2.0_f32;
                    // Top
                    self.rect_pipeline
                        .push_rect(pane_x, pane_y, pane.rect.width, bw, border_color);
                    // Bottom
                    self.rect_pipeline.push_rect(
                        pane_x,
                        pane_y + pane.rect.height - bw,
                        pane.rect.width,
                        bw,
                        border_color,
                    );
                    // Left
                    self.rect_pipeline.push_rect(
                        pane_x,
                        pane_y,
                        bw,
                        pane.rect.height,
                        border_color,
                    );
                    // Right
                    self.rect_pipeline.push_rect(
                        pane_x + pane.rect.width - bw,
                        pane_y,
                        bw,
                        pane.rect.height,
                        border_color,
                    );
                }
            }
        } else {
            // Single grid mode — full content area
            let grid_x = padding;
            let grid_y = content_top + padding;
            let grid_cols = grid.cols();
            let grid_rows = grid.rows();
            let grid_width = window_width - padding * 2.0;

            self.build_pane_grid(
                grid,
                selection,
                grid_x,
                grid_y,
                grid_cols,
                grid_rows,
                true, // always focused in single mode
                cursor_style,
                cursor_opacity,
                cursor_pos,
                copy_flash,
                brightness,
                search,
                window_height,
                grid_width,
            );
        }

        // Draw pane dividers (stored from add_pane_dividers call)
        if !self.pane_dividers.is_empty() {
            self.draw_pane_dividers(content_top);
        }

        // Visual bell flash — subtle screen-edge glow
        if self.bell_intensity > 0.0 {
            let alpha = self.bell_intensity * 0.15;
            let flash_color = [1.0, 1.0, 1.0, alpha];
            self.rect_pipeline
                .push_rect(0.0, 0.0, window_width, window_height, flash_color);
        }

        // ── LAYER 1: Harvest indicators (in gutter, alongside grid) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(harvest_info) = harvest {
            self.build_harvest_indicators(&harvest_info.indicators, content_top + padding);
        }

        // ── LAYER 2: Harvest cellar panel (slide-in overlay) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(harvest_info) = harvest {
            if let Some(ref cellar) = harvest_info.cellar {
                self.build_root_cellar(cellar, window_width, window_height);
            }
        }

        // ── LAYER 3: Harvest sprout menu (context menu overlay) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(harvest_info) = harvest {
            if let Some(ref menu) = harvest_info.sprout_menu {
                self.build_sprout_menu(menu);
            }
        }

        // ── LAYER 4: Search bar (overlays grid content) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(search_info) = search_bar {
            self.build_search_bar(search_info, window_width, tab_bar_height);
        }

        // ── LAYER 5: Git completion dropdown (overlays grid) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.git_completions.clone() {
            self.build_git_completions(info, cursor_pos, window_width, window_height);
        }

        // ── LAYER 8: Session dropdown (on top of everything) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(sd) = session_dropdown {
            self.build_session_dropdown(sd, window_width);
        }

        // ── LAYER 9: Session management panel (topmost overlay) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(sp) = session_panel {
            if sp.animation > 0.001 {
                self.build_session_management_panel(sp, window_width, window_height);
            }
        }

        // ── LAYER 10: Commands panel ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(cp) = commands_panel {
            if cp.animation > 0.001 {
                self.build_commands_panel(cp, window_width, window_height);
            }
        }

        // ── LAYER 10.5: Settings panel (centered modal) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(sp) = settings_panel {
            if sp.animation > 0.001 {
                self.build_settings_panel(sp, window_width, window_height);
            }
        }

        // ── LAYER 10.6: Settings dropdown overlay (on top of settings panel) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(sp) = settings_panel {
            if sp.animation > 0.001 && sp.dropdown_open {
                self.build_dropdown_overlay(sp, window_width, window_height);
            }
        }

        // ── LAYER 11: Command palette (topmost overlay) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(palette) = command_palette {
            if palette.animation > 0.001 {
                self.build_command_palette(palette, window_width, window_height);
            }
        }

        // ── LAYER 12: Shortcuts panel (centered modal, same tier as palette) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(sp) = shortcuts_panel {
            if sp.animation > 0.001 {
                self.build_shortcuts_panel(sp, window_width, window_height);
            }
        }

        // ── LAYER 13: Info overlay (above everything) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.info_overlay.clone() {
            if info.animation > 0.001 {
                self.build_info_overlay(info, window_width, window_height);
            }
        }

        // ── LAYER 13: Diff review overlay (topmost) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.diff_review.clone() {
            if info.animation > 0.001 {
                self.build_diff_review(info, window_width, window_height);
            }
        }

        // ── LAYER 14: Context picker overlay ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.context_picker.clone() {
            if info.animation > 0.001 {
                self.build_context_picker(info, window_width, window_height);
            }
        }

        // ── LAYER 15: Guardrail confirmation overlay (absolute topmost) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(ref info) = self.guardrail.clone() {
            if info.animation > 0.001 {
                self.build_guardrail_confirmation(info, window_width, window_height);
            }
        }

        // ── LAYER 16: Toasts (always on top — transient status notices) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if !self.toasts.is_empty() {
            let toasts = self.toasts.clone();
            self.build_toasts(&toasts, window_width, window_height);
        }
    }

    /// Render a single terminal grid within a pane viewport.
    ///
    /// This method handles cell rendering, cursor, scrollbar, selection, search
    /// highlights, underlines, and URL detection for one grid. It is called once
    /// in single-grid mode or once per pane in multi-pane mode.
    #[allow(clippy::too_many_arguments)]
    fn build_pane_grid(
        &mut self,
        grid: &Grid,
        selection: Option<&Selection>,
        offset_x: f32,
        offset_y: f32,
        pane_cols: usize,
        pane_rows: usize,
        is_focused: bool,
        cursor_style: CursorStyle,
        cursor_opacity: f32,
        cursor_pos: (f32, f32),
        copy_flash: f32,
        brightness: f32,
        search: &Search,
        window_height: f32,
        pane_width: f32,
    ) {
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        // Selection highlight color
        let selection_base = self.ui.selection;
        let selection_flash = self.ui.selection_flash;
        let selection_color =
            apply_brightness(mix(selection_base, selection_flash, copy_flash), brightness);

        let scroll_offset = grid.scroll_offset();
        let scroll_offset_i32 = i32::try_from(scroll_offset).unwrap_or(i32::MAX);
        let default_bg = TermColor::default_bg();

        let max_grid_y = window_height - self.padding;

        // Determine which rows to render based on filter mode
        let filter_mode = search.is_filter_mode() && !search.matching_rows().is_empty();
        let matching_rows = search.matching_rows();

        let rows_to_render: Vec<(usize, i32)> = if filter_mode {
            matching_rows
                .iter()
                .enumerate()
                .map(|(visual_row, &grid_row)| (visual_row, grid_row))
                .collect()
        } else {
            (0..pane_rows.min(grid.rows()))
                .map(|row| (row, row as i32 - scroll_offset_i32))
                .collect()
        };

        for (visual_row, grid_row) in rows_to_render {
            let row_data = if filter_mode {
                grid.row_at_offset(grid_row)
            } else {
                None
            };

            for col in 0..pane_cols.min(grid.cols()) {
                let cell = if filter_mode {
                    row_data.and_then(|r| r.get(col))
                } else {
                    grid.cell_scrolled(visual_row, col)
                };
                let Some(cell) = cell else { continue };

                let x = offset_x + col as f32 * cell_width;
                let y = offset_y + visual_row as f32 * cell_height;

                if y + cell_height > max_grid_y {
                    break;
                }

                let is_selected = is_focused
                    && selection
                        .map(|s| s.contains(grid_row, col))
                        .unwrap_or(false);

                let is_search_match = search.is_match(grid_row, col);
                let is_current_match = search.is_current_match(grid_row, col);

                let (fg, bg) = cell.apply_inverse();

                // Draw background
                if is_selected {
                    self.rect_pipeline
                        .push_rect(x, y, cell_width, cell_height, selection_color);
                } else if is_current_match {
                    self.rect_pipeline.push_rect(
                        x,
                        y,
                        cell_width,
                        cell_height,
                        apply_brightness(self.ui.search_match_current, brightness),
                    );
                } else if is_search_match {
                    self.rect_pipeline.push_rect(
                        x,
                        y,
                        cell_width,
                        cell_height,
                        apply_brightness(self.ui.search_match, brightness),
                    );
                } else if bg != default_bg {
                    self.rect_pipeline.push_rect(
                        x,
                        y,
                        cell_width,
                        cell_height,
                        [
                            bg.r as f32 / 255.0 * brightness,
                            bg.g as f32 / 255.0 * brightness,
                            bg.b as f32 / 255.0 * brightness,
                            1.0,
                        ],
                    );
                }

                // Draw character
                if let Some(c) = cell.content.char() {
                    if !c.is_whitespace() {
                        let text_color = if is_selected {
                            if let Some(sel_text) = self.ui.selection_text {
                                apply_brightness(sel_text, brightness)
                            } else {
                                [
                                    fg.r as f32 / 255.0 * brightness,
                                    fg.g as f32 / 255.0 * brightness,
                                    fg.b as f32 / 255.0 * brightness,
                                    1.0,
                                ]
                            }
                        } else {
                            [
                                fg.r as f32 / 255.0 * brightness,
                                fg.g as f32 / 255.0 * brightness,
                                fg.b as f32 / 255.0 * brightness,
                                1.0,
                            ]
                        };
                        self.text_pipeline.push_char(c, x, y, text_color);
                    }
                }

                // Draw underline styles
                if cell.attrs.has_underline() {
                    let underline_color = cell.underline_color.unwrap_or(cell.fg);
                    let ul_color = apply_brightness(
                        [
                            underline_color.r as f32 / 255.0,
                            underline_color.g as f32 / 255.0,
                            underline_color.b as f32 / 255.0,
                            1.0,
                        ],
                        brightness,
                    );

                    let underline_y = y + cell_height - 2.0;
                    let underline_thickness = 1.0;

                    if cell.attrs.contains(CellAttrs::CURLY_UNDERLINE) {
                        self.draw_wavy_underline(x, underline_y, cell_width, ul_color);
                    } else if cell.attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
                        self.rect_pipeline.push_rect(
                            x,
                            underline_y - 2.0,
                            cell_width,
                            underline_thickness,
                            ul_color,
                        );
                        self.rect_pipeline.push_rect(
                            x,
                            underline_y,
                            cell_width,
                            underline_thickness,
                            ul_color,
                        );
                    } else if cell.attrs.contains(CellAttrs::DOTTED_UNDERLINE) {
                        self.draw_dotted_underline(x, underline_y, cell_width, ul_color);
                    } else if cell.attrs.contains(CellAttrs::DASHED_UNDERLINE) {
                        self.draw_dashed_underline(x, underline_y, cell_width, ul_color);
                    } else {
                        self.rect_pipeline.push_rect(
                            x,
                            underline_y,
                            cell_width,
                            underline_thickness,
                            ul_color,
                        );
                    }
                }

                // Draw hyperlink underline
                let is_hyperlink = cell.flags.contains(CellFlags::HYPERLINK);
                let is_detected_url = self
                    .url_cache
                    .as_ref()
                    .map_or(false, |c| c.is_url(visual_row, col));
                if (is_hyperlink || is_detected_url) && !cell.attrs.has_underline() {
                    let link_color = apply_brightness(self.ui.url, brightness);
                    let underline_y = y + cell_height - 2.0;
                    self.draw_dotted_underline(x, underline_y, cell_width, link_color);
                }
            }
        }

        // Draw cursor (only for focused pane, not scrolled back, not in filter mode)
        if is_focused && cursor_opacity > 0.0 && scroll_offset == 0 && !filter_mode {
            let cursor = grid.cursor();
            if cursor.visible {
                let x = offset_x + cursor_pos.0 * cell_width;
                let y = offset_y + cursor_pos.1 * cell_height;

                if y + cell_height <= max_grid_y {
                    let cursor_color = apply_brightness(self.ui.cursor, brightness);
                    let cc = [
                        cursor_color[0],
                        cursor_color[1],
                        cursor_color[2],
                        cursor_opacity,
                    ];

                    match cursor_style {
                        CursorStyle::Block => {
                            self.rect_pipeline.push_rounded_rect(
                                x,
                                y,
                                cell_width,
                                cell_height,
                                cc,
                                2.0,
                            );
                            let cursor_row = cursor.row as usize;
                            let cursor_col = cursor.col as usize;
                            if let Some(cell) = grid.cell_scrolled(cursor_row, cursor_col) {
                                if let Some(c) = cell.content.char() {
                                    if !c.is_whitespace() {
                                        let ct = apply_brightness(self.ui.cursor_text, brightness);
                                        self.text_pipeline.push_char(
                                            c,
                                            x,
                                            y,
                                            [ct[0], ct[1], ct[2], cursor_opacity],
                                        );
                                    }
                                }
                            }
                        }
                        CursorStyle::Underline => {
                            let underline_height = 2.0_f32.max(cell_height * 0.1);
                            self.rect_pipeline.push_rect(
                                x,
                                y + cell_height - underline_height,
                                cell_width,
                                underline_height,
                                cc,
                            );
                        }
                        CursorStyle::Beam => {
                            let beam_width = 2.0_f32.max(cell_width * 0.1);
                            self.rect_pipeline
                                .push_rect(x, y, beam_width, cell_height, cc);
                        }
                    }
                }
            }
        }

        // Draw scrollbar (only when there is scrollback content)
        let scrollback_len = grid.scrollback_len();
        if scrollback_len > 0 {
            let visible_rows = pane_rows.min(grid.rows());
            let total_rows = scrollback_len + visible_rows;

            let scrollbar_width = 6.0_f32;
            let scrollbar_margin = 2.0_f32;
            let scrollbar_x = offset_x + pane_width - scrollbar_width - scrollbar_margin;
            let scrollbar_top = offset_y;
            let scrollbar_bottom = offset_y + pane_rows as f32 * cell_height;
            let scrollbar_height = scrollbar_bottom - scrollbar_top;

            if scrollbar_height > 20.0 {
                let track_color = [1.0_f32, 1.0, 1.0, 0.05];
                self.rect_pipeline.push_rounded_rect(
                    scrollbar_x,
                    scrollbar_top,
                    scrollbar_width,
                    scrollbar_height,
                    track_color,
                    3.0,
                );

                // Minimum thumb size of 32px keeps it targetable on HiDPI
                // displays even when scrollback vastly exceeds the viewport.
                let thumb_ratio = (visible_rows as f32) / (total_rows as f32);
                let thumb_height = (scrollbar_height * thumb_ratio)
                    .max(32.0)
                    .min(scrollbar_height);
                let max_offset = scrollback_len;
                let scroll_fraction = if max_offset > 0 {
                    scroll_offset as f32 / max_offset as f32
                } else {
                    0.0
                };
                let thumb_travel = scrollbar_height - thumb_height;
                let thumb_y = scrollbar_top + thumb_travel * (1.0 - scroll_fraction);

                let thumb_color = [1.0_f32, 1.0, 1.0, 0.25];
                self.rect_pipeline.push_rounded_rect(
                    scrollbar_x,
                    thumb_y,
                    scrollbar_width,
                    thumb_height,
                    thumb_color,
                    3.0,
                );
            }
        }
    }
}
