//! Glyph caching with texture atlas
//!
//! This module handles font loading, glyph rasterization, and texture atlas management.
//! Supports font ligatures through cosmic-text's advanced shaping.

use super::{
    FontCoverageAttempt, FontCoverageEntry, FontCoverageReport, FontCoverageRoute,
    FontCoverageSource, RasterizerMetrics,
};
use crate::config::{FontRangeOverride, MAX_FONT_FALLBACKS};
use anyhow::Result;
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FeatureTag, FontFeatures, FontSystem, Metrics, Shaping, Style,
    SwashCache, SwashContent, SwashImage, Weight,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use wgpu::*;

#[cfg(test)]
pub(crate) mod pressure_benchmark;

/// Size of each glyph-atlas page. Mask storage is capped at 8 MiB and color
/// glyph storage at 16 MiB.
const ATLAS_SIZE: u32 = 1024;
const MAX_ATLAS_PAGES: u32 = 8;
const MAX_COLOR_ATLAS_PAGES: u32 = 4;
const COLOR_PAGE_FLAG: u16 = 1 << 15;
const MAX_SHAPED_RUNS: usize = 4_096;
const MAX_SHAPED_RUN_BYTES: usize = 8 * 1024 * 1024;
const MAX_DEFERRED_RUN_BYTES: usize = 1024 * 1024;
const ATLAS_PADDING: u32 = 1;
pub(crate) const MAX_FONT_COVERAGE_INPUT_BYTES: usize = 256;
pub(crate) const MAX_FONT_COVERAGE_GRAPHEMES: usize = 16;
const MAX_FONT_COVERAGE_PLATFORM_FACES: usize = 8;
const MAX_FONT_COVERAGE_FAMILY_CHARS: usize = 256;

/// The atlas owns reusable glyph pixels. Keep Swash's scaler context, but do
/// not retain a second, unbounded map of CPU images after atlas upload/eviction.
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

    fn metrics(&self) -> RasterizerMetrics {
        RasterizerMetrics {
            rasterizations: self.rasterizations,
            retained_images: self.swash.image_cache.len(),
            retained_image_bytes: self
                .swash
                .image_cache
                .values()
                .flatten()
                .map(|image| image.data.capacity())
                .sum(),
        }
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

/// Key for cached ligature runs
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct LigatureKey {
    /// The string being shaped
    pub text: String,
    /// Font size as bits
    pub size_bits: u32,
    /// Requested SGR weight/slant.
    pub style: FontStyle,
    /// Exact line-height metric used by the shaping buffer.
    pub line_height_bits: u32,
    /// Whether programming-ligature OpenType features are enabled.
    pub ligatures: bool,
    /// Effective ordered shaping features, including explicit overrides.
    pub font_features: FontFeatures,
    /// Resolved primary font family.
    pub family: String,
    /// Ordered configured fallback identity.
    pub fallbacks: Vec<String>,
    /// Whether the generic monospace family replaced the requested family.
    pub monospace_fallback: bool,
    /// Configured ordinary-text OpenType `wght` value.
    pub normal_weight: u16,
    /// Configured bold-text OpenType `wght` value.
    pub bold_weight: u16,
}

impl LigatureKey {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        text: &str,
        font_size: f32,
        line_height: f32,
        style: FontStyle,
        ligatures: bool,
        font_features: FontFeatures,
        family: &str,
        fallbacks: &[String],
        monospace_fallback: bool,
        normal_weight: u16,
        bold_weight: u16,
    ) -> Self {
        Self {
            text: text.to_string(),
            size_bits: font_size.to_bits(),
            style,
            line_height_bits: line_height.to_bits(),
            ligatures,
            font_features,
            family: family.to_string(),
            fallbacks: fallbacks.to_vec(),
            monospace_fallback,
            normal_weight,
            bold_weight,
        }
    }
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
    pub deferred_bytes: usize,
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

#[derive(Clone, Copy, Debug, Default)]
struct AtlasPage {
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    occupied_pixels: u64,
    last_used_frame: u64,
    pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AtlasSlot {
    page: u16,
    x: u32,
    y: u32,
    evicted_page: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AtlasAllocationError {
    /// The glyph can never fit in a page with this atlas configuration.
    TooLarge,
    /// Capacity exists only on pages that are still referenced by this frame.
    FramePinned,
}

#[derive(Debug)]
struct PagedAtlasAllocator {
    page_size: u32,
    max_pages: u32,
    pages: Vec<AtlasPage>,
    frame: u64,
    generation: u64,
    evictions: u64,
    allocation_failures: u64,
    /// A frame-pinned miss may have placed the visible replacement glyph in a
    /// persistent row batch. Advance the generation at the next frame boundary
    /// so that batch is rebuilt after the current pins have been released.
    retry_on_next_frame: bool,
    protect_page_zero: bool,
}

impl PagedAtlasAllocator {
    fn new(page_size: u32, max_pages: u32) -> Self {
        Self::with_page_zero_policy(page_size, max_pages, true)
    }

    fn recyclable(page_size: u32, max_pages: u32) -> Self {
        Self::with_page_zero_policy(page_size, max_pages, false)
    }

    fn with_page_zero_policy(page_size: u32, max_pages: u32, protect_page_zero: bool) -> Self {
        assert!(page_size > 0);
        assert!(max_pages > 0);
        Self {
            page_size,
            max_pages,
            pages: vec![AtlasPage {
                pinned: protect_page_zero,
                ..AtlasPage::default()
            }],
            frame: 0,
            generation: 0,
            evictions: 0,
            allocation_failures: 0,
            retry_on_next_frame: false,
            protect_page_zero,
        }
    }

    fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        if self.retry_on_next_frame {
            self.generation = self.generation.wrapping_add(1);
            self.retry_on_next_frame = false;
        }
        for (index, page) in self.pages.iter_mut().enumerate() {
            // Page zero contains the permanent visible replacement glyph.
            page.pinned = self.protect_page_zero && index == 0;
        }
    }

    fn pin(&mut self, page: u16) {
        if let Some(state) = self.pages.get_mut(page as usize) {
            state.pinned = true;
            state.last_used_frame = self.frame;
        }
    }

    fn allocate(&mut self, width: u32, height: u32) -> Result<AtlasSlot, AtlasAllocationError> {
        if width == 0 || height == 0 {
            return Ok(AtlasSlot {
                page: 0,
                x: 0,
                y: 0,
                evicted_page: None,
            });
        }
        if width.saturating_add(ATLAS_PADDING) > self.page_size
            || height.saturating_add(ATLAS_PADDING) > self.page_size
        {
            self.allocation_failures += 1;
            return Err(AtlasAllocationError::TooLarge);
        }

        for page in 0..self.pages.len() {
            if let Some((x, y)) = self.allocate_on_page(page, width, height) {
                return Ok(AtlasSlot {
                    page: page as u16,
                    x,
                    y,
                    evicted_page: None,
                });
            }
        }

        if self.pages.len() < self.max_pages as usize {
            self.pages.push(AtlasPage::default());
            let page = self.pages.len() - 1;
            let (x, y) = self
                .allocate_on_page(page, width, height)
                .expect("a validated glyph fits on a fresh atlas page");
            return Ok(AtlasSlot {
                page: page as u16,
                x,
                y,
                evicted_page: None,
            });
        }

        let victim = self
            .pages
            .iter()
            .enumerate()
            .skip(usize::from(self.protect_page_zero))
            .filter(|(_, page)| !page.pinned)
            .min_by_key(|(_, page)| page.last_used_frame)
            .map(|(index, _)| index);
        let Some(victim) = victim else {
            self.allocation_failures += 1;
            self.retry_on_next_frame = true;
            return Err(AtlasAllocationError::FramePinned);
        };

        self.pages[victim] = AtlasPage::default();
        self.evictions += 1;
        self.generation = self.generation.wrapping_add(1);
        let (x, y) = self
            .allocate_on_page(victim, width, height)
            .expect("a validated glyph fits on a recycled atlas page");
        Ok(AtlasSlot {
            page: victim as u16,
            x,
            y,
            evicted_page: Some(victim as u16),
        })
    }

    fn allocate_on_page(
        &mut self,
        page_index: usize,
        width: u32,
        height: u32,
    ) -> Option<(u32, u32)> {
        let page = &mut self.pages[page_index];
        let mut x = page.cursor_x;
        let mut y = page.cursor_y;
        let mut row_height = page.row_height;
        if x.saturating_add(width).saturating_add(ATLAS_PADDING) > self.page_size {
            x = 0;
            y = y.saturating_add(row_height).saturating_add(ATLAS_PADDING);
            row_height = 0;
        }
        if y.saturating_add(height).saturating_add(ATLAS_PADDING) > self.page_size {
            return None;
        }
        page.cursor_x = x + width + ATLAS_PADDING;
        page.cursor_y = y;
        page.row_height = row_height.max(height);
        page.occupied_pixels += u64::from(width) * u64::from(height);
        page.last_used_frame = self.frame;
        page.pinned = true;
        Some((x, y))
    }

    fn reset(&mut self) {
        self.pages.clear();
        self.pages.push(AtlasPage {
            pinned: self.protect_page_zero,
            ..AtlasPage::default()
        });
        self.retry_on_next_frame = false;
        self.generation = self.generation.wrapping_add(1);
    }

    fn metrics(&self, hits: u64, misses: u64, bytes_per_pixel: u64) -> GlyphAtlasMetrics {
        GlyphAtlasMetrics {
            pages: self.pages.len() as u32,
            color_pages: 0,
            max_pages: self.max_pages,
            max_color_pages: 0,
            capacity_bytes: u64::from(self.page_size)
                * u64::from(self.page_size)
                * u64::from(self.max_pages)
                * bytes_per_pixel,
            occupied_pixels: self.pages.iter().map(|page| page.occupied_pixels).sum(),
            hits,
            misses,
            evictions: self.evictions,
            allocation_failures: self.allocation_failures,
        }
    }
}

/// A shaped glyph within a ligature run
#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    /// Atlas entry for this glyph
    pub entry: GlyphEntry,
    /// X offset from start of run
    pub x_offset: f32,
}

/// A cached ligature run (sequence of shaped glyphs)
#[derive(Clone, Debug)]
pub struct LigatureRun {
    /// The shaped glyphs
    pub glyphs: Vec<ShapedGlyph>,
}

/// Count and owned-payload bounds are independent. Hash-table bucket/control
/// storage is bounded by the entry ceiling, not included in payload accounting.
struct ShapedRunCache {
    entries: HashMap<LigatureKey, (LigatureRun, usize)>,
    bytes: usize,
    byte_limit: usize,
    skipped: u64,
}

impl ShapedRunCache {
    fn new(byte_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            byte_limit,
            skipped: 0,
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&self, key: &LigatureKey) -> Option<&LigatureRun> {
        self.entries.get(key).map(|(run, _)| run)
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn retain(&mut self, mut keep: impl FnMut(&LigatureRun) -> bool) {
        self.entries.retain(|_, (run, bytes)| {
            if keep(run) {
                true
            } else {
                self.bytes -= *bytes;
                false
            }
        });
    }

    /// Preserve the established whole-cache eviction policy. An oversized
    /// run bypasses caching without evicting useful entries or truncating text.
    /// Return only capacity evictions, not replacement of an identical key.
    fn insert(&mut self, key: LigatureKey, run: &LigatureRun) -> usize {
        let key_bytes = Self::key_bytes(&key);
        let minimum_bytes = key_bytes.saturating_add(
            run.glyphs
                .len()
                .saturating_mul(std::mem::size_of::<ShapedGlyph>()),
        );
        if minimum_bytes > self.byte_limit {
            self.skipped = self.skipped.saturating_add(1);
            return 0;
        }
        let stored = run.clone();
        let bytes = key_bytes.saturating_add(
            stored
                .glyphs
                .capacity()
                .saturating_mul(std::mem::size_of::<ShapedGlyph>()),
        );
        if bytes > self.byte_limit {
            self.skipped = self.skipped.saturating_add(1);
            return 0;
        }
        // Remove the old key too: equal keys can own different capacities.
        if let Some((_, (_, old_bytes))) = self.entries.remove_entry(&key) {
            self.bytes -= old_bytes;
        }
        let mut evicted = 0;
        if self.entries.len() >= MAX_SHAPED_RUNS || self.bytes > self.byte_limit - bytes {
            evicted = self.entries.len();
            self.clear();
        }
        self.entries.insert(key, (stored, bytes));
        self.bytes += bytes;
        evicted
    }

    fn key_bytes(key: &LigatureKey) -> usize {
        std::mem::size_of::<LigatureKey>()
            .saturating_add(std::mem::size_of::<(LigatureRun, usize)>())
            .saturating_add(key.text.capacity())
            .saturating_add(key.family.capacity())
            .saturating_add(
                key.font_features
                    .features
                    .capacity()
                    .saturating_mul(std::mem::size_of::<cosmic_text::Feature>()),
            )
            .saturating_add(
                key.fallbacks
                    .capacity()
                    .saturating_mul(std::mem::size_of::<String>()),
            )
            .saturating_add(
                key.fallbacks
                    .iter()
                    .fold(0usize, |sum, family| sum.saturating_add(family.capacity())),
            )
    }
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
    deferred_ligature_cache: ShapedRunCache,
    shaping_cache_metrics: ShapingCacheMetrics,
    atlas: PagedAtlasAllocator,
    color_atlas: PagedAtlasAllocator,
    atlas_hits: u64,
    atlas_misses: u64,
    fallback_entry: GlyphEntry,
    /// Cell dimensions
    cell_width: f32,
    cell_height: f32,
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
        let mut font_system = FontSystem::new();
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
        let (cell_width, cell_height, use_monospace_fallback) =
            measure_cell(&mut font_system, &metrics, &font_family, font_weight);

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
            format: TextureFormat::Rgba8UnormSrgb,
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
            deferred_ligature_cache: ShapedRunCache::new(MAX_DEFERRED_RUN_BYTES),
            shaping_cache_metrics: ShapingCacheMetrics::default(),
            atlas,
            color_atlas: PagedAtlasAllocator::recyclable(ATLAS_SIZE, MAX_COLOR_ATLAS_PAGES),
            atlas_hits: 0,
            atlas_misses: 0,
            fallback_entry,
            cell_width,
            cell_height,
            font_size,
            font_weight,
            bold_font_weight,
            line_height_multiplier,
            font_family,
            fallback_families,
            configured_families,
            range_overrides: range_overrides.to_vec(),
            configured_range_overrides,
            use_monospace_fallback,
            ligatures_enabled: ligatures,
            font_feature_overrides: FontFeatures::new(),
            reported_synthetic_styles: HashSet::new(),
            weight_axis_faces: HashMap::new(),
        })
    }

    /// Get cell dimensions
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
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
        self.deferred_ligature_cache.clear();
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

    pub(crate) fn rasterizer_metrics(&self) -> RasterizerMetrics {
        self.rasterizer.metrics()
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

        // Allocate space in atlas
        let entry = match self.allocate(
            image.width,
            image.height,
            image.bearing_x,
            image.bearing_y,
            image.pixels.is_color(),
            queue,
        ) {
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

        // Upload to GPU
        if image.width > 0 && image.height > 0 {
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
                image.pixels.data(),
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(image.width * image.pixels.bytes_per_pixel()),
                    rows_per_image: Some(image.height),
                },
                Extent3d {
                    width: image.width,
                    height: image.height,
                    depth_or_array_layers: 1,
                },
            );
        }

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

        if let Some(glyph) = self.rasterize_char_with_family(c, family_for(&family), style) {
            return Some(glyph);
        }

        // Preserve cosmic-text's platform fallback as the final safety net
        // when no configured family completely covers this scalar.
        for family in [Family::Monospace, Family::SansSerif] {
            if let Some(glyph) = self.rasterize_char_with_family(c, family, style) {
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
    ) -> Option<ResolvedChar> {
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        let attrs = font_attrs(family, style, self.font_weight, self.bold_font_weight);
        buffer.set_text(
            &c.to_string(),
            &attrs,
            Shaping::Basic,
            None, // No alignment needed for single chars
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        let (cache_key, font_id) = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .find(|glyph| glyph.glyph_id != 0)
            .map(|glyph| (glyph.physical((0.0, 0.0), 1.0).cache_key, glyph.font_id))?;

        let shaped_key = ShapedGlyphKey { cache_key, style };
        if let Some(entry) = self.shaped_entries.get(&shaped_key).copied() {
            return Some(ResolvedChar::Cached(entry));
        }
        if self.deferred_shaped_entries.contains(&shaped_key) {
            return Some(ResolvedChar::Deferred);
        }
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
        Some(ResolvedChar::Rasterized(RasterizedGlyph {
            cache_key,
            width,
            height: image.placement.height,
            bearing_x: image.placement.left,
            bearing_y: image.placement.top,
            pixels,
        }))
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
    pub fn clear(&mut self, queue: &Queue) {
        self.clear_atlas(queue);
    }

    /// Get current font size
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Get current font family
    pub fn font_family(&self) -> &str {
        &self.font_family
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
            self.deferred_ligature_cache.clear();
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
        self.deferred_ligature_cache.clear();
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
        self.shape_cached(text, style, 1.0, queue)
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
        self.shape_cached(text, style, 1.0, queue)
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
        self.shape_cached(text, style, font_scale.clamp(1.0 / 15.0, 7.0), queue)
    }

    fn shape_cached(
        &mut self,
        text: &str,
        style: FontStyle,
        font_scale: f32,
        queue: &Queue,
    ) -> Option<LigatureRun> {
        if text.is_empty() {
            return None;
        }
        let metrics = Metrics::new(
            self.metrics.font_size * font_scale,
            self.metrics.line_height * font_scale,
        );
        let font_features =
            terminal_font_features(self.ligatures_enabled, &self.font_feature_overrides);
        let key = LigatureKey::new(
            text,
            metrics.font_size,
            metrics.line_height,
            style,
            self.ligatures_enabled,
            font_features,
            &self.font_family,
            &self.fallback_families,
            self.use_monospace_fallback,
            self.font_weight.0,
            self.bold_font_weight.0,
        );
        if let Some(run) = self.ligature_cache.get(&key).cloned() {
            self.shaping_cache_metrics.hits += 1;
            self.atlas_hits += run.glyphs.len() as u64;
            for glyph in &run.glyphs {
                self.pin_entry(glyph.entry);
            }
            return Some(run);
        }
        if let Some(run) = self.deferred_ligature_cache.get(&key).cloned() {
            self.shaping_cache_metrics.hits += 1;
            self.atlas_hits += run.glyphs.len() as u64;
            for glyph in &run.glyphs {
                self.pin_entry(glyph.entry);
            }
            return Some(run);
        }
        self.shaping_cache_metrics.misses += 1;
        let (run, retry_next_frame) = self.shape_text_internal(text, style, metrics, queue)?;
        if retry_next_frame {
            self.shaping_cache_metrics.evictions +=
                self.deferred_ligature_cache.insert(key, &run) as u64;
        } else {
            self.shaping_cache_metrics.evictions += self.ligature_cache.insert(key, &run) as u64;
            self.shaping_cache_metrics.entries = self.ligature_cache.len();
        }
        Some(run)
    }

    pub(crate) fn shaping_cache_metrics(&self) -> ShapingCacheMetrics {
        ShapingCacheMetrics {
            entries: self.ligature_cache.len(),
            retained_bytes: self.ligature_cache.bytes,
            deferred_entries: self.deferred_ligature_cache.len(),
            deferred_bytes: self.deferred_ligature_cache.bytes,
            skipped_runs: self
                .ligature_cache
                .skipped
                .saturating_add(self.deferred_ligature_cache.skipped),
            ..self.shaping_cache_metrics
        }
    }

    #[cfg(test)]
    pub(crate) fn set_shaping_byte_limits_for_test(&mut self, persistent: usize, deferred: usize) {
        self.ligature_cache.clear();
        self.ligature_cache.byte_limit = persistent;
        self.deferred_ligature_cache.clear();
        self.deferred_ligature_cache.byte_limit = deferred;
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
        self.use_monospace_fallback = false;
        let (width, height, _) = measure_cell(
            &mut self.font_system,
            &self.metrics,
            family,
            self.font_weight,
        );
        self.cell_width = width;
        self.cell_height = height;
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
        queue: &Queue,
    ) -> Option<(LigatureRun, bool)> {
        let mut glyph_infos: Vec<(cosmic_text::CacheKey, cosmic_text::fontdb::ID, f32, f32)> =
            Vec::new();

        {
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
                for glyph in run.glyphs.iter() {
                    let physical = glyph.physical((0.0, 0.0), 1.0);
                    glyph_infos.push((physical.cache_key, glyph.font_id, glyph.x, glyph.w));
                }
            }
        }

        let mut glyphs = Vec::new();
        let mut retry_next_frame = false;
        for (cache_key, font_id, x_offset, _width) in glyph_infos {
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
            } else if let Some(image) = self.rasterizer.rasterize(&mut self.font_system, cache_key)
            {
                self.atlas_misses += 1;
                let synthesize_bold =
                    self.needs_synthetic_bold(font_id, cache_key.font_weight, style);
                let source_width = image.placement.width;
                let img_height = image.placement.height;
                let img_left = image.placement.left;
                let img_top = image.placement.top;
                let pixel_count = source_width as usize * img_height as usize;
                let pixels = glyph_pixels(image.content, &image.data, pixel_count)?;
                if !pixels.is_color() {
                    self.report_synthetic_style(font_id, style, synthesize_bold);
                }
                let (img_width, pixels) = match pixels {
                    RasterPixels::Mask(data) if synthesize_bold => {
                        let (width, data) = embolden_alpha_mask(source_width, img_height, &data);
                        (width, RasterPixels::Mask(data))
                    }
                    other => (source_width, other),
                };

                let entry = match self.allocate(
                    img_width,
                    img_height,
                    img_left,
                    img_top,
                    pixels.is_color(),
                    queue,
                ) {
                    Ok(entry) => entry,
                    Err(error) => {
                        self.pin_entry(self.fallback_entry);
                        if error == AtlasAllocationError::TooLarge {
                            self.shaped_entries.insert(shaped_key, self.fallback_entry);
                        } else {
                            retry_next_frame = true;
                            self.deferred_shaped_entries.insert(shaped_key);
                        }
                        glyphs.push(ShapedGlyph {
                            entry: self.fallback_entry,
                            x_offset,
                        });
                        continue;
                    }
                };

                if img_width > 0 && img_height > 0 {
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
                        pixels.data(),
                        TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(img_width * pixels.bytes_per_pixel()),
                            rows_per_image: Some(img_height),
                        },
                        Extent3d {
                            width: img_width,
                            height: img_height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                self.shaped_entries.insert(shaped_key, entry);
                entry
            } else {
                self.atlas_misses += 1;
                self.pin_entry(self.fallback_entry);
                self.shaped_entries.insert(shaped_key, self.fallback_entry);
                self.fallback_entry
            };

            glyphs.push(ShapedGlyph { entry, x_offset });
        }

        Some((LigatureRun { glyphs }, retry_next_frame))
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
        let (cell_width, cell_height, use_fallback) = measure_cell(
            &mut self.font_system,
            &self.metrics,
            &self.font_family,
            self.font_weight,
        );
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.use_monospace_fallback = use_fallback;

        self.clear_atlas(queue);

        log::info!(
            "Font size changed to {}, cell size: {}x{}",
            font_size,
            cell_width,
            cell_height
        );
    }

    /// Change font family, clearing the cache and recalculating metrics
    pub fn set_font_family(&mut self, font_family: String, queue: &Queue) {
        if self.font_family == font_family {
            return;
        }

        self.font_family = font_family;
        self.configured_families = collect_configured_families(
            &self.font_system,
            &self.font_family,
            &self.fallback_families,
        );

        let (cell_width, cell_height, use_fallback) = measure_cell(
            &mut self.font_system,
            &self.metrics,
            &self.font_family,
            self.font_weight,
        );
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.use_monospace_fallback = use_fallback;

        self.clear_atlas(queue);

        log::info!(
            "Font family changed to {}, cell size: {}x{}",
            self.font_family,
            cell_width,
            cell_height
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
        if self.font_family == resolved {
            if fallbacks_changed || ranges_changed {
                self.clear_atlas(queue);
            }
        } else {
            self.set_font_family(resolved, queue);
        }
    }

    /// Change line-height metrics, clearing cached layout/raster data.
    pub fn set_line_height(&mut self, multiplier: f32, queue: &Queue) {
        if (self.line_height_multiplier - multiplier).abs() < 0.001 {
            return;
        }
        self.line_height_multiplier = multiplier;
        self.metrics = Metrics::new(self.font_size, self.font_size * multiplier);
        let (cell_width, cell_height, use_fallback) = measure_cell(
            &mut self.font_system,
            &self.metrics,
            &self.font_family,
            self.font_weight,
        );
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.use_monospace_fallback = use_fallback;
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
        let (cell_width, cell_height, use_fallback) = measure_cell(
            &mut self.font_system,
            &self.metrics,
            &self.font_family,
            self.font_weight,
        );
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.use_monospace_fallback = use_fallback;
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
        self.deferred_ligature_cache.clear();
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

fn font_has_weight_axis(
    font_system: &mut FontSystem,
    font_id: cosmic_text::fontdb::ID,
    requested_weight: Weight,
) -> bool {
    font_system
        .get_font(font_id, requested_weight)
        .is_some_and(|font| {
            font.as_swash()
                .variations()
                .any(|axis| axis.tag().to_be_bytes() == *b"wght")
        })
}

fn should_synthesize_bold(
    style: FontStyle,
    requested_weight: Weight,
    selected_face_is_light: bool,
    has_weight_axis: bool,
) -> bool {
    // Cosmic Text applies a `wght` coordinate while rasterizing variable
    // fonts. Applying our static-face embolden fallback too would make
    // variable bold text artificially heavier than requested.
    style.bold
        && requested_weight.0 >= Weight::SEMIBOLD.0
        && selected_face_is_light
        && !has_weight_axis
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

fn font_attrs(
    family: Family<'_>,
    style: FontStyle,
    font_weight: Weight,
    bold_font_weight: Weight,
) -> Attrs<'_> {
    Attrs::new()
        .family(family)
        .weight(if style.bold {
            bold_font_weight
        } else {
            font_weight
        })
        .style(if style.italic {
            Style::Italic
        } else {
            Style::Normal
        })
}

fn terminal_font_features(ligatures: bool, overrides: &FontFeatures) -> FontFeatures {
    let mut features = FontFeatures::new();
    for feature in [
        FeatureTag::STANDARD_LIGATURES,
        FeatureTag::CONTEXTUAL_LIGATURES,
        FeatureTag::CONTEXTUAL_ALTERNATES,
        FeatureTag::DISCRETIONARY_LIGATURES,
    ] {
        if ligatures {
            features.enable(feature);
        } else {
            features.disable(feature);
        }
    }
    // Terminal advances are a fixed grid; proportional pair kerning must not
    // shift later cell origins.
    features.disable(FeatureTag::KERNING);
    features.features.extend_from_slice(&overrides.features);
    features
}

fn compile_font_feature_overrides(overrides: &BTreeMap<String, u32>) -> FontFeatures {
    let mut features = FontFeatures::new();
    for (tag, value) in overrides {
        let bytes = tag.as_bytes();
        debug_assert_eq!(bytes.len(), 4, "font feature tags are validated by Config");
        if let [first, second, third, fourth] = bytes {
            features.set(FeatureTag::new(&[*first, *second, *third, *fourth]), *value);
        }
    }
    features
}

#[cfg(test)]
pub(crate) fn deterministic_test_font_data() -> Vec<u8> {
    deterministic_test_font_data_named("FiraMono-Medium.ttf")
}

#[cfg(test)]
fn deterministic_test_font_data_named(filename: &str) -> Vec<u8> {
    use std::path::PathBuf;

    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .expect("Cargo home is unavailable");
    let registry_sources = cargo_home.join("registry/src");
    for registry in std::fs::read_dir(registry_sources).expect("Cargo registry is unavailable") {
        let registry = registry.expect("invalid Cargo registry entry").path();
        for package in std::fs::read_dir(registry).expect("Cargo registry index is unreadable") {
            let package = package.expect("invalid Cargo package entry").path();
            if package
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("cosmic-text-"))
            {
                let font = package.join("fonts").join(filename);
                if font.is_file() {
                    return std::fs::read(font).expect("cosmic-text test font is unreadable");
                }
            }
        }
    }
    panic!("cosmic-text's deterministic test font {filename} was not found");
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

fn collect_configured_families(
    font_system: &FontSystem,
    primary: &str,
    fallbacks: &[String],
) -> Vec<ConfiguredFamily> {
    let mut seen = HashSet::new();
    std::iter::once(primary)
        .chain(fallbacks.iter().map(String::as_str))
        .filter_map(|name| {
            let name = name.trim();
            if name.is_empty() || !seen.insert(name.to_ascii_lowercase()) {
                return None;
            }
            Some(ConfiguredFamily {
                name: name.to_string(),
                faces: configured_family_faces(font_system, name),
            })
        })
        .collect()
}

fn collect_configured_range_overrides(
    font_system: &FontSystem,
    rules: &[FontRangeOverride],
) -> Vec<ConfiguredRangeOverride> {
    let mut configured = rules
        .iter()
        .map(|rule| ConfiguredRangeOverride {
            start: rule.start,
            end: rule.end,
            family: ConfiguredFamily {
                name: rule.family.clone(),
                faces: configured_family_faces(font_system, &rule.family),
            },
        })
        .collect::<Vec<_>>();
    configured.sort_unstable_by_key(|rule| (rule.start, rule.end));
    configured
}

fn configured_family_faces(
    font_system: &FontSystem,
    family: &str,
) -> Vec<(cosmic_text::fontdb::ID, Weight)> {
    let requested = family_for(family);
    match requested {
        Family::Name(name) => font_system
            .db()
            .faces()
            .filter(|face| {
                face.families
                    .iter()
                    .any(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            })
            .map(|face| (face.id, face.weight))
            .collect(),
        generic => {
            let families = [generic];
            font_system
                .db()
                .query(&cosmic_text::fontdb::Query {
                    families: &families,
                    weight: Weight::NORMAL,
                    stretch: cosmic_text::fontdb::Stretch::Normal,
                    style: Style::Normal,
                })
                .and_then(|id| font_system.db().face(id).map(|face| (id, face.weight)))
                .into_iter()
                .collect()
        }
    }
}

fn first_configured_family_with_coverage(
    font_system: &mut FontSystem,
    families: &[ConfiguredFamily],
    text: &str,
) -> Option<usize> {
    if !text
        .chars()
        .any(|character| !is_font_default_ignorable(character))
    {
        return (!families.is_empty()).then_some(0);
    }
    families
        .iter()
        .position(|family| configured_family_covers(font_system, family, text))
}

fn configured_family_covers(
    font_system: &mut FontSystem,
    family: &ConfiguredFamily,
    text: &str,
) -> bool {
    family.faces.iter().any(|&(id, weight)| {
        font_system.get_font(id, weight).is_some_and(|font| {
            let charmap = font.as_swash().charmap();
            text.chars()
                .filter(|character| !is_font_default_ignorable(*character))
                .all(|character| charmap.map(character) != 0)
        })
    })
}

fn range_override_family_with_coverage<'a>(
    font_system: &mut FontSystem,
    rules: &'a [ConfiguredRangeOverride],
    grapheme: &str,
) -> Option<&'a str> {
    let rule = matching_range_override(rules, grapheme)?;
    configured_family_covers(font_system, &rule.family, grapheme)
        .then_some(rule.family.name.as_str())
}

fn matching_range_override<'a>(
    rules: &'a [ConfiguredRangeOverride],
    grapheme: &str,
) -> Option<&'a ConfiguredRangeOverride> {
    let codepoint = grapheme
        .chars()
        .find(|character| !is_font_default_ignorable(*character))? as u32;
    let candidate = rules.partition_point(|rule| rule.start <= codepoint);
    let rule = rules.get(candidate.checked_sub(1)?)?;
    if codepoint > rule.end {
        return None;
    }
    Some(rule)
}

#[allow(clippy::too_many_arguments)]
fn diagnose_font_coverage(
    font_system: &mut FontSystem,
    metrics: Metrics,
    font_weight: Weight,
    configured_primary: &str,
    fallback_families: &[String],
    range_overrides: &[ConfiguredRangeOverride],
    default_family: &str,
    text: &str,
) -> std::result::Result<FontCoverageReport, String> {
    if text.is_empty() {
        return Err("enter literal text or an exact Unicode scalar".into());
    }
    if text.len() > MAX_FONT_COVERAGE_INPUT_BYTES {
        return Err(format!(
            "coverage input must be no more than {MAX_FONT_COVERAGE_INPUT_BYTES} UTF-8 bytes"
        ));
    }
    if text.chars().any(char::is_control) {
        return Err("coverage input must not contain control characters".into());
    }

    let mut seen = HashSet::new();
    let configured = std::iter::once((FontCoverageSource::Primary, configured_primary.to_string()))
        .chain(
            fallback_families
                .iter()
                .take(MAX_FONT_FALLBACKS)
                .enumerate()
                .map(|(index, family)| {
                    (
                        FontCoverageSource::Fallback { index: index + 1 },
                        family.clone(),
                    )
                }),
        )
        .filter(|(_, family)| seen.insert(family.trim().to_ascii_lowercase()))
        .map(|(source, family)| {
            let configured = ConfiguredFamily {
                faces: configured_family_faces(font_system, &family),
                name: family,
            };
            (source, configured)
        })
        .collect::<Vec<_>>();

    let mut entries = Vec::new();
    let mut graphemes_truncated = false;
    for grapheme in text.graphemes(true) {
        if entries.len() == MAX_FONT_COVERAGE_GRAPHEMES {
            graphemes_truncated = true;
            break;
        }
        let codepoints = grapheme.chars().map(u32::from).collect::<Vec<_>>();
        let substantive = grapheme
            .chars()
            .any(|character| !is_font_default_ignorable(character));
        if !substantive {
            entries.push(FontCoverageEntry {
                grapheme: grapheme.into(),
                codepoints,
                attempts: Vec::new(),
                route: FontCoverageRoute::DefaultIgnorable {
                    family: default_family.into(),
                },
            });
            continue;
        }

        let mut attempts = Vec::new();
        let mut route = None;
        if let Some(rule) = matching_range_override(range_overrides, grapheme) {
            let complete = configured_family_covers(font_system, &rule.family, grapheme);
            let source = FontCoverageSource::Range {
                start: rule.start,
                end: rule.end,
            };
            attempts.push(FontCoverageAttempt {
                source: source.clone(),
                family: rule.family.name.clone(),
                installed: !rule.family.faces.is_empty(),
                complete,
            });
            if complete {
                route = Some(FontCoverageRoute::Explicit {
                    source,
                    family: rule.family.name.clone(),
                });
            }
        }
        if route.is_none() {
            for (source, family) in &configured {
                let complete = configured_family_covers(font_system, family, grapheme);
                attempts.push(FontCoverageAttempt {
                    source: source.clone(),
                    family: family.name.clone(),
                    installed: !family.faces.is_empty(),
                    complete,
                });
                if complete {
                    route = Some(FontCoverageRoute::Explicit {
                        source: source.clone(),
                        family: family.name.clone(),
                    });
                    break;
                }
            }
        }
        let route = route.unwrap_or_else(|| {
            platform_fallback_route(font_system, metrics, font_weight, default_family, grapheme)
        });
        entries.push(FontCoverageEntry {
            grapheme: grapheme.into(),
            codepoints,
            attempts,
            route,
        });
    }

    Ok(FontCoverageReport {
        entries,
        graphemes_truncated,
    })
}

fn platform_fallback_route(
    font_system: &mut FontSystem,
    metrics: Metrics,
    font_weight: Weight,
    default_family: &str,
    grapheme: &str,
) -> FontCoverageRoute {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text(
        grapheme,
        &font_attrs(
            family_for(default_family),
            FontStyle::default(),
            font_weight,
            font_weight,
        ),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    let glyphs = buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter())
        .map(|glyph| {
            let substantive_cluster = grapheme
                .get(glyph.start..glyph.end)
                .unwrap_or(grapheme)
                .chars()
                .any(|character| !is_font_default_ignorable(character));
            (glyph.font_id, glyph.glyph_id, substantive_cluster)
        })
        .collect::<Vec<_>>();
    if glyphs.is_empty()
        || glyphs
            .iter()
            .any(|(_, glyph_id, substantive)| *substantive && *glyph_id == 0)
        || !glyphs.iter().any(|(_, glyph_id, _)| *glyph_id != 0)
    {
        return FontCoverageRoute::Missing;
    }

    let mut seen = HashSet::new();
    let mut families = Vec::new();
    let mut names_truncated = false;
    for (font_id, glyph_id, _) in glyphs {
        if glyph_id == 0 {
            continue;
        }
        let name = font_system
            .db()
            .face(font_id)
            .and_then(|face| face.families.first())
            .map(|(family, _)| family.clone())
            .unwrap_or_else(|| format!("fontdb {:?}", font_id));
        let (name, name_truncated) = bounded_font_coverage_family(&name);
        names_truncated |= name_truncated;
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        if families.len() == MAX_FONT_COVERAGE_PLATFORM_FACES {
            names_truncated = true;
            break;
        }
        families.push(name);
    }
    if families.is_empty() {
        FontCoverageRoute::Missing
    } else {
        FontCoverageRoute::Platform {
            families,
            names_truncated,
        }
    }
}

fn bounded_font_coverage_family(name: &str) -> (String, bool) {
    let mut characters = name.chars();
    let mut value = characters
        .by_ref()
        .take(MAX_FONT_COVERAGE_FAMILY_CHARS)
        .collect::<String>();
    let truncated = characters.next().is_some();
    if truncated {
        value.pop();
        value.push('…');
    }
    (value, truncated)
}

fn configured_family_spans(
    font_system: &mut FontSystem,
    families: &[ConfiguredFamily],
    range_overrides: &[ConfiguredRangeOverride],
    text: &str,
    default_family: &str,
) -> Vec<(Range<usize>, String)> {
    coalesced_family_spans(text, |grapheme| {
        range_override_family_with_coverage(font_system, range_overrides, grapheme)
            .or_else(|| {
                first_configured_family_with_coverage(font_system, families, grapheme)
                    .map(|index| families[index].name.as_str())
            })
            .unwrap_or(default_family)
    })
}

/// Family/range policy is immutable during one call. Reuse only the previous
/// complete grapheme's resolution, and allocate a name only when a new span
/// starts. No borrowed text or family name survives this call.
fn coalesced_family_spans<'a>(
    text: &str,
    mut resolve: impl FnMut(&str) -> &'a str,
) -> Vec<(Range<usize>, String)> {
    let mut spans: Vec<(Range<usize>, String)> = Vec::new();
    let mut previous_grapheme = None;
    for (start, grapheme) in text.grapheme_indices(true) {
        let end = start + grapheme.len();
        let family = match previous_grapheme {
            Some((previous, family)) if previous == grapheme => family,
            _ => resolve(grapheme),
        };
        previous_grapheme = Some((grapheme, family));
        if let Some((range, previous)) = spans.last_mut()
            && previous == family
        {
            range.end = end;
        } else {
            spans.push((start..end, family.to_owned()));
        }
    }
    spans
}

/// Match the shaping engine's default-ignorable policy. Some Unicode
/// default-ignorable codepoints are intentionally excluded because HarfBuzz
/// preserves them as spacing glyphs or shaping controls.
fn is_font_default_ignorable(character: char) -> bool {
    matches!(character, '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{FEFF}')
        || ('\u{17B4}'..='\u{17B5}').contains(&character)
        || ('\u{180B}'..='\u{180E}').contains(&character)
        || ('\u{200B}'..='\u{200F}').contains(&character)
        || ('\u{202A}'..='\u{202E}').contains(&character)
        || ('\u{2060}'..='\u{206F}').contains(&character)
        || ('\u{FE00}'..='\u{FE0F}').contains(&character)
        || ('\u{FFF0}'..='\u{FFF8}').contains(&character)
        || ('\u{1D173}'..='\u{1D17A}').contains(&character)
        || ('\u{E0000}'..='\u{E0FFF}').contains(&character)
}

/// Measure cell dimensions for monospace grid
///
/// For terminal rendering, we need a consistent cell width. We measure the
/// advance width of a single character with basic shaping. Using 'M' as it's
/// typically representative of monospace font width.
///
/// Returns (cell_width, cell_height, use_monospace_fallback)
fn measure_cell(
    font_system: &mut FontSystem,
    metrics: &Metrics,
    font_family: &str,
    font_weight: Weight,
) -> (f32, f32, bool) {
    let family = family_for(font_family);
    let attrs = Attrs::new().family(family).weight(font_weight);

    // Measure a single 'M' character to get the natural width
    let mut buffer = Buffer::new(font_system, *metrics);
    buffer.set_text("M", &attrs, Shaping::Basic, None);
    buffer.shape_until_scroll(font_system, false);

    let mut measured_width: f32 = 0.0;
    let mut font_id: Option<cosmic_text::fontdb::ID> = None;
    if let Some(run) = buffer.layout_runs().next() {
        if let Some(glyph) = run.glyphs.first() {
            measured_width = glyph.w;
            font_id = Some(glyph.font_id);
        }
    }

    // Try to get the actual font name and check if it's monospace
    let (actual_font_name, is_monospace) = font_id
        .map(|id| {
            let name = font_system
                .db()
                .face(id)
                .map(|face| {
                    face.families
                        .first()
                        .map(|(name, _)| name.clone())
                        .unwrap_or_else(|| "unknown".to_string())
                })
                .unwrap_or_else(|| "unknown".to_string());
            let is_mono = font_system.is_monospace(id);
            (name, is_mono)
        })
        .unwrap_or_else(|| ("none".to_string(), false));

    // Check if the requested font was found by comparing names
    let requested_lower = font_family.to_lowercase();
    let actual_lower = actual_font_name.to_lowercase();

    // Font matches if:
    // 1. User requested generic "monospace" - always matches
    // 2. Actual font name contains or is contained in requested name
    let is_generic_request = matches!(requested_lower.as_str(), "monospace" | "mono" | "default");
    let name_matches =
        actual_lower.contains(&requested_lower) || requested_lower.contains(&actual_lower);
    let font_was_found = is_generic_request || name_matches;

    // Only use fallback if font wasn't found AND got a non-monospace fallback
    // If the font WAS found (name matches), trust it even if is_monospace reports false
    let use_fallback = !font_was_found && !is_monospace;

    // Standard monospace fonts have width/height ratio around 0.5-0.6
    // If measured width is too large (> 0.75 * font_size), use heuristic for cell spacing
    let expected_max = metrics.font_size * 0.75;
    let width = if measured_width > 0.0 && measured_width <= expected_max {
        // Width looks reasonable, use it
        measured_width.round().max(1.0)
    } else if use_fallback {
        // Font not found and got proportional fallback - measure system monospace
        let mut mono_buffer = Buffer::new(font_system, *metrics);
        mono_buffer.set_text(
            "M",
            &Attrs::new().family(Family::Monospace).weight(font_weight),
            Shaping::Basic,
            None,
        );
        mono_buffer.shape_until_scroll(font_system, false);

        let mut mono_width: f32 = 0.0;
        if let Some(run) = mono_buffer.layout_runs().next() {
            if let Some(glyph) = run.glyphs.first() {
                mono_width = glyph.w;
            }
        }

        log::warn!(
            "Font '{}' not found (got '{}'), using system monospace for rendering",
            font_family,
            actual_font_name
        );

        if mono_width > 0.0 {
            mono_width.round().max(1.0)
        } else {
            (metrics.font_size * 0.6).round().max(1.0)
        }
    } else {
        // Font was found but width seems off - still use measured width, just log it
        log::debug!(
            "Font '{}' measured width {:.2} exceeds expected max {:.2}, but using it anyway",
            font_family,
            measured_width,
            expected_max
        );
        measured_width.round().max(1.0)
    };

    let height = metrics.line_height.round().max(1.0);

    log::info!(
        "Cell size for '{}' (actual='{}', found={}, fallback={}): {}x{} (measured_w={:.2})",
        font_family,
        actual_font_name,
        font_was_found,
        use_fallback,
        width,
        height,
        measured_width
    );

    (width, height, use_fallback)
}

fn family_for(font_family: &str) -> Family<'_> {
    let trimmed = font_family.trim();
    if trimmed.is_empty() {
        return Family::Monospace;
    }

    let lower = trimmed.to_ascii_lowercase();
    if matches!(lower.as_str(), "monospace" | "mono" | "default") {
        Family::Monospace
    } else {
        Family::Name(trimmed)
    }
}

fn resolve_configured_family(
    font_system: &FontSystem,
    primary: &str,
    fallbacks: &[String],
) -> String {
    let is_generic = |name: &str| {
        matches!(
            name.trim().to_ascii_lowercase().as_str(),
            "monospace" | "mono" | "default"
        )
    };
    let installed = |name: &str| {
        font_system.db().faces().any(|face| {
            face.families
                .iter()
                .any(|(family, _)| family.eq_ignore_ascii_case(name.trim()))
        })
    };

    std::iter::once(primary)
        .chain(fallbacks.iter().map(String::as_str))
        .find(|name| is_generic(name) || installed(name))
        .unwrap_or("monospace")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn cache_fixture(text: &str, count: usize) -> (LigatureKey, LigatureRun) {
        let key = LigatureKey::new(
            text,
            16.0,
            20.0,
            FontStyle::default(),
            false,
            FontFeatures::new(),
            "monospace",
            &["fallback".into()],
            false,
            400,
            700,
        );
        let run = LigatureRun {
            glyphs: vec![
                ShapedGlyph {
                    entry: GlyphEntry {
                        page: 1,
                        color: false,
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                        bearing_x: 0,
                        bearing_y: 0
                    },
                    x_offset: 0.0,
                };
                count
            ],
        };
        (key, run)
    }

    // Independently enumerate stored capacities rather than trusting charges.
    fn recount_run_cache(cache: &ShapedRunCache) -> usize {
        cache
            .entries
            .iter()
            .map(|(key, (run, _))| {
                std::mem::size_of::<LigatureKey>()
                    + std::mem::size_of::<(LigatureRun, usize)>()
                    + key.text.capacity()
                    + key.family.capacity()
                    + key.font_features.features.capacity()
                        * std::mem::size_of::<cosmic_text::Feature>()
                    + key.fallbacks.capacity() * std::mem::size_of::<String>()
                    + key.fallbacks.iter().map(String::capacity).sum::<usize>()
                    + run.glyphs.capacity() * std::mem::size_of::<ShapedGlyph>()
            })
            .sum()
    }

    #[test]
    fn shaped_cache_accounts_capacities_replacement_retention_and_clear() {
        let mut cache = ShapedRunCache::new(64 * 1024);
        let (mut key, run) = cache_fixture("same", 3);
        key.text.reserve(400);
        key.family.reserve(300);
        key.font_features.features.reserve(32);
        key.fallbacks.reserve(16);
        key.fallbacks[0].reserve(200);
        cache.insert(key.clone(), &run);
        assert_eq!(cache.bytes, recount_run_cache(&cache));
        let before = cache.bytes;
        cache.insert(key, &run);
        assert_eq!(cache.len(), 1);
        assert!(
            cache.bytes > before,
            "replacement must account the moved key's reserved capacity"
        );
        assert_eq!(cache.bytes, recount_run_cache(&cache));
        let (other_key, mut other_run) = cache_fixture("other", 1);
        other_run.glyphs[0].entry.page = 2;
        cache.insert(other_key, &other_run);
        cache.retain(|run| run.glyphs.iter().all(|glyph| glyph.entry.page != 1));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes, recount_run_cache(&cache));
        cache.clear();
        assert_eq!(cache.bytes, 0);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn shaped_cache_byte_pressure_and_oversized_bypass_preserve_limits() {
        let (key, run) = cache_fixture("first", 100);
        let mut measuring = ShapedRunCache::new(usize::MAX);
        measuring.insert(key.clone(), &run);
        let size = recount_run_cache(&measuring);
        for budget in [size, size * 2] {
            let mut cache = ShapedRunCache::new(budget);
            cache.insert(key.clone(), &run);
            assert!(cache.get(&key).is_some());
            let mut evictions = 0;
            for name in ["other", "third", "forth"] {
                let (key, run) = cache_fixture(name, 100);
                evictions += cache.insert(key, &run);
                assert!(cache.bytes <= budget);
                assert_eq!(cache.bytes, recount_run_cache(&cache));
            }
            assert!(evictions > 0 && cache.len() < MAX_SHAPED_RUNS);
            let before = (cache.len(), cache.bytes);
            let (huge_key, huge_run) = cache_fixture(&"x".repeat(budget + 1), 100);
            assert_eq!(cache.insert(huge_key, &huge_run), 0);
            assert_eq!((cache.len(), cache.bytes), before);
            assert_eq!(cache.skipped, 1);
            assert_eq!(
                huge_run.glyphs.len(),
                100,
                "bypass must not truncate the caller's output"
            );
        }
    }

    #[test]
    fn shaped_cache_count_ceiling_remains_independent_of_byte_ceiling() {
        let mut cache = ShapedRunCache::new(MAX_SHAPED_RUN_BYTES);
        for index in 0..MAX_SHAPED_RUNS {
            let (key, run) = cache_fixture(&format!("{index:04}"), 1);
            assert_eq!(cache.insert(key, &run), 0);
        }
        assert_eq!(cache.len(), MAX_SHAPED_RUNS);
        assert!(cache.bytes < MAX_SHAPED_RUN_BYTES);
        let (key, run) = cache_fixture("extra", 1);
        assert_eq!(cache.insert(key, &run), MAX_SHAPED_RUNS);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes, recount_run_cache(&cache));
    }

    #[test]
    fn uncached_rasterization_preserves_pixels_without_retaining_cpu_images() {
        let mut database = cosmic_text::fontdb::Database::new();
        database.load_font_source(cosmic_text::fontdb::Source::Binary(Arc::new(
            deterministic_test_font_data(),
        )));
        let mut fonts = FontSystem::new_with_locale_and_db("en-US".into(), database);
        let text: String = ('!'..='~').collect();
        let mut keys = Vec::new();
        for size in [24.0, 48.0, 96.0] {
            let mut buffer = Buffer::new(&mut fonts, Metrics::new(size, size * 1.2));
            buffer.set_text(
                &text,
                &Attrs::new().family(Family::Name("Fira Mono")),
                Shaping::Basic,
                None,
            );
            buffer.shape_until_scroll(&mut fonts, false);
            keys.extend(
                buffer
                    .layout_runs()
                    .flat_map(|run| run.glyphs.iter())
                    .map(|glyph| glyph.physical((0.0, 0.0), 1.0).cache_key),
            );
        }
        assert_eq!(keys.len(), 94 * 3);
        let mut legacy = SwashCache::new();
        let mut rasterizer = GlyphRasterizer::new();
        for key in &keys {
            let expected = legacy.get_image(&mut fonts, *key).as_ref().unwrap();
            let actual = rasterizer.rasterize(&mut fonts, *key).unwrap();
            assert_eq!(actual.content, expected.content);
            assert_eq!(actual.data, expected.data);
            assert_eq!(
                (
                    actual.placement.width,
                    actual.placement.height,
                    actual.placement.left,
                    actual.placement.top
                ),
                (
                    expected.placement.width,
                    expected.placement.height,
                    expected.placement.left,
                    expected.placement.top
                ),
            );
            assert_eq!(rasterizer.metrics().retained_images, 0);
            assert_eq!(rasterizer.metrics().retained_image_bytes, 0);
        }
        let old_bytes: usize = legacy
            .image_cache
            .values()
            .flatten()
            .map(|image| image.data.capacity())
            .sum();
        assert!(
            old_bytes > 64 * 1024,
            "fixture must expose meaningful retained payload"
        );
        // Revisit an old size/key: its bitmap must still be reproducible even
        // though the rasterizer deliberately did not keep it alive.
        let again = rasterizer.rasterize(&mut fonts, keys[0]).unwrap();
        assert_eq!(
            again.data,
            legacy.get_image(&mut fonts, keys[0]).as_ref().unwrap().data
        );
        assert_eq!(rasterizer.metrics().rasterizations, keys.len() as u64 + 1);
        assert_eq!(rasterizer.metrics().retained_image_bytes, 0);
        eprintln!(
            "raster retention fixture: {} cached images, {old_bytes} old payload bytes; 0 new retained images/bytes",
            legacy.image_cache.len()
        );
    }

    fn shape_signature(
        font_data: Vec<u8>,
        text: &str,
        ligatures: bool,
    ) -> Vec<(u16, usize, usize)> {
        shape_signature_with_overrides(font_data, text, ligatures, &BTreeMap::new())
    }

    fn shape_signature_with_overrides(
        font_data: Vec<u8>,
        text: &str,
        ligatures: bool,
        overrides: &BTreeMap<String, u32>,
    ) -> Vec<(u16, usize, usize)> {
        let source = cosmic_text::fontdb::Source::Binary(Arc::new(font_data));
        let mut font_system = FontSystem::new_with_fonts([source]);
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(16.0, 20.0));
        let overrides = compile_font_feature_overrides(overrides);
        let attrs = Attrs::new()
            .family(Family::Name("Fira Mono"))
            .font_features(terminal_font_features(ligatures, &overrides));
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut font_system, false);
        buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .map(|glyph| (glyph.glyph_id, glyph.start, glyph.end))
            .collect()
    }

    fn variable_weight_raster(weight: Weight) -> (CacheKey, Vec<u8>, bool) {
        let source = cosmic_text::fontdb::Source::Binary(Arc::new(
            deterministic_test_font_data_named("InterVariable.ttf"),
        ));
        let mut database = cosmic_text::fontdb::Database::new();
        database.load_font_source(source);
        let mut font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), database);
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(32.0, 38.4));
        buffer.set_text(
            "M",
            &Attrs::new()
                .family(Family::Name("Inter Variable"))
                .weight(weight),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);
        let cache_key = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .find(|glyph| glyph.glyph_id != 0)
            .expect("Inter Variable should shape M")
            .physical((0.0, 0.0), 1.0)
            .cache_key;
        let has_weight_axis =
            font_has_weight_axis(&mut font_system, cache_key.font_id, cache_key.font_weight);
        let image = SwashCache::new()
            .get_image(&mut font_system, cache_key)
            .as_ref()
            .expect("Inter Variable should rasterize M")
            .clone();
        (cache_key, image.data, has_weight_axis)
    }

    #[test]
    fn configured_font_resolution_keeps_generic_primary() {
        let font_system = FontSystem::new();
        assert_eq!(
            resolve_configured_family(&font_system, "monospace", &[]),
            "monospace"
        );
    }

    #[test]
    fn configured_font_resolution_uses_ordered_generic_fallback() {
        let font_system = FontSystem::new();
        let fallbacks = vec!["also-not-installed".into(), "monospace".into()];
        assert_eq!(
            resolve_configured_family(&font_system, "definitely-not-installed", &fallbacks),
            "monospace"
        );
    }

    #[test]
    fn family_span_coalescing_resolves_only_adjacent_identical_graphemes() {
        for text in [
            String::new(),
            "W".repeat(10_000),
            "abcabc".into(),
            "e\u{301}e\u{301}éé👩\u{200d}💻👩\u{200d}💻a".into(),
            "\u{FE0F}\u{FE0F}شَشَ\r\n\r\n".into(),
        ] {
            let mut resolutions = Vec::new();
            let spans = coalesced_family_spans(&text, |grapheme| {
                resolutions.push(grapheme.to_owned());
                "one family"
            });
            let mut expected = text.graphemes(true).collect::<Vec<_>>();
            expected.dedup();
            assert_eq!(resolutions, expected);
            if text.is_empty() {
                assert!(spans.is_empty());
            } else {
                assert_eq!(spans, vec![(0..text.len(), "one family".into())]);
            }
        }

        let spans = coalesced_family_spans("aaaشَشَbbشَ", |grapheme| {
            if grapheme.starts_with('ش') {
                "Arabic"
            } else {
                "Latin"
            }
        });
        assert_eq!(
            spans,
            vec![
                (0..3, "Latin".into()),
                (3..11, "Arabic".into()),
                (11..13, "Latin".into()),
                (13..17, "Arabic".into()),
            ]
        );
        // Resolutions are local, so a subsequent font policy takes effect.
        assert_eq!(
            coalesced_family_spans("aaa", |_| "changed"),
            vec![(0..3, "changed".into())]
        );
    }

    #[test]
    fn coalesced_font_routing_matches_uncached_reference_for_complete_graphemes() {
        let mut database = cosmic_text::fontdb::Database::new();
        for name in ["FiraMono-Medium.ttf", "NotoSansArabic.ttf"] {
            database.load_font_data(deterministic_test_font_data_named(name));
        }
        let mut fonts = FontSystem::new_with_locale_and_db("en-US".into(), database);
        let families =
            collect_configured_families(&fonts, "Fira Mono", &["Noto Sans Arabic".into()]);
        let rules = collect_configured_range_overrides(
            &fonts,
            &[
                FontRangeOverride {
                    start: 0x30,
                    end: 0x39,
                    family: "Noto Sans Arabic".into(),
                },
                // Incomplete override must fall through to the global Arabic face.
                FontRangeOverride {
                    start: 0x600,
                    end: 0x6ff,
                    family: "Fira Mono".into(),
                },
                FontRangeOverride {
                    start: 0xe000,
                    end: 0xf8ff,
                    family: "Missing".into(),
                },
            ],
        );
        for configured in [families.as_slice(), &[]] {
            for overrides in [rules.as_slice(), &[]] {
                for text in [
                    "",
                    "WWWW001122",
                    "abcشَشَabc",
                    "e\u{301}e\u{301}éé",
                    "👩\u{200d}💻👩\u{200d}💻",
                    "\u{FE0F}\u{FE0F}ش",
                    "\u{E000}\u{E000}x",
                    "\r\n\r\n",
                ] {
                    // Former per-grapheme owned-name algorithm, intentionally
                    // independent of the new coalescer/memoization helper.
                    let mut expected: Vec<(Range<usize>, String)> = Vec::new();
                    for (start, grapheme) in text.grapheme_indices(true) {
                        let family =
                            range_override_family_with_coverage(&mut fonts, overrides, grapheme)
                                .map(str::to_owned)
                                .or_else(|| {
                                    first_configured_family_with_coverage(
                                        &mut fonts, configured, grapheme,
                                    )
                                    .map(|index| configured[index].name.clone())
                                })
                                .unwrap_or_else(|| "Fira Mono".to_owned());
                        let end = start + grapheme.len();
                        if let Some((range, previous)) = expected.last_mut()
                            && previous == &family
                        {
                            range.end = end;
                        } else {
                            expected.push((start..end, family));
                        }
                    }
                    assert_eq!(
                        configured_family_spans(
                            &mut fonts,
                            configured,
                            overrides,
                            text,
                            "Fira Mono"
                        ),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn configured_fallback_order_selects_complete_grapheme_coverage() {
        let sources = [
            cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
                "FiraMono-Medium.ttf",
            ))),
            cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
                "NotoSansArabic.ttf",
            ))),
        ];
        let mut font_system = FontSystem::new_with_fonts(sources);
        let fallbacks = vec![
            "Definitely Missing".to_string(),
            "Noto Sans Arabic".to_string(),
        ];
        let families = collect_configured_families(&font_system, "Fira Mono", &fallbacks);

        assert_eq!(
            first_configured_family_with_coverage(&mut font_system, &families, "abc"),
            Some(0)
        );
        let arabic = first_configured_family_with_coverage(&mut font_system, &families, "ش")
            .expect("the configured Arabic fallback should cover the grapheme");
        assert_eq!(families[arabic].name, "Noto Sans Arabic");

        assert_eq!(
            configured_family_spans(&mut font_system, &families, &[], "abcش", "Fira Mono"),
            vec![
                (0..3, "Fira Mono".to_string()),
                (3..5, "Noto Sans Arabic".to_string())
            ]
        );
    }

    #[test]
    fn range_overrides_precede_global_fallback_without_splitting_graphemes() {
        let sources = [
            cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
                "FiraMono-Medium.ttf",
            ))),
            cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
                "NotoSansArabic.ttf",
            ))),
        ];
        let mut font_system = FontSystem::new_with_fonts(sources);
        let fallbacks = vec!["Noto Sans Arabic".to_string()];
        let families = collect_configured_families(&font_system, "Fira Mono", &fallbacks);
        let rules = collect_configured_range_overrides(
            &font_system,
            &[
                FontRangeOverride {
                    start: 0x0600,
                    end: 0x06FF,
                    family: "Noto Sans Arabic".into(),
                },
                // Deliberately supplied out of interval order; validated app
                // config guarantees no overlap, while compilation sorts once.
                FontRangeOverride {
                    start: 0x0030,
                    end: 0x0039,
                    family: "Noto Sans Arabic".into(),
                },
            ],
        );

        assert_eq!(
            range_override_family_with_coverage(&mut font_system, &rules, "0"),
            Some("Noto Sans Arabic")
        );
        assert_eq!(
            range_override_family_with_coverage(&mut font_system, &rules, "\u{FE0F}ش"),
            Some("Noto Sans Arabic"),
            "default-ignorable selectors must not own range lookup"
        );
        assert_eq!(
            configured_family_spans(&mut font_system, &families, &rules, "01ش", "Fira Mono"),
            vec![(0..4, "Noto Sans Arabic".to_string())]
        );
        assert_eq!(
            range_override_family_with_coverage(&mut font_system, &rules, "0🧡"),
            None,
            "a mapped family that lacks part of the grapheme must fall through"
        );

        let incomplete = collect_configured_range_overrides(
            &font_system,
            &[FontRangeOverride {
                start: 0x0600,
                end: 0x06FF,
                family: "Fira Mono".into(),
            }],
        );
        assert_eq!(
            configured_family_spans(&mut font_system, &families, &incomplete, "ش", "Fira Mono"),
            vec![(0..2, "Noto Sans Arabic".to_string())],
            "incomplete mapped faces must retain the proven global fallback chain"
        );
    }

    #[test]
    fn missing_range_family_and_unmatched_boundaries_fall_through() {
        let mut font_system = FontSystem::new_with_fonts([cosmic_text::fontdb::Source::Binary(
            Arc::new(deterministic_test_font_data_named("FiraMono-Medium.ttf")),
        )]);
        let rules = collect_configured_range_overrides(
            &font_system,
            &[FontRangeOverride {
                start: 0xE000,
                end: 0xF8FF,
                family: "Definitely Missing".into(),
            }],
        );
        assert_eq!(
            range_override_family_with_coverage(&mut font_system, &rules, "\u{E000}"),
            None
        );
        assert_eq!(
            range_override_family_with_coverage(&mut font_system, &rules, "\u{D7FF}"),
            None
        );
        assert_eq!(
            range_override_family_with_coverage(&mut font_system, &rules, "\u{F900}"),
            None
        );
    }

    #[test]
    fn coverage_report_matches_range_and_ordered_fallback_precedence() {
        let sources = [
            cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
                "FiraMono-Medium.ttf",
            ))),
            cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
                "NotoSansArabic.ttf",
            ))),
        ];
        let mut font_system = FontSystem::new_with_fonts(sources);
        let rules = collect_configured_range_overrides(
            &font_system,
            &[
                FontRangeOverride {
                    start: 0x0600,
                    end: 0x06FF,
                    family: "Fira Mono".into(),
                },
                FontRangeOverride {
                    start: 0x0030,
                    end: 0x0039,
                    family: "Noto Sans Arabic".into(),
                },
            ],
        );
        let fallbacks = vec!["Fira Mono".into(), "Noto Sans Arabic".into()];
        let report = diagnose_font_coverage(
            &mut font_system,
            Metrics::new(14.0, 16.8),
            Weight::NORMAL,
            "Missing Primary",
            &fallbacks,
            &rules,
            "Fira Mono",
            "0شّ",
        )
        .unwrap();
        assert_eq!(report.entries.len(), 2);
        assert_eq!(
            report.entries[0].route,
            FontCoverageRoute::Explicit {
                source: FontCoverageSource::Range {
                    start: 0x0030,
                    end: 0x0039,
                },
                family: "Noto Sans Arabic".into(),
            }
        );
        assert_eq!(report.entries[0].attempts.len(), 1);
        assert!(report.entries[0].attempts[0].installed);
        assert!(report.entries[0].attempts[0].complete);

        let arabic = &report.entries[1];
        assert_eq!(arabic.grapheme, "شّ");
        assert_eq!(arabic.attempts.len(), 4);
        assert_eq!(
            arabic.attempts[0].source,
            FontCoverageSource::Range {
                start: 0x0600,
                end: 0x06FF,
            }
        );
        assert!(arabic.attempts[0].installed);
        assert!(!arabic.attempts[0].complete);
        assert_eq!(arabic.attempts[1].source, FontCoverageSource::Primary);
        assert!(!arabic.attempts[1].installed);
        assert_eq!(
            arabic.route,
            FontCoverageRoute::Explicit {
                source: FontCoverageSource::Fallback { index: 2 },
                family: "Noto Sans Arabic".into(),
            }
        );
    }

    #[test]
    fn coverage_report_names_platform_fallback_and_missing_results() {
        let sources = [
            cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
                "FiraMono-Medium.ttf",
            ))),
            cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
                "NotoSansArabic.ttf",
            ))),
        ];
        let mut font_system = FontSystem::new_with_fonts(sources);
        let metrics = Metrics::new(14.0, 16.8);
        let platform = diagnose_font_coverage(
            &mut font_system,
            metrics,
            Weight::NORMAL,
            "Fira Mono",
            &[],
            &[],
            "Fira Mono",
            "ش",
        )
        .unwrap();
        assert!(matches!(
            &platform.entries[0].route,
            FontCoverageRoute::Platform { families, .. }
                if families == &["Noto Sans Arabic".to_string()]
        ));

        let missing = diagnose_font_coverage(
            &mut font_system,
            metrics,
            Weight::NORMAL,
            "Fira Mono",
            &[],
            &[],
            "Fira Mono",
            "\u{10FFFF}",
        )
        .unwrap();
        assert_eq!(missing.entries[0].route, FontCoverageRoute::Missing);

        let ignorable = diagnose_font_coverage(
            &mut font_system,
            metrics,
            Weight::NORMAL,
            "Fira Mono",
            &[],
            &[],
            "Fira Mono",
            "\u{FE0F}",
        )
        .unwrap();
        assert_eq!(
            ignorable.entries[0].route,
            FontCoverageRoute::DefaultIgnorable {
                family: "Fira Mono".into()
            }
        );
    }

    #[test]
    fn font_default_ignorable_predicate_matches_shaping_semantics() {
        for character in [
            '\u{00AD}',
            '\u{034F}',
            '\u{061C}',
            '\u{17B4}',
            '\u{180E}',
            '\u{200B}',
            '\u{206F}',
            '\u{FE0F}',
            '\u{FEFF}',
            '\u{FFF8}',
            '\u{1D17A}',
            '\u{E0000}',
            '\u{E0FFF}',
        ] {
            assert!(
                is_font_default_ignorable(character),
                "U+{:04X} should be default-ignorable",
                u32::from(character)
            );
        }
        for character in [
            'A',
            '\u{00AE}',
            // HarfBuzz preserves these default-ignorable codepoints because
            // fonts use them as spacing glyphs or shaping controls.
            '\u{115F}',
            '\u{1161}',
            '\u{180F}',
            '\u{1810}',
            '\u{2010}',
            '\u{2070}',
            '\u{3164}',
            '\u{FFA0}',
            '\u{1BCA0}',
            '\u{E1000}',
        ] {
            assert!(
                !is_font_default_ignorable(character),
                "U+{:04X} should remain substantive",
                u32::from(character)
            );
        }
    }

    #[test]
    fn coverage_report_bounds_input_without_splitting_the_retained_graphemes() {
        let mut font_system = FontSystem::new_with_fonts([cosmic_text::fontdb::Source::Binary(
            Arc::new(deterministic_test_font_data_named("FiraMono-Medium.ttf")),
        )]);
        let text = "a".repeat(MAX_FONT_COVERAGE_GRAPHEMES + 1);
        let report = diagnose_font_coverage(
            &mut font_system,
            Metrics::new(14.0, 16.8),
            Weight::NORMAL,
            "Fira Mono",
            &[],
            &[],
            "Fira Mono",
            &text,
        )
        .unwrap();
        assert_eq!(report.entries.len(), MAX_FONT_COVERAGE_GRAPHEMES);
        assert!(report.graphemes_truncated);
        assert!(report.entries.iter().all(|entry| entry.grapheme == "a"));
        assert!(
            diagnose_font_coverage(
                &mut font_system,
                Metrics::new(14.0, 16.8),
                Weight::NORMAL,
                "Fira Mono",
                &[],
                &[],
                "Fira Mono",
                &"a".repeat(MAX_FONT_COVERAGE_INPUT_BYTES + 1),
            )
            .is_err()
        );
        assert!(
            diagnose_font_coverage(
                &mut font_system,
                Metrics::new(14.0, 16.8),
                Weight::NORMAL,
                "Fira Mono",
                &[],
                &[],
                "Fira Mono",
                "bad\ninput",
            )
            .is_err()
        );
    }

    #[test]
    fn cell_height_uses_configured_line_height() {
        let mut font_system = FontSystem::new();
        let metrics = Metrics::new(14.0, 21.0);
        let (_, height, _) = measure_cell(&mut font_system, &metrics, "monospace", Weight::NORMAL);
        assert_eq!(height, 21.0);
    }

    #[test]
    fn rgba_glyphs_preserve_color_while_masks_remain_compact() {
        let rgba = [10, 20, 30, 40, 50, 60, 70, 80];
        assert_eq!(
            glyph_pixels(SwashContent::Color, &rgba, 2),
            Some(RasterPixels::Color(rgba.to_vec()))
        );
        assert_eq!(
            glyph_pixels(SwashContent::SubpixelMask, &rgba, 2),
            Some(RasterPixels::Mask(vec![40, 80]))
        );
        assert_eq!(
            glyph_pixels(SwashContent::Mask, &[12, 34], 2),
            Some(RasterPixels::Mask(vec![12, 34]))
        );
        assert_eq!(glyph_pixels(SwashContent::Color, &[1, 2, 3], 1), None);
    }

    #[test]
    fn sgr_face_requests_are_part_of_every_glyph_cache_key() {
        let regular = FontStyle::default();
        let bold_italic = FontStyle {
            bold: true,
            italic: true,
        };
        assert_ne!(
            GlyphKey::new('x', 14.0, regular, 400),
            GlyphKey::new('x', 14.0, bold_italic, 700)
        );
        assert_ne!(
            LigatureKey::new(
                "e\u{301}",
                14.0,
                16.8,
                regular,
                true,
                FontFeatures::new(),
                "Mono",
                &[],
                false,
                400,
                700,
            ),
            LigatureKey::new(
                "e\u{301}",
                14.0,
                16.8,
                bold_italic,
                true,
                FontFeatures::new(),
                "Mono",
                &[],
                false,
                400,
                700,
            )
        );
        let attrs = font_attrs(Family::Monospace, bold_italic, Weight(350), Weight(775));
        assert_eq!(attrs.weight, Weight(775));
        assert_eq!(attrs.style, Style::Italic);

        assert_ne!(
            GlyphKey::new('x', 14.0, regular, 350),
            GlyphKey::new('x', 14.0, regular, 450),
            "normal weight must participate in scalar cache identity"
        );
        assert_ne!(
            LigatureKey::new(
                "fi",
                14.0,
                16.8,
                regular,
                true,
                FontFeatures::new(),
                "Mono",
                &[],
                false,
                350,
                700,
            ),
            LigatureKey::new(
                "fi",
                14.0,
                16.8,
                regular,
                true,
                FontFeatures::new(),
                "Mono",
                &[],
                false,
                450,
                800,
            ),
            "configured weight pair must participate in shaped-run identity"
        );
    }

    #[test]
    fn variable_font_weight_reaches_shaping_and_rasterization_without_double_bold() {
        let (light_key, light_pixels, light_has_axis) = variable_weight_raster(Weight(300));
        let (heavy_key, heavy_pixels, heavy_has_axis) = variable_weight_raster(Weight(800));

        assert_eq!(light_key.font_weight, Weight(300));
        assert_eq!(heavy_key.font_weight, Weight(800));
        assert_ne!(light_key, heavy_key);
        assert_ne!(light_pixels, heavy_pixels);
        assert!(light_has_axis && heavy_has_axis);
        assert!(!should_synthesize_bold(
            FontStyle {
                bold: true,
                italic: false,
            },
            Weight(800),
            true,
            true,
        ));
        assert!(should_synthesize_bold(
            FontStyle {
                bold: true,
                italic: false,
            },
            Weight(800),
            true,
            false,
        ));
    }

    #[test]
    fn synthetic_bold_is_a_bounded_one_pixel_embolden() {
        let (width, mask) = embolden_alpha_mask(2, 2, &[0, 200, 50, 0]);
        assert_eq!(width, 3);
        assert_eq!(mask, vec![0, 200, 200, 50, 50, 0]);
    }

    #[test]
    fn tiny_paged_atlas_grows_then_evicts_the_lru_unpinned_page() {
        let mut atlas = PagedAtlasAllocator::new(16, 3);
        // Page zero is permanently protected by the visible fallback glyph.
        let fallback = atlas.allocate(14, 14).expect("fallback page");
        assert_eq!(fallback.page, 0);
        let second = atlas.allocate(14, 14).expect("second page");
        let third = atlas.allocate(14, 14).expect("third page");
        assert_eq!((second.page, third.page), (1, 2));
        assert_eq!(atlas.metrics(0, 0, 1).pages, 3);

        atlas.begin_frame();
        atlas.pin(2);
        let recycled = atlas.allocate(14, 14).expect("recycled page");
        assert_eq!(recycled.page, 1);
        assert_eq!(recycled.evicted_page, Some(1));
        assert_eq!(atlas.metrics(0, 0, 1).evictions, 1);
    }

    #[test]
    fn tiny_paged_atlas_never_recycles_a_page_used_in_the_current_frame() {
        let mut atlas = PagedAtlasAllocator::new(16, 2);
        atlas.allocate(14, 14).unwrap();
        atlas.allocate(14, 14).unwrap();
        atlas.begin_frame();
        atlas.pin(1);
        let generation = atlas.generation;
        assert_eq!(
            atlas.allocate(14, 14),
            Err(AtlasAllocationError::FramePinned)
        );
        assert_eq!(atlas.metrics(0, 0, 1).allocation_failures, 1);

        // The failed frame may have cached a row containing the temporary
        // replacement glyph. Invalidate that row only after this frame's pins
        // are released, then allow the real glyph to reclaim the old page.
        assert_eq!(atlas.generation, generation);
        atlas.begin_frame();
        assert_ne!(atlas.generation, generation);
        assert!(atlas.allocate(14, 14).is_ok());
    }

    #[test]
    fn oversized_and_frame_pinned_allocations_are_distinguished() {
        let mut atlas = PagedAtlasAllocator::new(16, 2);
        assert_eq!(atlas.allocate(16, 1), Err(AtlasAllocationError::TooLarge));

        atlas.allocate(14, 14).unwrap();
        atlas.allocate(14, 14).unwrap();
        atlas.begin_frame();
        atlas.pin(1);
        assert_eq!(
            atlas.allocate(14, 14),
            Err(AtlasAllocationError::FramePinned)
        );
    }

    #[test]
    fn color_page_zero_is_recyclable_and_has_a_disjoint_row_cache_key() {
        let mut atlas = PagedAtlasAllocator::recyclable(16, 1);
        let first = atlas.allocate(14, 14).unwrap();
        assert_eq!(first.page, 0);
        atlas.begin_frame();
        let recycled = atlas.allocate(14, 14).unwrap();
        assert_eq!(recycled.evicted_page, Some(0));

        let mask = GlyphEntry {
            page: 0,
            color: false,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            bearing_x: 0,
            bearing_y: 0,
        };
        assert_ne!(
            mask.page_key(),
            GlyphEntry {
                color: true,
                ..mask
            }
            .page_key()
        );
    }

    #[test]
    fn atlas_capacity_is_bounded_and_reset_advances_generation() {
        let mut atlas = PagedAtlasAllocator::new(64, 4);
        assert_eq!(atlas.metrics(3, 2, 1).capacity_bytes, 16_384);
        assert_eq!(atlas.metrics(3, 2, 1).hits, 3);
        let generation = atlas.generation;
        atlas.reset();
        assert_ne!(atlas.generation, generation);
        assert_eq!(atlas.metrics(0, 0, 1).pages, 1);
    }

    #[test]
    fn explicit_feature_toggle_changes_a_known_ligature_font_run() {
        let data = deterministic_test_font_data();
        let changed = ["fi", "ffi"]
            .into_iter()
            .filter(|text| {
                shape_signature(data.clone(), text, true)
                    != shape_signature(data.clone(), text, false)
            })
            .collect::<Vec<_>>();
        assert!(
            !changed.is_empty(),
            "test font did not expose any OpenType ligature difference"
        );
    }

    #[test]
    fn explicit_font_features_follow_the_ligature_preset_and_enter_cache_identity() {
        let disabled = ["liga", "clig", "calt", "dlig"]
            .into_iter()
            .map(|tag| (tag.to_string(), 0))
            .collect::<BTreeMap<_, _>>();
        let enabled = ["liga", "clig", "calt", "dlig"]
            .into_iter()
            .map(|tag| (tag.to_string(), 1))
            .collect::<BTreeMap<_, _>>();
        let data = deterministic_test_font_data();
        let text = ["fi", "ffi"]
            .into_iter()
            .find(|text| {
                shape_signature(data.clone(), text, true)
                    != shape_signature(data.clone(), text, false)
            })
            .expect("test font should expose a ligature difference");
        assert_eq!(
            shape_signature_with_overrides(data.clone(), text, true, &disabled),
            shape_signature(data.clone(), text, false),
            "explicit zero values must override the enabled preset"
        );
        assert_eq!(
            shape_signature_with_overrides(data.clone(), text, false, &enabled),
            shape_signature(data, text, true),
            "explicit one values must override the disabled preset"
        );

        let default_key = LigatureKey::new(
            text,
            14.0,
            16.8,
            FontStyle::default(),
            true,
            terminal_font_features(true, &FontFeatures::new()),
            "Fira Mono",
            &[],
            false,
            400,
            700,
        );
        let custom = compile_font_feature_overrides(&enabled);
        let custom_key = LigatureKey::new(
            text,
            14.0,
            16.8,
            FontStyle::default(),
            true,
            terminal_font_features(true, &custom),
            "Fira Mono",
            &[],
            false,
            400,
            700,
        );
        assert_ne!(default_key, custom_key);
    }
}
