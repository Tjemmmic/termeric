//! Glyph caching with texture atlas
//!
//! This module handles font loading, glyph rasterization, and texture atlas management.
//! Supports font ligatures through cosmic-text's advanced shaping.

use super::{
    FontCoverageAttempt, FontCoverageEntry, FontCoverageReport, FontCoverageRoute,
    FontCoverageSource,
};
use crate::config::{FontRangeOverride, MAX_FONT_FALLBACKS};
use anyhow::Result;
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FeatureTag, FontFeatures, FontSystem, Metrics, Shaping, Style,
    SwashCache, SwashContent, SwashImage, Weight,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use crate::ui::text_layout::{LaidLine, LaidRun, TextLayout, TextSize, TextSpan, UiFace};
use unicode_segmentation::UnicodeSegmentation;
use wgpu::*;

#[cfg(test)]
pub(crate) mod pressure_benchmark;

mod atlas;
mod caches;
mod coverage;
mod fonts;

use atlas::{AtlasAllocationError, PagedAtlasAllocator};
use caches::{ShapedRunCache, UiLayoutCache};
use coverage::{
    char_glyph, configured_family_faces, configured_family_spans, diagnose_font_coverage,
    first_configured_family_with_coverage, range_override_family_with_coverage,
};
use fonts::{
    CellMetrics, collect_configured_families, collect_configured_range_overrides,
    compile_font_feature_overrides, desktop_font_system, family_for, font_attrs,
    font_has_weight_axis, measure_cell, resolve_configured_family, should_synthesize_bold,
    terminal_font_features, ui_face_attrs, ui_text_metrics,
};

/// Size of each glyph-atlas page. Mask storage is capped at 8 MiB and color
/// glyph storage at 16 MiB.
const ATLAS_SIZE: u32 = 1024;
const MAX_ATLAS_PAGES: u32 = 8;
const MAX_COLOR_ATLAS_PAGES: u32 = 4;
const COLOR_PAGE_FLAG: u16 = 1 << 15;
const MAX_SHAPED_RUNS: usize = 4_096;
const MAX_SHAPED_RUN_BYTES: usize = 8 * 1024 * 1024;
const ATLAS_PADDING: u32 = 1;
pub(crate) const MAX_FONT_COVERAGE_INPUT_BYTES: usize = 256;
pub(crate) const MAX_FONT_COVERAGE_GRAPHEMES: usize = 16;
const MAX_FONT_COVERAGE_PLATFORM_FACES: usize = 8;
const MAX_FONT_COVERAGE_FAMILY_CHARS: usize = 256;

/// The atlas owns reusable glyph pixels. Keep Swash's scaler context, but
/// rasterize uncached so no second map of CPU images outlives the upload.
struct GlyphRasterizer {
    swash: SwashCache,
    rasterizations: u64,
}

impl GlyphRasterizer {
    fn new() -> Self {
        Self {
            swash: SwashCache::new(),
            rasterizations: 0,
        }
    }

    fn rasterize(&mut self, fonts: &mut FontSystem, key: CacheKey) -> Option<SwashImage> {
        self.rasterizations = self.rasterizations.saturating_add(1);
        self.swash.get_image_uncached(fonts, key)
    }
}

/// Font face request resolved from terminal SGR attributes.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub(super) struct FontStyle {
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum SyntheticStyle {
    Bold,
    Italic,
}

/// A cached glyph entry
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphEntry {
    /// Texture-array layer containing the glyph.
    pub page: u16,
    /// True when the glyph samples the RGBA color atlas rather than the R8
    /// theme-tinted mask atlas.
    pub color: bool,
    /// X position in atlas
    pub x: u32,
    /// Y position in atlas
    pub y: u32,
    /// Glyph width in pixels
    pub width: u32,
    /// Glyph height in pixels
    pub height: u32,
    /// X bearing (offset from origin)
    pub bearing_x: i32,
    /// Y bearing (offset from baseline)
    pub bearing_y: i32,
}

impl GlyphEntry {
    pub(super) fn page_key(self) -> u16 {
        if self.color {
            self.page | COLOR_PAGE_FLAG
        } else {
            self.page
        }
    }
}

/// Key for cached glyphs
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct GlyphKey {
    /// The character
    pub c: char,
    /// Font size as bits (for exact matching)
    pub size_bits: u32,
    /// Requested SGR weight/slant.
    pub style: FontStyle,
    /// Effective OpenType `wght` request.
    pub weight: u16,
}

impl GlyphKey {
    pub fn new(c: char, font_size: f32, style: FontStyle, weight: u16) -> Self {
        Self {
            c,
            size_bits: font_size.to_bits(),
            style,
            weight,
        }
    }
}

/// Key for cached shaped runs. Only what can differ between runs of one
/// font configuration is keyed: any change to the family, fallbacks, weights,
/// line height, ligatures, or OpenType features clears the run caches
/// (`clear_atlas`, `set_ligatures`, `set_font_features`).
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct LigatureKey {
    /// The string being shaped
    pub text: String,
    /// Font size as bits; OSC 66 scaled text shapes at several sizes, and the
    /// line height follows from it.
    pub size_bits: u32,
    /// Requested SGR weight/slant.
    pub style: FontStyle,
    /// Whether glyph offsets follow the cell grid or the shaped advances.
    pub placement: GlyphPlacement,
    /// Terminal font configuration or the desktop's UI sans-serif face.
    pub face: TextFace,
}

impl LigatureKey {
    pub fn new(
        text: &str,
        font_size: f32,
        style: FontStyle,
        placement: GlyphPlacement,
        face: TextFace,
    ) -> Self {
        Self {
            text: text.to_string(),
            size_bits: font_size.to_bits(),
            style,
            placement,
            face,
        }
    }
}

/// Which font family a shaped run uses.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum TextFace {
    /// The configured terminal family, fallbacks, weights, and features.
    Terminal,
    /// The desktop's generic sans-serif family, for application chrome.
    UiSans,
    /// The desktop's generic monospace family, for code in chrome.
    UiMono,
}

/// Vertical metrics of the UI face at one pixel size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiFontMetrics {
    pub ascent: f32,
    pub descent: f32,
}

impl UiFontMetrics {
    /// Whole-pixel baseline that centres the face's ascent and descent in a
    /// box starting at `top` with `height` pixels.
    pub fn centered_baseline(self, top: f32, height: f32) -> f32 {
        (top + (height - (self.ascent + self.descent)) / 2.0 + self.ascent).round()
    }
}

/// Bounded memo of UI text widths, keyed by pixel size then text.
const MAX_UI_WIDTH_ENTRIES: usize = 4096;

/// How shaped glyph positions map onto terminal cells.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum GlyphPlacement {
    /// Keep shaped positions relative to the run origin. Used for text that
    /// occupies one cell (a grapheme) or one scaled block (OSC 66).
    Shaped,
    /// Every scalar is one terminal cell. Glyphs start at their cell's origin
    /// so fractional font advances cannot drift off the whole-pixel grid.
    CellGrid,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct ShapedGlyphKey {
    cache_key: CacheKey,
    style: FontStyle,
}

#[derive(Clone, Debug)]
struct ConfiguredFamily {
    name: String,
    faces: Vec<(cosmic_text::fontdb::ID, Weight)>,
}

#[derive(Clone, Debug)]
struct ConfiguredRangeOverride {
    start: u32,
    end: u32,
    family: ConfiguredFamily,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShapingCacheMetrics {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub entries: usize,
    pub retained_bytes: usize,
    pub deferred_entries: usize,
    pub skipped_runs: u64,
}

/// Runtime atlas diagnostics. Capacity is fixed even while logical pages are
/// activated lazily, so memory use stays predictable on every backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GlyphAtlasMetrics {
    pub pages: u32,
    pub color_pages: u32,
    pub max_pages: u32,
    pub max_color_pages: u32,
    pub capacity_bytes: u64,
    pub occupied_pixels: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub allocation_failures: u64,
}

/// A shaped glyph within a ligature run
#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    /// Atlas entry for this glyph
    pub entry: GlyphEntry,
    /// Cell holding the glyph's cluster; always zero for shaped placement.
    pub cell: u32,
    /// Whole-pixel x offset from that cell's origin. Any subpixel remainder is
    /// already rasterized into the atlas bitmap.
    pub x_offset: f32,
}

impl ShapedGlyph {
    /// Offset from the run origin when each cell advances by `cell_width`.
    pub fn run_offset(&self, cell_width: f32) -> f32 {
        self.cell as f32 * cell_width + self.x_offset
    }
}

/// A laid-out glyph resolved to its cell, whole-pixel offset, and raster key.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PlacedGlyph {
    cache_key: CacheKey,
    font_id: cosmic_text::fontdb::ID,
    cell: u32,
    x_offset: f32,
}

/// Resolve one laid-out line into cells and raster keys.
///
/// Each glyph is rasterized at its final position: the whole-pixel part
/// becomes the quad offset and only the remainder selects a subpixel bin, so a
/// fractional position is never applied twice. With cell-grid placement every
/// scalar starts a cell after the columns of the scalars before it (two for a
/// wide character, such as kana), and a glyph keeps only its offset from the
/// first glyph of its cell, so a cluster's leading glyph lands exactly on the
/// cell origin.
fn place_glyphs(
    text: &str,
    glyphs: &[cosmic_text::LayoutGlyph],
    placement: GlyphPlacement,
) -> Vec<PlacedGlyph> {
    // Each scalar's byte offset and the cell it starts in.
    let cell_starts: Vec<(usize, u32)> = match placement {
        GlyphPlacement::Shaped => Vec::new(),
        GlyphPlacement::CellGrid => {
            let mut cell = 0u32;
            text.char_indices()
                .map(|(start, character)| {
                    let starts = (start, cell);
                    // Narrow and zero-width scalars keep one cell each, as
                    // they always have.
                    let columns =
                        termeric_core::unicode::grapheme_width(character.encode_utf8(&mut [0; 4]))
                            .clamp(1, 2);
                    cell += columns as u32;
                    starts
                })
                .collect()
        }
    };
    // The shaped x of the first glyph seen in the current cell.
    let mut cell_origin: Option<(u32, f32)> = None;
    glyphs
        .iter()
        .map(|glyph| {
            let (cell, position) = match placement {
                GlyphPlacement::Shaped => (0, glyph.x),
                GlyphPlacement::CellGrid => {
                    let index = cell_starts
                        .partition_point(|&(start, _)| start <= glyph.start)
                        .saturating_sub(1);
                    let cell = cell_starts.get(index).map_or(0, |&(_, cell)| cell);
                    let origin_x = match cell_origin {
                        Some((origin_cell, origin_x)) if origin_cell == cell => origin_x,
                        _ => {
                            cell_origin = Some((cell, glyph.x));
                            glyph.x
                        }
                    };
                    (cell, glyph.x - origin_x)
                }
            };
            let physical = glyph.physical((position - glyph.x, 0.0), 1.0);
            PlacedGlyph {
                cache_key: physical.cache_key,
                font_id: glyph.font_id,
                cell,
                x_offset: physical.x as f32,
            }
        })
        .collect()
}

/// A cached ligature run (sequence of shaped glyphs)
#[derive(Clone, Debug)]
pub struct LigatureRun {
    /// The shaped glyphs
    pub glyphs: Vec<ShapedGlyph>,
}

/// Glyph cache with texture atlas
pub struct GlyphCache {
    /// The font system
    font_system: FontSystem,
    /// Scaler context without a duplicate persistent CPU image cache.
    rasterizer: GlyphRasterizer,
    /// Font metrics
    metrics: Metrics,
    /// The atlas texture
    pub(crate) texture: Texture,
    /// Texture view
    pub(crate) texture_view: TextureView,
    /// Fixed-capacity RGBA atlas used by native color fonts.
    pub(crate) color_texture: Texture,
    pub(crate) color_texture_view: TextureView,
    /// Sampler
    pub(crate) sampler: Sampler,
    /// Cached glyph entries
    entries: HashMap<GlyphKey, GlyphEntry>,
    /// Cached ligature runs
    ligature_cache: ShapedRunCache,
    /// Rasterized glyphs produced by shaped runs, keyed by the actual fallback
    /// face/glyph/size/subpixel identity supplied by cosmic-text.
    shaped_entries: HashMap<ShapedGlyphKey, GlyphEntry>,
    /// Allocation failures caused only by current-frame pins. These entries
    /// suppress repeated raster work within a frame but are retried after the
    /// next `begin_frame` releases the pins.
    deferred_entries: HashSet<GlyphKey>,
    deferred_shaped_entries: HashSet<ShapedGlyphKey>,
    /// Runs that drew a deferred glyph this frame, so repeats reuse the
    /// replacement; the frame's own text bounds them.
    deferred_runs: HashMap<LigatureKey, LigatureRun>,
    shaping_cache_metrics: ShapingCacheMetrics,
    atlas: PagedAtlasAllocator,
    color_atlas: PagedAtlasAllocator,
    atlas_hits: u64,
    atlas_misses: u64,
    fallback_entry: GlyphEntry,
    /// Cell dimensions and baseline of the primary face
    cell: CellMetrics,
    /// Font size
    font_size: f32,
    /// OpenType `wght` request for ordinary terminal text.
    font_weight: Weight,
    /// OpenType `wght` request for SGR bold terminal text.
    bold_font_weight: Weight,
    /// Configured line-height multiplier
    line_height_multiplier: f32,
    /// Font family name (as requested by user)
    font_family: String,
    /// Ordered configured fallbacks to try when the primary has no glyph.
    fallback_families: Vec<String>,
    /// Installed faces for the primary and configured fallback families in
    /// exact preference order.
    configured_families: Vec<ConfiguredFamily>,
    /// Validated range preferences and their installed face metadata.
    range_overrides: Vec<FontRangeOverride>,
    configured_range_overrides: Vec<ConfiguredRangeOverride>,
    /// Whether to use system monospace fallback (when requested font not found/not monospace)
    use_monospace_fallback: bool,
    /// Whether ligatures are enabled
    ligatures_enabled: bool,
    /// Validated explicit OpenType values applied after the ligature preset.
    font_feature_overrides: FontFeatures,
    /// Avoid repeating synthetic-face diagnostics for every glyph.
    reported_synthetic_styles: HashSet<SyntheticStyle>,
    /// Parsed `wght`-axis capability by resolved face. Variation axes are
    /// intrinsic to a face, so this avoids reparsing `fvar` on glyph misses.
    weight_axis_faces: HashMap<cosmic_text::fontdb::ID, bool>,
    /// UI text widths by pixel size and face, bounded by
    /// `MAX_UI_WIDTH_ENTRIES`.
    ui_widths: HashMap<(u32, UiFace), HashMap<Box<str>, f32>>,
    /// UI face vertical metrics by pixel size.
    ui_metrics: HashMap<u32, UiFontMetrics>,
    /// Wrapped UI paragraphs kept between frames.
    ui_layouts: UiLayoutCache,
}

impl GlyphCache {
    /// Create a new glyph cache
    pub fn new(
        device: &Device,
        queue: &Queue,
        font_size: f32,
        font_family: &str,
        fallback_families: &[String],
        range_overrides: &[FontRangeOverride],
        font_weight: u16,
        bold_font_weight: u16,
        line_height_multiplier: f32,
        ligatures: bool,
    ) -> Result<Self> {
        let mut font_system = desktop_font_system();
        let rasterizer = GlyphRasterizer::new();
        let fallback_families = fallback_families.to_vec();
        let font_family = resolve_configured_family(&font_system, font_family, &fallback_families);
        let configured_families =
            collect_configured_families(&font_system, &font_family, &fallback_families);
        let configured_range_overrides =
            collect_configured_range_overrides(&font_system, range_overrides);
        let font_weight = Weight(font_weight);
        let bold_font_weight = Weight(bold_font_weight);

        // Calculate metrics
        let line_height = font_size * line_height_multiplier;
        let metrics = Metrics::new(font_size, line_height);

        // Measure a character to get cell dimensions and check if font was found
        let cell = measure_cell(&mut font_system, &metrics, &font_family, font_weight);

        // Create a fixed-capacity texture array. Logical pages are activated
        // lazily, but the bind group and texture lifetime stay frame-stable.
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("Glyph Atlas"),
            size: Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: MAX_ATLAS_PAGES,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::R8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&TextureViewDescriptor {
            label: Some("Glyph Atlas Array View"),
            dimension: Some(TextureViewDimension::D2Array),
            ..Default::default()
        });

        let color_texture = device.create_texture(&TextureDescriptor {
            label: Some("Color Glyph Atlas"),
            size: Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: MAX_COLOR_ATLAS_PAGES,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: super::ENCODED_RGBA_FORMAT,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let color_texture_view = color_texture.create_view(&TextureViewDescriptor {
            label: Some("Color Glyph Atlas Array View"),
            dimension: Some(TextureViewDimension::D2Array),
            ..Default::default()
        });

        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("Glyph Sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        // Initialize with transparent pixels
        let clear_data = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * MAX_ATLAS_PAGES) as usize];
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &clear_data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE),
                rows_per_image: Some(ATLAS_SIZE),
            },
            Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: MAX_ATLAS_PAGES,
            },
        );
        let clear_color_data =
            vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * MAX_COLOR_ATLAS_PAGES * 4) as usize];
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &color_texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &clear_color_data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE * 4),
                rows_per_image: Some(ATLAS_SIZE),
            },
            Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: MAX_COLOR_ATLAS_PAGES,
            },
        );

        let mut atlas = PagedAtlasAllocator::new(ATLAS_SIZE, MAX_ATLAS_PAGES);
        let fallback_bitmap = replacement_glyph_bitmap();
        let fallback_slot = atlas
            .allocate(7, 9)
            .expect("replacement glyph fits on an empty atlas");
        let fallback_entry = GlyphEntry {
            page: fallback_slot.page,
            color: false,
            x: fallback_slot.x,
            y: fallback_slot.y,
            width: 7,
            height: 9,
            bearing_x: 0,
            bearing_y: 8,
        };
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d {
                    x: fallback_entry.x,
                    y: fallback_entry.y,
                    z: u32::from(fallback_entry.page),
                },
                aspect: TextureAspect::All,
            },
            &fallback_bitmap,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(fallback_entry.width),
                rows_per_image: Some(fallback_entry.height),
            },
            Extent3d {
                width: fallback_entry.width,
                height: fallback_entry.height,
                depth_or_array_layers: 1,
            },
        );

        Ok(Self {
            font_system,
            rasterizer,
            metrics,
            texture,
            texture_view,
            color_texture,
            color_texture_view,
            sampler,
            entries: HashMap::new(),
            ligature_cache: ShapedRunCache::new(MAX_SHAPED_RUN_BYTES),
            shaped_entries: HashMap::new(),
            deferred_entries: HashSet::new(),
            deferred_shaped_entries: HashSet::new(),
            deferred_runs: HashMap::new(),
            shaping_cache_metrics: ShapingCacheMetrics::default(),
            atlas,
            color_atlas: PagedAtlasAllocator::recyclable(ATLAS_SIZE, MAX_COLOR_ATLAS_PAGES),
            atlas_hits: 0,
            atlas_misses: 0,
            fallback_entry,
            cell,
            font_size,
            font_weight,
            bold_font_weight,
            line_height_multiplier,
            font_family,
            fallback_families,
            configured_families,
            range_overrides: range_overrides.to_vec(),
            configured_range_overrides,
            use_monospace_fallback: cell.use_monospace_fallback,
            ligatures_enabled: ligatures,
            font_feature_overrides: FontFeatures::new(),
            reported_synthetic_styles: HashSet::new(),
            weight_axis_faces: HashMap::new(),
            ui_widths: HashMap::new(),
            ui_metrics: HashMap::new(),
            ui_layouts: UiLayoutCache::default(),
        })
    }

    /// Get cell dimensions
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell.width, self.cell.height)
    }

    /// Whole-pixel distance from a cell's top edge to the glyph baseline.
    pub fn cell_baseline(&self) -> f32 {
        self.cell.baseline
    }

    /// Adopt freshly measured primary-face metrics.
    fn apply_cell_metrics(&mut self, cell: CellMetrics) {
        self.cell = cell;
        self.use_monospace_fallback = cell.use_monospace_fallback;
    }

    /// Get the atlas size
    pub fn atlas_size(&self) -> (u32, u32) {
        (ATLAS_SIZE, ATLAS_SIZE)
    }

    /// Start a render frame and release page pins from the prior submission.
    pub(super) fn begin_frame(&mut self) {
        self.atlas.begin_frame();
        self.color_atlas.begin_frame();
        self.deferred_entries.clear();
        self.deferred_shaped_entries.clear();
        self.deferred_runs.clear();
    }

    /// Keep atlas pages referenced by persistent row geometry alive until the
    /// current frame has been submitted.
    pub(super) fn pin_pages(&mut self, pages: &[u16]) {
        for &page in pages {
            if page & COLOR_PAGE_FLAG != 0 {
                self.color_atlas.pin(page & !COLOR_PAGE_FLAG);
            } else {
                self.atlas.pin(page);
            }
        }
    }

    fn pin_entry(&mut self, entry: GlyphEntry) {
        if entry.color {
            self.color_atlas.pin(entry.page);
        } else {
            self.atlas.pin(entry.page);
        }
    }

    pub(super) fn atlas_generation(&self) -> u64 {
        self.atlas.generation.wrapping_mul(0x9E37_79B1_85EB_CA87)
            ^ self.color_atlas.generation.rotate_left(29)
    }

    /// Glyphs rasterized so far; atlas hits do not rasterize.
    pub(crate) fn rasterizations(&self) -> u64 {
        self.rasterizer.rasterizations
    }

    pub(crate) fn atlas_metrics(&self) -> GlyphAtlasMetrics {
        let mut metrics = self.atlas.metrics(self.atlas_hits, self.atlas_misses, 1);
        let color = self.color_atlas.metrics(0, 0, 4);
        metrics.color_pages = color.pages;
        metrics.max_color_pages = color.max_pages;
        metrics.capacity_bytes += color.capacity_bytes;
        metrics.occupied_pixels += color.occupied_pixels;
        metrics.evictions += color.evictions;
        metrics.allocation_failures += color.allocation_failures;
        metrics
    }

    /// Get or create a glyph entry
    #[cfg(test)]
    pub fn get_or_insert(&mut self, c: char, queue: &Queue) -> Option<GlyphEntry> {
        self.get_or_insert_styled(c, FontStyle::default(), queue)
    }

    /// Get or create a glyph entry with an SGR font-face request.
    pub(super) fn get_or_insert_styled(
        &mut self,
        c: char,
        style: FontStyle,
        queue: &Queue,
    ) -> Option<GlyphEntry> {
        let key = GlyphKey::new(c, self.font_size, style, self.effective_weight(style).0);

        // Check cache
        if let Some(entry) = self.entries.get(&key).copied() {
            self.atlas_hits += 1;
            self.pin_entry(entry);
            return Some(entry);
        }
        if self.deferred_entries.contains(&key) {
            self.atlas_hits += 1;
            self.pin_entry(self.fallback_entry);
            return Some(self.fallback_entry);
        }
        self.atlas_misses += 1;

        // Resolve an existing shaped atlas entry before doing raster work.
        let image = match self.rasterize_char(c, style) {
            Some(ResolvedChar::Cached(entry)) => {
                self.atlas_hits += 1;
                self.pin_entry(entry);
                self.entries.insert(key, entry);
                return Some(entry);
            }
            Some(ResolvedChar::Deferred) => {
                self.atlas_hits += 1;
                self.deferred_entries.insert(key);
                self.pin_entry(self.fallback_entry);
                return Some(self.fallback_entry);
            }
            Some(ResolvedChar::Rasterized(image)) => image,
            None => {
                self.pin_entry(self.fallback_entry);
                self.entries.insert(key, self.fallback_entry);
                return Some(self.fallback_entry);
            }
        };

        let shaped_key = ShapedGlyphKey {
            cache_key: image.cache_key,
            style,
        };

        let entry = match self.upload_glyph(&image, queue) {
            Ok(entry) => entry,
            Err(error) => {
                self.pin_entry(self.fallback_entry);
                if error == AtlasAllocationError::TooLarge {
                    self.entries.insert(key, self.fallback_entry);
                    self.shaped_entries.insert(shaped_key, self.fallback_entry);
                } else {
                    self.deferred_entries.insert(key);
                    self.deferred_shaped_entries.insert(shaped_key);
                }
                return Some(self.fallback_entry);
            }
        };

        self.entries.insert(key, entry);
        self.shaped_entries.insert(shaped_key, entry);
        Some(entry)
    }

    /// Resolve a character's font, reusing an atlas entry or rasterizing on miss.
    fn rasterize_char(&mut self, c: char, style: FontStyle) -> Option<ResolvedChar> {
        let text = c.to_string();
        let family = range_override_family_with_coverage(
            &mut self.font_system,
            &self.configured_range_overrides,
            &text,
        )
        .map(str::to_owned)
        .or_else(|| {
            first_configured_family_with_coverage(
                &mut self.font_system,
                &self.configured_families,
                &text,
            )
            .map(|index| self.configured_families[index].name.clone())
        })
        .unwrap_or_else(|| self.font_family.clone());

        if let Some(glyph) =
            self.rasterize_char_with_family(c, family_for(&family), style, Shaping::Basic)
        {
            return Some(glyph);
        }

        // cosmic-text's platform fallback is the final safety net when no
        // configured family covers this scalar. It chooses a font by script
        // (Noto Sans CJK for kana, for instance), which only advanced
        // shaping does; basic shaping would try the generic family alone.
        for family in [Family::Monospace, Family::SansSerif] {
            if let Some(glyph) =
                self.rasterize_char_with_family(c, family, style, Shaping::Advanced)
            {
                return Some(glyph);
            }
        }

        log::debug!("No installed font contains glyph U+{:04X}", c as u32);
        None
    }

    fn rasterize_char_with_family(
        &mut self,
        c: char,
        family: Family<'_>,
        style: FontStyle,
        shaping: Shaping,
    ) -> Option<ResolvedChar> {
        let attrs = font_attrs(family, style, self.font_weight, self.bold_font_weight);
        let (cache_key, font_id) =
            char_glyph(&mut self.font_system, self.metrics, c, &attrs, shaping)?;

        let shaped_key = ShapedGlyphKey { cache_key, style };
        if let Some(entry) = self.shaped_entries.get(&shaped_key).copied() {
            return Some(ResolvedChar::Cached(entry));
        }
        if self.deferred_shaped_entries.contains(&shaped_key) {
            return Some(ResolvedChar::Deferred);
        }
        self.rasterize_glyph(cache_key, font_id, style)
            .map(ResolvedChar::Rasterized)
    }

    /// Rasterize one glyph, emboldening its mask when the selected face is
    /// too light for a bold request. `None` when the rasterizer produces no
    /// image or malformed pixels.
    fn rasterize_glyph(
        &mut self,
        cache_key: CacheKey,
        font_id: cosmic_text::fontdb::ID,
        style: FontStyle,
    ) -> Option<RasterizedGlyph> {
        let synthesize_bold = self.needs_synthetic_bold(font_id, cache_key.font_weight, style);
        let image = self
            .rasterizer
            .rasterize(&mut self.font_system, cache_key)?;
        let pixel_count = image.placement.width as usize * image.placement.height as usize;
        let pixels = glyph_pixels(image.content, &image.data, pixel_count)?;
        if !pixels.is_color() {
            self.report_synthetic_style(font_id, style, synthesize_bold);
        }
        let (width, pixels) = match pixels {
            RasterPixels::Mask(data) if synthesize_bold => {
                let (width, data) =
                    embolden_alpha_mask(image.placement.width, image.placement.height, &data);
                (width, RasterPixels::Mask(data))
            }
            // Color fonts are already authored bitmaps/layers. Synthetic
            // emboldening would smear their RGBA edges and alter the artwork.
            other => (image.placement.width, other),
        };
        Some(RasterizedGlyph {
            cache_key,
            width,
            height: image.placement.height,
            bearing_x: image.placement.left,
            bearing_y: image.placement.top,
            pixels,
        })
    }

    /// Place a rasterized glyph in its atlas and upload its pixels.
    fn upload_glyph(
        &mut self,
        glyph: &RasterizedGlyph,
        queue: &Queue,
    ) -> Result<GlyphEntry, AtlasAllocationError> {
        let entry = self.allocate(
            glyph.width,
            glyph.height,
            glyph.bearing_x,
            glyph.bearing_y,
            glyph.pixels.is_color(),
            queue,
        )?;
        if glyph.width > 0 && glyph.height > 0 {
            queue.write_texture(
                TexelCopyTextureInfo {
                    texture: if entry.color {
                        &self.color_texture
                    } else {
                        &self.texture
                    },
                    mip_level: 0,
                    origin: Origin3d {
                        x: entry.x,
                        y: entry.y,
                        z: u32::from(entry.page),
                    },
                    aspect: TextureAspect::All,
                },
                glyph.pixels.data(),
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(glyph.width * glyph.pixels.bytes_per_pixel()),
                    rows_per_image: Some(glyph.height),
                },
                Extent3d {
                    width: glyph.width,
                    height: glyph.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(entry)
    }

    /// Allocate space in the atlas
    fn allocate(
        &mut self,
        width: u32,
        height: u32,
        bearing_x: i32,
        bearing_y: i32,
        color: bool,
        queue: &Queue,
    ) -> Result<GlyphEntry, AtlasAllocationError> {
        // Handle empty glyphs
        if width == 0 || height == 0 {
            return Ok(GlyphEntry {
                page: 0,
                color,
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                bearing_x: 0,
                bearing_y: 0,
            });
        }

        let slot = if color {
            self.color_atlas.allocate(width, height)?
        } else {
            self.atlas.allocate(width, height)?
        };
        if let Some(page) = slot.evicted_page {
            self.evict_page(page, color, queue);
        }

        Ok(GlyphEntry {
            page: slot.page,
            color,
            x: slot.x,
            y: slot.y,
            width,
            height,
            bearing_x,
            bearing_y,
        })
    }

    fn evict_page(&mut self, page: u16, color: bool, queue: &Queue) {
        let belongs_to_page = |entry: &GlyphEntry| entry.page == page && entry.color == color;
        self.entries.retain(|_, entry| !belongs_to_page(entry));
        self.shaped_entries
            .retain(|_, entry| !belongs_to_page(entry));
        let before = self.ligature_cache.len();
        self.ligature_cache.retain(|run| {
            run.glyphs
                .iter()
                .all(|glyph| !belongs_to_page(&glyph.entry))
        });
        self.shaping_cache_metrics.evictions += (before - self.ligature_cache.len()) as u64;
        self.shaping_cache_metrics.entries = self.ligature_cache.len();
        self.clear_texture_page(page, color, queue);
    }

    fn clear_texture_page(&self, page: u16, color: bool, queue: &Queue) {
        let bytes_per_pixel = if color { 4 } else { 1 };
        let clear_data = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * bytes_per_pixel) as usize];
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: if color {
                    &self.color_texture
                } else {
                    &self.texture
                },
                mip_level: 0,
                origin: Origin3d {
                    x: 0,
                    y: 0,
                    z: u32::from(page),
                },
                aspect: TextureAspect::All,
            },
            &clear_data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE * bytes_per_pixel),
                rows_per_image: Some(ATLAS_SIZE),
            },
            Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Clear the glyph cache, resetting atlas and entries.
    #[cfg(test)]
    pub fn clear(&mut self, queue: &Queue) {
        self.clear_atlas(queue);
    }

    /// Check if ligatures are enabled
    pub fn ligatures_enabled(&self) -> bool {
        self.ligatures_enabled
    }

    /// Enable or disable ligatures
    pub fn set_ligatures(&mut self, enabled: bool) {
        if self.ligatures_enabled != enabled {
            self.ligatures_enabled = enabled;
            self.ligature_cache.clear();
            self.deferred_runs.clear();
            self.shaping_cache_metrics.entries = 0;
        }
    }

    /// Replace the bounded validated OpenType overrides. Returns whether the
    /// effective shaping configuration changed.
    pub fn set_font_features(&mut self, overrides: &BTreeMap<String, u32>) -> bool {
        let overrides = compile_font_feature_overrides(overrides);
        if self.font_feature_overrides == overrides {
            return false;
        }
        self.font_feature_overrides = overrides;
        self.ligature_cache.clear();
        self.deferred_runs.clear();
        self.shaping_cache_metrics.entries = 0;
        true
    }

    /// Shape a text run with ligatures
    ///
    /// Returns a LigatureRun with all shaped glyphs and their positions.
    /// Both enabled and disabled states use advanced shaping; the setting only
    /// changes the explicit OpenType feature values.
    pub(super) fn shape_run_styled(
        &mut self,
        text: &str,
        style: FontStyle,
        queue: &Queue,
    ) -> Option<LigatureRun> {
        self.shape_cached(text, style, 1.0, GlyphPlacement::CellGrid, queue)
    }

    /// Shape a multi-scalar grapheme regardless of the programming-ligature
    /// preference. Combining marks and emoji composition are correctness, not
    /// an optional typographic feature.
    pub(super) fn shape_grapheme_styled(
        &mut self,
        text: &str,
        style: FontStyle,
        queue: &Queue,
    ) -> Option<LigatureRun> {
        self.shape_cached(text, style, 1.0, GlyphPlacement::Shaped, queue)
    }

    /// Shape OSC 66 content at a bounded multiple of the configured base font.
    pub(super) fn shape_multicell_styled(
        &mut self,
        text: &str,
        style: FontStyle,
        font_scale: f32,
        queue: &Queue,
    ) -> Option<LigatureRun> {
        if font_scale <= 0.0 {
            return None;
        }
        self.shape_cached(
            text,
            style,
            font_scale.clamp(1.0 / 15.0, 7.0),
            GlyphPlacement::Shaped,
            queue,
        )
    }

    fn shape_cached(
        &mut self,
        text: &str,
        style: FontStyle,
        font_scale: f32,
        placement: GlyphPlacement,
        queue: &Queue,
    ) -> Option<LigatureRun> {
        if text.is_empty() {
            return None;
        }
        let metrics = Metrics::new(
            self.metrics.font_size * font_scale,
            self.metrics.line_height * font_scale,
        );
        let key = LigatureKey::new(
            text,
            metrics.font_size,
            style,
            placement,
            TextFace::Terminal,
        );
        Some(self.shape_keyed(key, text, style, metrics, queue))
    }

    /// Shape application-chrome text in a desktop UI face at an explicit
    /// pixel size. Positions follow the shaped advances.
    pub(super) fn shape_ui_text(
        &mut self,
        text: &str,
        size_px: f32,
        face: UiFace,
        queue: &Queue,
    ) -> Option<LigatureRun> {
        if text.is_empty() {
            return None;
        }
        let metrics = ui_text_metrics(size_px);
        let style = FontStyle {
            bold: face.bold,
            italic: face.italic,
        };
        let key = LigatureKey::new(
            text,
            metrics.font_size,
            style,
            GlyphPlacement::Shaped,
            if face.mono {
                TextFace::UiMono
            } else {
                TextFace::UiSans
            },
        );
        Some(self.shape_keyed(key, text, style, metrics, queue))
    }

    /// Advance width of UI text without rasterizing it.
    pub(super) fn ui_text_width(&mut self, text: &str, size_px: f32) -> f32 {
        self.ui_text_width_in(text, UiFace::SANS, size_px)
    }

    /// Advance width of UI text in `face` without rasterizing it.
    pub(super) fn ui_text_width_in(&mut self, text: &str, face: UiFace, size_px: f32) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let key = (size_px.to_bits(), face);
        if let Some(width) = self.ui_widths.get(&key).and_then(|sizes| sizes.get(text)) {
            return *width;
        }
        let mut buffer = Buffer::new(&mut self.font_system, ui_text_metrics(size_px));
        buffer.set_text(text, &ui_face_attrs(face), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        let width = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max);
        if self.ui_widths.values().map(HashMap::len).sum::<usize>() >= MAX_UI_WIDTH_ENTRIES {
            self.ui_widths.clear();
        }
        self.ui_widths
            .entry(key)
            .or_default()
            .insert(text.into(), width);
        width
    }

    /// Lays out styled UI text, wrapped at `width` when given. Layouts are
    /// cached, so an unchanged paragraph costs one lookup per frame.
    pub(super) fn layout_ui_text(
        &mut self,
        spans: &[TextSpan<'_>],
        size: TextSize,
        width: Option<f32>,
    ) -> Arc<TextLayout> {
        let size_bits = (size.font_px.to_bits(), size.line_px.to_bits());
        let width_bits = width.map(f32::to_bits);
        if let Some(layout) = self.ui_layouts.get(spans, size_bits, width_bits) {
            return layout;
        }
        let layout = Arc::new(self.lay_out_ui_text(spans, size, width));
        self.ui_layouts
            .insert(spans, size_bits, width_bits, Arc::clone(&layout));
        layout
    }

    fn lay_out_ui_text(
        &mut self,
        spans: &[TextSpan<'_>],
        size: TextSize,
        width: Option<f32>,
    ) -> TextLayout {
        let metrics = Metrics::new(size.font_px, size.line_px);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(cosmic_text::Wrap::WordOrGlyph);
        buffer.set_size(width, None);
        let default = ui_face_attrs(UiFace::SANS);
        buffer.set_rich_text(
            spans
                .iter()
                .enumerate()
                .map(|(index, span)| (span.text, ui_face_attrs(span.face).metadata(index))),
            &default,
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        // Byte offset where each hard line starts in the joined spans.
        let mut line_starts = vec![0];
        let mut offset = 0;
        for span in spans {
            for (index, _) in span.text.match_indices('\n') {
                line_starts.push(offset + index + 1);
            }
            offset += span.text.len();
        }
        let mut layout = TextLayout::default();
        for run in buffer.layout_runs() {
            let line_start = line_starts.get(run.line_i).copied().unwrap_or(0);
            let mut runs: Vec<LaidRun> = Vec::new();
            let mut previous_end = None;
            for glyph in run.glyphs {
                let text = &run.text[glyph.start..glyph.end];
                match runs.last_mut() {
                    // Extend while the span and the byte order both continue.
                    Some(last)
                        if last.span == glyph.metadata && previous_end == Some(glyph.start) =>
                    {
                        last.text.push_str(text);
                        last.width = glyph.x + glyph.w - last.x;
                    }
                    _ => runs.push(LaidRun {
                        x: glyph.x,
                        width: glyph.w,
                        text: text.to_owned(),
                        span: glyph.metadata,
                        start: line_start + glyph.start,
                    }),
                }
                previous_end = Some(glyph.end);
            }
            layout.width = layout.width.max(run.line_w);
            layout.lines.push(LaidLine {
                top: run.line_top,
                baseline: run.line_y,
                height: run.line_height,
                runs,
            });
        }
        if layout.lines.is_empty() {
            let ascent = self.ui_font_metrics(size.font_px).ascent;
            layout.lines.push(LaidLine {
                top: 0.0,
                baseline: ((size.line_px - size.font_px) / 2.0 + ascent).round(),
                height: size.line_px,
                runs: Vec::new(),
            });
        }
        layout.height = layout
            .lines
            .last()
            .map_or(0.0, |line| line.top + line.height);
        layout
    }

    /// Ascent and descent of the UI face at a pixel size.
    pub(super) fn ui_font_metrics(&mut self, size_px: f32) -> UiFontMetrics {
        if let Some(metrics) = self.ui_metrics.get(&size_px.to_bits()) {
            return *metrics;
        }
        let mut buffer = Buffer::new(&mut self.font_system, ui_text_metrics(size_px));
        buffer.set_text("Hg", &ui_face_attrs(UiFace::SANS), Shaping::Advanced, None);
        let metrics = buffer
            .line_layout(&mut self.font_system, 0)
            .and_then(|lines| lines.first())
            .map(|line| UiFontMetrics {
                ascent: line.max_ascent,
                descent: line.max_descent,
            })
            .unwrap_or(UiFontMetrics {
                ascent: size_px * 0.8,
                descent: size_px * 0.2,
            });
        self.ui_metrics.insert(size_px.to_bits(), metrics);
        metrics
    }

    fn shape_keyed(
        &mut self,
        key: LigatureKey,
        text: &str,
        style: FontStyle,
        metrics: Metrics,
        queue: &Queue,
    ) -> LigatureRun {
        if let Some(run) = self.ligature_cache.get(&key).cloned() {
            self.shaping_cache_metrics.hits += 1;
            self.atlas_hits += run.glyphs.len() as u64;
            for glyph in &run.glyphs {
                self.pin_entry(glyph.entry);
            }
            return run;
        }
        if let Some(run) = self.deferred_runs.get(&key).cloned() {
            self.shaping_cache_metrics.hits += 1;
            self.atlas_hits += run.glyphs.len() as u64;
            for glyph in &run.glyphs {
                self.pin_entry(glyph.entry);
            }
            return run;
        }
        self.shaping_cache_metrics.misses += 1;
        let (run, retry_next_frame) =
            self.shape_text_internal(text, style, metrics, key.placement, key.face, queue);
        if retry_next_frame {
            self.deferred_runs.insert(key, run.clone());
        } else {
            self.shaping_cache_metrics.evictions += self.ligature_cache.insert(key, &run) as u64;
            self.shaping_cache_metrics.entries = self.ligature_cache.len();
        }
        run
    }

    pub(crate) fn shaping_cache_metrics(&self) -> ShapingCacheMetrics {
        ShapingCacheMetrics {
            entries: self.ligature_cache.len(),
            retained_bytes: self.ligature_cache.bytes,
            deferred_entries: self.deferred_runs.len(),
            skipped_runs: self.ligature_cache.skipped,
            ..self.shaping_cache_metrics
        }
    }

    #[cfg(test)]
    pub(crate) fn set_shaping_byte_limit_for_test(&mut self, limit: usize) {
        self.ligature_cache.clear();
        self.ligature_cache.byte_limit = limit;
    }

    /// A cache on the system's default monospace face at `size` pixels,
    /// with the default weights and line height.
    #[cfg(test)]
    pub(crate) fn for_test(
        device: &Device,
        queue: &Queue,
        size: f32,
        ligatures: bool,
    ) -> Result<Self> {
        Self::new(
            device,
            queue,
            size,
            "monospace",
            &[],
            &[],
            400,
            700,
            1.2,
            ligatures,
        )
    }

    #[cfg(test)]
    pub(crate) fn install_test_font(&mut self, data: Vec<u8>, family: &str, queue: &Queue) {
        self.font_system.db_mut().load_font_data(data);
        self.weight_axis_faces.clear();
        self.font_family = family.to_string();
        self.fallback_families.clear();
        self.range_overrides.clear();
        self.configured_range_overrides.clear();
        self.configured_families =
            collect_configured_families(&self.font_system, family, &self.fallback_families);
        let cell = measure_cell(
            &mut self.font_system,
            &self.metrics,
            family,
            self.font_weight,
        );
        self.apply_cell_metrics(CellMetrics {
            use_monospace_fallback: false,
            ..cell
        });
        self.clear_atlas(queue);
    }

    #[cfg(test)]
    pub(crate) fn use_tiny_test_atlas(&mut self, page_size: u32, max_pages: u32, queue: &Queue) {
        assert!(page_size <= ATLAS_SIZE);
        assert!(max_pages <= MAX_ATLAS_PAGES);
        self.atlas = PagedAtlasAllocator::new(page_size, max_pages);
        self.clear_atlas(queue);
    }

    /// Internal method to shape text and rasterize glyphs
    fn shape_text_internal(
        &mut self,
        text: &str,
        style: FontStyle,
        metrics: Metrics,
        placement: GlyphPlacement,
        face: TextFace,
        queue: &Queue,
    ) -> (LigatureRun, bool) {
        let mut placed = Vec::new();

        if matches!(face, TextFace::UiSans | TextFace::UiMono) {
            let mut buffer = Buffer::new(&mut self.font_system, metrics);
            let face = UiFace {
                mono: face == TextFace::UiMono,
                bold: style.bold,
                italic: style.italic,
            };
            buffer.set_text(text, &ui_face_attrs(face), Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            for run in buffer.layout_runs() {
                placed.extend(place_glyphs(text, run.glyphs, placement));
            }
        } else {
            let mut buffer = Buffer::new(&mut self.font_system, metrics);

            let default_family = if self.use_monospace_fallback {
                "monospace".to_string()
            } else {
                self.font_family.clone()
            };
            let span_specs = configured_family_spans(
                &mut self.font_system,
                &self.configured_families,
                &self.configured_range_overrides,
                text,
                &default_family,
            );
            let font_features =
                terminal_font_features(self.ligatures_enabled, &self.font_feature_overrides);
            let attrs = font_attrs(
                family_for(&default_family),
                style,
                self.font_weight,
                self.bold_font_weight,
            )
            .font_features(font_features.clone());
            let spans = span_specs.iter().map(|(range, family)| {
                (
                    &text[range.clone()],
                    font_attrs(
                        family_for(family),
                        style,
                        self.font_weight,
                        self.bold_font_weight,
                    )
                    .font_features(font_features.clone()),
                )
            });
            buffer.set_rich_text(spans, &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);

            for run in buffer.layout_runs() {
                placed.extend(place_glyphs(text, run.glyphs, placement));
            }
        }

        let mut glyphs = Vec::new();
        let mut retry_next_frame = false;
        for PlacedGlyph {
            cache_key,
            font_id,
            cell,
            x_offset,
        } in placed
        {
            let shaped_key = ShapedGlyphKey { cache_key, style };
            let entry = if let Some(entry) = self.shaped_entries.get(&shaped_key).copied() {
                self.atlas_hits += 1;
                self.pin_entry(entry);
                entry
            } else if self.deferred_shaped_entries.contains(&shaped_key) {
                self.atlas_hits += 1;
                retry_next_frame = true;
                self.pin_entry(self.fallback_entry);
                self.fallback_entry
            } else if let Some(glyph) = self.rasterize_glyph(cache_key, font_id, style) {
                self.atlas_misses += 1;
                match self.upload_glyph(&glyph, queue) {
                    Ok(entry) => {
                        self.shaped_entries.insert(shaped_key, entry);
                        entry
                    }
                    Err(error) => {
                        self.pin_entry(self.fallback_entry);
                        if error == AtlasAllocationError::TooLarge {
                            self.shaped_entries.insert(shaped_key, self.fallback_entry);
                        } else {
                            retry_next_frame = true;
                            self.deferred_shaped_entries.insert(shaped_key);
                        }
                        self.fallback_entry
                    }
                }
            } else {
                // No image, or a malformed one that won't improve on retry:
                // draw the replacement and keep the rest of the run.
                self.atlas_misses += 1;
                self.pin_entry(self.fallback_entry);
                self.shaped_entries.insert(shaped_key, self.fallback_entry);
                self.fallback_entry
            };

            glyphs.push(ShapedGlyph {
                entry,
                cell,
                x_offset,
            });
        }

        (LigatureRun { glyphs }, retry_next_frame)
    }

    fn effective_weight(&self, style: FontStyle) -> Weight {
        if style.bold {
            self.bold_font_weight
        } else {
            self.font_weight
        }
    }

    fn needs_synthetic_bold(
        &mut self,
        font_id: cosmic_text::fontdb::ID,
        requested_weight: Weight,
        style: FontStyle,
    ) -> bool {
        if !style.bold || requested_weight.0 < Weight::SEMIBOLD.0 {
            return false;
        }
        let selected_face_is_light = self
            .font_system
            .db()
            .face(font_id)
            .is_some_and(|face| face.weight.0 < Weight::SEMIBOLD.0);
        if !selected_face_is_light {
            return false;
        }
        let has_weight_axis = if let Some(has_axis) = self.weight_axis_faces.get(&font_id) {
            *has_axis
        } else {
            let has_axis = font_has_weight_axis(&mut self.font_system, font_id, requested_weight);
            self.weight_axis_faces.insert(font_id, has_axis);
            has_axis
        };
        should_synthesize_bold(
            style,
            requested_weight,
            selected_face_is_light,
            has_weight_axis,
        )
    }

    fn report_synthetic_style(
        &mut self,
        font_id: cosmic_text::fontdb::ID,
        style: FontStyle,
        synthesize_bold: bool,
    ) {
        if synthesize_bold && self.reported_synthetic_styles.insert(SyntheticStyle::Bold) {
            log::info!(
                "Font '{}' has no matching bold face for at least one glyph; using a controlled one-pixel embolden fallback",
                self.font_family
            );
        }
        let synthesize_italic = style.italic
            && self
                .font_system
                .db()
                .face(font_id)
                .is_some_and(|face| face.style == Style::Normal);
        if synthesize_italic
            && self
                .reported_synthetic_styles
                .insert(SyntheticStyle::Italic)
        {
            log::info!(
                "Font '{}' has no matching italic face for at least one glyph; cosmic-text is applying its controlled 14-degree slant fallback",
                self.font_family
            );
        }
    }

    /// Get all installed monospace font families
    ///
    /// Returns a sorted list of unique monospace font family names available on the system.
    /// Filters out emoji fonts and duplicates.
    pub fn get_monospace_fonts(&self) -> Vec<String> {
        use std::collections::HashSet;

        let mut seen = HashSet::new();
        let mut fonts: Vec<String> = self
            .font_system
            .db()
            .faces()
            .filter(|face| {
                // Must be monospace
                if !face.monospaced {
                    return false;
                }
                // Filter out emoji and symbol fonts
                let name = face.families.first().map(|(n, _)| n.as_str()).unwrap_or("");
                !name.contains("Emoji") && !name.contains("Symbol") && !name.contains("Icons")
            })
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .filter(|name| seen.insert(name.clone())) // Deduplicate
            .collect();

        fonts.sort_by_key(|a| a.to_lowercase());

        // Always include "monospace" as the first option (system default)
        let mut result = vec!["monospace".to_string()];
        result.extend(
            fonts
                .into_iter()
                .filter(|f| f.to_lowercase() != "monospace"),
        );

        result
    }

    /// Explain the exact configured-family routing for a bounded text query.
    /// This diagnostic is explicit user work and never runs on the cell or
    /// frame hot path.
    pub(crate) fn inspect_font_coverage(
        &mut self,
        configured_primary: &str,
        text: &str,
    ) -> std::result::Result<FontCoverageReport, String> {
        let default_family = if self.use_monospace_fallback {
            "monospace"
        } else {
            &self.font_family
        };
        diagnose_font_coverage(
            &mut self.font_system,
            self.metrics,
            self.font_weight,
            configured_primary,
            &self.fallback_families,
            &self.configured_range_overrides,
            default_family,
            text,
        )
    }

    /// Change font size, clearing the cache and recalculating metrics
    pub fn set_font_size(&mut self, font_size: f32, queue: &Queue) {
        if (self.font_size - font_size).abs() < 0.01 {
            return; // No change needed
        }

        self.font_size = font_size;

        // Recalculate metrics
        let line_height = font_size * self.line_height_multiplier;
        self.metrics = Metrics::new(font_size, line_height);

        // Remeasure cell dimensions
        let cell = measure_cell(
            &mut self.font_system,
            &self.metrics,
            &self.font_family,
            self.font_weight,
        );
        self.apply_cell_metrics(cell);

        self.clear_atlas(queue);

        log::info!(
            "Font size changed to {}, cell size: {}x{}",
            font_size,
            cell.width,
            cell.height
        );
    }

    /// Apply primary/fallback family selection. The first installed configured
    /// primary wins for cell metrics, while the complete ordered chain is used
    /// for each scalar or grapheme before platform fallback.
    pub fn set_font_families(
        &mut self,
        primary: String,
        fallbacks: &[String],
        range_overrides: &[FontRangeOverride],
        queue: &Queue,
    ) {
        let fallbacks_changed = self.fallback_families != fallbacks;
        let ranges_changed = self.range_overrides != range_overrides;
        self.fallback_families = fallbacks.to_vec();
        self.range_overrides = range_overrides.to_vec();
        let resolved = resolve_configured_family(&self.font_system, &primary, fallbacks);
        self.configured_families =
            collect_configured_families(&self.font_system, &resolved, fallbacks);
        self.configured_range_overrides =
            collect_configured_range_overrides(&self.font_system, range_overrides);
        if self.font_family != resolved {
            // A new primary family measures new cells.
            self.font_family = resolved;
            let cell = measure_cell(
                &mut self.font_system,
                &self.metrics,
                &self.font_family,
                self.font_weight,
            );
            self.apply_cell_metrics(cell);
            self.clear_atlas(queue);
            log::info!(
                "Font family changed to {}, cell size: {}x{}",
                self.font_family,
                cell.width,
                cell.height
            );
        } else if fallbacks_changed || ranges_changed {
            self.clear_atlas(queue);
        }
    }

    /// Change line-height metrics, clearing cached layout/raster data.
    pub fn set_line_height(&mut self, multiplier: f32, queue: &Queue) {
        if (self.line_height_multiplier - multiplier).abs() < 0.001 {
            return;
        }
        self.line_height_multiplier = multiplier;
        self.metrics = Metrics::new(self.font_size, self.font_size * multiplier);
        let cell = measure_cell(
            &mut self.font_system,
            &self.metrics,
            &self.font_family,
            self.font_weight,
        );
        self.apply_cell_metrics(cell);
        self.clear_atlas(queue);
    }

    /// Change normal and bold OpenType `wght` requests, clearing all shaped
    /// and rasterized data whose identity depends on those values.
    pub fn set_font_weights(
        &mut self,
        font_weight: u16,
        bold_font_weight: u16,
        queue: &Queue,
    ) -> bool {
        let font_weight = Weight(font_weight);
        let bold_font_weight = Weight(bold_font_weight);
        if self.font_weight == font_weight && self.bold_font_weight == bold_font_weight {
            return false;
        }
        self.font_weight = font_weight;
        self.bold_font_weight = bold_font_weight;
        let cell = measure_cell(
            &mut self.font_system,
            &self.metrics,
            &self.font_family,
            self.font_weight,
        );
        self.apply_cell_metrics(cell);
        self.clear_atlas(queue);
        true
    }

    /// Clear the glyph atlas, entry caches, and reset packing cursor.
    fn clear_atlas(&mut self, queue: &Queue) {
        self.entries.clear();
        self.ligature_cache.clear();
        self.shaped_entries.clear();
        self.deferred_entries.clear();
        self.deferred_shaped_entries.clear();
        self.deferred_runs.clear();
        self.shaping_cache_metrics.entries = 0;
        self.reported_synthetic_styles.clear();

        self.atlas.reset();
        self.color_atlas.reset();

        let clear_data = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * MAX_ATLAS_PAGES) as usize];
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &clear_data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE),
                rows_per_image: Some(ATLAS_SIZE),
            },
            Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: MAX_ATLAS_PAGES,
            },
        );
        let clear_color_data =
            vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * MAX_COLOR_ATLAS_PAGES * 4) as usize];
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &self.color_texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &clear_color_data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_SIZE * 4),
                rows_per_image: Some(ATLAS_SIZE),
            },
            Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: MAX_COLOR_ATLAS_PAGES,
            },
        );

        let fallback_slot = self
            .atlas
            .allocate(7, 9)
            .expect("replacement glyph fits on a reset atlas");
        self.fallback_entry = GlyphEntry {
            page: fallback_slot.page,
            color: false,
            x: fallback_slot.x,
            y: fallback_slot.y,
            width: 7,
            height: 9,
            bearing_x: 0,
            bearing_y: 8,
        };
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: Origin3d {
                    x: self.fallback_entry.x,
                    y: self.fallback_entry.y,
                    z: u32::from(self.fallback_entry.page),
                },
                aspect: TextureAspect::All,
            },
            &replacement_glyph_bitmap(),
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.fallback_entry.width),
                rows_per_image: Some(self.fallback_entry.height),
            },
            Extent3d {
                width: self.fallback_entry.width,
                height: self.fallback_entry.height,
                depth_or_array_layers: 1,
            },
        );
    }
}

fn replacement_glyph_bitmap() -> Vec<u8> {
    let mut pixels = vec![0; 7 * 9];
    for y in 0..9 {
        for x in 0..7 {
            if x == 0 || x == 6 || y == 0 || y == 8 || (x == 3 && (3..=5).contains(&y)) {
                pixels[y * 7 + x] = 255;
            }
        }
    }
    pixels
}

#[cfg(test)]
pub(crate) fn deterministic_test_font_data() -> Vec<u8> {
    deterministic_test_font_data_named("FiraMono-Medium.ttf")
}

/// One of the fonts tests shape with, built into the test binary from
/// `tests/fixtures/fonts/` (each beside its SIL Open Font License), so
/// they shape the same text the same way on every machine and build. Two
/// are subsets: that directory's README says what each keeps.
#[cfg(test)]
fn deterministic_test_font_data_named(filename: &str) -> Vec<u8> {
    macro_rules! fixture {
        ($name:literal) => {
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/fonts/",
                $name
            ))
            .as_slice()
        };
    }
    let data = match filename {
        "FiraMono-Medium.ttf" => fixture!("FiraMono-Medium.ttf"),
        "InterVariable.ttf" => fixture!("InterVariable.ttf"),
        "NotoSansArabic.ttf" => fixture!("NotoSansArabic.ttf"),
        _ => panic!("no test font fixture named {filename}"),
    };
    data.to_vec()
}

/// Expand an alpha mask by one pixel to the right. This is used only when the
/// selected fallback family has no semibold/bold face. It preserves the real
/// face when one exists and keeps synthesis bounded and deterministic.
fn embolden_alpha_mask(width: u32, height: u32, data: &[u8]) -> (u32, Vec<u8>) {
    if width == 0 || height == 0 {
        return (width, data.to_vec());
    }
    let old_width = width as usize;
    let new_width = old_width + 1;
    let mut bold = vec![0; new_width * height as usize];
    for y in 0..height as usize {
        for x in 0..new_width {
            let current = if x < old_width {
                data[y * old_width + x]
            } else {
                0
            };
            let left = x
                .checked_sub(1)
                .filter(|left| *left < old_width)
                .map(|left| data[y * old_width + left])
                .unwrap_or(0);
            bold[y * new_width + x] = current.max(left);
        }
    }
    (new_width as u32, bold)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RasterPixels {
    Mask(Vec<u8>),
    Color(Vec<u8>),
}

impl RasterPixels {
    fn is_color(&self) -> bool {
        matches!(self, Self::Color(_))
    }

    fn bytes_per_pixel(&self) -> u32 {
        if self.is_color() { 4 } else { 1 }
    }

    fn data(&self) -> &[u8] {
        match self {
            Self::Mask(data) | Self::Color(data) => data,
        }
    }
}

fn glyph_pixels(content: SwashContent, data: &[u8], pixel_count: usize) -> Option<RasterPixels> {
    match content {
        SwashContent::Mask => {
            (data.len() >= pixel_count).then(|| RasterPixels::Mask(data[..pixel_count].to_vec()))
        }
        SwashContent::Color => (data.len() >= pixel_count.saturating_mul(4))
            .then(|| RasterPixels::Color(data[..pixel_count * 4].to_vec())),
        SwashContent::SubpixelMask => (data.len() >= pixel_count.saturating_mul(4)).then(|| {
            RasterPixels::Mask(
                data.as_chunks::<4>()
                    .0
                    .iter()
                    .take(pixel_count)
                    .map(|rgba| *rgba.iter().max().unwrap_or(&0))
                    .collect(),
            )
        }),
    }
}

/// Resolution may reuse pixels already owned by the atlas or defer a retry.
enum ResolvedChar {
    Cached(GlyphEntry),
    Deferred,
    Rasterized(RasterizedGlyph),
}

/// A transient raster ready for upload.
struct RasterizedGlyph {
    cache_key: CacheKey,
    width: u32,
    height: u32,
    bearing_x: i32,
    bearing_y: i32,
    pixels: RasterPixels,
}

#[cfg(test)]
mod tests;
