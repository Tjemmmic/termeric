//! GPU rendering pipeline using wgpu
//!
//! This module handles all GPU-accelerated rendering including:
//! - Text glyph rendering with texture atlas
//! - Background rectangle rendering
//! - Cursor rendering
//! - Tab bar rendering

mod agent_view;
mod background_image;
mod glyph_cache;
mod image;
#[cfg(test)]
mod image_crop_smoke;
mod ime;
mod rect;
mod row_cache;
mod style;
mod text;

#[cfg(test)]
mod benchmark;
mod buffer;
mod colors;
mod cursor;
mod device;
mod drag_picture;
pub(crate) mod draw;
mod frame;
pub use frame::{ChromeFrame, Frame};
mod git;
mod grid;
mod harvest;
#[cfg(test)]
mod headless_tests;
mod icons;
mod overlays;
mod palette;
mod panels;
mod popup_menu;
mod quick_select;
mod search_bar;
mod settings_render;
mod shortcuts_render;
mod tab_bar;
mod toasts;
pub mod types;
mod ui_text;

pub use popup_menu::PopupMenuInfo;
pub use types::*;

#[cfg(test)]
pub(crate) use background_image::headless_background_render_smoke;
pub(crate) use background_image::{
    BackgroundImageStamp, DecodedBackgroundImage, load_background_image,
    resolve_background_image_path,
};
pub use glyph_cache::GlyphCache;
pub(crate) use glyph_cache::{MAX_FONT_COVERAGE_GRAPHEMES, MAX_FONT_COVERAGE_INPUT_BYTES};

#[cfg(test)]
pub(crate) use benchmark::benchmark_headless_renderer;
pub(crate) use colors::{ENCODED_RGBA_FORMAT, UiPalette};
use colors::{TerminalPalette, contrast_text, select_surface_formats};
#[cfg(test)]
pub(crate) use device::discover_adapter;
pub(crate) use device::{RendererDeviceFailure, RendererDiagnostics, RendererFailureCallback};
use device::{request_adapter_device, select_adapter};
use grid::{cursor_cell_geometry, draws_char, draws_grapheme, gliding_cursor_origin};
#[cfg(test)]
pub(crate) use headless_tests::{
    headless_color_emoji_render_smoke, headless_glyph_atlas_stress_smoke,
    headless_gui_visual_snapshot, headless_image_render_smoke, headless_ligature_render_smoke,
    headless_palette_input_smoke, headless_sgr_render_smoke, headless_theme_color_fidelity_smoke,
    headless_ui_text_smoke, headless_unicode_render_smoke,
};

use image::{ImagePipeline, presentation_geometry};

use crate::config::theme::Theme;
use crate::config::{CursorStyle, RendererBackend, TabBarPosition};
use crate::terminal::search::Search;
use crate::terminal::url::UrlCache;
use crate::ui::color::{contrast_ratio, high_contrast_text, luminance, mix, scale_rgb, with_alpha};
use crate::ui::search_bar::SearchBarInfo;
use crate::ui::settings::SettingsPanelInfo;
use anyhow::Result;
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
#[cfg(test)]
use termeric_core::CellAttrs;
use termeric_core::grid::ColorSpec;
use termeric_core::{CellContent, CellFlags, Color as TermColor, Grid, Selection};
use wgpu::*;

use winit::window::Window;

use rect::RectPipeline;
use row_cache::{RowCacheKey, TerminalRowCache};
use style::ResolvedCellStyle;
use text::{TextClip, TextPipeline, TextRunAccumulator};

use crate::tabs::TabInfo;
use crate::ui::command_palette::PaletteInfo;
use crate::ui::shortcuts_panel::ShortcutsPanelInfo;

/// Logical height of the tab bar, sized for 10pt UI labels.
const TAB_BAR_HEIGHT: f32 = 34.0;

/// Desktop reference density: a point is 1/72 inch and a logical pixel 1/96
/// inch, so configured sizes match other applications' point sizes.
const PIXELS_PER_POINT: f32 = 96.0 / 72.0;

/// Physical rasterization size for a configured `font.size` in points.
fn physical_font_size(points: f32, scale_factor: f32) -> f32 {
    (points * PIXELS_PER_POINT * scale_factor).max(1.0)
}

#[cfg(test)]
#[test]
fn configured_points_convert_to_physical_pixels() {
    assert_eq!(physical_font_size(12.0, 1.0), 16.0);
    assert_eq!(physical_font_size(12.0, 2.0), 32.0);
    assert_eq!(physical_font_size(9.0, 1.25), 15.0);
    assert_eq!(physical_font_size(0.1, 1.0), 1.0);
}

/// Main renderer that manages all GPU resources
pub struct Renderer {
    window: Arc<Window>,
    surface: Surface<'static>,
    device: Device,
    queue: Queue,
    diagnostics: RendererDiagnostics,
    config: SurfaceConfiguration,
    /// Non-sRGB view format of the surface texture; see [`ENCODED_RGBA_FORMAT`].
    render_format: TextureFormat,
    size: (u32, u32),
    // Rendering pipelines
    text_pipeline: TextPipeline,
    rect_pipeline: RectPipeline,
    background_image_pipeline: ImagePipeline,
    image_pipeline: ImagePipeline,
    background_image: background_image::BackgroundImageState,
    terminal_row_cache: TerminalRowCache,
    // Glyph cache
    glyph_cache: GlyphCache,
    // Padding around terminal content
    padding: f32,
    padding_logical: f32,
    scale_factor: f32,
    font_size: f32,
    // Theme-driven UI palette
    ui: UiPalette,
    high_contrast: bool,
    // Theme-resolved terminal defaults and ANSI 0-15 colors
    terminal_palette: TerminalPalette,
    // Explicit compatibility policy for SGR bold + ANSI colors 0-7.
    bold_is_bright: bool,
    // Blink rates present in the terminal rows rendered by the last frame.
    visible_text_blink: VisibleTextBlink,
    // Window opacity (0.0 - 1.0)
    opacity: f32,
    // Tab bar hover state for icon animations
    tab_bar_hover: Option<TabBarClick>,
    tab_bar_hover_progress: f32,
    tab_drag: TabDragVisual,
    /// Where this frame drew the dragged tab, lifted over the others.
    dragged_tab_surface: Option<crate::ui::geometry::Bounds>,
    /// The dragged tab copied from the last frame that asked for it.
    dragged_tab_picture: Option<DraggedTabPicture>,
    /// The surface can be copied from, so the dragged tab can be pictured.
    surface_readable: bool,
    tab_bar_keyboard_focus: Option<TabBarClick>,
    tab_bar_viewport_anchor: tab_bar::TabBarViewportAnchor,
    tab_bar_position: TabBarPosition,
    // Git completion overlay state
    git_completions: Option<GitCompletionInfo>,
    /// The docked agent view's scene, drawn right of the terminals.
    agent_view: Option<std::sync::Arc<crate::ui::agent_view::scene::Scene>>,
    /// Width reserved on the right for the agent view.
    dock_width: f32,
    // Bounded information/error/choice overlay state
    info_overlay: Option<InfoOverlayInfo>,
    diff_review: Option<DiffReviewInfo>,
    context_picker: Option<ContextPickerInfo>,
    guardrail: Option<GuardrailInfo>,
    /// Transient toast notifications for the current frame
    toasts: Vec<crate::ui::toast::ToastInfo>,
    /// Visual bell intensity (0.0 = off, 1.0 = full flash)
    bell_intensity: f32,
}

impl Renderer {
    /// Create a new renderer for the given window
    pub fn new(
        window: Arc<Window>,
        font_size: f32,
        font_family: &str,
        fallback_families: &[String],
        range_overrides: &[crate::config::FontRangeOverride],
        font_weight: u16,
        bold_font_weight: u16,
        line_height_multiplier: f32,
        padding: f32,
        scale_factor: f32,
        theme: &Theme,
        high_contrast: bool,
        ligatures: bool,
        font_features: &BTreeMap<String, u32>,
        backend_preference: RendererBackend,
        failure_callback: Option<RendererFailureCallback>,
    ) -> Result<Self> {
        let size = window.inner_size();

        // Create wgpu instance
        let instance_descriptor = InstanceDescriptor::from_env_or_default();
        let enabled_backends = instance_descriptor.backends;
        let instance = Instance::new(&instance_descriptor);

        // Create surface - window is Arc so it lives long enough
        let surface = instance.create_surface(window.clone())?;

        let ((adapter, device, queue), fallback_requested) =
            select_adapter(backend_preference, |force_fallback| {
                request_adapter_device(&instance, enabled_backends, &surface, force_fallback)
            })?;
        let diagnostics =
            RendererDiagnostics::new(backend_preference, &adapter.get_info(), fallback_requested);
        if diagnostics.software {
            log::warn!(
                "Using software renderer; performance will be lower ({})",
                diagnostics.summary()
            );
        } else {
            log::info!("Renderer diagnostics: {}", diagnostics.summary());
        }
        if let Some(callback) = failure_callback {
            device.set_device_lost_callback(move |reason, message| {
                callback(RendererDeviceFailure { reason, message });
            });
        }

        // Configure surface
        let surface_caps = surface.get_capabilities(&adapter);
        let formats = select_surface_formats(&surface_caps.formats)
            .ok_or_else(|| anyhow::anyhow!("selected adapter exposes no presentation format"))?;
        let alpha_mode =
            if surface_caps
                .alpha_modes
                .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
            {
                wgpu::CompositeAlphaMode::PreMultiplied
            } else {
                surface_caps.alpha_modes.first().copied().ok_or_else(|| {
                    anyhow::anyhow!("selected adapter exposes no surface alpha mode")
                })?
            };

        // Reading frames back lets a dragged tab leave the window as a
        // picture; without it the pointer only changes shape.
        let surface_readable = surface_caps.usages.contains(TextureUsages::COPY_SRC);
        let config = SurfaceConfiguration {
            usage: if surface_readable {
                TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC
            } else {
                TextureUsages::RENDER_ATTACHMENT
            },
            format: formats.surface,
            width: size.width,
            height: size.height,
            present_mode: PresentMode::Fifo,
            alpha_mode,
            view_formats: if formats.render == formats.surface {
                vec![]
            } else {
                vec![formats.render]
            },
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Create glyph cache
        let scaled_font_size = physical_font_size(font_size, scale_factor);
        let mut glyph_cache = GlyphCache::new(
            &device,
            &queue,
            scaled_font_size,
            font_family,
            fallback_families,
            range_overrides,
            font_weight,
            bold_font_weight,
            line_height_multiplier,
            ligatures,
        )?;
        glyph_cache.set_font_features(font_features);

        // Create pipelines
        let text_pipeline = TextPipeline::new(&device, formats.render, &glyph_cache)?;
        let rect_pipeline = RectPipeline::new(&device, formats.render)?;
        let background_image_pipeline = ImagePipeline::new(&device, formats.render)?;
        let image_pipeline = ImagePipeline::new(&device, formats.render)?;

        let padding_physical = (padding * scale_factor).round().max(0.0);
        Ok(Self {
            window,
            surface,
            device,
            queue,
            diagnostics,
            config,
            render_format: formats.render,
            size: (size.width, size.height),
            text_pipeline,
            rect_pipeline,
            background_image_pipeline,
            image_pipeline,
            background_image: background_image::BackgroundImageState::default(),
            terminal_row_cache: TerminalRowCache::default(),
            glyph_cache,
            padding: padding_physical,
            padding_logical: padding,
            scale_factor,
            font_size,
            ui: UiPalette::from_theme_with_contrast(theme, high_contrast),
            high_contrast,
            terminal_palette: TerminalPalette::from_theme(theme),
            bold_is_bright: false,
            visible_text_blink: VisibleTextBlink::default(),
            opacity: 1.0,
            tab_bar_hover: None,
            tab_bar_hover_progress: 0.0,
            tab_drag: TabDragVisual::default(),
            dragged_tab_surface: None,
            dragged_tab_picture: None,
            surface_readable,
            tab_bar_keyboard_focus: None,
            tab_bar_viewport_anchor: tab_bar::TabBarViewportAnchor::default(),
            tab_bar_position: TabBarPosition::Top,
            git_completions: None,
            agent_view: None,
            dock_width: 0.0,
            info_overlay: None,
            diff_review: None,
            context_picker: None,
            guardrail: None,
            toasts: Vec::new(),
            bell_intensity: 0.0,
        })
    }

    pub(crate) fn diagnostics(&self) -> &RendererDiagnostics {
        &self.diagnostics
    }

    /// Resize the renderer
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            if self.size != (width, height) {
                self.invalidate_terminal_rows();
            }
            self.size = (width, height);
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);

            // Update uniforms
            self.text_pipeline.resize(&self.queue, width, height);
            self.rect_pipeline.resize(&self.queue, width, height);
            self.background_image_pipeline
                .resize(&self.queue, width, height);
            self.image_pipeline.resize(&self.queue, width, height);
        }
    }

    /// Display scale factor the renderer lays out chrome for.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// Cell width and height in physical pixels.
    pub fn cell_size(&self) -> (f32, f32) {
        self.glyph_cache.cell_size()
    }

    /// Calculate terminal dimensions from the physical window viewport.
    pub(crate) fn calculate_grid_geometry(
        &self,
        width: u32,
        height: u32,
    ) -> crate::terminal::geometry::TerminalGeometry {
        crate::terminal::geometry::TerminalGeometry::from_window_pixels(
            width,
            height,
            self.padding,
            self.tab_bar_width_for(width as f32)
                + (width as f32 - self.content_right_for(width as f32)),
            self.tab_bar_height(),
            self.cell_width(),
            self.cell_height(),
        )
    }

    /// The terminal area of a `width` by `height` window: its grid geometry
    /// and the cell size it was measured with.
    pub(crate) fn terminal_area(
        &self,
        width: u32,
        height: u32,
    ) -> crate::terminal::geometry::TerminalArea {
        crate::terminal::geometry::TerminalArea::new(
            self.calculate_grid_geometry(width, height),
            self.cell_width(),
            self.cell_height(),
        )
    }

    /// Where the terminal area sits in the window, which places its panes.
    pub(crate) fn pane_placement(&self) -> PanePlacement {
        PanePlacement {
            left: self.tab_bar_width(),
            top: self.content_top(),
            right: self.content_right(),
            bottom: self.size.1 as f32,
            padding: self.padding,
        }
    }

    /// Calculate terminal cell dimensions from window size.
    pub fn calculate_grid_size(&self, width: u32, height: u32) -> (usize, usize) {
        let geometry = self.calculate_grid_geometry(width, height);
        (geometry.cols, geometry.rows)
    }

    /// Get the tab bar height
    pub fn tab_bar_height(&self) -> f32 {
        (TAB_BAR_HEIGHT * self.scale_factor).round()
    }

    /// Change font size
    pub fn set_font_size(&mut self, font_size: f32) {
        self.invalidate_terminal_rows();
        self.font_size = font_size;
        let scaled = physical_font_size(font_size, self.scale_factor);
        self.glyph_cache.set_font_size(scaled, &self.queue);
    }

    /// Apply a primary font and ordered configured fallbacks.
    pub fn set_font_families(
        &mut self,
        primary: String,
        fallbacks: &[String],
        range_overrides: &[crate::config::FontRangeOverride],
    ) {
        self.invalidate_terminal_rows();
        self.glyph_cache
            .set_font_families(primary, fallbacks, range_overrides, &self.queue);
    }

    /// Change the configured line-height multiplier.
    pub fn set_line_height(&mut self, multiplier: f32) {
        self.invalidate_terminal_rows();
        self.glyph_cache.set_line_height(multiplier, &self.queue);
    }

    /// Apply the normal and SGR-bold OpenType `wght` requests.
    pub fn set_font_weights(&mut self, font_weight: u16, bold_font_weight: u16) {
        if self
            .glyph_cache
            .set_font_weights(font_weight, bold_font_weight, &self.queue)
        {
            self.invalidate_terminal_rows();
        }
    }

    /// Get all installed monospace font families
    pub fn get_monospace_fonts(&self) -> Vec<String> {
        self.glyph_cache.get_monospace_fonts()
    }

    /// Build a bounded read-only report from the same family-selection state
    /// used by terminal rendering.
    pub(crate) fn inspect_font_coverage(
        &mut self,
        configured_primary: &str,
        text: &str,
    ) -> std::result::Result<FontCoverageReport, String> {
        self.glyph_cache
            .inspect_font_coverage(configured_primary, text)
    }

    /// Enable or disable ligature rendering
    pub fn set_ligatures(&mut self, enabled: bool) {
        if self.glyph_cache.ligatures_enabled() != enabled {
            self.invalidate_terminal_rows();
            self.glyph_cache.set_ligatures(enabled);
        }
    }

    /// Replace explicit OpenType shaping feature values.
    pub fn set_font_features(&mut self, features: &BTreeMap<String, u32>) {
        if self.glyph_cache.set_font_features(features) {
            self.invalidate_terminal_rows();
        }
    }

    /// Set the explicit bold-bright compatibility policy.
    pub fn set_bold_is_bright(&mut self, enabled: bool) {
        if self.bold_is_bright != enabled {
            self.bold_is_bright = enabled;
            self.invalidate_terminal_rows();
        }
    }

    /// Blink rates found in visible terminal cells during the last frame.
    pub fn visible_text_blink(&self) -> VisibleTextBlink {
        self.visible_text_blink
    }

    /// Get current padding
    pub fn padding(&self) -> f32 {
        self.padding
    }

    /// Get current window size in physical pixels
    pub fn window_size(&self) -> (u32, u32) {
        self.size
    }

    /// Cell width in physical pixels.
    pub fn cell_width(&self) -> f32 {
        self.glyph_cache.cell_size().0
    }

    /// Cell height in physical pixels.
    pub fn cell_height(&self) -> f32 {
        self.glyph_cache.cell_size().1
    }

    /// Change padding
    pub fn set_padding(&mut self, padding: f32) {
        self.invalidate_terminal_rows();
        self.padding_logical = padding.max(0.0);
        self.padding = (self.padding_logical * self.scale_factor).round().max(0.0);
    }

    /// Update scale factor for HiDPI displays
    pub fn set_scale_factor(&mut self, scale_factor: f32) {
        self.invalidate_terminal_rows();
        self.scale_factor = scale_factor.max(0.1);
        let scaled = physical_font_size(self.font_size, self.scale_factor);
        self.glyph_cache.set_font_size(scaled, &self.queue);
        self.padding = (self.padding_logical * self.scale_factor).round().max(0.0);
    }

    /// Set the active theme for UI rendering
    pub fn set_theme(&mut self, theme: &Theme) {
        self.invalidate_terminal_rows();
        self.ui = UiPalette::from_theme_with_contrast(theme, self.high_contrast);
        self.terminal_palette = TerminalPalette::from_theme(theme);
    }

    /// Apply a stronger application-chrome palette without rewriting colors
    /// emitted by terminal applications.
    pub fn set_high_contrast(&mut self, theme: &Theme, enabled: bool) {
        if self.high_contrast != enabled {
            self.high_contrast = enabled;
            self.ui = UiPalette::from_theme_with_contrast(theme, enabled);
        }
    }
    /// Set window opacity (1.0 = fully opaque), held to the range the file allows.
    /// Set window opacity (0.0 = fully transparent, 1.0 = fully opaque)
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(
            *crate::config::WINDOW_OPACITY_RANGE.start(),
            *crate::config::WINDOW_OPACITY_RANGE.end(),
        );
    }

    fn invalidate_terminal_rows(&mut self) {
        self.terminal_row_cache.clear();
    }

    /// Update tab bar hover state for icon animations
    pub fn set_tab_bar_hover(&mut self, hover: Option<TabBarClick>, progress: f32) {
        self.tab_bar_hover = hover;
        self.tab_bar_hover_progress = progress;
    }

    /// The tab being dragged and the tabs sliding into place.
    pub fn set_tab_drag(&mut self, visual: TabDragVisual) {
        self.tab_drag = visual;
    }

    pub fn set_tab_bar_keyboard_focus(&mut self, focus: Option<TabBarClick>) {
        self.tab_bar_keyboard_focus = focus;
        self.tab_bar_viewport_anchor.set_focus(focus);
    }

    pub fn set_tab_bar_position(&mut self, position: TabBarPosition) {
        if self.tab_bar_position != position {
            self.tab_bar_position = position;
            self.invalidate_terminal_rows();
        }
    }

    pub fn tab_bar_position(&self) -> TabBarPosition {
        self.tab_bar_position
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

    pub fn set_bell_intensity(&mut self, intensity: f32) {
        self.bell_intensity = intensity;
    }

    /// Top edge of the terminal area, directly below the tab bar. Overlays
    /// such as the find box float above it and never move it, so the grid
    /// size computed from the window always matches what is drawn.
    pub fn content_top(&self) -> f32 {
        self.tab_bar_height()
    }
}
