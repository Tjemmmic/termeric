//! Glyph caching with texture atlas
//!
//! This module handles font loading, glyph rasterization, and texture atlas management.
//! Supports font ligatures through cosmic-text's advanced shaping.

use anyhow::Result;
use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache,
};
use std::collections::HashMap;
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
    /// Font family name
    font_family: String,
    /// Whether ligatures are enabled
    ligatures_enabled: bool,
}

impl GlyphCache {
    /// Create a new glyph cache
    pub fn new(device: &Device, queue: &Queue, font_size: f32, font_family: &str, ligatures: bool) -> Result<Self> {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();

        // Calculate metrics
        let line_height = font_size * 1.2;
        let metrics = Metrics::new(font_size, line_height);

        // Measure a character to get cell dimensions
        let (cell_width, cell_height) = measure_cell(&mut font_system, &metrics, font_family);

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
        let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
        let family = family_for(&self.font_family);
        buffer.set_text(
            &mut self.font_system,
            &c.to_string(),
            Attrs::new().family(family),
            Shaping::Advanced,
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

    /// Clear the cache
    pub fn clear(&mut self) {
        self.entries.clear();
        self.ligature_cache.clear();
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_height = 0;
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
        // For single characters or disabled ligatures, no point in ligature shaping
        if !self.ligatures_enabled || text.chars().count() <= 1 {
            return None;
        }

        let key = LigatureKey::new(text, self.font_size);

        // Check cache
        if let Some(run) = self.ligature_cache.get(&key) {
            return Some(run.clone());
        }

        // Shape the text
        let run = self.shape_text_internal(text, queue)?;
        self.ligature_cache.insert(key, run.clone());
        Some(run)
    }

    /// Internal method to shape text and rasterize glyphs
    fn shape_text_internal(&mut self, text: &str, queue: &Queue) -> Option<LigatureRun> {
        // First, shape the text and collect glyph info
        let mut glyph_infos: Vec<(cosmic_text::CacheKey, f32, f32)> = Vec::new();

        {
            let mut buffer = Buffer::new(&mut self.font_system, self.metrics);
            let family = family_for(&self.font_family);
            buffer.set_text(
                &mut self.font_system,
                text,
                Attrs::new().family(family),
                Shaping::Advanced,
            );

            for run in buffer.layout_runs() {
                for glyph in run.glyphs.iter() {
                    let physical = glyph.physical((0.0, 0.0), 1.0);
                    glyph_infos.push((physical.cache_key, glyph.x, glyph.w));
                }
            }
        }

        // Now rasterize each glyph
        let mut glyphs = Vec::new();
        let mut total_width: f32 = 0.0;

        for (cache_key, x_offset, width) in glyph_infos {
            // Rasterize the glyph
            let entry = if let Some(image) = self.swash_cache.get_image(&mut self.font_system, cache_key) {
                // Copy image data before allocating to avoid borrow issues
                let img_width = image.placement.width;
                let img_height = image.placement.height;
                let img_left = image.placement.left;
                let img_top = image.placement.top;
                let img_data = image.data.clone();

                // Allocate space in atlas
                let entry = self.allocate(img_width, img_height, img_left, img_top)?;

                // Upload to GPU
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
                // Empty glyph
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

    /// Change font size, clearing the cache and recalculating metrics
    pub fn set_font_size(&mut self, font_size: f32, queue: &Queue) {
        if (self.font_size - font_size).abs() < 0.01 {
            return; // No change needed
        }

        self.font_size = font_size;

        // Recalculate metrics
        let line_height = font_size * 1.2;
        self.metrics = Metrics::new(font_size, line_height);

        // Remeasure cell dimensions
        let (cell_width, cell_height) = measure_cell(&mut self.font_system, &self.metrics, &self.font_family);
        self.cell_width = cell_width;
        self.cell_height = cell_height;

        // Clear the atlas and caches
        self.entries.clear();
        self.ligature_cache.clear();
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_height = 0;

        // Clear the texture
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

        log::info!("Font size changed to {}, cell size: {}x{}", font_size, cell_width, cell_height);
    }

    /// Change font family, clearing the cache and recalculating metrics
    pub fn set_font_family(&mut self, font_family: String, queue: &Queue) {
        if self.font_family == font_family {
            return;
        }

        self.font_family = font_family;

        let (cell_width, cell_height) = measure_cell(&mut self.font_system, &self.metrics, &self.font_family);
        self.cell_width = cell_width;
        self.cell_height = cell_height;

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

        log::info!("Font family changed to {}, cell size: {}x{}", self.font_family, cell_width, cell_height);
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

/// Measure cell dimensions using a sample character
fn measure_cell(font_system: &mut FontSystem, metrics: &Metrics, font_family: &str) -> (f32, f32) {
    let mut buffer = Buffer::new(font_system, *metrics);
    let family = family_for(font_family);
    buffer.set_text(
        font_system,
        "M",
        Attrs::new().family(family),
        Shaping::Advanced,
    );

    // Get width from first glyph
    let mut width = metrics.font_size * 0.6; // Default fallback
    for run in buffer.layout_runs() {
        for glyph in run.glyphs.iter() {
            width = glyph.w;
            break;
        }
    }

    let width = width.round().max(1.0);
    let height = metrics.line_height.round().max(1.0);
    (width, height)
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
