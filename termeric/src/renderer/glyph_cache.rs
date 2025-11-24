//! Glyph caching with texture atlas
//!
//! This module handles font loading, glyph rasterization, and texture atlas management.

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
    /// Current packing position
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    /// Cell dimensions
    cell_width: f32,
    cell_height: f32,
    /// Font size
    font_size: f32,
}

impl GlyphCache {
    /// Create a new glyph cache
    pub fn new(device: &Device, queue: &Queue, font_size: f32) -> Result<Self> {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();

        // Calculate metrics
        let line_height = font_size * 1.2;
        let metrics = Metrics::new(font_size, line_height);

        // Measure a character to get cell dimensions
        let (cell_width, cell_height) = measure_cell(&mut font_system, &metrics);

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
            ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &clear_data,
            ImageDataLayout {
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
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
            cell_width,
            cell_height,
            font_size,
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
                ImageCopyTexture {
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
                ImageDataLayout {
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
        buffer.set_text(
            &mut self.font_system,
            &c.to_string(),
            Attrs::new().family(Family::Monospace),
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
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_height = 0;
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
fn measure_cell(font_system: &mut FontSystem, metrics: &Metrics) -> (f32, f32) {
    let mut buffer = Buffer::new(font_system, *metrics);
    buffer.set_text(
        font_system,
        "M",
        Attrs::new().family(Family::Monospace),
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

    (width, metrics.line_height)
}
