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

mod buffer;
mod cursor;
mod drag_picture;
pub(crate) mod draw;
mod git;
mod harvest;
mod icons;
pub use harvest::sprout_menu_hit_test;
mod overlays;
mod palette;
#[cfg(test)]
pub(crate) use palette::headless_palette_input_smoke;
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

use image::{ImagePipeline, presentation_geometry};

use crate::config::theme::Theme;
use crate::config::{CursorStyle, RendererBackend, TabBarPosition};
use crate::terminal::search::Search;
use crate::terminal::url::UrlCache;
use crate::ui::color::{luminance, mix, scale_rgb, with_alpha};
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

fn hash_text_clip<H: Hasher>(clip: TextClip, hasher: &mut H) {
    clip.left.to_bits().hash(hasher);
    clip.top.to_bits().hash(hasher);
    clip.right.to_bits().hash(hasher);
    clip.bottom.to_bits().hash(hasher);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RendererDeviceFailure {
    pub reason: DeviceLostReason,
    pub message: String,
}

pub(crate) type RendererFailureCallback =
    Arc<dyn Fn(RendererDeviceFailure) + Send + Sync + 'static>;

/// Stable adapter information included in startup and recovery diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RendererDiagnostics {
    pub preference: RendererBackend,
    pub adapter: String,
    pub backend: String,
    pub device_type: String,
    pub software: bool,
}

impl RendererDiagnostics {
    pub(crate) fn summary(&self) -> String {
        format!(
            "preference={} adapter={:?} backend={} device_type={} software={}",
            self.preference, self.adapter, self.backend, self.device_type, self.software
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AdapterAttempt {
    force_fallback: bool,
    label: &'static str,
}

impl AdapterAttempt {
    fn accepts(self, device_type: DeviceType) -> bool {
        // `force_fallback_adapter: false` is a preference, not a guarantee:
        // WGPU may return a CPU adapter when no GPU is available. Reject that
        // candidate so `gpu` remains strict and `auto` advances to its explicit
        // software attempt instead of reporting a CPU adapter as hardware.
        self.force_fallback || device_type != DeviceType::Cpu
    }
}

fn adapter_attempts(preference: RendererBackend) -> &'static [AdapterAttempt] {
    const AUTO: &[AdapterAttempt] = &[
        AdapterAttempt {
            force_fallback: false,
            label: "hardware",
        },
        AdapterAttempt {
            force_fallback: true,
            label: "software fallback",
        },
    ];
    const GPU: &[AdapterAttempt] = &[AdapterAttempt {
        force_fallback: false,
        label: "hardware",
    }];
    const SOFTWARE: &[AdapterAttempt] = &[AdapterAttempt {
        force_fallback: true,
        label: "software fallback",
    }];
    match preference {
        RendererBackend::Auto => AUTO,
        RendererBackend::Gpu => GPU,
        RendererBackend::Software => SOFTWARE,
    }
}

fn select_adapter<T>(
    preference: RendererBackend,
    mut request: impl FnMut(AdapterAttempt) -> std::result::Result<Option<T>, String>,
) -> std::result::Result<(T, AdapterAttempt), Vec<String>> {
    let mut failures = Vec::new();
    for attempt in adapter_attempts(preference).iter().copied() {
        match request(attempt) {
            Ok(Some(selected)) => return Ok((selected, attempt)),
            Ok(None) => failures.push(format!("{} adapter was unavailable", attempt.label)),
            Err(error) => failures.push(format!("{} adapter failed: {error}", attempt.label)),
        }
    }
    Err(failures)
}

/// Ask WGPU for the requested adapter class and recover from backends that
/// enumerate a CPU adapter but do not return it for `force_fallback_adapter`.
/// The enumeration path is deliberately CPU-only so strict software mode can
/// never become an undocumented hardware selection.
pub(crate) fn discover_adapter(
    instance: &Instance,
    backends: Backends,
    compatible_surface: Option<&Surface<'_>>,
    power_preference: PowerPreference,
    force_fallback_adapter: bool,
) -> Option<Adapter> {
    let requested = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference,
        compatible_surface,
        force_fallback_adapter,
    }));
    if requested.is_some() || !force_fallback_adapter {
        return requested;
    }

    instance
        .enumerate_adapters(backends)
        .into_iter()
        .find(|adapter| {
            is_enumerated_software_candidate(
                adapter.get_info().device_type,
                compatible_surface.is_none_or(|surface| adapter.is_surface_supported(surface)),
            )
        })
}

fn is_enumerated_software_candidate(device_type: DeviceType, surface_supported: bool) -> bool {
    device_type == DeviceType::Cpu && surface_supported
}

#[derive(Clone)]
pub(crate) struct UiPalette {
    pub(crate) background: wgpu::Color,
    pub(crate) selection: [f32; 4],
    pub(crate) selection_flash: [f32; 4],
    pub(crate) selection_text: Option<[f32; 4]>,
    pub(crate) cursor: [f32; 4],
    pub(crate) cursor_text: [f32; 4],
    /// Chrome strip behind the tabs, a step off the terminal background.
    pub(crate) tab_bar_bg: [f32; 4],
    /// Raised surface of the active tab.
    pub(crate) tab_active: [f32; 4],
    /// Hover surface of inactive tabs and toolbar buttons.
    pub(crate) tab_inactive: [f32; 4],
    /// Theme accent for the active-tab indicator.
    pub(crate) tab_accent: [f32; 4],
    // Popup menus
    pub(crate) menu_bg: [f32; 4],
    pub(crate) menu_border: [f32; 4],
    pub(crate) menu_highlight: [f32; 4],
    pub(crate) menu_text: [f32; 4],
    pub(crate) menu_text_dim: [f32; 4],
    pub(crate) tab_activity: [f32; 4],
    /// Status hues from the terminal's own palette (yellow, red, green), for
    /// agent badges, toasts, diffs, and warnings.
    pub(crate) warning: [f32; 4],
    pub(crate) error: [f32; 4],
    pub(crate) success: [f32; 4],
    pub(crate) tab_border: [f32; 4],
    pub(crate) tab_text_active: [f32; 4],
    pub(crate) tab_text_inactive: [f32; 4],
    // Settings panel colors
    pub(crate) settings_bg: [f32; 4],
    pub(crate) settings_header: [f32; 4],
    pub(crate) settings_text: [f32; 4],
    pub(crate) settings_text_dim: [f32; 4],
    pub(crate) settings_accent: [f32; 4],
    pub(crate) settings_separator: [f32; 4],
    pub(crate) scrim: [f32; 4],
    // Search colors
    pub(crate) search_match: [f32; 4],
    pub(crate) search_match_current: [f32; 4],
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
    pub(crate) palette_border: [f32; 4],
}

impl UiPalette {
    fn from_theme_with_contrast(theme: &Theme, high_contrast: bool) -> Self {
        let bg = theme.ui.background;
        let fg = theme.ui.foreground;
        let selection = theme.ui.selection;
        let cursor = theme.ui.cursor;
        let tab_active = theme.ui.tab_active;
        let tab_inactive = theme.ui.tab_inactive;
        let tab_border = theme.ui.border;

        // Quiet chrome: the theme's inactive-tab color is the strip, raised and
        // hover surfaces step from it toward the foreground (so light and dark
        // themes both work), and the theme's active-tab color is reserved for
        // the thin active indicator.
        let strip = with_alpha(tab_inactive.to_f32_array(), 1.0);
        let chrome = |amount: f32| mix(strip, fg.to_f32_array(), amount);
        let tab_text_active = fg.to_f32_array();
        let tab_text_inactive = with_alpha(fg.to_f32_array(), 0.62);
        let mut palette = Self {
            background: bg.to_wgpu(),
            selection: selection.to_f32_array(),
            selection_flash: with_alpha(fg.to_f32_array(), 0.85),
            selection_text: theme.ui.selection_text.map(|c| c.to_f32_array()),
            cursor: cursor.to_f32_array(),
            cursor_text: theme.ui.cursor_text.to_f32_array(),
            tab_bar_bg: strip,
            tab_active: chrome(0.08),
            tab_inactive: chrome(0.05),
            tab_accent: tab_active.to_f32_array(),
            menu_bg: chrome(0.03),
            menu_border: chrome(0.14),
            menu_highlight: chrome(0.11),
            menu_text: fg.to_f32_array(),
            menu_text_dim: with_alpha(fg.to_f32_array(), 0.55),
            tab_activity: theme.primary.yellow.to_f32_array(),
            warning: theme.primary.yellow.to_f32_array(),
            error: theme.primary.red.to_f32_array(),
            success: theme.primary.green.to_f32_array(),
            tab_border: tab_border.to_f32_array(),
            tab_text_active,
            tab_text_inactive,
            // Settings panel
            settings_bg: with_alpha(scale_rgb(bg.to_f32_array(), 0.85), 0.97),
            settings_header: fg.to_f32_array(),
            settings_text: fg.to_f32_array(),
            settings_text_dim: with_alpha(fg.to_f32_array(), 0.55),
            settings_accent: tab_active.to_f32_array(),
            settings_separator: with_alpha(tab_border.to_f32_array(), 0.4),
            // Warm scrim derived from theme background
            scrim: with_alpha(scale_rgb(bg.to_f32_array(), 0.3), 0.30),
            // Search highlights and the find box's error color
            search_match: theme.ui.search_match.with_alpha(0.35).to_f32_array(),
            search_match_current: theme.ui.search_match.with_alpha(0.65).to_f32_array(),
            search_error: theme.ansi_color(1).to_f32_array(), // red
            // URL/hyperlink color from theme
            url: theme.ui.url.to_f32_array(),
            // Command palette — fully theme-derived
            palette_scrim: with_alpha(scale_rgb(bg.to_f32_array(), 0.0), 0.3),
            palette_bg: scale_rgb(bg.to_f32_array(), 0.65),
            palette_input_bg: scale_rgb(bg.to_f32_array(), 0.50),
            palette_accent: cursor.to_f32_array(),
            palette_text: fg.to_f32_array(),
            palette_text_dim: with_alpha(fg.to_f32_array(), 0.50),
            palette_match_highlight: cursor.to_f32_array(),
            palette_border: with_alpha(tab_border.to_f32_array(), 0.5),
        };
        if high_contrast {
            palette.enforce_high_contrast();
        }
        palette
    }

    fn enforce_high_contrast(&mut self) {
        self.settings_bg[3] = 1.0;
        let settings_text = high_contrast_text(self.settings_bg);
        self.settings_header = settings_text;
        self.settings_text = settings_text;
        self.settings_text_dim = settings_text;
        self.settings_accent = settings_text;
        self.settings_separator = settings_text;

        self.tab_bar_bg[3] = 1.0;
        self.tab_active[3] = 1.0;
        self.tab_inactive[3] = 1.0;
        self.tab_text_active = high_contrast_text(self.tab_active);
        self.tab_text_inactive = high_contrast_text(self.tab_bar_bg);
        self.tab_accent = self.tab_text_active;

        self.menu_bg[3] = 1.0;
        self.menu_highlight[3] = 1.0;
        self.menu_text = high_contrast_text(self.menu_bg);
        self.menu_text_dim = self.menu_text;
        self.menu_border = self.menu_text;
        self.tab_border = high_contrast_text(self.tab_bar_bg);

        // Errors draw on menu surfaces (find box, manager buttons).
        self.search_error = self.menu_text;

        self.palette_bg[3] = 1.0;
        self.palette_input_bg[3] = 1.0;
        self.palette_input_bg = self.palette_bg;
        let palette_text = high_contrast_text(self.palette_bg);
        self.palette_text = palette_text;
        self.palette_text_dim = palette_text;
        self.palette_border = palette_text;
        self.palette_accent = palette_text;
        self.scrim[3] = self.scrim[3].max(0.65);
        self.palette_scrim[3] = self.palette_scrim[3].max(0.72);
    }
}

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

/// Termeric composes in display-encoded sRGB space, as Qt, GTK and browsers
/// do: theme hex values reach the screen unchanged and alpha blends operate on
/// encoded values. Render targets therefore use non-sRGB views, so the GPU
/// never encodes those values a second time, and textures that store
/// sRGB-encoded pixels (images, color glyphs) use this format so sampling
/// returns the stored values instead of decoding them to linear light.
pub(crate) const ENCODED_RGBA_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// The presentation format a surface is configured with and the non-sRGB
/// view format that frames are rendered into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SurfaceFormats {
    surface: TextureFormat,
    render: TextureFormat,
}

/// Prefer 8-bit formats because wider or floating-point presentation formats
/// can carry different transfer semantics. An sRGB-only surface is still
/// usable: frames render through a non-sRGB view of the same texture.
fn select_surface_formats(available: &[TextureFormat]) -> Option<SurfaceFormats> {
    const PREFERRED: [TextureFormat; 4] = [
        TextureFormat::Bgra8Unorm,
        TextureFormat::Rgba8Unorm,
        TextureFormat::Bgra8UnormSrgb,
        TextureFormat::Rgba8UnormSrgb,
    ];
    let surface = PREFERRED
        .into_iter()
        .find(|format| available.contains(format))
        .or_else(|| available.iter().copied().find(|format| !format.is_srgb()))
        .or_else(|| available.first().copied())?;
    Some(SurfaceFormats {
        surface,
        render: surface.remove_srgb_suffix(),
    })
}

#[cfg(test)]
mod surface_format_tests {
    use super::*;

    #[test]
    fn prefers_a_non_srgb_eight_bit_surface_even_when_srgb_is_listed_first() {
        let formats = select_surface_formats(&[
            TextureFormat::Bgra8UnormSrgb,
            TextureFormat::Rgba16Float,
            TextureFormat::Bgra8Unorm,
        ]);
        assert_eq!(
            formats,
            Some(SurfaceFormats {
                surface: TextureFormat::Bgra8Unorm,
                render: TextureFormat::Bgra8Unorm,
            })
        );
    }

    #[test]
    fn srgb_only_surfaces_render_through_a_non_srgb_view() {
        let formats = select_surface_formats(&[TextureFormat::Rgba8UnormSrgb]);
        assert_eq!(
            formats,
            Some(SurfaceFormats {
                surface: TextureFormat::Rgba8UnormSrgb,
                render: TextureFormat::Rgba8Unorm,
            })
        );
    }

    #[test]
    fn unlisted_formats_fall_back_without_ever_rendering_through_srgb() {
        let wide = select_surface_formats(&[TextureFormat::Rgb10a2Unorm]).unwrap();
        assert_eq!(wide.render, TextureFormat::Rgb10a2Unorm);
        for available in [
            vec![TextureFormat::Bgra8UnormSrgb],
            vec![TextureFormat::Rgba8UnormSrgb, TextureFormat::Bgra8UnormSrgb],
            vec![TextureFormat::Rgb10a2Unorm, TextureFormat::Rgba8UnormSrgb],
        ] {
            let formats = select_surface_formats(&available).unwrap();
            assert!(!formats.render.is_srgb(), "{available:?} -> {formats:?}");
        }
        assert_eq!(select_surface_formats(&[]), None);
    }
}

fn contrast_text(bg: [f32; 4]) -> [f32; 4] {
    if luminance(bg) > 0.6 {
        [0.12, 0.09, 0.07, 1.0]
    } else {
        [0.95, 0.92, 0.88, 1.0]
    }
}

fn high_contrast_text(bg: [f32; 4]) -> [f32; 4] {
    let dark = [0.04, 0.03, 0.02, 1.0];
    let light = [1.0, 0.98, 0.95, 1.0];
    if contrast_ratio(bg, dark) >= contrast_ratio(bg, light) {
        dark
    } else {
        light
    }
}

fn relative_luminance(color: [f32; 4]) -> f32 {
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
}

fn contrast_ratio(first: [f32; 4], second: [f32; 4]) -> f32 {
    let first = relative_luminance(first);
    let second = relative_luminance(second);
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

#[derive(Clone)]
struct TerminalPalette {
    default_fg: TermColor,
    default_bg: TermColor,
    ansi: [TermColor; 16],
}

impl TerminalPalette {
    fn from_theme(theme: &Theme) -> Self {
        let convert =
            |color: crate::config::theme::Color| TermColor::rgb(color.r, color.g, color.b);
        Self {
            default_fg: convert(theme.ui.foreground),
            default_bg: convert(theme.ui.background),
            ansi: std::array::from_fn(|index| convert(theme.ansi_color(index as u8))),
        }
    }

    fn resolve(
        &self,
        dynamic: &termeric_core::DynamicColors,
        spec: ColorSpec,
        resolved: TermColor,
        default: TermColor,
    ) -> TermColor {
        match spec {
            ColorSpec::Default => default,
            ColorSpec::Indexed(index) => dynamic.palette(index).unwrap_or_else(|| {
                if index < 16 {
                    self.ansi[index as usize]
                } else {
                    resolved
                }
            }),
            ColorSpec::TrueColor(_) => resolved,
        }
    }

    fn default_fg(&self, dynamic: &termeric_core::DynamicColors) -> TermColor {
        dynamic.foreground().unwrap_or(self.default_fg)
    }

    fn default_bg(&self, dynamic: &termeric_core::DynamicColors) -> TermColor {
        dynamic.background().unwrap_or(self.default_bg)
    }

    #[cfg(test)]
    fn resolve_cell(
        &self,
        dynamic: &termeric_core::DynamicColors,
        cell: &termeric_core::Cell,
    ) -> (TermColor, TermColor) {
        let fg = self.resolve(dynamic, cell.fg_spec, cell.fg, self.default_fg(dynamic));
        let bg = self.resolve(dynamic, cell.bg_spec, cell.bg, self.default_bg(dynamic));
        if cell.attrs.contains(CellAttrs::INVERSE) {
            (bg, fg)
        } else {
            (fg, bg)
        }
    }
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

        let selection = select_adapter(backend_preference, |attempt| {
            let Some(adapter) = discover_adapter(
                &instance,
                enabled_backends,
                Some(&surface),
                PowerPreference::HighPerformance,
                attempt.force_fallback,
            ) else {
                return Ok(None);
            };
            let adapter_info = adapter.get_info();
            if !attempt.accepts(adapter_info.device_type) {
                return Err(format!(
                    "WGPU returned CPU adapter {:?}; a GPU adapter is required",
                    adapter_info.name
                ));
            }
            pollster::block_on(adapter.request_device(
                &DeviceDescriptor {
                    required_features: Features::empty(),
                    required_limits: Limits::default(),
                    memory_hints: MemoryHints::default(),
                    label: Some("Termeric Device"),
                },
                None,
            ))
            .map(|(device, queue)| Some((adapter, device, queue)))
            .map_err(|error| format!("could not create a device: {error}"))
        });
        let ((adapter, device, queue), selected_attempt) = selection.map_err(|failures| {
            anyhow::anyhow!(
                "renderer initialization failed with preference {backend_preference}: {}. \
                 Try `termeric --renderer software`, install current graphics/Mesa drivers, \
                 or set renderer.backend = \"software\" in config.toml",
                failures.join("; ")
            )
        })?;
        let fallback_requested = selected_attempt.force_fallback;
        let adapter_info = adapter.get_info();
        let diagnostics = RendererDiagnostics {
            preference: backend_preference,
            adapter: adapter_info.name.clone(),
            backend: format!("{:?}", adapter_info.backend),
            device_type: format!("{:?}", adapter_info.device_type),
            software: fallback_requested || adapter_info.device_type == DeviceType::Cpu,
        };
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
        let (cell_width, cell_height) = glyph_cache.cell_size();

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
            cell_width,
            cell_height,
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
        (self.cell_width, self.cell_height)
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
            self.cell_width,
            self.cell_height,
        )
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

    /// Get current font size
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Change font size
    pub fn set_font_size(&mut self, font_size: f32) {
        self.invalidate_terminal_rows();
        self.font_size = font_size;
        let scaled = physical_font_size(font_size, self.scale_factor);
        self.glyph_cache.set_font_size(scaled, &self.queue);
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
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
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
    }

    /// Change the configured line-height multiplier.
    pub fn set_line_height(&mut self, multiplier: f32) {
        self.invalidate_terminal_rows();
        self.glyph_cache.set_line_height(multiplier, &self.queue);
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
    }

    /// Apply the normal and SGR-bold OpenType `wght` requests.
    pub fn set_font_weights(&mut self, font_weight: u16, bold_font_weight: u16) {
        if self
            .glyph_cache
            .set_font_weights(font_weight, bold_font_weight, &self.queue)
        {
            self.invalidate_terminal_rows();
            let (cell_width, cell_height) = self.glyph_cache.cell_size();
            self.cell_width = cell_width;
            self.cell_height = cell_height;
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
        self.cell_width
    }

    /// Cell height in physical pixels.
    pub fn cell_height(&self) -> f32 {
        self.cell_height
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
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
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

    /// Set window opacity (0.0 = fully transparent, 1.0 = fully opaque)
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.1, 1.0);
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

    pub fn set_bell_intensity(&mut self, intensity: f32) {
        self.bell_intensity = intensity;
    }

    /// Top edge of the terminal area, directly below the tab bar. Overlays
    /// such as the find box float above it and never move it, so the grid
    /// size computed from the window always matches what is drawn.
    pub fn content_top(&self) -> f32 {
        self.tab_bar_height()
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
        terminal_key: usize,
        grid: &Grid,
        selection: Option<&Selection>,
        dynamic_colors: &termeric_core::DynamicColors,
        extra_cursors: &termeric_core::ExtraCursorState,
        cursor_opacity: f32,
        cursor_pos: (f32, f32),
        tabs: &[TabInfo],
        copy_flash: f32,
        focus_dim: f32,
        settings_panel: Option<&SettingsPanelInfo>,
        search_bar: Option<&SearchBarInfo>,
        ime_preedit: Option<&ImePreeditRenderInfo>,
        search: &Search,
        url_cache: &UrlCache,
        copy_mode: Option<&CopyModeRenderInfo>,
        images: Option<&[termeric_core::PresentationImage]>,
        workspace_menu: Option<&PopupMenuInfo>,
        session_panel: Option<&crate::session::panel::SessionPanelInfo>,
        commands_panel: Option<&crate::ui::commands::panel::CommandsPanelInfo>,
        agents_panel: Option<&crate::ui::agents_panel::AgentsPanelInfo>,
        command_palette: Option<&PaletteInfo>,
        shortcuts_panel: Option<&ShortcutsPanelInfo>,
        cursor_style: CursorStyle,
        text_blink: TextBlinkPhases,
        panes: &[PaneRenderData],
        pane_dividers: &[crate::panes::Divider],
        harvest: Option<&HarvestRenderInfo>,
    ) -> Result<(), SurfaceError> {
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
        self.build_render_data(
            grid,
            selection,
            dynamic_colors,
            extra_cursors,
            cursor_opacity,
            cursor_pos,
            tabs,
            copy_flash,
            focus_dim,
            settings_panel,
            search_bar,
            ime_preedit,
            search,
            url_cache,
            copy_mode,
            workspace_menu,
            session_panel,
            commands_panel,
            agents_panel,
            command_palette,
            shortcuts_panel,
            cursor_style,
            text_blink,
            panes,
            pane_dividers,
            harvest,
        );

        // Build image render data if any
        self.build_background_image_data();
        self.build_image_data(terminal_key, grid, images, panes);

        // Upload vertex data
        self.rect_pipeline.upload(&self.device, &self.queue);
        self.text_pipeline
            .upload(&self.device, &self.queue, &mut self.glyph_cache);
        self.background_image_pipeline
            .upload(&self.device, &self.queue);
        self.image_pipeline.upload(&self.device, &self.queue);
        self.terminal_row_cache.finish_frame();
        log::trace!(
            "Glyph cache: atlas {:?}; rasterizer {:?}; shaped runs {:?}",
            self.glyph_cache.atlas_metrics(),
            self.glyph_cache.rasterizer_metrics(),
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
                self.text_pipeline
                    .draw_layer(&mut render_pass, &self.glyph_cache, layer);
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
                    grid.cols() as f32 * self.cell_width,
                    grid.rows() as f32 * self.cell_height,
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
                self.cell_width,
                self.cell_height,
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
    fn build_render_data(
        &mut self,
        grid: &Grid,
        selection: Option<&Selection>,
        dynamic_colors: &termeric_core::DynamicColors,
        extra_cursors: &termeric_core::ExtraCursorState,
        cursor_opacity: f32,
        cursor_pos: (f32, f32),
        tabs: &[TabInfo],
        copy_flash: f32,
        focus_dim: f32,
        settings_panel: Option<&SettingsPanelInfo>,
        search_bar: Option<&SearchBarInfo>,
        ime_preedit: Option<&ImePreeditRenderInfo>,
        search: &Search,
        url_cache: &UrlCache,
        copy_mode: Option<&CopyModeRenderInfo>,
        workspace_menu: Option<&PopupMenuInfo>,
        session_panel: Option<&crate::session::panel::SessionPanelInfo>,
        commands_panel: Option<&crate::ui::commands::panel::CommandsPanelInfo>,
        agents_panel: Option<&crate::ui::agents_panel::AgentsPanelInfo>,
        command_palette: Option<&PaletteInfo>,
        shortcuts_panel: Option<&ShortcutsPanelInfo>,
        cursor_style: CursorStyle,
        text_blink: TextBlinkPhases,
        panes: &[PaneRenderData],
        pane_dividers: &[crate::panes::Divider],
        harvest: Option<&HarvestRenderInfo>,
    ) {
        self.rect_pipeline.clear();
        self.text_pipeline.clear();
        self.visible_text_blink = VisibleTextBlink::default();

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
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
        // Names and width metadata are shared; cloning this frame does not
        // duplicate or scan the complete branch list.
        if let Some(ref info) = self.git_completions.clone()
            && let Some(anchor) = git::CompletionAnchor::for_view(
                cursor_pos,
                panes,
                (content_left, content_top),
                (cell_width, cell_height),
                padding,
            )
        {
            self.build_git_completions(
                info,
                anchor,
                content_left,
                content_top,
                content_width,
                window_height,
            );
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
                    if let Some(ref info) = self.context_picker.clone()
                        && info.snapshot.animation > 0.001
                    {
                        self.build_context_picker(info, window_width, window_height);
                    }
                }
                ModalLayer::DiffReview => {
                    if let Some(ref info) = self.diff_review.clone()
                        && info.animation > 0.001
                    {
                        self.build_diff_review(info, window_width, window_height);
                    }
                }
                ModalLayer::InfoOverlay => {
                    if let Some(ref info) = self.info_overlay.clone()
                        && info.animation > 0.001
                    {
                        self.build_info_overlay(info, window_width, window_height);
                    }
                }
                ModalLayer::Guardrail => {
                    if let Some(ref info) = self.guardrail.clone()
                        && info.animation > 0.001
                    {
                        self.build_guardrail_confirmation(info, window_width, window_height);
                    }
                }
            }
        }

        // ── Toasts, above every modal layer and below only the IME preedit ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if !self.toasts.is_empty() {
            let toasts = self.toasts.clone();
            // Toasts sit in the terminal area's corner, clear of the dock.
            let toast_width = self.content_right_for(window_width);
            self.build_toasts(&toasts, toast_width, window_height);
        }

        // ── Platform IME preedit (never PTY-owned) ──
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        if let Some(preedit) = ime_preedit {
            self.build_ime_preedit(preedit, window_width, window_height);
        }
    }

    /// Render a single terminal grid within a pane viewport.
    ///
    /// This method handles cell rendering, cursor, scrollbar, selection, search
    /// highlights, underlines, and URL detection for one grid. It is called once
    /// in single-grid mode or once per pane in multi-pane mode.
    fn build_pane_grid(
        &mut self,
        grid: &Grid,
        selection: Option<&Selection>,
        dynamic_colors: &termeric_core::DynamicColors,
        extra_cursors: &termeric_core::ExtraCursorState,
        offset_x: f32,
        offset_y: f32,
        pane_cols: usize,
        pane_rows: usize,
        is_focused: bool,
        cursor_style: CursorStyle,
        cursor_opacity: f32,
        cursor_pos: (f32, f32),
        text_blink: TextBlinkPhases,
        copy_flash: f32,
        brightness: f32,
        search: &Search,
        url_cache: &UrlCache,
        window_height: f32,
        pane_width: f32,
    ) {
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        // Selection highlight color
        let selection_base = self.ui.selection;
        let selection_flash = self.ui.selection_flash;
        let selection_color =
            scale_rgb(mix(selection_base, selection_flash, copy_flash), brightness);

        let scroll_offset = grid.scroll_offset();
        let scroll_offset_i32 = i32::try_from(scroll_offset).unwrap_or(i32::MAX);
        let default_bg = self.terminal_palette.default_bg(dynamic_colors);
        if dynamic_colors.background().is_some() {
            self.rect_pipeline.push_rect(
                offset_x,
                offset_y,
                pane_width,
                pane_rows as f32 * cell_height,
                default_bg.to_f32_array(),
            );
        }

        let pane_clip = TextClip {
            left: offset_x,
            top: offset_y,
            right: offset_x + pane_width,
            bottom: (offset_y + pane_rows as f32 * cell_height).min(window_height - self.padding),
        };
        let max_grid_y = pane_clip.bottom;

        // Determine which rows to render based on filter mode
        let matching_rows = search.filtered_rows(pane_rows);
        let filter_mode = is_focused && !matching_rows.is_empty();

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

        // Text runs break around the cell the cursor rests on, never its
        // gliding position: that changes every frame and can sit between cells.
        let cursor_cell =
            (is_focused && cursor_opacity > 0.0 && scroll_offset == 0 && !filter_mode)
                .then(|| grid.cursor())
                .map(|cursor| (cursor.row, cursor.col));
        for (visual_row, grid_row) in rows_to_render {
            let row_data = if filter_mode {
                grid.row_at_offset(grid_row)
            } else {
                None
            };

            let key = RowCacheKey {
                grid_id: grid as *const Grid as usize,
                visual_row,
            };
            let cursor_break_col =
                cursor_cell.and_then(|(row, col)| (row == visual_row).then_some(col));
            let mut hasher = DefaultHasher::new();
            dynamic_colors.generation().hash(&mut hasher);
            grid_row.hash(&mut hasher);
            pane_cols.hash(&mut hasher);
            is_focused.hash(&mut hasher);
            offset_x.to_bits().hash(&mut hasher);
            offset_y.to_bits().hash(&mut hasher);
            hash_text_clip(pane_clip, &mut hasher);
            cell_width.to_bits().hash(&mut hasher);
            cell_height.to_bits().hash(&mut hasher);
            brightness.to_bits().hash(&mut hasher);
            cursor_break_col.hash(&mut hasher);
            for component in selection_color {
                component.to_bits().hash(&mut hasher);
            }
            let mut row_blink = VisibleTextBlink::default();
            for col in 0..pane_cols.min(grid.cols()) {
                let cell = if filter_mode {
                    row_data.as_ref().and_then(|row| row.get(col))
                } else {
                    grid.cell_scrolled(visual_row, col)
                };
                match cell {
                    Some(cell) => {
                        cell.hash(&mut hasher);
                        let cell_blink = style::visible_blink_for_cell(&cell);
                        row_blink.slow |= cell_blink.slow;
                        row_blink.rapid |= cell_blink.rapid;
                    }
                    None => 0_u8.hash(&mut hasher),
                }
                let selected = is_focused
                    && selection.is_some_and(|selection| selection.contains(grid_row, col));
                selected.hash(&mut hasher);
                (is_focused && search.is_match(grid_row, col)).hash(&mut hasher);
                (is_focused && search.is_current_match(grid_row, col)).hash(&mut hasher);
                url_cache.is_url(visual_row, col).hash(&mut hasher);
            }
            self.visible_text_blink.slow |= row_blink.slow;
            self.visible_text_blink.rapid |= row_blink.rapid;
            if row_blink.slow {
                text_blink.slow_visible.hash(&mut hasher);
            }
            if row_blink.rapid {
                text_blink.rapid_visible.hash(&mut hasher);
            }
            self.glyph_cache.atlas_generation().hash(&mut hasher);
            let signature = hasher.finish();
            if self.terminal_row_cache.reuse(key, signature) {
                let pages = self.terminal_row_cache.referenced_atlas_pages(key);
                self.glyph_cache.pin_pages(&pages);
                continue;
            }

            let rect_checkpoint = self.rect_pipeline.checkpoint();
            let text_checkpoint = self.text_pipeline.queued_checkpoint();
            let mut text_run = TextRunAccumulator::default();

            for col in 0..pane_cols.min(grid.cols()) {
                if cursor_break_col == Some(col) {
                    text_run.flush(&mut self.text_pipeline);
                }
                let cell = if filter_mode {
                    row_data.as_ref().and_then(|r| r.get(col))
                } else {
                    grid.cell_scrolled(visual_row, col)
                };
                let Some(cell) = cell else {
                    text_run.flush(&mut self.text_pipeline);
                    continue;
                };

                let x = offset_x + col as f32 * cell_width;
                let y = offset_y + visual_row as f32 * cell_height;

                if y + cell_height > max_grid_y {
                    text_run.flush(&mut self.text_pipeline);
                    break;
                }

                let is_selected = is_focused
                    && selection
                        .map(|s| s.contains(grid_row, col))
                        .unwrap_or(false);

                let is_search_match = is_focused && search.is_match(grid_row, col);
                let is_current_match = is_focused && search.is_current_match(grid_row, col);

                let style = ResolvedCellStyle::resolve(
                    &self.terminal_palette,
                    dynamic_colors,
                    &cell,
                    self.bold_is_bright,
                    text_blink,
                );

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
                        scale_rgb(self.ui.search_match_current, brightness),
                    );
                } else if is_search_match {
                    self.rect_pipeline.push_rect(
                        x,
                        y,
                        cell_width,
                        cell_height,
                        scale_rgb(self.ui.search_match, brightness),
                    );
                } else if style.background != default_bg {
                    self.rect_pipeline.push_rect(
                        x,
                        y,
                        cell_width,
                        cell_height,
                        scale_rgb(style.background.to_f32_array(), brightness),
                    );
                }

                // Draw the complete grapheme leader. Wide spacers are empty
                // attributed cells and intentionally queue no text.
                if style.paint_foreground && !cell.content.is_empty() {
                    let text_color = style.text_color(
                        is_selected.then_some(self.ui.selection_text).flatten(),
                        brightness,
                    );
                    if cell.multicell.is_leader() {
                        text_run.flush(&mut self.text_pipeline);
                        self.text_pipeline.push_styled_multicell(
                            &cell.content.to_text(),
                            x,
                            y,
                            text_color,
                            style.font,
                            pane_clip,
                            cell_width,
                            cell_height,
                            cell.multicell,
                        );
                    } else if cell.multicell.is_present() {
                        text_run.flush(&mut self.text_pipeline);
                    } else {
                        match &cell.content {
                            CellContent::Char(character)
                                if !character.is_whitespace()
                                    && *character != termeric_core::IMAGE_PLACEHOLDER
                                    && !cell.flags.contains(CellFlags::WIDE_CHAR) =>
                            {
                                text_run.push(
                                    &mut self.text_pipeline,
                                    *character,
                                    x,
                                    y,
                                    text_color,
                                    style.font,
                                    pane_clip,
                                    cell_width,
                                );
                            }
                            CellContent::Char(character)
                                if !character.is_whitespace()
                                    && *character != termeric_core::IMAGE_PLACEHOLDER =>
                            {
                                text_run.flush(&mut self.text_pipeline);
                                self.text_pipeline.push_styled_char(
                                    *character, x, y, text_color, style.font, pane_clip,
                                );
                            }
                            CellContent::Grapheme(grapheme)
                                if !grapheme.starts_with(termeric_core::IMAGE_PLACEHOLDER)
                                    && !grapheme.chars().all(char::is_whitespace) =>
                            {
                                text_run.flush(&mut self.text_pipeline);
                                self.text_pipeline.push_styled_grapheme(
                                    grapheme, x, y, text_color, style.font, pane_clip,
                                );
                            }
                            _ => text_run.flush(&mut self.text_pipeline),
                        }
                    }
                } else {
                    text_run.flush(&mut self.text_pipeline);
                }
                if cursor_break_col == Some(col) {
                    text_run.flush(&mut self.text_pipeline);
                }

                if style.paint_foreground {
                    self.draw_sgr_decorations(style, x, y, cell_width, brightness, None);
                }

                // Draw hyperlink underline
                let is_hyperlink = cell.flags.contains(CellFlags::HYPERLINK);
                let is_detected_url = url_cache.is_url(visual_row, col);
                if style.paint_foreground
                    && (is_hyperlink || is_detected_url)
                    && style.underline.is_none()
                {
                    let link_color = scale_rgb(self.ui.url, brightness);
                    let underline_y = y + cell_height - 2.0;
                    self.draw_dotted_underline(x, underline_y, cell_width, link_color);
                }
            }
            text_run.flush(&mut self.text_pipeline);

            let rects = self.rect_pipeline.take_since(rect_checkpoint);
            let queued = self.text_pipeline.take_queued_since(text_checkpoint);
            let text = TextPipeline::build_batch(queued, &self.queue, &mut self.glyph_cache);
            self.terminal_row_cache
                .replace(key, signature, rects, text, &self.device, &self.queue);
        }

        // Extra cursors are application-owned and remain visible even when
        // DECTCEM hides the main cursor or this pane is not focused.
        if !filter_mode {
            self.build_extra_cursors(
                grid,
                extra_cursors,
                dynamic_colors,
                offset_x,
                offset_y,
                max_grid_y,
                pane_clip,
                cursor_style,
                cursor_opacity,
                brightness,
                text_blink,
            );
        }

        // Draw cursor (only for focused pane, not scrolled back, not in filter mode)
        if is_focused && cursor_opacity > 0.0 && scroll_offset == 0 && !filter_mode {
            let cursor = grid.cursor();
            if cursor.visible {
                let (cursor_top, cursor_left, cursor_rows, cursor_columns) =
                    cursor_cell_geometry(grid, cursor.row, cursor.col);
                let (glide_col, glide_row) = gliding_cursor_origin(
                    (cursor_left, cursor_top),
                    (cursor.col, cursor.row),
                    cursor_pos,
                );
                let x = offset_x + glide_col * cell_width;
                let y = offset_y + glide_row * cell_height;
                let cursor_width = cursor_columns as f32 * cell_width;
                let cursor_height = cursor_rows as f32 * cell_height;

                if y + cursor_height <= max_grid_y {
                    let cursor_color = scale_rgb(
                        dynamic_colors
                            .cursor()
                            .map_or(self.ui.cursor, |color| color.to_f32_array()),
                        brightness,
                    );
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
                                cursor_width,
                                cursor_height,
                                cc,
                                2.0,
                            );
                            let cursor_row = cursor_top;
                            let cursor_col = cursor_left;
                            if let Some(cell) = grid.cell_scrolled(cursor_row, cursor_col) {
                                let style = ResolvedCellStyle::resolve(
                                    &self.terminal_palette,
                                    dynamic_colors,
                                    &cell,
                                    self.bold_is_bright,
                                    text_blink,
                                );
                                if style.paint_foreground {
                                    let mut color =
                                        style.text_color(Some(self.ui.cursor_text), brightness);
                                    color[3] = cursor_opacity;
                                    if cell.multicell.is_leader() {
                                        self.text_pipeline.push_styled_multicell(
                                            &cell.content.to_text(),
                                            x,
                                            y,
                                            color,
                                            style.font,
                                            pane_clip,
                                            cell_width,
                                            cell_height,
                                            cell.multicell,
                                        );
                                    } else {
                                        match &cell.content {
                                            CellContent::Char(character)
                                                if !character.is_whitespace()
                                                    && *character
                                                        != termeric_core::IMAGE_PLACEHOLDER =>
                                            {
                                                self.text_pipeline.push_styled_char(
                                                    *character, x, y, color, style.font, pane_clip,
                                                );
                                            }
                                            CellContent::Grapheme(grapheme)
                                                if !grapheme.starts_with(
                                                    termeric_core::IMAGE_PLACEHOLDER,
                                                ) && !grapheme
                                                    .chars()
                                                    .all(char::is_whitespace) =>
                                            {
                                                self.text_pipeline.push_styled_grapheme(
                                                    grapheme, x, y, color, style.font, pane_clip,
                                                );
                                            }
                                            _ => {}
                                        }
                                    }
                                    self.draw_sgr_decorations(
                                        style,
                                        x,
                                        y,
                                        cursor_width,
                                        brightness,
                                        Some((self.ui.cursor_text, cursor_opacity)),
                                    );
                                }
                            }
                        }
                        CursorStyle::Underline => {
                            let underline_height = 2.0_f32.max(cell_height * 0.1);
                            self.rect_pipeline.push_rect(
                                x,
                                y + cursor_height - underline_height,
                                cursor_width,
                                underline_height,
                                cc,
                            );
                        }
                        CursorStyle::Beam => {
                            let beam_width = 2.0_f32.max(cell_width * 0.1);
                            self.rect_pipeline
                                .push_rect(x, y, beam_width, cursor_height, cc);
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

fn cursor_cell_geometry(grid: &Grid, row: usize, col: usize) -> (usize, usize, usize, usize) {
    grid.multicell_region_at(row, col)
        .map_or((row, col, 1, 1), |region| {
            (
                region.top,
                region.left,
                region.descriptor.rows(),
                region.descriptor.columns(),
            )
        })
}

/// Where to draw the cursor, in cells: the origin of the cell it rests on,
/// moved by the distance its glide still has to travel. `cursor` is the grid
/// cursor and `glide` the animated position, both `(col, row)`. At rest the
/// result is exactly `rest`, so a cursor inside a scaled cell lands on it.
fn gliding_cursor_origin(
    rest: (usize, usize),
    cursor: (usize, usize),
    glide: (f32, f32),
) -> (f32, f32) {
    (
        rest.0 as f32 + glide.0 - cursor.0 as f32,
        rest.1 as f32 + glide.1 - cursor.1 as f32,
    )
}

#[cfg(test)]
#[test]
fn the_cursor_glides_toward_the_cell_it_rests_on() {
    // At rest on a plain cell and on a scaled cell's interior.
    assert_eq!(
        gliding_cursor_origin((5, 2), (5, 2), (5.0, 2.0)),
        (5.0, 2.0)
    );
    assert_eq!(
        gliding_cursor_origin((1, 0), (4, 1), (4.0, 1.0)),
        (1.0, 0.0)
    );
    // Mid-glide, the remaining distance carries over to the scaled origin.
    assert_eq!(
        gliding_cursor_origin((1, 0), (4, 1), (2.5, 3.0)),
        (-0.5, 2.0)
    );
}

#[cfg(test)]
#[test]
fn multicell_cursor_geometry_expands_from_every_covered_cell() {
    let mut terminal = termeric_core::Terminal::new(10, 4);
    terminal.process(b".\x1b]66;s=2:w=2;X\x07");
    assert_eq!(cursor_cell_geometry(terminal.grid(), 0, 1), (0, 1, 2, 4));
    assert_eq!(cursor_cell_geometry(terminal.grid(), 1, 4), (0, 1, 2, 4));
    assert_eq!(cursor_cell_geometry(terminal.grid(), 3, 9), (3, 9, 1, 1));
}

#[cfg(test)]
pub(crate) struct HeadlessRendererBenchmark {
    pub adapter: String,
    pub frame_ms: Vec<f64>,
    pub glyph_warm_ms: Vec<f64>,
    pub glyph_cache_hit_percent: Vec<f64>,
    pub steady_frame_buffer_allocations: u64,
    pub ligature_code_ms: Vec<f64>,
    pub ligature_cache_hit_percent: Vec<f64>,
    pub shaping_pressure: glyph_cache::pressure_benchmark::PressureBenchmark,
}

#[cfg(test)]
const HEADLESS_IMAGE_WIDTH: u32 = 64;
#[cfg(test)]
const HEADLESS_IMAGE_CELL_HEIGHT: u32 = 4;
#[cfg(test)]
const HEADLESS_IMAGE_ROWS: u32 = 52;
#[cfg(test)]
const HEADLESS_IMAGE_HEIGHT: u32 = HEADLESS_IMAGE_CELL_HEIGHT * HEADLESS_IMAGE_ROWS;

#[cfg(test)]
fn headless_image_row_color(row: u32) -> [u8; 4] {
    [
        32 + (row * 3 % 200) as u8,
        48 + (row * 5 % 180) as u8,
        64 + (row * 7 % 160) as u8,
        255,
    ]
}

#[cfg(test)]
fn headless_image_fixture() -> termeric_core::StoredImage {
    let source_width = HEADLESS_IMAGE_WIDTH / 2;
    let mut data = Vec::with_capacity((source_width * HEADLESS_IMAGE_HEIGHT * 4) as usize);
    for pixel_row in 0..HEADLESS_IMAGE_HEIGHT {
        let color = headless_image_row_color(pixel_row / HEADLESS_IMAGE_CELL_HEIGHT);
        for _ in 0..source_width {
            data.extend_from_slice(&color);
        }
    }
    termeric_core::StoredImage {
        id: termeric_core::ImageId(7),
        width: source_width,
        height: HEADLESS_IMAGE_HEIGHT,
        data: data.into(),
        revision: 1,
    }
}

#[cfg(test)]
fn headless_image_presentations(
    image: &termeric_core::StoredImage,
    row_origin: i64,
) -> Vec<termeric_core::PresentationImage> {
    let mut presentations = Vec::with_capacity(HEADLESS_IMAGE_ROWS as usize + 1);
    presentations.push(termeric_core::PresentationImage {
        placement: termeric_core::ImagePlacement {
            image_id: image.id,
            placement_id: Some(1),
            col: 0,
            row: row_origin,
            alternate_screen: false,
            source_x: 0,
            source_y: 0,
            source_width: Some(image.width),
            source_height: Some(image.height),
            x_offset: 0,
            y_offset: 0,
            width: Some(4),
            height: Some(HEADLESS_IMAGE_ROWS),
            pixel_width: Some(image.width),
            pixel_height: Some(image.height),
            z_index: -1,
        },
        image: image.clone(),
        pixel_rect: None,
        source_uv: None,
    });
    for row in 0..HEADLESS_IMAGE_ROWS {
        presentations.push(termeric_core::PresentationImage {
            placement: termeric_core::ImagePlacement {
                image_id: image.id,
                placement_id: Some(2),
                col: 4,
                row: row_origin + i64::from(row),
                alternate_screen: false,
                source_x: 0,
                source_y: row * HEADLESS_IMAGE_CELL_HEIGHT,
                source_width: Some(image.width),
                source_height: Some(HEADLESS_IMAGE_CELL_HEIGHT),
                x_offset: 0,
                y_offset: 0,
                width: Some(4),
                height: Some(1),
                pixel_width: Some(image.width),
                pixel_height: Some(HEADLESS_IMAGE_CELL_HEIGHT),
                // Kitty Unicode placeholders are specified as text-backed
                // virtual placements and render below the placeholder text.
                z_index: -1,
            },
            image: image.clone(),
            pixel_rect: Some([
                0.0,
                0.0,
                image.width as f32,
                HEADLESS_IMAGE_CELL_HEIGHT as f32,
            ]),
            source_uv: Some([
                0.0,
                row as f32 / HEADLESS_IMAGE_ROWS as f32,
                1.0,
                (row + 1) as f32 / HEADLESS_IMAGE_ROWS as f32,
            ]),
        });
    }
    presentations
}

#[cfg(test)]
fn render_headless_image_state(
    device: &Device,
    queue: &Queue,
    pipeline: &mut ImagePipeline,
    key: (usize, termeric_core::ImageId),
    presentations: &[termeric_core::PresentationImage],
    scroll_offset: usize,
) -> Result<Vec<u8>> {
    pipeline.clear();
    for presentation in presentations {
        if let Some(geometry) = presentation_geometry(
            presentation,
            8.0,
            HEADLESS_IMAGE_CELL_HEIGHT as f32,
            scroll_offset,
            Some((HEADLESS_IMAGE_WIDTH as f32, HEADLESS_IMAGE_HEIGHT as f32)),
        ) {
            pipeline.push_image(
                key,
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                geometry.u_min,
                geometry.v_min,
                geometry.u_max,
                geometry.v_max,
                1.0,
                presentation.placement.z_index,
            );
        }
    }
    pipeline.upload(device, queue);

    let target = device.create_texture(&TextureDescriptor {
        label: Some("Kitty multi-row graphics smoke target"),
        size: Extent3d {
            width: HEADLESS_IMAGE_WIDTH,
            height: HEADLESS_IMAGE_HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Kitty multi-row graphics smoke readback"),
        size: u64::from(HEADLESS_IMAGE_WIDTH * HEADLESS_IMAGE_HEIGHT * 4),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("Kitty multi-row graphics smoke encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Kitty multi-row graphics smoke pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pipeline.draw_below_text(&mut pass);
        pipeline.draw_above_text(&mut pass);
    }
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(HEADLESS_IMAGE_WIDTH * 4),
                rows_per_image: Some(HEADLESS_IMAGE_HEIGHT),
            },
        },
        Extent3d {
            width: HEADLESS_IMAGE_WIDTH,
            height: HEADLESS_IMAGE_HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
    let _ = device.poll(Maintain::wait_for(submission));
    receiver.recv()?.map_err(|error| anyhow::anyhow!(error))?;
    let pixels = readback.slice(..).get_mapped_range().to_vec();
    readback.unmap();
    Ok(pixels)
}

#[cfg(test)]
fn verify_headless_image_state(
    pixels: &[u8],
    source_row_offset: u32,
    visible_rows: u32,
    label: &str,
) -> Result<()> {
    anyhow::ensure!(
        pixels.len() == (HEADLESS_IMAGE_WIDTH * HEADLESS_IMAGE_HEIGHT * 4) as usize,
        "{label} graphics readback has the wrong size"
    );
    let half_width = HEADLESS_IMAGE_WIDTH / 2;
    let visible_height = visible_rows * HEADLESS_IMAGE_CELL_HEIGHT;
    for y in 0..HEADLESS_IMAGE_HEIGHT {
        for x in 0..half_width {
            let left_offset = ((y * HEADLESS_IMAGE_WIDTH + x) * 4) as usize;
            let right_offset = ((y * HEADLESS_IMAGE_WIDTH + x + half_width) * 4) as usize;
            let left = &pixels[left_offset..left_offset + 4];
            let right = &pixels[right_offset..right_offset + 4];
            if y < visible_height {
                let expected =
                    headless_image_row_color(source_row_offset + y / HEADLESS_IMAGE_CELL_HEIGHT);
                for channel in 0..4 {
                    anyhow::ensure!(
                        left[channel].abs_diff(expected[channel]) <= 3,
                        "{label} classic image row {} channel {channel} was {} instead of {}",
                        y / HEADLESS_IMAGE_CELL_HEIGHT,
                        left[channel],
                        expected[channel]
                    );
                    anyhow::ensure!(
                        right[channel].abs_diff(expected[channel]) <= 3,
                        "{label} Unicode-placeholder row {} channel {channel} was {} instead of {}",
                        y / HEADLESS_IMAGE_CELL_HEIGHT,
                        right[channel],
                        expected[channel]
                    );
                }
            } else {
                anyhow::ensure!(
                    left[..3].iter().all(|channel| *channel == 0)
                        && right[..3].iter().all(|channel| *channel == 0),
                    "{label} painted outside the expected scrolled image footprint at row {y}"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn expected_headless_image_state(source_row_offset: u32, visible_rows: u32) -> Vec<u8> {
    let mut pixels = vec![0; (HEADLESS_IMAGE_WIDTH * HEADLESS_IMAGE_HEIGHT * 4) as usize];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    let visible_height = visible_rows * HEADLESS_IMAGE_CELL_HEIGHT;
    for y in 0..visible_height {
        let color = headless_image_row_color(source_row_offset + y / HEADLESS_IMAGE_CELL_HEIGHT);
        for x in 0..HEADLESS_IMAGE_WIDTH {
            let offset = ((y * HEADLESS_IMAGE_WIDTH + x) * 4) as usize;
            pixels[offset..offset + 4].copy_from_slice(&color);
        }
    }
    pixels
}

#[cfg(test)]
mod headless_image_readback_tests {
    use super::*;

    #[test]
    fn verifier_rejects_a_row_at_a_time_placeholder_frame() {
        let mut pixels = expected_headless_image_state(0, HEADLESS_IMAGE_ROWS);
        let half_width = HEADLESS_IMAGE_WIDTH / 2;
        for y in HEADLESS_IMAGE_CELL_HEIGHT..HEADLESS_IMAGE_HEIGHT {
            for x in half_width..HEADLESS_IMAGE_WIDTH {
                let offset = ((y * HEADLESS_IMAGE_WIDTH + x) * 4) as usize;
                pixels[offset..offset + 3].fill(0);
            }
        }

        let error =
            verify_headless_image_state(&pixels, 0, HEADLESS_IMAGE_ROWS, "row-at-a-time fixture")
                .unwrap_err();
        assert!(error.to_string().contains("Unicode-placeholder row 1"));
    }

    #[test]
    fn verifier_rejects_pixels_below_the_scrolled_footprint() {
        let visible_rows = HEADLESS_IMAGE_ROWS - 3;
        let mut pixels = expected_headless_image_state(3, visible_rows);
        let first_hidden_y = visible_rows * HEADLESS_IMAGE_CELL_HEIGHT;
        let offset = (first_hidden_y * HEADLESS_IMAGE_WIDTH * 4) as usize;
        pixels[offset] = 1;

        let error =
            verify_headless_image_state(&pixels, 3, visible_rows, "scrolled fixture").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("painted outside the expected scrolled image footprint")
        );
    }
}

/// Read back complete multi-row classic and Unicode-placeholder images before
/// and after terminal scrolling. Every pixel row must be present in the first
/// frame; scrolling must clip it once and restoring scrollback must reveal the
/// complete image again without incrementally discovering rows.
#[cfg(test)]
pub(crate) fn headless_image_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    let mut pipeline = ImagePipeline::new(device, TextureFormat::Rgba8Unorm)?;
    pipeline.resize(queue, HEADLESS_IMAGE_WIDTH, HEADLESS_IMAGE_HEIGHT);
    let image = headless_image_fixture();
    let key = (1, image.id);
    pipeline.upload_image(device, queue, key, &image);

    let initial = headless_image_presentations(&image, 0);
    let pixels = render_headless_image_state(device, queue, &mut pipeline, key, &initial, 0)?;
    verify_headless_image_state(&pixels, 0, HEADLESS_IMAGE_ROWS, "initial frame")?;

    let scrolled = headless_image_presentations(&image, -3);
    let pixels = render_headless_image_state(device, queue, &mut pipeline, key, &scrolled, 0)?;
    verify_headless_image_state(&pixels, 3, HEADLESS_IMAGE_ROWS - 3, "output scroll")?;

    let pixels = render_headless_image_state(device, queue, &mut pipeline, key, &scrolled, 3)?;
    verify_headless_image_state(&pixels, 0, HEADLESS_IMAGE_ROWS, "scrollback restore")?;
    image_crop_smoke::run(device, queue)?;
    Ok(())
}

/// Theme hex values and image pixels must reach the frame byte for byte. The
/// target uses the view format selected for an sRGB-only surface, the hardest
/// case: rendering through an sRGB view or sampling an sRGB-decoding texture
/// lightens every mid-tone (Turnip's `#18151A` background presents as
/// `#56515A`), which fails the exact comparisons below.
#[cfg(test)]
pub(crate) fn headless_theme_color_fidelity_smoke(device: &Device, queue: &Queue) -> Result<()> {
    use crate::config::theme::Color as ThemeColor;

    // Readback rows must be 256-byte aligned: 64 RGBA pixels.
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 4;
    let background = ThemeColor::hex("#18151A");
    let rect_color = ThemeColor::hex("#9B5BA0");
    let image_pixel = [0x7B, 0x9B, 0x5C, 0xFF];
    let format = select_surface_formats(&[TextureFormat::Rgba8UnormSrgb])
        .ok_or_else(|| anyhow::anyhow!("sRGB-only surface produced no render format"))?
        .render;

    let mut rects = RectPipeline::new(device, format)?;
    rects.resize(queue, WIDTH, HEIGHT);
    rects.push_rect(0.0, 0.0, 16.0, HEIGHT as f32, rect_color.to_f32_array());
    rects.upload(device, queue);

    let mut images = ImagePipeline::new(device, format)?;
    images.resize(queue, WIDTH, HEIGHT);
    let image = termeric_core::StoredImage {
        id: termeric_core::ImageId(1),
        width: 1,
        height: 1,
        data: image_pixel.to_vec().into(),
        revision: 1,
    };
    let key = (1, image.id);
    images.upload_image(device, queue, key, &image);
    images.push_image(
        key,
        16.0,
        0.0,
        16.0,
        HEIGHT as f32,
        0.0,
        0.0,
        1.0,
        1.0,
        1.0,
        0,
    );
    images.upload(device, queue);

    let extent = Extent3d {
        width: WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&TextureDescriptor {
        label: Some("Theme color fidelity target"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Theme color fidelity readback"),
        size: u64::from(WIDTH * HEIGHT * 4),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("Theme color fidelity encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Theme color fidelity pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(background.to_wgpu()),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rects.draw_layer(&mut pass, 0);
        images.draw_above_text(&mut pass);
    }
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: Some(HEIGHT),
            },
        },
        extent,
    );
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
    let _ = device.poll(Maintain::wait_for(submission));
    receiver.recv()?.map_err(|error| anyhow::anyhow!(error))?;
    let pixels = readback.slice(..).get_mapped_range().to_vec();
    readback.unmap();

    let rect_rgb = [rect_color.r, rect_color.g, rect_color.b];
    let image_rgb = [image_pixel[0], image_pixel[1], image_pixel[2]];
    let background_rgb = [background.r, background.g, background.b];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (label, expected) = match x {
                0..16 => ("theme rectangle", rect_rgb),
                16..32 => ("image pixel", image_rgb),
                _ => ("theme background", background_rgb),
            };
            let offset = ((y * WIDTH + x) * 4) as usize;
            let actual = &pixels[offset..offset + 3];
            anyhow::ensure!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| actual.abs_diff(expected) <= 1),
                "{label} at ({x}, {y}) presented {actual:?}, expected {expected:?}"
            );
        }
    }
    Ok(())
}

/// Chrome labels shape in the desktop's UI face through the shared atlas,
/// measure consistently with what is drawn, and land on whole pixels.
#[cfg(test)]
pub(crate) fn headless_ui_text_smoke(device: &Device, queue: &Queue) -> Result<()> {
    let mut glyph_cache = GlyphCache::new(
        device,
        queue,
        16.0,
        "monospace",
        &[],
        &[],
        400,
        700,
        1.2,
        false,
    )?;
    let size = physical_font_size(ui_text::UI_LABEL_POINTS, 1.0);
    let metrics = glyph_cache.ui_font_metrics(size);
    anyhow::ensure!(
        metrics.ascent > metrics.descent && metrics.descent > 0.0,
        "implausible UI font metrics {metrics:?}"
    );
    let width = glyph_cache.ui_text_width("~/dev/termeric", size);
    anyhow::ensure!(width > size * 4.0, "UI label measured only {width}px");
    anyhow::ensure!(
        glyph_cache.ui_text_width("~/dev/termeric", size) == width,
        "UI width memo changed its answer"
    );
    let baseline = metrics.centered_baseline(0.0, 34.0);
    let mut text = TextPipeline::new(device, TextureFormat::Rgba8Unorm, &glyph_cache)?;
    text.push_ui_text("~/dev/termeric", 10.4, baseline, size, [1.0; 4], None);
    let queued = text.take_queued_since(0);
    let batch = TextPipeline::build_batch(queued, queue, &mut glyph_cache);
    anyhow::ensure!(
        batch.vertices.len() >= 4 * 12,
        "UI label produced {} vertices",
        batch.vertices.len()
    );
    for vertex in &batch.vertices {
        anyhow::ensure!(
            vertex.position[0].fract() == 0.0 && vertex.position[1].fract() == 0.0,
            "UI glyph quad at fractional position {:?}",
            vertex.position
        );
    }
    let right = batch
        .vertices
        .iter()
        .map(|vertex| vertex.position[0])
        .fold(f32::MIN, f32::max);
    anyhow::ensure!(
        (right - (10.0 + width)).abs() <= size,
        "drawn label ends at {right}, measured {width} from 10"
    );
    Ok(())
}

/// Shape and submit representative multi-scalar terminal graphemes through the
/// real text atlas/pipeline. The queued-content assertions act as a small
/// geometry golden: no case may collapse to its first scalar before shaping.
#[cfg(test)]
pub(crate) fn headless_unicode_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    const WIDTH: u32 = 512;
    const HEIGHT: u32 = 96;
    let mut glyph_cache = GlyphCache::new(
        device,
        queue,
        14.0,
        "monospace",
        &[],
        &[],
        400,
        700,
        1.2,
        false,
    )?;
    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    text.resize(queue, WIDTH, HEIGHT);
    let cases = [
        "e\u{301}",
        "界",
        "☺\u{fe0f}",
        "🇺🇸",
        "👍🏽",
        "👩\u{200d}💻",
        // Complete Unicode output of the VT100 DEC Special Graphics range
        // `_` through `~`; mapping itself is golden-tested in terminal core.
        " ",
        "◆",
        "▒",
        "␉",
        "␌",
        "␍",
        "␊",
        "°",
        "±",
        "␤",
        "␋",
        "┘",
        "┐",
        "┌",
        "└",
        "┼",
        "⎺",
        "⎻",
        "─",
        "⎼",
        "⎽",
        "├",
        "┤",
        "┴",
        "┬",
        "│",
        "≤",
        "≥",
        "π",
        "≠",
        "£",
        "·",
    ];
    for (index, grapheme) in cases.iter().enumerate() {
        text.push_grapheme(
            grapheme,
            8.0 + (index % 16) as f32 * 30.0,
            8.0 + (index / 16) as f32 * 28.0,
            [0.92, 0.88, 0.78, 1.0],
        );
    }
    let queued = text.take_queued_since(0);
    anyhow::ensure!(
        queued.len() == cases.len(),
        "Unicode golden queue lost a cluster"
    );
    for (queued, expected) in queued.iter().zip(cases) {
        match &queued.content {
            text::QueuedContent::Grapheme(actual) => {
                anyhow::ensure!(actual.as_ref() == expected, "Unicode golden was truncated");
            }
            text::QueuedContent::Char(actual) => {
                let mut characters = expected.chars();
                anyhow::ensure!(
                    characters.next() == Some(*actual) && characters.next().is_none(),
                    "multi-scalar golden became one char"
                );
            }
            text::QueuedContent::Run { .. } => {
                anyhow::bail!("standalone Unicode golden unexpectedly became a row run");
            }
            text::QueuedContent::MultiCell { .. } => {
                anyhow::bail!("standalone Unicode golden unexpectedly became a multicell");
            }
            text::QueuedContent::UiText { .. } => {
                anyhow::bail!("terminal Unicode golden unexpectedly became UI text");
            }
        }
    }
    let batch = TextPipeline::build_batch(queued, queue, &mut glyph_cache);
    anyhow::ensure!(
        !batch.vertices.is_empty(),
        "Unicode golden produced no glyph geometry"
    );

    for (index, grapheme) in cases.iter().enumerate() {
        text.push_grapheme(
            grapheme,
            8.0 + (index % 16) as f32 * 30.0,
            8.0 + (index / 16) as f32 * 28.0,
            [0.92, 0.88, 0.78, 1.0],
        );
    }
    text.upload(device, queue, &mut glyph_cache);
    let target = device.create_texture(&TextureDescriptor {
        label: Some("Termeric Unicode render smoke target"),
        size: Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("Termeric Unicode render smoke encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Termeric Unicode render smoke pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        text.draw_layer(&mut pass, &glyph_cache, 0);
    }
    queue.submit([encoder.finish()]);
    Ok(())
}

/// Load a real COLR/CBDT emoji font and carry its RGBA pixels through the
/// dedicated atlas, vertex flag, bind group, and shader compilation path.
#[cfg(test)]
pub(crate) fn headless_color_emoji_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    let font_path = [
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
        "/usr/share/fonts/noto/NotoColorEmoji.ttf",
    ]
    .into_iter()
    .map(std::path::Path::new)
    .find(|path| path.is_file())
    .ok_or_else(|| anyhow::anyhow!("Noto Color Emoji is required for the GPU render smoke"))?;
    let font_data = std::fs::read(font_path)?;
    let mut glyph_cache = GlyphCache::new(
        device,
        queue,
        32.0,
        "monospace",
        &[],
        &[],
        400,
        700,
        1.2,
        false,
    )?;
    glyph_cache.install_test_font(font_data, "Noto Color Emoji", queue);
    glyph_cache.begin_frame();

    let run = glyph_cache
        .shape_grapheme_styled("😀", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("Noto did not shape the emoji fixture"))?;
    anyhow::ensure!(!run.glyphs.is_empty(), "emoji fixture produced no glyphs");
    anyhow::ensure!(
        run.glyphs.iter().all(|glyph| glyph.entry.color),
        "emoji RGBA data fell back to the monochrome atlas"
    );

    let mut text = TextPipeline::new(device, TextureFormat::Rgba8Unorm, &glyph_cache)?;
    text.resize(queue, 96, 64);
    text.push_grapheme("😀", 8.0, 8.0, [0.1, 0.9, 0.2, 1.0]);
    text.upload(device, queue, &mut glyph_cache);
    let metrics = glyph_cache.atlas_metrics();
    anyhow::ensure!(metrics.color_pages >= 1 && metrics.max_color_pages == 4);
    anyhow::ensure!(metrics.capacity_bytes == 24 * 1024 * 1024);
    anyhow::ensure!(glyph_cache.rasterizer_metrics().retained_images == 0);
    anyhow::ensure!(glyph_cache.rasterizer_metrics().retained_image_bytes == 0);
    Ok(())
}

/// Force the real raster/text path beyond a tiny multi-page atlas. Every
/// scalar must still produce visible geometry, even when all frame-safe pages
/// are pinned and the permanent replacement glyph is required.
#[cfg(test)]
pub(crate) fn headless_glyph_atlas_stress_smoke(device: &Device, queue: &Queue) -> Result<()> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    const GLYPHS: usize = 2_048;
    let mut glyph_cache = GlyphCache::new(
        device,
        queue,
        18.0,
        "monospace",
        &[],
        &[],
        400,
        700,
        1.2,
        false,
    )?;
    glyph_cache.use_tiny_test_atlas(64, 3, queue);
    glyph_cache.begin_frame();

    let mut queued = Vec::with_capacity(GLYPHS);
    for index in 0..GLYPHS {
        // Seed enough installed printable glyph/style identities to fill the
        // tiny pages, then broaden the workload with CJK ideographs, which
        // draw through system fallback or the permanent replacement. (A range
        // with invisible scalars such as Hangul fillers would legitimately
        // draw nothing for them on systems with those fonts.)
        let scalar = if index < 94 * 4 {
            0x21 + (index % 94) as u32
        } else {
            0x4E00 + (index % 512) as u32
        };
        let character = char::from_u32(scalar).expect("stress scalar is valid");
        let style_index = (index / 94) % 4;
        queued.push(text::QueuedChar {
            content: text::QueuedContent::Char(character),
            x: (index % 64) as f32 * 8.0,
            y: (index / 64) as f32 * 10.0,
            color: [0.9, 0.8, 0.7, 1.0],
            style: glyph_cache::FontStyle {
                bold: style_index % 2 == 1,
                italic: style_index >= 2,
            },
            clip: None,
        });
    }
    let batch = TextPipeline::build_batch(queued, queue, &mut glyph_cache);
    anyhow::ensure!(
        batch.vertices.len() == GLYPHS * 4,
        "atlas pressure dropped glyph geometry"
    );
    anyhow::ensure!(
        batch.atlas_pages.len() > 1,
        "stress never crossed an atlas page"
    );
    let metrics = glyph_cache.atlas_metrics();
    anyhow::ensure!(metrics.pages == 3 && metrics.max_pages == 3);
    anyhow::ensure!(
        metrics.allocation_failures > 0,
        "stress never exercised the visible replacement path"
    );

    // A pressure miss is temporary: repeated content in the same frame must
    // reuse the visible replacement without rerasterizing, then retry after
    // frame pins are released. Exercise both the shaped-run and scalar paths.
    // Force retry runs to bypass their byte budget. Per-glyph deferral must
    // still suppress rerasterization and allow next-frame recovery.
    glyph_cache.set_shaping_byte_limits_for_test(8 * 1024 * 1024, 1);
    let failures_before_probe = metrics.allocation_failures;
    let pressure_run = glyph_cache
        .shape_run_styled("é", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("pressure-shaped glyph should remain renderable"))?;
    anyhow::ensure!(pressure_run.glyphs.len() == 1);
    let after_shaped_pressure = glyph_cache.atlas_metrics();
    anyhow::ensure!(after_shaped_pressure.allocation_failures > failures_before_probe);
    let shaped_rasterizations = glyph_cache.rasterizer_metrics().rasterizations;
    let _ = glyph_cache
        .shape_run_styled("é", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("deferred shaped glyph should remain renderable"))?;
    anyhow::ensure!(
        glyph_cache.atlas_metrics().allocation_failures
            == after_shaped_pressure.allocation_failures,
        "same-frame shaped pressure rerasterized a known deferred glyph"
    );
    anyhow::ensure!(glyph_cache.rasterizer_metrics().rasterizations == shaped_rasterizations);
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().deferred_entries == 0);
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().deferred_bytes == 0);
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().skipped_runs >= 2);

    let _ = glyph_cache
        .get_or_insert_styled('ñ', glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("pressure scalar should remain renderable"))?;
    let after_scalar_pressure = glyph_cache.atlas_metrics();
    let scalar_rasterizations = glyph_cache.rasterizer_metrics().rasterizations;
    anyhow::ensure!(
        after_scalar_pressure.allocation_failures > after_shaped_pressure.allocation_failures
    );
    let _ = glyph_cache
        .get_or_insert_styled('ñ', glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("deferred scalar should remain renderable"))?;
    anyhow::ensure!(
        glyph_cache.atlas_metrics().allocation_failures
            == after_scalar_pressure.allocation_failures,
        "same-frame scalar pressure rerasterized a known deferred glyph"
    );
    anyhow::ensure!(glyph_cache.rasterizer_metrics().rasterizations == scalar_rasterizations);

    let evictions_before_retry = after_scalar_pressure.evictions;
    glyph_cache.begin_frame();
    let _ = glyph_cache
        .shape_run_styled("é", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("next-frame shaped retry should render"))?;
    let _ = glyph_cache
        .get_or_insert_styled('ñ', glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("next-frame scalar retry should render"))?;
    anyhow::ensure!(
        glyph_cache.atlas_metrics().evictions > evictions_before_retry,
        "next-frame pressure retry never reclaimed an unpinned page"
    );

    // Rebuild at a different font size to prove old raster entries are not
    // reused across size/scale identity changes.
    let generation = glyph_cache.atlas_generation();
    glyph_cache.set_font_size(22.0, queue);
    anyhow::ensure!(glyph_cache.atlas_generation() != generation);
    let regular = glyph_cache
        .get_or_insert_styled('W', glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("resized glyph should remain renderable"))?;
    let bold = glyph_cache
        .get_or_insert_styled(
            'W',
            glyph_cache::FontStyle {
                bold: true,
                italic: false,
            },
            queue,
        )
        .ok_or_else(|| anyhow::anyhow!("resized styled glyph should remain renderable"))?;
    anyhow::ensure!(regular.width > 0 && bold.width > 0);
    let before_hits = glyph_cache.rasterizer_metrics();
    let again = glyph_cache.get_or_insert_styled('W', glyph_cache::FontStyle::default(), queue);
    anyhow::ensure!(again == Some(regular));
    anyhow::ensure!(glyph_cache.rasterizer_metrics() == before_hits);
    anyhow::ensure!(before_hits.retained_images == 0 && before_hits.retained_image_bytes == 0);
    let shaped = glyph_cache.shape_run_styled("W", glyph_cache::FontStyle::default(), queue);
    anyhow::ensure!(shaped.is_some());
    let before_shaped_hit = glyph_cache.rasterizer_metrics();
    let shaped_again = glyph_cache.shape_run_styled("W", glyph_cache::FontStyle::default(), queue);
    anyhow::ensure!(shaped_again.is_some());
    anyhow::ensure!(glyph_cache.rasterizer_metrics() == before_shaped_hit);
    glyph_cache.clear(queue);
    glyph_cache.begin_frame();
    let shared = glyph_cache
        .shape_run_styled("W", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("cross-path glyph should shape"))?;
    anyhow::ensure!(shared.glyphs.len() == 1);
    let before_cross_path = glyph_cache.rasterizer_metrics();
    let scalar = glyph_cache.get_or_insert_styled('W', glyph_cache::FontStyle::default(), queue);
    anyhow::ensure!(scalar == Some(shared.glyphs[0].entry));
    anyhow::ensure!(
        glyph_cache.rasterizer_metrics() == before_cross_path,
        "scalar alias rerasterized an existing shaped atlas entry"
    );
    glyph_cache.set_shaping_byte_limits_for_test(1, 1);
    let uncached = glyph_cache
        .shape_run_styled("WWW", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("over-budget run must still render"))?;
    anyhow::ensure!(uncached.glyphs.len() == 3);
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().entries == 0);
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().retained_bytes == 0);

    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    text.resize(queue, 64, 64);
    text.push_char('W', 8.0, 8.0, [1.0; 4]);
    text.upload(device, queue, &mut glyph_cache);
    let target = device.create_texture(&TextureDescriptor {
        label: Some("Termeric atlas stress target"),
        size: Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("Termeric atlas stress encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Termeric atlas stress pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        text.draw_layer(&mut pass, &glyph_cache, 0);
    }
    queue.submit([encoder.finish()]);
    Ok(())
}

#[cfg(test)]
fn queue_styled_test_run(
    text: &mut TextPipeline,
    content: &str,
    origin_x: f32,
    origin_y: f32,
    cell_width: f32,
    color: [f32; 4],
    style: glyph_cache::FontStyle,
    clip: TextClip,
) {
    let mut run = TextRunAccumulator::default();
    for (index, character) in content.chars().enumerate() {
        run.push(
            text,
            character,
            origin_x + index as f32 * cell_width,
            origin_y,
            color,
            style,
            clip,
            cell_width,
        );
    }
    run.flush(text);
}

#[cfg(test)]
fn queue_test_run(text: &mut TextPipeline, content: &str, cell_width: f32, clip: TextClip) {
    queue_styled_test_run(
        text,
        content,
        12.0,
        12.0,
        cell_width,
        [0.92, 0.88, 0.78, 1.0],
        glyph_cache::FontStyle::default(),
        clip,
    );
}

#[cfg(test)]
fn render_text_snapshot(
    device: &Device,
    queue: &Queue,
    text: &mut TextPipeline,
    glyph_cache: &mut GlyphCache,
    content: &str,
) -> Result<Vec<u8>> {
    render_queued_text_snapshot(device, queue, text, glyph_cache, |text, cell_width| {
        queue_styled_test_run(
            text,
            content,
            12.0,
            12.0,
            cell_width,
            [0.92, 0.88, 0.78, 1.0],
            glyph_cache::FontStyle::default(),
            TextClip {
                left: 0.0,
                top: 0.0,
                right: 256.0,
                bottom: 64.0,
            },
        );
    })
}

#[cfg(test)]
fn render_queued_text_snapshot(
    device: &Device,
    queue: &Queue,
    text: &mut TextPipeline,
    glyph_cache: &mut GlyphCache,
    queue_content: impl FnOnce(&mut TextPipeline, f32),
) -> Result<Vec<u8>> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 64;
    text.clear();
    let (cell_width, _) = glyph_cache.cell_size();
    queue_content(text, cell_width);
    text.upload(device, queue, glyph_cache);

    let target = device.create_texture(&TextureDescriptor {
        label: Some("Termeric ligature snapshot target"),
        size: Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Termeric ligature snapshot readback"),
        size: u64::from(WIDTH * HEIGHT * 4),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("Termeric ligature snapshot encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Termeric ligature snapshot pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        text.draw_layer(&mut pass, glyph_cache, 0);
    }
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: Some(HEIGHT),
            },
        },
        Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
    let _ = device.poll(Maintain::wait_for(submission));
    receiver.recv()?.map_err(|error| anyhow::anyhow!(error))?;
    let pixels = readback.slice(..).get_mapped_range().to_vec();
    readback.unmap();
    Ok(pixels)
}

/// Produce a small, deterministic renderer snapshot for the GPU render
/// smoke's visual baseline. The bundled test font removes host font discovery
/// from the baseline while retaining Termeric's real atlas, shaping, upload,
/// shader, blending, and GPU readback paths.
#[cfg(test)]
pub(crate) fn headless_gui_visual_snapshot(
    device: &Device,
    queue: &Queue,
) -> Result<(Vec<u8>, u32, u32)> {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 64;
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    let mut glyph_cache = GlyphCache::new(
        device,
        queue,
        16.0,
        "monospace",
        &[],
        &[],
        400,
        700,
        1.2,
        false,
    )?;
    glyph_cache.install_test_font(
        glyph_cache::deterministic_test_font_data(),
        "Fira Mono",
        queue,
    );
    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    text.resize(queue, WIDTH, HEIGHT);
    let pixels = render_text_snapshot(
        device,
        queue,
        &mut text,
        &mut glyph_cache,
        "Termeric GUI != -> 17",
    )?;
    Ok((pixels, WIDTH, HEIGHT))
}

/// Render deterministic enabled/disabled ligature snapshots through the real
/// atlas and GPU pipeline, and verify unchanged frames reuse shaped runs.
#[cfg(test)]
pub(crate) fn headless_ligature_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    let mut glyph_cache = GlyphCache::new(
        device,
        queue,
        16.0,
        "monospace",
        &[],
        &[],
        400,
        700,
        1.2,
        true,
    )?;
    glyph_cache.install_test_font(
        glyph_cache::deterministic_test_font_data(),
        "Fira Mono",
        queue,
    );
    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    text.resize(queue, 256, 64);

    let enabled = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "ffi")?;
    let repeated = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "ffi")?;
    anyhow::ensure!(
        enabled == repeated,
        "unchanged ligature snapshot was unstable"
    );
    let warm_metrics = glyph_cache.shaping_cache_metrics();
    anyhow::ensure!(warm_metrics.misses == 1 && warm_metrics.hits >= 1);

    glyph_cache.set_ligatures(false);
    let disabled = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "ffi")?;
    anyhow::ensure!(
        enabled != disabled,
        "ligature setting did not change rendered pixels"
    );
    let toggled_metrics = glyph_cache.shaping_cache_metrics();
    anyhow::ensure!(toggled_metrics.misses == 2 && toggled_metrics.entries == 1);

    glyph_cache.set_ligatures(true);
    let explicit_disabled = ["liga", "clig", "calt", "dlig"]
        .into_iter()
        .map(|tag| (tag.to_string(), 0))
        .collect::<BTreeMap<_, _>>();
    anyhow::ensure!(glyph_cache.set_font_features(&explicit_disabled));
    anyhow::ensure!(
        glyph_cache.shaping_cache_metrics().entries == 0,
        "OpenType feature change retained stale shaping entries"
    );
    let overridden = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "ffi")?;
    anyhow::ensure!(
        overridden == disabled,
        "explicit OpenType values did not override the ligature preset"
    );
    anyhow::ensure!(!glyph_cache.set_font_features(&explicit_disabled));
    anyhow::ensure!(
        glyph_cache.shaping_cache_metrics().entries == 1,
        "assigning identical OpenType features invalidated a warm run"
    );
    glyph_cache.set_font_features(&BTreeMap::new());

    let mixed_snapshot =
        |text: &mut TextPipeline, glyph_cache: &mut GlyphCache| -> Result<Vec<u8>> {
            render_queued_text_snapshot(device, queue, text, glyph_cache, |text, cell_width| {
                let clip = TextClip {
                    left: 0.0,
                    top: 0.0,
                    right: 218.0,
                    bottom: 64.0,
                };
                queue_styled_test_run(
                    text,
                    "ffi",
                    12.0,
                    12.0,
                    cell_width,
                    [0.92, 0.88, 0.78, 1.0],
                    glyph_cache::FontStyle::default(),
                    clip,
                );
                queue_styled_test_run(
                    text,
                    "ffi",
                    84.0,
                    12.0,
                    cell_width,
                    [0.35, 0.78, 0.62, 1.0],
                    glyph_cache::FontStyle {
                        bold: true,
                        italic: false,
                    },
                    clip,
                );
                queue_styled_test_run(
                    text,
                    "ffi",
                    174.0,
                    12.0,
                    cell_width,
                    [0.78, 0.52, 0.86, 1.0],
                    glyph_cache::FontStyle {
                        bold: false,
                        italic: true,
                    },
                    clip,
                );
            })
        };

    glyph_cache.set_ligatures(true);
    let mixed_enabled = mixed_snapshot(&mut text, &mut glyph_cache)?;
    let mixed_repeated = mixed_snapshot(&mut text, &mut glyph_cache)?;
    anyhow::ensure!(
        mixed_enabled == mixed_repeated,
        "mixed-style ligature snapshot was unstable"
    );
    glyph_cache.set_ligatures(false);
    let mixed_disabled = mixed_snapshot(&mut text, &mut glyph_cache)?;
    anyhow::ensure!(
        mixed_enabled != mixed_disabled,
        "mixed-style ligature snapshot did not change when features were disabled"
    );

    let metrics_before_ranges = glyph_cache.cell_size();
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().entries > 0);
    glyph_cache.set_font_families(
        "Fira Mono".into(),
        &[],
        &[crate::config::FontRangeOverride {
            start: u32::from('0'),
            end: u32::from('9'),
            family: "Fira Mono".into(),
        }],
        queue,
    );
    anyhow::ensure!(
        glyph_cache.shaping_cache_metrics().entries == 0,
        "font-range policy change retained stale shaping entries"
    );
    anyhow::ensure!(
        glyph_cache.cell_size() == metrics_before_ranges,
        "font-range policy changed primary cell metrics"
    );
    let ranged = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "012")?;
    anyhow::ensure!(
        ranged.iter().any(|pixel| *pixel != 0),
        "font-range scalar/run path produced no pixels"
    );
    Ok(())
}

/// Exercise the parser-facing SGR style model through real glyph and rectangle
/// pipelines. The assertions are a portable geometry golden: they verify face
/// requests, foreground suppression, decoration primitive counts, and pane
/// clipping without depending on platform-specific font pixels.
#[cfg(test)]
pub(crate) fn headless_sgr_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 128;
    let theme = Theme::load_builtin("turmeric")?;
    let palette = TerminalPalette::from_theme(&theme);
    let dynamic_colors = termeric_core::DynamicColors::default();
    let mut glyph_cache = GlyphCache::new(
        device,
        queue,
        14.0,
        "monospace",
        &[],
        &[],
        400,
        700,
        1.2,
        false,
    )?;
    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    let mut rects = RectPipeline::new(device, FORMAT)?;
    text.resize(queue, WIDTH, HEIGHT);
    rects.resize(queue, WIDTH, HEIGHT);
    let (_, cell_height) = glyph_cache.cell_size();
    let clip = TextClip {
        left: 0.0,
        top: 0.0,
        right: 96.0,
        bottom: 64.0,
    };

    let text_cases = [
        (CellAttrs::empty(), false, false),
        (CellAttrs::BOLD, true, false),
        (CellAttrs::ITALIC, false, true),
        (CellAttrs::BOLD | CellAttrs::ITALIC, true, true),
        (CellAttrs::DIM, false, false),
        (CellAttrs::INVERSE, false, false),
    ];
    for (index, (attrs, bold, italic)) in text_cases.iter().copied().enumerate() {
        let mut cell = termeric_core::Cell::with_char((b'A' + index as u8) as char);
        cell.attrs = attrs;
        let style = ResolvedCellStyle::resolve(
            &palette,
            &dynamic_colors,
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        anyhow::ensure!(style.font.bold == bold && style.font.italic == italic);
        text.push_styled_char(
            (b'A' + index as u8) as char,
            8.0 + index as f32 * 13.0,
            8.0,
            style.text_color(None, 1.0),
            style.font,
            clip,
        );
    }

    for (attrs, phases) in [
        (CellAttrs::HIDDEN, TextBlinkPhases::default()),
        (
            CellAttrs::BLINK,
            TextBlinkPhases {
                slow_visible: false,
                rapid_visible: true,
            },
        ),
        (
            CellAttrs::RAPID_BLINK,
            TextBlinkPhases {
                slow_visible: true,
                rapid_visible: false,
            },
        ),
    ] {
        let mut cell = termeric_core::Cell::with_char('X');
        cell.attrs = attrs;
        let style = ResolvedCellStyle::resolve(&palette, &dynamic_colors, &cell, false, phases);
        anyhow::ensure!(
            !style.paint_foreground,
            "concealed SGR text would be painted"
        );
    }

    let queued = text.take_queued_since(0);
    anyhow::ensure!(queued.len() == text_cases.len());
    let text_batch = TextPipeline::build_batch(queued, queue, &mut glyph_cache);
    anyhow::ensure!(!text_batch.vertices.is_empty());
    anyhow::ensure!(text_batch.vertices.iter().all(|vertex| {
        vertex.position[0] >= clip.left
            && vertex.position[0] <= clip.right
            && vertex.position[1] >= clip.top
            && vertex.position[1] <= clip.bottom
    }));

    let decoration_goldens = [
        (CellAttrs::UNDERLINE, 1_usize),
        (CellAttrs::DOUBLE_UNDERLINE, 2),
        (CellAttrs::CURLY_UNDERLINE, 8),
        (CellAttrs::DOTTED_UNDERLINE, 4),
        (CellAttrs::DASHED_UNDERLINE, 2),
        (CellAttrs::STRIKE, 1),
        (CellAttrs::UNDERLINE | CellAttrs::STRIKE, 2),
    ];
    for (attrs, expected_rectangles) in decoration_goldens {
        let checkpoint = rects.checkpoint();
        let mut cell = termeric_core::Cell::with_char('x');
        cell.attrs = attrs;
        let style = ResolvedCellStyle::resolve(
            &palette,
            &dynamic_colors,
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        style::queue_sgr_decorations(&mut rects, style, 4.0, 48.0, 12.0, cell_height, 1.0, None);
        let batch = rects.take_since(checkpoint);
        anyhow::ensure!(
            batch.indices.len() / 6 == expected_rectangles,
            "SGR geometry golden changed for {attrs:?}"
        );
    }

    for (index, (attrs, _, _)) in text_cases.iter().copied().enumerate() {
        let mut cell = termeric_core::Cell::with_char((b'A' + index as u8) as char);
        cell.attrs = attrs;
        let style = ResolvedCellStyle::resolve(
            &palette,
            &dynamic_colors,
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        text.push_styled_char(
            (b'A' + index as u8) as char,
            8.0 + index as f32 * 13.0,
            8.0,
            style.text_color(None, 1.0),
            style.font,
            clip,
        );
    }
    let mut decorated = termeric_core::Cell::with_char('x');
    decorated.attrs = CellAttrs::CURLY_UNDERLINE | CellAttrs::STRIKE;
    let decorated_style = ResolvedCellStyle::resolve(
        &palette,
        &dynamic_colors,
        &decorated,
        false,
        TextBlinkPhases::default(),
    );
    style::queue_sgr_decorations(
        &mut rects,
        decorated_style,
        8.0,
        48.0,
        80.0,
        cell_height,
        1.0,
        None,
    );
    text.upload(device, queue, &mut glyph_cache);
    rects.upload(device, queue);

    let target = device.create_texture(&TextureDescriptor {
        label: Some("Termeric SGR render smoke target"),
        size: Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("Termeric SGR render smoke encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Termeric SGR render smoke pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rects.draw_layer(&mut pass, 0);
        text.draw_layer(&mut pass, &glyph_cache, 0);
    }
    queue.submit([encoder.finish()]);
    Ok(())
}

/// Exercise Termeric's real glyph, text, rectangle, upload, and draw pipelines
/// against an offscreen texture. Presentation and window-system latency remain
/// part of the optional process-level benchmark lane.
#[cfg(test)]
pub(crate) fn benchmark_headless_renderer() -> Result<Option<HeadlessRendererBenchmark>> {
    use std::time::Instant;

    const WIDTH: u32 = 1_280;
    const HEIGHT: u32 = 720;
    const COLS: usize = 120;
    const ROWS: usize = 40;
    const SAMPLES: usize = 5;
    const FRAMES_PER_SAMPLE: usize = 10;
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

    let instance_descriptor = InstanceDescriptor::from_env_or_default();
    let enabled_backends = instance_descriptor.backends;
    let instance = Instance::new(&instance_descriptor);
    let Some(adapter) = discover_adapter(
        &instance,
        enabled_backends,
        None,
        PowerPreference::LowPower,
        true,
    )
    .or_else(|| {
        discover_adapter(
            &instance,
            enabled_backends,
            None,
            PowerPreference::LowPower,
            false,
        )
    }) else {
        return Ok(None);
    };
    let adapter_info = adapter.get_info();
    let adapter_name = format!(
        "name={:?}; backend={:?}; device_type={:?}; driver={:?}; driver_info={:?}; vendor={:#06x}; device={:#06x}",
        adapter_info.name,
        adapter_info.backend,
        adapter_info.device_type,
        adapter_info.driver,
        adapter_info.driver_info,
        adapter_info.vendor,
        adapter_info.device,
    );
    let (device, queue) = pollster::block_on(adapter.request_device(
        &DeviceDescriptor {
            required_features: Features::empty(),
            required_limits: Limits::downlevel_defaults(),
            memory_hints: MemoryHints::default(),
            label: Some("Termeric benchmark device"),
        },
        None,
    ))?;

    let texture = device.create_texture(&TextureDescriptor {
        label: Some("Termeric benchmark target"),
        size: Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&TextureViewDescriptor::default());
    let mut glyph_cache = GlyphCache::new(
        &device,
        &queue,
        14.0,
        "monospace",
        &[],
        &[],
        400,
        700,
        1.2,
        false,
    )?;
    let mut text = TextPipeline::new(&device, FORMAT, &glyph_cache)?;
    let mut rect = RectPipeline::new(&device, FORMAT)?;
    text.resize(&queue, WIDTH, HEIGHT);
    rect.resize(&queue, WIDTH, HEIGHT);
    let (cell_width, cell_height) = glyph_cache.cell_size();
    let glyphs: Vec<char> =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789[]{}()<>+-=*/_界αβ"
            .chars()
            .collect();

    let mut glyph_warm_ms = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        glyph_cache.clear(&queue);
        let started = Instant::now();
        for &character in &glyphs {
            let _ = glyph_cache.get_or_insert(character, &queue);
        }
        glyph_warm_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    let before_common_hits = glyph_cache.atlas_metrics();
    for _ in 0..100 {
        for &character in &glyphs {
            let _ = glyph_cache.get_or_insert(character, &queue);
        }
    }
    let after_common_hits = glyph_cache.atlas_metrics();
    let common_hits = after_common_hits.hits - before_common_hits.hits;
    let common_misses = after_common_hits.misses - before_common_hits.misses;
    let common_hit_percent = common_hits as f64 / (common_hits + common_misses) as f64 * 100.0;
    assert!(common_hit_percent >= 99.0);
    let glyph_cache_hit_percent = vec![common_hit_percent; SAMPLES];

    let submit_frame =
        |text: &mut TextPipeline, rect: &mut RectPipeline, glyph_cache: &mut GlyphCache| -> f64 {
            let started = Instant::now();
            text.clear();
            rect.clear();
            for row in 0..ROWS {
                for col in 0..COLS {
                    let character = glyphs[(row * COLS + col) % glyphs.len()];
                    text.push_char(
                        character,
                        col as f32 * cell_width,
                        row as f32 * cell_height,
                        [0.92, 0.88, 0.78, 1.0],
                    );
                    if (row + col) % 23 == 0 {
                        rect.push_rect(
                            col as f32 * cell_width,
                            row as f32 * cell_height,
                            cell_width,
                            cell_height,
                            [0.18, 0.12, 0.08, 1.0],
                        );
                    }
                }
            }
            text.upload(&device, &queue, glyph_cache);
            rect.upload(&device, &queue);
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Termeric benchmark encoder"),
            });
            {
                let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                    label: Some("Termeric benchmark pass"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: Operations {
                            load: LoadOp::Clear(Color::BLACK),
                            store: StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rect.draw_layer(&mut pass, 0);
                text.draw_layer(&mut pass, glyph_cache, 0);
            }
            let submission = queue.submit([encoder.finish()]);
            let _ = device.poll(Maintain::wait_for(submission));
            started.elapsed().as_secs_f64() * 1_000.0
        };

    let _ = submit_frame(&mut text, &mut rect, &mut glyph_cache);
    let allocations_after_warmup = text.buffer_allocations() + rect.buffer_allocations();
    let mut frame_ms = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for _ in 0..FRAMES_PER_SAMPLE {
            let _ = submit_frame(&mut text, &mut rect, &mut glyph_cache);
        }
        frame_ms.push(started.elapsed().as_secs_f64() * 1_000.0 / FRAMES_PER_SAMPLE as f64);
    }
    let allocations_after_steady_frames = text.buffer_allocations() + rect.buffer_allocations();
    let steady_frame_buffer_allocations =
        allocations_after_steady_frames - allocations_after_warmup;
    assert_eq!(steady_frame_buffer_allocations, 0);

    // Measure the row-run path on code-heavy tokens after warming every
    // distinct content key. This isolates shaping/cache cost from GPU submit.
    glyph_cache.set_ligatures(true);
    let code_tokens = [
        "let", "result", "left", "!=", "right", "=>", "compute", "value", "===", "expected",
    ];
    let code_clip = TextClip {
        left: 0.0,
        top: 0.0,
        right: WIDTH as f32,
        bottom: HEIGHT as f32,
    };
    let build_code_tokens =
        |text: &mut TextPipeline, glyph_cache: &mut GlyphCache, repetitions: usize| {
            for _ in 0..repetitions {
                for token in code_tokens {
                    queue_test_run(text, token, cell_width, code_clip);
                }
            }
            let queued = text.take_queued_since(0);
            TextPipeline::build_batch(queued, &queue, glyph_cache)
        };
    let _ = build_code_tokens(&mut text, &mut glyph_cache, 1);
    let before = glyph_cache.shaping_cache_metrics();
    let mut ligature_code_ms = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let batch = build_code_tokens(&mut text, &mut glyph_cache, 100);
        assert!(!batch.vertices.is_empty());
        ligature_code_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    let after = glyph_cache.shaping_cache_metrics();
    let hits = after.hits - before.hits;
    let misses = after.misses - before.misses;
    let hit_percent = hits as f64 / (hits + misses) as f64 * 100.0;
    assert!(hit_percent >= 99.0);
    let ligature_cache_hit_percent = vec![hit_percent; SAMPLES];
    let shaping_pressure = glyph_cache::pressure_benchmark::measure(&device, &queue)?;

    Ok(Some(HeadlessRendererBenchmark {
        adapter: adapter_name,
        frame_ms,
        glyph_warm_ms,
        glyph_cache_hit_percent,
        steady_frame_buffer_allocations,
        ligature_code_ms,
        ligature_cache_hit_percent,
        shaping_pressure,
    }))
}

#[cfg(test)]
mod row_cache_signature_tests {
    use super::*;

    fn clip_signature(clip: TextClip) -> u64 {
        let mut hasher = DefaultHasher::new();
        hash_text_clip(clip, &mut hasher);
        hasher.finish()
    }

    #[test]
    fn every_pane_clip_boundary_participates_in_the_row_signature() {
        let base = TextClip {
            left: 1.0,
            top: 2.0,
            right: 101.0,
            bottom: 42.0,
        };
        let base_signature = clip_signature(base);
        for changed in [
            TextClip { left: 1.5, ..base },
            TextClip { top: 2.5, ..base },
            TextClip {
                right: 101.5,
                ..base
            },
            TextClip {
                bottom: 42.5,
                ..base
            },
        ] {
            assert_ne!(clip_signature(changed), base_signature);
        }
    }
}

#[cfg(test)]
mod terminal_palette_tests {
    use super::*;

    fn term_color(color: crate::config::theme::Color) -> TermColor {
        TermColor::rgb(color.r, color.g, color.b)
    }

    #[test]
    fn terminal_palette_resolves_theme_defaults_and_indexed_colors() {
        let theme = Theme::load_builtin("nord").unwrap();
        let palette = TerminalPalette::from_theme(&theme);
        let dynamic = termeric_core::DynamicColors::default();
        let mut cell = termeric_core::Cell::with_char('x');

        assert_eq!(
            palette.resolve_cell(&dynamic, &cell),
            (
                term_color(theme.ui.foreground),
                term_color(theme.ui.background)
            )
        );

        cell.fg_spec = ColorSpec::Indexed(1);
        cell.fg = TermColor::rgb(205, 0, 0);
        assert_eq!(
            palette.resolve_cell(&dynamic, &cell).0,
            term_color(theme.primary.red)
        );

        cell.attrs.insert(termeric_core::CellAttrs::INVERSE);
        assert_eq!(
            palette.resolve_cell(&dynamic, &cell),
            (
                term_color(theme.ui.background),
                term_color(theme.primary.red)
            )
        );
    }

    #[test]
    fn terminal_local_dynamic_colors_override_and_reset_theme_resolution() {
        let theme = Theme::load_builtin("nord").unwrap();
        let palette = TerminalPalette::from_theme(&theme);
        let mut terminal = termeric_core::Terminal::new(4, 2);
        let override_color = TermColor::rgb(1, 2, 3);
        terminal.apply_dynamic_color(termeric_core::DynamicColorOperation::Set {
            target: termeric_core::DynamicColorTarget::Palette(1),
            color: override_color,
        });
        let mut cell = termeric_core::Cell::with_char('x');
        cell.fg_spec = ColorSpec::Indexed(1);
        assert_eq!(
            palette.resolve_cell(terminal.dynamic_colors(), &cell).0,
            override_color
        );
        terminal.apply_dynamic_color(termeric_core::DynamicColorOperation::Reset(
            termeric_core::DynamicColorTarget::Palette(1),
        ));
        assert_eq!(
            palette.resolve_cell(terminal.dynamic_colors(), &cell).0,
            term_color(theme.primary.red)
        );
    }
}

#[cfg(test)]
mod ui_palette_accessibility_tests {
    use super::*;

    #[test]
    fn high_contrast_chrome_meets_aa_text_contrast() {
        for theme_name in Theme::list_available() {
            let theme = Theme::load_builtin(theme_name).unwrap();
            let palette = UiPalette::from_theme_with_contrast(&theme, true);
            for (foreground, background) in [
                (palette.settings_text, palette.settings_bg),
                (palette.settings_text_dim, palette.settings_bg),
                (palette.tab_text_active, palette.tab_active),
                (palette.tab_text_inactive, palette.tab_bar_bg),
                (palette.tab_text_inactive, palette.tab_inactive),
                (palette.menu_text, palette.menu_bg),
                (palette.menu_text, palette.menu_highlight),
                (palette.menu_text_dim, palette.menu_bg),
                (palette.search_error, palette.menu_bg),
                (palette.palette_text, palette.menu_bg),
                (palette.palette_text, palette.palette_bg),
                (palette.palette_text, palette.palette_input_bg),
                (palette.palette_text_dim, palette.palette_bg),
            ] {
                assert!(
                    contrast_ratio(foreground, background) >= 4.5,
                    "{theme_name} contrast was {}",
                    contrast_ratio(foreground, background)
                );
            }
        }
    }

    #[test]
    fn high_contrast_policy_does_not_change_terminal_palette() {
        let theme = Theme::load_builtin("turmeric").unwrap();
        let before = TerminalPalette::from_theme(&theme);
        let _chrome = UiPalette::from_theme_with_contrast(&theme, true);
        let after = TerminalPalette::from_theme(&theme);
        assert_eq!(before.default_fg, after.default_fg);
        assert_eq!(before.default_bg, after.default_bg);
        assert_eq!(before.ansi, after.ansi);
    }
}

#[cfg(test)]
mod backend_selection_tests {
    use super::*;

    #[test]
    fn automatic_selection_falls_back_but_forced_modes_do_not() {
        assert_eq!(
            adapter_attempts(RendererBackend::Auto),
            [
                AdapterAttempt {
                    force_fallback: false,
                    label: "hardware"
                },
                AdapterAttempt {
                    force_fallback: true,
                    label: "software fallback"
                }
            ]
        );
        assert!(!adapter_attempts(RendererBackend::Gpu)[0].force_fallback);
        assert!(adapter_attempts(RendererBackend::Software)[0].force_fallback);
        assert!(!adapter_attempts(RendererBackend::Gpu)[0].accepts(DeviceType::Cpu));
        assert!(adapter_attempts(RendererBackend::Gpu)[0].accepts(DeviceType::DiscreteGpu));
        assert!(adapter_attempts(RendererBackend::Software)[0].accepts(DeviceType::Cpu));
        assert!(is_enumerated_software_candidate(DeviceType::Cpu, true));
        assert!(!is_enumerated_software_candidate(
            DeviceType::DiscreteGpu,
            true
        ));
        assert!(!is_enumerated_software_candidate(DeviceType::Cpu, false));
    }

    #[test]
    fn simulated_adapter_and_device_failures_follow_the_declared_policy() {
        let mut calls = Vec::new();
        let (selected, attempt) = select_adapter(RendererBackend::Auto, |attempt| {
            calls.push(attempt.label);
            if attempt.force_fallback {
                Ok(Some("llvmpipe"))
            } else {
                Err("device creation rejected".into())
            }
        })
        .unwrap();
        assert_eq!(selected, "llvmpipe");
        assert!(attempt.force_fallback);
        assert_eq!(calls, ["hardware", "software fallback"]);

        let failures = select_adapter::<()>(RendererBackend::Gpu, |_| Ok(None)).unwrap_err();
        assert_eq!(failures, ["hardware adapter was unavailable"]);
    }

    #[test]
    fn diagnostics_identify_adapter_backend_type_and_software_state() {
        let diagnostics = RendererDiagnostics {
            preference: RendererBackend::Software,
            adapter: "llvmpipe".into(),
            backend: "Vulkan".into(),
            device_type: "Cpu".into(),
            software: true,
        };
        let summary = diagnostics.summary();
        assert!(summary.contains("preference=software"));
        assert!(summary.contains("adapter=\"llvmpipe\""));
        assert!(summary.contains("backend=Vulkan"));
        assert!(summary.contains("device_type=Cpu"));
        assert!(summary.contains("software=true"));
    }
}
