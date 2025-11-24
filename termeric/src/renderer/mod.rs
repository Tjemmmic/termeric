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

pub use glyph_cache::GlyphCache;

use anyhow::Result;
use std::sync::Arc;
use wgpu::*;
use wgpu::util::DeviceExt;
use winit::window::Window;
use termeric_core::{Grid, Selection};

use text::TextPipeline;
use rect::RectPipeline;

use crate::tabs::TabInfo;

/// Height of the tab bar in pixels
const TAB_BAR_HEIGHT: f32 = 28.0;

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
        let font_size = 35.0;
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

    /// Calculate terminal dimensions from window size (accounting for tab bar)
    pub fn calculate_grid_size(&self, width: u32, height: u32) -> (usize, usize) {
        let cols = (width as f32 / self.cell_width).floor() as usize;
        // Subtract tab bar height from available height
        let available_height = (height as f32 - TAB_BAR_HEIGHT).max(0.0);
        let rows = (available_height / self.cell_height).floor() as usize;
        (cols.max(1), rows.max(1))
    }

    /// Get the tab bar height
    pub fn tab_bar_height(&self) -> f32 {
        TAB_BAR_HEIGHT
    }

    /// Get current font size
    pub fn font_size(&self) -> f32 {
        self.glyph_cache.font_size()
    }

    /// Change font size
    pub fn set_font_size(&mut self, font_size: f32) {
        self.glyph_cache.set_font_size(font_size, &self.queue);
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
    }

    /// Render the terminal grid with tab bar
    /// cursor_opacity: 0.0 = invisible, 1.0 = fully visible (for smooth blink animation)
    /// cursor_pos: animated cursor position (col, row) as floats for smooth movement
    pub fn render(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo]) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        // Build render data from grid and tabs
        self.build_render_data(grid, selection, cursor_opacity, cursor_pos, tabs);

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
                            r: 0.110, // #1C1410 - Dark brown (dried turmeric skin)
                            g: 0.078,
                            b: 0.063,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Draw rectangles (backgrounds, cursor, tab bar)
            self.rect_pipeline.draw(&mut render_pass);

            // Draw text (terminal content and tab labels)
            self.text_pipeline.draw(&mut render_pass, &self.glyph_cache);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Build render data from the grid and tabs
    fn build_render_data(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo]) {
        self.rect_pipeline.clear();
        self.text_pipeline.clear();

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let window_width = self.size.0 as f32;

        // Render tab bar first
        self.build_tab_bar(tabs, window_width);

        // Selection highlight color - earthy yellow (inner layer) with transparency
        let selection_color = [0.79, 0.64, 0.15, 0.35]; // #C9A227 at 35% opacity

        // Get scroll offset to determine if cursor should be shown
        let scroll_offset = grid.scroll_offset();

        // Iterate over grid cells (offset by tab bar height)
        for row in 0..grid.rows() {
            for col in 0..grid.cols() {
                let Some(cell) = grid.cell_scrolled(row, col) else { continue };

                let x = col as f32 * cell_width;
                let y = TAB_BAR_HEIGHT + row as f32 * cell_height;

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

        // Draw cursor (only when not scrolled back into history)
        if cursor_opacity > 0.0 && scroll_offset == 0 {
            let cursor = grid.cursor();
            if cursor.visible {
                // Use animated cursor position for smooth movement
                let x = cursor_pos.0 * cell_width;
                let y = TAB_BAR_HEIGHT + cursor_pos.1 * cell_height;

                // Bright orange cursor (turmeric core) with animated opacity
                self.rect_pipeline.push_rect(
                    x, y,
                    cell_width, cell_height,
                    [0.88, 0.48, 0.13, cursor_opacity], // #E07B20 with animated alpha
                );
            }
        }
    }

    /// Build tab bar render data
    fn build_tab_bar(&mut self, tabs: &[TabInfo], window_width: f32) {
        // Tab bar background - medium brown (between skin tones)
        let tab_bar_bg = [0.18, 0.14, 0.13, 1.0]; // #2D2420
        self.rect_pipeline.push_rect(0.0, 0.0, window_width, TAB_BAR_HEIGHT, tab_bar_bg);

        // Tab colors - turmeric-inspired
        let active_tab_bg = [0.88, 0.48, 0.13, 1.0];   // #E07B20 - Bright orange (core)
        let inactive_tab_bg = [0.24, 0.19, 0.16, 1.0]; // #3D3028 - Lighter brown
        let activity_indicator = [0.79, 0.64, 0.15, 1.0]; // #C9A227 - Earthy yellow (inner layer)
        let active_text_color = [0.11, 0.08, 0.06, 1.0];  // Dark brown text on orange
        let inactive_text_color = [0.83, 0.77, 0.66, 1.0]; // #D4C4A8 - Cream/tan text

        // Calculate tab width - minimum 80px, maximum 200px, or divide space evenly
        let tab_count = tabs.len().max(1);
        let full_tab_width = ((window_width / tab_count as f32).min(200.0)).max(80.0);
        let tab_padding = 8.0;
        let tab_height = TAB_BAR_HEIGHT - 4.0; // Leave some margin

        // Track cumulative x position (accounting for animated widths)
        let mut current_x = 0.0;

        for (i, tab) in tabs.iter().enumerate() {
            // Apply ease-out curve for smoother animation: t' = 1 - (1-t)^2
            let t = tab.slide_progress;
            let eased_progress = 1.0 - (1.0 - t) * (1.0 - t);

            // Animated tab width grows from 0 to full width
            let tab_width = full_tab_width * eased_progress;

            let x = current_x;
            let y = 2.0; // Small top margin

            // Only render if the tab has some visible width
            if tab_width > 2.0 {
                // Tab background with animated alpha for fade-in effect
                let base_bg_color = if tab.is_active {
                    active_tab_bg
                } else if tab.has_activity {
                    activity_indicator
                } else {
                    inactive_tab_bg
                };

                // Fade in the background
                let bg_color = [
                    base_bg_color[0],
                    base_bg_color[1],
                    base_bg_color[2],
                    base_bg_color[3] * eased_progress,
                ];

                self.rect_pipeline.push_rect(x + 1.0, y, tab_width - 2.0, tab_height, bg_color);

                // Tab text - only render when animation is far enough along
                if eased_progress > 0.3 {
                    let text_alpha = ((eased_progress - 0.3) / 0.7).min(1.0); // Fade in text after 30%

                    let base_text_color = if tab.is_active {
                        active_text_color
                    } else {
                        inactive_text_color
                    };

                    let text_color = [
                        base_text_color[0],
                        base_text_color[1],
                        base_text_color[2],
                        base_text_color[3] * text_alpha,
                    ];

                    // Render tab title (truncate if needed)
                    let title = if tab.title.len() > 15 {
                        format!("{}...", &tab.title[..12])
                    } else {
                        tab.title.clone()
                    };

                    // Add tab number prefix
                    let label = format!("{}: {}", i + 1, title);

                    // Render each character of the label
                    let text_y = y + (tab_height - self.cell_height) / 2.0; // Center vertically
                    let mut text_x = x + tab_padding;

                    for c in label.chars() {
                        if text_x + self.cell_width > x + tab_width - tab_padding {
                            break; // Don't overflow tab
                        }
                        if !c.is_whitespace() {
                            self.text_pipeline.push_char(c, text_x, text_y, text_color);
                        }
                        text_x += self.cell_width * 0.6; // Tighter spacing for tab labels
                    }
                }
            }

            current_x += tab_width;
        }

        // Bottom border of tab bar - earthy yellow accent
        let border_color = [0.79, 0.64, 0.15, 0.6]; // #C9A227 at 60% opacity
        self.rect_pipeline.push_rect(0.0, TAB_BAR_HEIGHT - 1.0, window_width, 1.0, border_color);
    }
}
