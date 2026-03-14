//! Glyph caching with texture atlas
//!
//! This module handles font loading, glyph rasterization, and texture atlas management.
//! Supports font ligatures through cosmic-text's advanced shaping.

use anyhow::Result;
use cosmic_text::{
    Attrs, Buffer, Family, FeatureTag, FontFeatures, FontSystem, Metrics, Shaping, SwashCache,
};
use std::collections::HashMap;
use crate::ui::constants::LINE_HEIGHT_MULTIPLIER;
use wgpu::*;

/// Size of the glyph atlas texture
const ATLAS_SIZE: u32 = 1024;

/// A cached glyph entry
#[derive(Clone, Copy, Debug)]
pub struct GlyphEntry {
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

/// Key for cached glyphs
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct GlyphKey {
    /// The character
    pub c: char,
    /// Font size as bits (for exact matching)
    pub size_bits: u32,
}

impl GlyphKey {
    pub fn new(c: char, font_size: f32) -> Self {
        Self {
            c,
            size_bits: font_size.to_bits(),
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
}

impl LigatureKey {
    pub fn new(text: &str, font_size: f32) -> Self {
        Self {
            text: text.to_string(),
            size_bits: font_size.to_bits(),
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
    /// Width this glyph occupies in the run
    pub width: f32,
}

/// A cached ligature run (sequence of shaped glyphs)
#[derive(Clone, Debug)]
pub struct LigatureRun {
    /// The shaped glyphs
    pub glyphs: Vec<ShapedGlyph>,
    /// Total width of the run
    pub total_width: f32,
}

/// Glyph cache with texture atlas
pub struct GlyphCache {
    /// The font system
    font_system: FontSystem,
    /// Swash cache for rasterization
    swash_cache: SwashCache,
    /// Font metrics
    metrics: Metrics,
    /// The atlas texture
    pub(crate) texture: Texture,
    /// Texture view
    pub(crate) texture_view: TextureView,
    /// Sampler
    pub(crate) sampler: Sampler,
    /// Cached glyph entries
    entries: HashMap<GlyphKey, GlyphEntry>,
    /// Cached ligature runs
    ligature_cache: HashMap<LigatureKey, LigatureRun>,
    /// Current packing position
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    /// Cell dimensions
    cell_width: f32,
    cell_height: f32,
    /// Font size
    font_size: f32,
    /// Font family name (as requested by user)
    font_family: String,
    /// Whether to use system monospace fallback (when requested font not found/not monospace)
    use_monospace_fallback: bool,
    /// Whether ligatures are enabled
    ligatures_enabled: bool,
}

impl GlyphCache {
    /// Create a new glyph cache
    pub fn new(device: &Device, queue: &Queue, font_size: f32, font_family: &str, ligatures: bool) -> Result<Self> {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();

        // Calculate metrics
        let line_height = font_size * LINE_HEIGHT_MULTIPLIER;
        let metrics = Metrics::new(font_size, line_height);

        // Measure a character to get cell dimensions and check if font was found
        let (cell_width, cell_height, use_monospace_fallback) = measure_cell(&mut font_system, &metrics, font_family);

        // Create atlas texture
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("Glyph Atlas"),
            size: Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::R8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&TextureViewDescriptor::default());

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
        let clear_data = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE) as usize];
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
                depth_or_array_layers: 1,
            },
        );

        Ok(Self {
            font_system,
            swash_cache,
            metrics,
            texture,
            texture_view,
            sampler,
            entries: HashMap::new(),
            ligature_cache: HashMap::new(),
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
            cell_width,
            cell_height,
            font_size,
            font_family: font_family.to_string(),
            use_monospace_fallback,
            ligatures_enabled: ligatures,
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

    /// Get or create a glyph entry
    pub fn get_or_insert(&mut self, c: char, queue: &Queue) -> Option<GlyphEntry> {
        let key = GlyphKey::new(c, self.font_size);

        // Check cache
        if let Some(entry) = self.entries.get(&key) {
            return Some(*entry);
        }

        // Rasterize the glyph
        let image = self.rasterize_char(c)?;

        // Allocate space in atlas
        let entry = self.allocate(image.width, image.height, image.bearing_x, image.bearing_y)?;

        // Upload to GPU
        if image.width > 0 && image.height > 0 {
            queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: entry.x,
                        y: entry.y,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                &image.data,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(image.width),
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
        Some(entry)
    }

    /// Rasterize a character
    fn rasterize_char(&mut self, c: char) -> Option<RasterizedGlyph> {
        // Create a buffer with just this character
        // Use Shaping::Basic to match measure_cell() - this ensures consistent font selection
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);

        // Use monospace fallback if the requested font wasn't found or isn't monospace
        let family = if self.use_monospace_fallback {
            Family::Monospace
        } else {
            family_for(&self.font_family)
        };

        let attrs = Attrs::new().family(family);
        buffer.set_text(
            &mut self.font_system,
            &c.to_string(),
            &attrs,
            Shaping::Basic,
            None, // No alignment needed for single chars
        );

        // Get the first glyph
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                // Get the cache key from cosmic-text
                let physical = glyph.physical((0.0, 0.0), 1.0);

                // Rasterize using swash
                if let Some(image) = self.swash_cache.get_image(&mut self.font_system, physical.cache_key) {
                    return Some(RasterizedGlyph {
                        width: image.placement.width,
                        height: image.placement.height,
                        bearing_x: image.placement.left,
                        bearing_y: image.placement.top,
                        data: image.data.clone(),
                    });
                }
            }
        }

        // Return empty glyph for missing characters
        Some(RasterizedGlyph {
            width: 0,
            height: 0,
            bearing_x: 0,
            bearing_y: 0,
            data: vec![],
        })
    }

    /// Allocate space in the atlas
    fn allocate(&mut self, width: u32, height: u32, bearing_x: i32, bearing_y: i32) -> Option<GlyphEntry> {
        let padding = 1;

        // Handle empty glyphs
        if width == 0 || height == 0 {
            return Some(GlyphEntry {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                bearing_x: 0,
                bearing_y: 0,
            });
        }

        // Check if glyph fits in current row
        if self.cursor_x + width + padding > ATLAS_SIZE {
            // Move to next row
            self.cursor_x = 0;
            self.cursor_y += self.row_height + padding;
            self.row_height = 0;
        }

        // Check if glyph fits in atlas
        if self.cursor_y + height + padding > ATLAS_SIZE {
            log::warn!("Glyph atlas is full");
            return None;
        }

        let entry = GlyphEntry {
            x: self.cursor_x,
            y: self.cursor_y,
            width,
            height,
            bearing_x,
            bearing_y,
        };

        self.cursor_x += width + padding;
        self.row_height = self.row_height.max(height);

        Some(entry)
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
        }
    }

    /// Shape a text run with ligatures
    ///
    /// Returns a LigatureRun with all shaped glyphs and their positions.
    /// If ligatures are disabled or the text is a single character, falls back to individual glyphs.
    pub fn shape_run(&mut self, text: &str, queue: &Queue) -> Option<LigatureRun> {
        if !self.ligatures_enabled || text.chars().count() <= 1 {
            return None;
        }

        let key = LigatureKey::new(text, self.font_size);

        if let Some(run) = self.ligature_cache.get(&key) {
            return Some(run.clone());
        }

        let run = self.shape_text_internal(text, queue)?;
        self.ligature_cache.insert(key, run.clone());
        Some(run)
    }

    /// Internal method to shape text and rasterize glyphs
    fn shape_text_internal(&mut self, text: &str, queue: &Queue) -> Option<LigatureRun> {
        let mut glyph_infos: Vec<(cosmic_text::CacheKey, f32, f32)> = Vec::new();

        {
            let mut buffer = Buffer::new(&mut self.font_system, self.metrics);

            let family = if self.use_monospace_fallback {
                Family::Monospace
            } else {
                family_for(&self.font_family)
            };

            let mut features = FontFeatures::new();
            features.enable(FeatureTag::STANDARD_LIGATURES);
            features.enable(FeatureTag::CONTEXTUAL_ALTERNATES);
            features.enable(FeatureTag::DISCRETIONARY_LIGATURES);

            let attrs = Attrs::new().family(family).font_features(features);
            buffer.set_text(
                &mut self.font_system,
                text,
                &attrs,
                Shaping::Advanced,
                None,
            );

            for run in buffer.layout_runs() {
                for glyph in run.glyphs.iter() {
                    let physical = glyph.physical((0.0, 0.0), 1.0);
                    glyph_infos.push((physical.cache_key, glyph.x, glyph.w));
                }
            }
        }

        let mut glyphs = Vec::new();
        let mut total_width: f32 = 0.0;

        for (cache_key, x_offset, width) in glyph_infos {
            let entry = if let Some(image) = self.swash_cache.get_image(&mut self.font_system, cache_key) {
                let img_width = image.placement.width;
                let img_height = image.placement.height;
                let img_left = image.placement.left;
                let img_top = image.placement.top;
                let img_data = image.data.clone();

                let entry = self.allocate(img_width, img_height, img_left, img_top)?;

                if img_width > 0 && img_height > 0 {
                    queue.write_texture(
                        TexelCopyTextureInfo {
                            texture: &self.texture,
                            mip_level: 0,
                            origin: Origin3d {
                                x: entry.x,
                                y: entry.y,
                                z: 0,
                            },
                            aspect: TextureAspect::All,
                        },
                        &img_data,
                        TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(img_width),
                            rows_per_image: Some(img_height),
                        },
                        Extent3d {
                            width: img_width,
                            height: img_height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                entry
            } else {
                GlyphEntry {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                    bearing_x: 0,
                    bearing_y: 0,
                }
            };

            glyphs.push(ShapedGlyph {
                entry,
                x_offset,
                width,
            });

            total_width = total_width.max(x_offset + width);
        }

        Some(LigatureRun {
            glyphs,
            total_width,
        })
    }

    /// Get all installed monospace font families
    ///
    /// Returns a sorted list of unique monospace font family names available on the system.
    /// Filters out emoji fonts and duplicates.
    pub fn get_monospace_fonts(&self) -> Vec<String> {
        use std::collections::HashSet;

        let mut seen = HashSet::new();
        let mut fonts: Vec<String> = self.font_system
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
            .filter_map(|face| {
                face.families.first().map(|(name, _)| name.clone())
            })
            .filter(|name| seen.insert(name.clone())) // Deduplicate
            .collect();

        fonts.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

        // Always include "monospace" as the first option (system default)
        let mut result = vec!["monospace".to_string()];
        result.extend(fonts.into_iter().filter(|f| f.to_lowercase() != "monospace"));

        result
    }

    /// Change font size, clearing the cache and recalculating metrics
    pub fn set_font_size(&mut self, font_size: f32, queue: &Queue) {
        if (self.font_size - font_size).abs() < 0.01 {
            return; // No change needed
        }

        self.font_size = font_size;

        // Recalculate metrics
        let line_height = font_size * LINE_HEIGHT_MULTIPLIER;
        self.metrics = Metrics::new(font_size, line_height);

        // Remeasure cell dimensions
        let (cell_width, cell_height, use_fallback) = measure_cell(&mut self.font_system, &self.metrics, &self.font_family);
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.use_monospace_fallback = use_fallback;

        self.clear_atlas(queue);

        log::info!("Font size changed to {}, cell size: {}x{}", font_size, cell_width, cell_height);
    }

    /// Change font family, clearing the cache and recalculating metrics
    pub fn set_font_family(&mut self, font_family: String, queue: &Queue) {
        if self.font_family == font_family {
            return;
        }

        self.font_family = font_family;

        let (cell_width, cell_height, use_fallback) = measure_cell(&mut self.font_system, &self.metrics, &self.font_family);
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.use_monospace_fallback = use_fallback;

        self.clear_atlas(queue);

        log::info!("Font family changed to {}, cell size: {}x{}", self.font_family, cell_width, cell_height);
    }

    /// Clear the glyph atlas, entry caches, and reset packing cursor.
    fn clear_atlas(&mut self, queue: &Queue) {
        self.entries.clear();
        self.ligature_cache.clear();

        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_height = 0;

        let clear_data = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE) as usize];
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
                depth_or_array_layers: 1,
            },
        );
    }
}

/// A rasterized glyph image
struct RasterizedGlyph {
    width: u32,
    height: u32,
    bearing_x: i32,
    bearing_y: i32,
    data: Vec<u8>,
}

/// Measure cell dimensions for monospace grid
///
/// For terminal rendering, we need a consistent cell width. We measure the
/// advance width of a single character with basic shaping. Using 'M' as it's
/// typically representative of monospace font width.
///
/// Returns (cell_width, cell_height, use_monospace_fallback)
fn measure_cell(font_system: &mut FontSystem, metrics: &Metrics, font_family: &str) -> (f32, f32, bool) {
    let family = family_for(font_family);
    let attrs = Attrs::new().family(family);

    // Measure a single 'M' character to get the natural width
    let mut buffer = Buffer::new(font_system, *metrics);
    buffer.set_text(
        font_system,
        "M",
        &attrs,
        Shaping::Basic,
        None,
    );

    let mut measured_width: f32 = 0.0;
    let mut font_id: Option<cosmic_text::fontdb::ID> = None;
    if let Some(run) = buffer.layout_runs().next() {
        if let Some(glyph) = run.glyphs.first() {
            measured_width = glyph.w;
            font_id = Some(glyph.font_id);
        }
    }

    // Try to get the actual font name and check if it's monospace
    let (actual_font_name, is_monospace) = font_id.map(|id| {
        let name = font_system.db().face(id).map(|face| {
            face.families.first().map(|(name, _)| name.clone()).unwrap_or_else(|| "unknown".to_string())
        }).unwrap_or_else(|| "unknown".to_string());
        let is_mono = font_system.is_monospace(id);
        (name, is_mono)
    }).unwrap_or_else(|| ("none".to_string(), false));

    // Check if the requested font was found by comparing names
    let requested_lower = font_family.to_lowercase();
    let actual_lower = actual_font_name.to_lowercase();

    // Font matches if:
    // 1. User requested generic "monospace" - always matches
    // 2. Actual font name contains or is contained in requested name
    let is_generic_request = matches!(requested_lower.as_str(), "monospace" | "mono" | "default");
    let name_matches = actual_lower.contains(&requested_lower) ||
                       requested_lower.contains(&actual_lower);
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
            font_system,
            "M",
            &Attrs::new().family(Family::Monospace),
            Shaping::Basic,
            None,
        );

        let mut mono_width: f32 = 0.0;
        if let Some(run) = mono_buffer.layout_runs().next() {
            if let Some(glyph) = run.glyphs.first() {
                mono_width = glyph.w;
            }
        }

        log::warn!(
            "Font '{}' not found (got '{}'), using system monospace for rendering",
            font_family, actual_font_name
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
            font_family, measured_width, expected_max
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
