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
use winit::window::Window;
use termeric_core::{Grid, Selection, Color as TermColor};
use crate::theme::Theme;

use text::TextPipeline;
use rect::RectPipeline;

use crate::tabs::TabInfo;
use crate::command::CommandDef;

/// Command bar display information
#[derive(Debug, Clone)]
pub struct CommandBarInfo {
    /// The current command input text
    pub input: String,
    /// Cursor position in the input
    pub cursor_pos: usize,
    /// Message to display (success or error)
    pub message: Option<String>,
    /// Whether the message is an error
    pub is_error: bool,
    /// Autocomplete suggestions to display
    pub suggestions: Vec<CommandDef>,
    /// Index of selected suggestion
    pub selected_suggestion: usize,
    /// Current input phase
    pub phase: crate::command::CommandInputPhase,
    /// Scroll offset for message display (in lines)
    pub message_scroll_offset: usize,
}

#[derive(Clone)]
struct UiPalette {
    background: wgpu::Color,
    foreground: [f32; 4],
    selection: [f32; 4],
    selection_flash: [f32; 4],
    cursor: [f32; 4],
    tab_bar_bg: [f32; 4],
    tab_active: [f32; 4],
    tab_inactive: [f32; 4],
    tab_activity: [f32; 4],
    tab_border: [f32; 4],
    tab_text_active: [f32; 4],
    tab_text_inactive: [f32; 4],
    command_bg: [f32; 4],
    command_border: [f32; 4],
    command_text: [f32; 4],
    command_error: [f32; 4],
    command_success: [f32; 4],
    command_hint: [f32; 4],
    prompt_selecting: [f32; 4],
    prompt_args: [f32; 4],
    prompt_history: [f32; 4],
    dropdown_bg: [f32; 4],
    dropdown_border: [f32; 4],
    dropdown_shadow: [f32; 4],
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
        let command_bg = shade(bg.to_f32_array(), 0.9);
        let command_text = fg.to_f32_array();

        Self {
            background: bg.to_wgpu(),
            foreground: fg.to_f32_array(),
            selection: selection.to_f32_array(),
            selection_flash: adjust_alpha(fg.to_f32_array(), 0.85),
            cursor: cursor.to_f32_array(),
            tab_bar_bg: shade(tab_inactive.to_f32_array(), 0.85),
            tab_active: tab_active.to_f32_array(),
            tab_inactive: tab_inactive.to_f32_array(),
            tab_activity: theme.primary.yellow.to_f32_array(),
            tab_border: tab_border.to_f32_array(),
            tab_text_active,
            tab_text_inactive,
            command_bg,
            command_border: tab_border.to_f32_array(),
            command_text,
            command_error: theme.primary.red.to_f32_array(),
            command_success: theme.primary.green.to_f32_array(),
            command_hint: adjust_alpha(fg.to_f32_array(), 0.6),
            prompt_selecting: cursor.to_f32_array(),
            prompt_args: theme.primary.yellow.to_f32_array(),
            prompt_history: theme.primary.blue.to_f32_array(),
            dropdown_bg: shade(bg.to_f32_array(), 0.75),
            dropdown_border: tab_border.to_f32_array(),
            dropdown_shadow: [0.0, 0.0, 0.0, 0.3],
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

fn adjust_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
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
}

impl Renderer {
    /// Create a new renderer for the given window
    pub fn new(window: Arc<Window>, font_size: f32, font_family: &str, padding: f32, scale_factor: f32, theme: &Theme) -> Result<Self> {
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
        let scaled_font_size = (font_size * scale_factor).max(1.0);
        let glyph_cache = GlyphCache::new(&device, &queue, scaled_font_size, font_family)?;
        let (cell_width, cell_height) = glyph_cache.cell_size();

        // Create pipelines
        let text_pipeline = TextPipeline::new(&device, surface_format, &glyph_cache)?;
        let rect_pipeline = RectPipeline::new(&device, surface_format)?;

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
            glyph_cache,
            cell_width,
            cell_height,
            padding: padding_physical,
            padding_logical: padding,
            scale_factor,
            font_size,
            ui: UiPalette::from_theme(theme),
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

    /// Get current font family
    pub fn font_family(&self) -> &str {
        self.glyph_cache.font_family()
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

    /// Change font family
    pub fn set_font_family(&mut self, font_family: String) {
        self.glyph_cache.set_font_family(font_family, &self.queue);
        let (cell_width, cell_height) = self.glyph_cache.cell_size();
        self.cell_width = cell_width;
        self.cell_height = cell_height;
    }

    /// Get current padding
    pub fn padding(&self) -> f32 {
        self.padding
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

    fn command_bar_base_height(&self) -> f32 {
        (self.cell_height + 16.0).round().max(28.0)
    }

    /// Render the terminal grid with tab bar
    /// cursor_opacity: 0.0 = invisible, 1.0 = fully visible (for smooth blink animation)
    /// cursor_pos: animated cursor position (col, row) as floats for smooth movement
    /// copy_flash: 0.0 = no flash, 1.0 = full flash (for copy confirmation)
    /// focus_dim: 0.0 = fully bright (focused), higher = dimmed (unfocused)
    /// command_bar: optional command bar info when in command mode
    pub fn render(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo], copy_flash: f32, focus_dim: f32, command_bar: Option<&CommandBarInfo>) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        // Build render data from grid and tabs
        self.build_render_data(grid, selection, cursor_opacity, cursor_pos, tabs, copy_flash, focus_dim, command_bar);

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
                        load: LoadOp::Clear(self.ui.background),
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
    fn build_render_data(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo], copy_flash: f32, focus_dim: f32, command_bar: Option<&CommandBarInfo>) {
        self.rect_pipeline.clear();
        self.text_pipeline.clear();

        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let window_width = self.size.0 as f32;
        let window_height = self.size.1 as f32;
        let padding = self.padding;
        let tab_bar_height = self.tab_bar_height();

        // Calculate command bar height to know where grid ends
        let command_bar_height = if let Some(cmd_bar) = command_bar {
            self.calculate_command_bar_height(cmd_bar, window_width, window_height)
        } else {
            0.0
        };

        // Calculate dropdown height if showing suggestions
        let dropdown_height = if let Some(cmd_bar) = command_bar {
            if !cmd_bar.suggestions.is_empty() {
                self.calculate_dropdown_height(cmd_bar, window_height - command_bar_height)
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Calculate brightness multiplier for focus dimming (1.0 = full brightness, lower = dimmed)
        let brightness = 1.0 - focus_dim;

        // Render tab bar first
        self.build_tab_bar(tabs, window_width, tab_bar_height);

        // Selection highlight color - earthy yellow (inner layer) with transparency
        // When copy_flash > 0, brighten the selection to white for a flash effect
        let selection_base = self.ui.selection;
        let selection_flash = self.ui.selection_flash;
        let selection_color = apply_brightness(mix(selection_base, selection_flash, copy_flash), brightness);

        // Get scroll offset to determine if cursor should be shown
        let scroll_offset = grid.scroll_offset();
        let scroll_offset_i32 = i32::try_from(scroll_offset).unwrap_or(i32::MAX);
        let default_bg = TermColor::default_bg();

        // Calculate maximum y position for grid content (don't overlap command bar or dropdown)
        let max_grid_y = window_height - padding - command_bar_height - dropdown_height - 8.0; // 8px gap for dropdown

        // Iterate over grid cells (offset by tab bar height)
        for row in 0..grid.rows() {
            for col in 0..grid.cols() {
                let Some(cell) = grid.cell_scrolled(row, col) else { continue };

                let x = padding + col as f32 * cell_width;
                let y = tab_bar_height + padding + row as f32 * cell_height;

                // Skip rendering if this row would overlap with command bar
                if y + cell_height > max_grid_y {
                    break;
                }

                // Check if cell is selected
                let is_selected = selection
                    .map(|s| s.contains(row as i32 - scroll_offset_i32, col))
                    .unwrap_or(false);

                // Draw background if not default or if selected
                let bg = cell.bg;
                if is_selected {
                    self.rect_pipeline.push_rect(
                        x, y,
                        cell_width, cell_height,
                        selection_color,
                    );
                } else if bg != default_bg {
                    self.rect_pipeline.push_rect(
                        x, y,
                        cell_width, cell_height,
                        [
                            bg.r as f32 / 255.0 * brightness,
                            bg.g as f32 / 255.0 * brightness,
                            bg.b as f32 / 255.0 * brightness,
                            1.0
                        ],
                    );
                }

                // Draw character with focus dimming applied
                if let Some(c) = cell.content.char() {
                    if !c.is_whitespace() {
                        let fg = cell.fg;
                        self.text_pipeline.push_char(
                            c,
                            x, y,
                            [
                                fg.r as f32 / 255.0 * brightness,
                                fg.g as f32 / 255.0 * brightness,
                                fg.b as f32 / 255.0 * brightness,
                                1.0
                            ],
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
                let x = padding + cursor_pos.0 * cell_width;
                let y = tab_bar_height + padding + cursor_pos.1 * cell_height;

                // Only draw cursor if it won't overlap with command bar
                if y + cell_height <= max_grid_y {
                    // Bright orange cursor (turmeric core) with animated opacity and focus dimming
                    let cursor_color = apply_brightness(self.ui.cursor, brightness);
                    self.rect_pipeline.push_rect(
                        x, y,
                        cell_width, cell_height,
                        [cursor_color[0], cursor_color[1], cursor_color[2], cursor_opacity],
                    );
                }
            }
        }

        // Render command bar last (on top of everything)
        if let Some(cmd_bar) = command_bar {
            self.build_command_bar(cmd_bar, window_width, window_height, command_bar_height);
        }
    }

    /// Build tab bar render data
    fn build_tab_bar(&mut self, tabs: &[TabInfo], window_width: f32, tab_bar_height: f32) {
        let tab_bar_bg = self.ui.tab_bar_bg;
        self.rect_pipeline.push_rect(0.0, 0.0, window_width, tab_bar_height, tab_bar_bg);

        let active_tab_bg = self.ui.tab_active;
        let inactive_tab_bg = self.ui.tab_inactive;
        let activity_indicator = self.ui.tab_activity;
        let active_text_color = self.ui.tab_text_active;
        let inactive_text_color = self.ui.tab_text_inactive;

        // Calculate tab width - minimum 80px, maximum 200px, or divide space evenly
        let tab_count = tabs.len().max(1);
        let full_tab_width = ((window_width / tab_count as f32).min(200.0)).max(80.0);
        let tab_padding = 8.0;
        let tab_height = tab_bar_height - 4.0; // Leave some margin

        // Track cumulative x position (accounting for animated widths)
        let mut current_x = 0.0;

        for (i, tab) in tabs.iter().enumerate() {
            // Calculate animation multiplier based on slide-in and close animations
            let anim_multiplier = if tab.close_progress > 0.0 {
                // Closing: shrink from full to 0 with ease-in curve (accelerate)
                let t = tab.close_progress;
                let eased = t * t; // ease-in: starts slow, speeds up
                1.0 - eased
            } else {
                // Opening: grow from 0 to full with ease-out curve (decelerate)
                let t = tab.slide_progress;
                1.0 - (1.0 - t) * (1.0 - t) // ease-out
            };

            // Animated tab width
            let tab_width = full_tab_width * anim_multiplier;

            let x = current_x;
            let y = 2.0; // Small top margin

            // Only render if the tab has some visible width
            if tab_width > 2.0 {
                // Tab background with animated alpha for fade effect
                let base_bg_color = if tab.is_active {
                    active_tab_bg
                } else if tab.has_activity {
                    activity_indicator
                } else {
                    inactive_tab_bg
                };

                // Fade the background based on animation progress
                let bg_color = [
                    base_bg_color[0],
                    base_bg_color[1],
                    base_bg_color[2],
                    base_bg_color[3] * anim_multiplier,
                ];

                self.rect_pipeline.push_rect(x + 1.0, y, tab_width - 2.0, tab_height, bg_color);

                // Tab text - only render when animation progress is significant
                if anim_multiplier > 0.3 {
                    let text_alpha = ((anim_multiplier - 0.3) / 0.7).min(1.0); // Fade text

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
                        text_x += self.cell_width * 0.85;
                    }
                }
            }

            current_x += tab_width;
        }

        let border_color = adjust_alpha(self.ui.tab_border, 0.7);
        self.rect_pipeline.push_rect(0.0, tab_bar_height - 1.0, window_width, 1.0, border_color);
    }

    /// Calculate command bar height based on content
    fn calculate_command_bar_height(&self, info: &CommandBarInfo, window_width: f32, window_height: f32) -> f32 {
        let padding = 8.0;

        if let Some(msg) = &info.message {
            // Count lines in message (considering newlines and wrapping)
            let max_chars_per_line = ((window_width - padding * 2.0) / self.cell_width) as usize;
            let mut line_count = 0;
            let mut current_line_len = 0;

            for c in msg.chars() {
                if c == '\n' {
                    line_count += 1;
                    current_line_len = 0;
                } else {
                    current_line_len += 1;
                    if current_line_len >= max_chars_per_line {
                        line_count += 1;
                        current_line_len = 0;
                    }
                }
            }
            if current_line_len > 0 {
                line_count += 1;
            }

            // Use dynamic height for messages, capped at 60% of window height
            let needed_height = (line_count as f32 * self.cell_height) + padding * 2.0;
            needed_height.min(window_height * 0.6).max(self.command_bar_base_height())
        } else {
            self.command_bar_base_height()
        }
    }

    /// Calculate dropdown height based on number of suggestions
    fn calculate_dropdown_height(&self, info: &CommandBarInfo, available_height: f32) -> f32 {
        const SUGGESTION_HEIGHT: f32 = 28.0;
        const PADDING: f32 = 12.0;
        const SPACING: f32 = 1.0;
        const BORDER_WIDTH: f32 = 1.0;
        let actual_available_height = available_height - self.tab_bar_height() - 8.0;
        let max_suggestions_that_fit = ((actual_available_height - PADDING * 2.0 - BORDER_WIDTH * 2.0) / (SUGGESTION_HEIGHT + SPACING)).floor() as usize;
        let num_suggestions = info.suggestions.len().min(6).min(max_suggestions_that_fit.max(1));

        if num_suggestions == 0 {
            return 0.0;
        }

        num_suggestions as f32 * (SUGGESTION_HEIGHT + SPACING) - SPACING + PADDING * 2.0 + BORDER_WIDTH * 2.0
    }

    /// Build command bar render data
    fn build_command_bar(&mut self, info: &CommandBarInfo, window_width: f32, window_height: f32, bar_height: f32) {
        let padding = self.padding.max(8.0);
        let bar_y = window_height - bar_height;

        let bar_bg = self.ui.command_bg;
        self.rect_pipeline.push_rect(0.0, bar_y, window_width, bar_height, bar_bg);

        let border_color = adjust_alpha(self.ui.command_border, 0.8);
        self.rect_pipeline.push_rect(0.0, bar_y, window_width, 1.0, border_color);

        // Render message and/or input
        let text_y = bar_y + padding;
        let mut text_x = padding;

        // Reserve space for input line if we're typing
        let has_input = !info.input.is_empty();
        let input_line_height = if has_input { self.cell_height + padding } else { 0.0 };

        if let Some(msg) = &info.message {
            // Show message (supports multi-line and scrolling)
            let text_color = if info.is_error {
                self.ui.command_error
            } else {
                self.ui.command_success
            };

            let mut current_x = text_x;
            let max_y = bar_y + bar_height - padding - input_line_height; // Leave room for input if typing
            let mut line_number = 0; // Track which line we're on (accounting for wraps)
            let scroll_offset = info.message_scroll_offset;
            let mut visible_line_y = text_y; // Y position for rendering (after applying scroll)

            for c in msg.chars() {
                if c == '\n' {
                    // Move to next line
                    line_number += 1;
                    current_x = text_x;
                    // Update visible Y if we're past scroll offset
                    if line_number > scroll_offset {
                        visible_line_y += self.cell_height;
                        if visible_line_y + self.cell_height > max_y {
                            break;
                        }
                    }
                    continue;
                }

                // Word wrap if line is too long
                if current_x + self.cell_width > window_width - padding {
                    line_number += 1;
                    current_x = text_x;
                    if line_number > scroll_offset {
                        visible_line_y += self.cell_height;
                        if visible_line_y + self.cell_height > max_y {
                            break;
                        }
                    }
                }

                // Only render if we're past the scroll offset
                if line_number >= scroll_offset {
                    self.text_pipeline.push_char(c, current_x, visible_line_y, text_color);
                }
                current_x += self.cell_width; // Use same spacing as terminal for readability
            }
        }

        // Show command input (always show if there's input, positioned at bottom when message is present)
        if has_input || info.message.is_none() {
            let input_text_y = if info.message.is_some() && has_input {
                // Position at bottom when message is showing
                bar_y + bar_height - self.cell_height - padding
            } else {
                // Center text vertically when no message
                bar_y + (bar_height - self.cell_height) / 2.0
            };

            text_x = padding; // Reset x position

            let (prompt_text, prompt_color) = match info.phase {
                crate::command::CommandInputPhase::SelectingCommand => {
                    ("> ", self.ui.prompt_selecting)
                }
                crate::command::CommandInputPhase::EnteringArguments => {
                    (": ", self.ui.prompt_args)
                }
                crate::command::CommandInputPhase::BrowsingHistory => {
                    ("⏮ ", self.ui.prompt_history)
                }
            };
            let text_color = self.ui.command_text;

            // Draw prompt
            for c in prompt_text.chars() {
                self.text_pipeline.push_char(c, text_x, input_text_y, prompt_color);
                text_x += self.cell_width;
            }
            text_x += self.cell_width * 0.2; // Small space after prompt

            // Draw input text
            let mut char_index = 0;
            for c in info.input.chars() {
                if text_x + self.cell_width > window_width - padding {
                    break;
                }

                // Draw cursor at current position
                if char_index == info.cursor_pos {
                    let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
                    self.rect_pipeline.push_rect(
                        text_x, input_text_y,
                        2.0, self.cell_height,
                        cursor_color,
                    );
                }

                self.text_pipeline.push_char(c, text_x, input_text_y, text_color);
                text_x += self.cell_width;
                char_index += c.len_utf8();
            }

            // Draw cursor at end if at end of input
            if info.cursor_pos >= info.input.len() {
                let cursor_color = adjust_alpha(self.ui.cursor, 0.8);
                self.rect_pipeline.push_rect(
                    text_x, input_text_y,
                    2.0, self.cell_height,
                    cursor_color,
                );
            }

            // Phase-aware hint text on the right (only when not showing message)
            if info.message.is_none() {
                let hint = match info.phase {
                    crate::command::CommandInputPhase::SelectingCommand => {
                        "↑↓ Navigate  ⭾ Complete  Ctrl+↑↓ History  Esc Cancel"
                    }
                    crate::command::CommandInputPhase::EnteringArguments => {
                        "↑↓ Navigate  ⭾ Complete  ⏎ Execute  Esc Cancel"
                    }
                    crate::command::CommandInputPhase::BrowsingHistory => {
                        "Ctrl+↑↓ Browse  ⏎ Select  Esc Cancel"
                    }
                };
                let hint_color = self.ui.command_hint;
                let hint_width = hint.chars().count() as f32 * self.cell_width * 0.6;
                let mut hint_x = window_width - hint_width - padding;

                for c in hint.chars() {
                    self.text_pipeline.push_char(c, hint_x, input_text_y, hint_color);
                    hint_x += self.cell_width * 0.6;
                }
            }
        }

        // Render autocomplete dropdown above command bar
        if !info.suggestions.is_empty() {
            self.build_autocomplete_dropdown(info, bar_y, window_width, self.tab_bar_height());
        }
    }

    /// Build autocomplete dropdown above command bar
    fn build_autocomplete_dropdown(&mut self, info: &CommandBarInfo, command_bar_y: f32, window_width: f32, tab_bar_height: f32) {
        const SUGGESTION_HEIGHT: f32 = 28.0; // Slightly more compact
        const PADDING: f32 = 12.0; // More padding for breathing room
        const SPACING: f32 = 1.0; // Tighter spacing between items
        const MARGIN: f32 = 16.0; // Margin from window edges
        const BORDER_WIDTH: f32 = 1.0;

        // Calculate how many suggestions can fit above the command bar
        let available_height = command_bar_y - tab_bar_height - 8.0; // 8px gap from tab bar
        let max_suggestions_that_fit = ((available_height - PADDING * 2.0 - BORDER_WIDTH * 2.0) / (SUGGESTION_HEIGHT + SPACING)).floor() as usize;
        let num_suggestions = info.suggestions.len().min(6).min(max_suggestions_that_fit.max(1));

        // Dropdown dimensions - centered with margins
        let dropdown_width = window_width - MARGIN * 2.0;
        let dropdown_height = num_suggestions as f32 * (SUGGESTION_HEIGHT + SPACING) - SPACING + PADDING * 2.0 + BORDER_WIDTH * 2.0;
        let dropdown_x = MARGIN;
        let dropdown_y = command_bar_y - dropdown_height - 8.0; // 8px gap above command bar

        // Shadow effect - draw multiple slightly offset darker rects
        let shadow_color = self.ui.dropdown_shadow;
        for i in 1..=3 {
            let offset = i as f32 * 2.0;
            self.rect_pipeline.push_rect(
                dropdown_x + offset,
                dropdown_y + offset,
                dropdown_width,
                dropdown_height,
                shadow_color,
            );
        }

        // Main dropdown background - solid dark background
        let dropdown_bg = self.ui.dropdown_bg;
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y, dropdown_width, dropdown_height, dropdown_bg);

        let border_color = adjust_alpha(self.ui.dropdown_border, 0.6);
        // Top border
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y, dropdown_width, BORDER_WIDTH, border_color);
        // Bottom border
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y + dropdown_height - BORDER_WIDTH, dropdown_width, BORDER_WIDTH, border_color);
        // Left border
        self.rect_pipeline.push_rect(dropdown_x, dropdown_y, BORDER_WIDTH, dropdown_height, border_color);
        // Right border
        self.rect_pipeline.push_rect(dropdown_x + dropdown_width - BORDER_WIDTH, dropdown_y, BORDER_WIDTH, dropdown_height, border_color);

        // Render each suggestion (only those that fit)
        for (idx, suggestion) in info.suggestions.iter().take(num_suggestions).enumerate() {
            let suggestion_y = dropdown_y + BORDER_WIDTH + PADDING + idx as f32 * (SUGGESTION_HEIGHT + SPACING);
            let is_selected = idx == info.selected_suggestion;

            // Selection highlight with rounded effect
            if is_selected {
                let selection_color = adjust_alpha(self.ui.cursor, 0.2);
                let highlight_margin = 4.0;
                self.rect_pipeline.push_rect(
                    dropdown_x + highlight_margin,
                    suggestion_y,
                    dropdown_width - highlight_margin * 2.0,
                    SUGGESTION_HEIGHT,
                    selection_color,
                );

                // Left accent bar for selected item
                let accent_color = self.ui.cursor;
                let accent_width = 3.0;
                self.rect_pipeline.push_rect(
                    dropdown_x + highlight_margin,
                    suggestion_y,
                    accent_width,
                    SUGGESTION_HEIGHT,
                    accent_color,
                );
            }

            let text_y = suggestion_y + (SUGGESTION_HEIGHT - self.cell_height) / 2.0;
            let mut text_x = dropdown_x + PADDING + (if is_selected { 10.0 } else { 6.0 });

            // Category icon/prefix
            let category_color = if is_selected {
                self.ui.cursor
            } else {
                adjust_alpha(self.ui.prompt_args, 0.7)
            };

            let category_icon = match suggestion.category {
                "Theme" => "◆",
                "Font" => "A",
                "Config" => "⚙",
                "Help" => "?",
                _ => "▸",
            };

            for c in category_icon.chars() {
                self.text_pipeline.push_char(c, text_x, text_y, category_color);
                text_x += self.cell_width; // Standard character width
            }
            text_x += self.cell_width * 0.5; // Space after icon

            // Command name
            let name_color = if is_selected {
                self.ui.foreground
            } else {
                self.ui.command_text
            };

            for c in suggestion.name.chars() {
                self.text_pipeline.push_char(c, text_x, text_y, name_color);
                text_x += self.cell_width;
            }

            // Alias (if present)
            if let Some(alias) = suggestion.alias {
                text_x += self.cell_width * 0.5;
                let alias_color = adjust_alpha(self.ui.command_text, 0.7);

                self.text_pipeline.push_char('(', text_x, text_y, alias_color);
                text_x += self.cell_width;

                for c in alias.chars() {
                    self.text_pipeline.push_char(c, text_x, text_y, alias_color);
                    text_x += self.cell_width;
                }

                self.text_pipeline.push_char(')', text_x, text_y, alias_color);
                text_x += self.cell_width;
            }

            // Description (right side) - add some spacing
            text_x += self.cell_width * 2.0; // Add spacing before description
            let max_desc_x = dropdown_x + dropdown_width - PADDING - 4.0; // Leave margin on right

            let desc_color = if is_selected {
                adjust_alpha(self.ui.command_text, 0.95)
            } else {
                adjust_alpha(self.ui.command_text, 0.75)
            };

            // Add separator before description
            let separator = "—";
            for c in separator.chars() {
                if text_x + self.cell_width < max_desc_x {
                    self.text_pipeline.push_char(c, text_x, text_y, adjust_alpha(self.ui.command_text, 0.5));
                    text_x += self.cell_width * 0.8;
                }
            }
            text_x += self.cell_width * 0.5; // Small gap after separator

            for c in suggestion.description.chars() {
                if text_x + self.cell_width * 3.0 > max_desc_x {
                    // Add ellipsis if we're running out of space
                    if text_x + self.cell_width * 1.5 <= max_desc_x {
                        self.text_pipeline.push_char('.', text_x, text_y, desc_color);
                        text_x += self.cell_width * 0.4;
                        self.text_pipeline.push_char('.', text_x, text_y, desc_color);
                        text_x += self.cell_width * 0.4;
                        self.text_pipeline.push_char('.', text_x, text_y, desc_color);
                    }
                    break;
                }

                self.text_pipeline.push_char(c, text_x, text_y, desc_color);
                text_x += self.cell_width * 0.95; // Slightly compressed for descriptions
            }
        }
    }
}
