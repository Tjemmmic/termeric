//! GPU rendering pipeline using wgpu
//!
//! This module handles all GPU-accelerated rendering including:
//! - Text glyph rendering with texture atlas
//! - Background rectangle rendering
//! - Cursor rendering

mod glyph_cache;
mod text;
mod rect;

pub use glyph_cache::GlyphCache;

use anyhow::Result;
use std::sync::Arc;
use wgpu::*;
use wgpu::util::DeviceExt;
use winit::window::Window;
use termeric_core::{Grid, Selection};

use text::TextPipeline;
use rect::RectPipeline;

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
    // Glyph cache
    glyph_cache: GlyphCache,
    // Cell dimensions
    cell_width: f32,
    cell_height: f32,
}

impl Renderer {
    /// Create a new renderer for the given window
    pub fn new(window: Arc<Window>) -> Result<Self> {
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
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Create glyph cache
        let font_size = 14.0;
        let glyph_cache = GlyphCache::new(&device, &queue, font_size)?;
        let (cell_width, cell_height) = glyph_cache.cell_size();

        // Create pipelines
        let text_pipeline = TextPipeline::new(&device, surface_format, &glyph_cache)?;
        let rect_pipeline = RectPipeline::new(&device, surface_format)?;

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            size: (size.width, size.height),
            text_pipeline,
            rect_pipeline,
            glyph_cache,
            cell_width,
            cell_height,
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
        }
    }

    /// Get the cell dimensions
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Calculate terminal dimensions from window size
    pub fn calculate_grid_size(&self, width: u32, height: u32) -> (usize, usize) {
        let cols = (width as f32 / self.cell_width).floor() as usize;
        let rows = (height as f32 / self.cell_height).floor() as usize;
        (cols.max(1), rows.max(1))
    }

    /// Render the terminal grid
    pub fn render(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_visible: bool) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        // Build render data from grid
        self.build_render_data(grid, selection, cursor_visible);

        // Upload vertex data
        self.rect_pipeline.upload(&self.device, &self.queue);
        self.text_pipeline.upload(&self.device, &self.queue, &mut self.glyph_cache);

        {
            let mut render_pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.102, // #1A1612 - Turmeric background
                            g: 0.086,
                            b: 0.071,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Draw rectangles (backgrounds, cursor)
            self.rect_pipeline.draw(&mut render_pass);

            // Draw text
            self.text_pipeline.draw(&mut render_pass, &self.glyph_cache);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Build render data from the grid
    fn build_render_data(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_visible: bool) {
        self.rect_pipeline.clear();
        self.text_pipeline.clear();

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;

        // Selection highlight color - turmeric gold with transparency
        let selection_color = [0.91, 0.66, 0.22, 0.3]; // #E8A838 at 30% opacity

        // Iterate over grid cells
        for row in 0..grid.rows() {
            for col in 0..grid.cols() {
                let Some(cell) = grid.cell(row, col) else { continue };

                let x = col as f32 * cell_width;
                let y = row as f32 * cell_height;

                // Check if cell is selected
                let is_selected = selection
                    .map(|s| s.contains(row as i32, col))
                    .unwrap_or(false);

                // Draw background if not default or if selected
                let bg = cell.bg;
                if is_selected {
                    self.rect_pipeline.push_rect(
                        x, y,
                        cell_width, cell_height,
                        selection_color,
                    );
                } else if bg.r != 0 || bg.g != 0 || bg.b != 0 {
                    self.rect_pipeline.push_rect(
                        x, y,
                        cell_width, cell_height,
                        [bg.r as f32 / 255.0, bg.g as f32 / 255.0, bg.b as f32 / 255.0, 1.0],
                    );
                }

                // Draw character
                if let Some(c) = cell.content.char() {
                    if !c.is_whitespace() {
                        let fg = cell.fg;
                        self.text_pipeline.push_char(
                            c,
                            x, y,
                            [fg.r as f32 / 255.0, fg.g as f32 / 255.0, fg.b as f32 / 255.0, 1.0],
                        );
                    }
                }
            }
        }

        // Draw cursor
        if cursor_visible {
            let cursor = grid.cursor();
            if cursor.visible {
                let x = cursor.col as f32 * cell_width;
                let y = cursor.row as f32 * cell_height;

                // Turmeric gold cursor
                self.rect_pipeline.push_rect(
                    x, y,
                    cell_width, cell_height,
                    [0.91, 0.66, 0.22, 1.0], // #E8A838
                );
            }
        }
    }
}
