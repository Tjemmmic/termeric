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
use crate::settings::SettingsPanelInfo;

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
    // Settings panel colors
    settings_bg: [f32; 4],
    settings_sidebar_bg: [f32; 4],
    settings_header: [f32; 4],
    settings_text: [f32; 4],
    settings_text_dim: [f32; 4],
    settings_selected: [f32; 4],
    settings_value: [f32; 4],
    settings_accent: [f32; 4],
    settings_separator: [f32; 4],
    scrim: [f32; 4],
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
            dropdown_shadow: shade(bg.to_f32_array(), 0.0),
            // Settings panel
            settings_bg: adjust_alpha(shade(bg.to_f32_array(), 0.85), 0.97),
            settings_sidebar_bg: adjust_alpha(shade(bg.to_f32_array(), 0.7), 0.97),
            settings_header: fg.to_f32_array(),
            settings_text: fg.to_f32_array(),
            settings_text_dim: adjust_alpha(fg.to_f32_array(), 0.55),
            settings_selected: adjust_alpha(cursor.to_f32_array(), 0.25),
            settings_value: cursor.to_f32_array(),
            settings_accent: tab_active.to_f32_array(),
            settings_separator: adjust_alpha(tab_border.to_f32_array(), 0.4),
            // Warm scrim derived from theme background
            scrim: adjust_alpha(shade(bg.to_f32_array(), 0.3), 0.30),
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
    // Window opacity (0.0 - 1.0)
    opacity: f32,
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
            alpha_mode: if surface_caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
                wgpu::CompositeAlphaMode::PreMultiplied
            } else {
                surface_caps.alpha_modes[0]
            },
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
            opacity: 1.0,
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

    /// Set window opacity (0.0 = fully transparent, 1.0 = fully opaque)
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.1, 1.0);
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
    pub fn render(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo], copy_flash: f32, focus_dim: f32, command_bar: Option<&CommandBarInfo>, settings_panel: Option<&SettingsPanelInfo>) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        // Build render data from grid and tabs
        self.build_render_data(grid, selection, cursor_opacity, cursor_pos, tabs, copy_flash, focus_dim, command_bar, settings_panel);

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
    fn build_render_data(&mut self, grid: &Grid, selection: Option<&Selection>, cursor_opacity: f32, cursor_pos: (f32, f32), tabs: &[TabInfo], copy_flash: f32, focus_dim: f32, command_bar: Option<&CommandBarInfo>, settings_panel: Option<&SettingsPanelInfo>) {
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
                    self.rect_pipeline.push_rounded_rect(
                        x, y,
                        cell_width, cell_height,
                        [cursor_color[0], cursor_color[1], cursor_color[2], cursor_opacity],
                        2.0,
                    );
                }
            }
        }

        // Render command bar last (on top of everything)
        if let Some(cmd_bar) = command_bar {
            self.build_command_bar(cmd_bar, window_width, window_height, command_bar_height);
        }

        // Render settings panel overlay (on top of everything except command bar)
        if let Some(sp) = settings_panel {
            if sp.animation > 0.001 {
                self.build_settings_panel(sp, window_width, window_height);
            }
        }

        // Render dropdown overlay absolutely last (on top of everything)
        if let Some(sp) = settings_panel {
            if sp.animation > 0.001 && sp.dropdown_open {
                self.build_dropdown_overlay(sp, window_width, window_height);
            }
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
        // Reserve space for the gear icon on the right
        let gear_button_width = tab_bar_height;
        let available_tab_width = window_width - gear_button_width;
        let tab_count = tabs.len().max(1);
        let full_tab_width = ((available_tab_width / tab_count as f32).min(200.0)).max(80.0);
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

                self.rect_pipeline.push_rounded_rect(x + 1.0, y, tab_width - 2.0, tab_height, bg_color, 4.0);

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

        // ── Gear icon button (right side of tab bar) ──
        let gear_size = tab_bar_height;
        let gear_x = window_width - gear_size;
        let gear_y = 0.0;
        // Subtle background for the gear button
        let gear_bg = adjust_alpha(self.ui.tab_inactive, 0.5);
        self.rect_pipeline.push_rounded_rect(
            gear_x + 2.0, gear_y + 2.0,
            gear_size - 4.0, tab_bar_height - 6.0,
            gear_bg, 4.0,
        );
        // Render the gear icon character centered
        let gear_char_x = gear_x + (gear_size - self.cell_width) / 2.0;
        let gear_char_y = gear_y + (tab_bar_height - self.cell_height) / 2.0;
        self.text_pipeline.push_char('⚙', gear_char_x, gear_char_y, self.ui.tab_text_inactive);
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
            self.rect_pipeline.push_rounded_rect(
                dropdown_x + offset,
                dropdown_y + offset,
                dropdown_width,
                dropdown_height,
                shadow_color,
                8.0,
            );
        }

        // Main dropdown background - solid dark background
        let dropdown_bg = self.ui.dropdown_bg;
        self.rect_pipeline.push_rounded_rect(dropdown_x, dropdown_y, dropdown_width, dropdown_height, dropdown_bg, 8.0);

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
                self.rect_pipeline.push_rounded_rect(
                    dropdown_x + highlight_margin,
                    suggestion_y,
                    dropdown_width - highlight_margin * 2.0,
                    SUGGESTION_HEIGHT,
                    selection_color,
                    4.0,
                );

                // Left accent bar for selected item
                let accent_color = self.ui.cursor;
                let accent_width = 3.0;
                self.rect_pipeline.push_rounded_rect(
                    dropdown_x + highlight_margin,
                    suggestion_y,
                    accent_width,
                    SUGGESTION_HEIGHT,
                    accent_color,
                    2.0,
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

    /// Build settings panel render data
    fn build_settings_panel(&mut self, info: &SettingsPanelInfo, window_width: f32, window_height: f32) {
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        // UI character width — capped so the settings panel stays usable
        // with wide/large fonts. Terminal grid uses self.cell_width directly.
        let ui_cw = self.cell_width.min(10.0);

        // Panel dimensions — all derived from font metrics so layout adapts
        // Sidebar: padding + icon(3 cells) + longest label("Scrollback"=10) + padding = ~16.5 cells
        let padding_x = ui_cw * 1.5;
        let sidebar_width = padding_x + ui_cw * 3.0 + ui_cw * 10.0 + padding_x;
        // Content: label area + gap + value area + padding
        let min_content_width = ui_cw * 30.0; // needs room for labels + values
        let panel_width = ((sidebar_width + min_content_width).max(window_width * 0.35)).min(window_width * 0.55);
        let panel_x = window_width - panel_width * anim; // slide in from right
        let panel_y = 0.0;
        let panel_h = window_height;

        // Content area is whatever remains after sidebar
        let content_x = panel_x + sidebar_width;
        let content_width = panel_width - sidebar_width;

        let row_height = (self.cell_height * 2.2).max(32.0);
        let padding_y = self.cell_height * 0.5;

        // ── Scrim (warm-tinted dimmed overlay on terminal behind panel) ──
        let scrim = self.ui.scrim;
        self.rect_pipeline.push_rect(
            0.0, 0.0,
            panel_x, window_height,
            [scrim[0], scrim[1], scrim[2], scrim[3] * anim],
        );

        // ── Sidebar background ──
        self.rect_pipeline.push_rect(
            panel_x, panel_y,
            sidebar_width, panel_h,
            self.ui.settings_sidebar_bg,
        );

        // ── Main panel background ──
        self.rect_pipeline.push_rect(
            content_x, panel_y,
            content_width, panel_h,
            self.ui.settings_bg,
        );

        // ── Sidebar border (thin line between sidebar and content) ──
        self.rect_pipeline.push_rect(
            content_x - 1.0, panel_y,
            1.0, panel_h,
            self.ui.settings_separator,
        );

        // ── Panel title in sidebar ──
        let title = "Settings";
        let title_y = panel_y + self.cell_height * 1.5;
        let title_x = panel_x + padding_x;
        for (i, c) in title.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                title_x + i as f32 * ui_cw,
                title_y,
                self.ui.settings_accent,
            );
        }

        // ── Section buttons in sidebar ──
        let section_start_y = title_y + self.cell_height * 2.5;
        for (idx, (section, is_active)) in info.sections.iter().enumerate() {
            let sy = section_start_y + idx as f32 * row_height;

            // Active section highlight bar
            if *is_active {
                // Left accent bar
                self.rect_pipeline.push_rounded_rect(
                    panel_x, sy,
                    3.0, row_height,
                    self.ui.settings_accent,
                    2.0,
                );
                // Highlight background
                self.rect_pipeline.push_rounded_rect(
                    panel_x + 3.0, sy,
                    sidebar_width - 3.0, row_height,
                    self.ui.settings_selected,
                    4.0,
                );
            }

            // Section icon
            let icon_x = panel_x + padding_x;
            let text_y = sy + (row_height - self.cell_height) / 2.0;
            for (i, c) in section.icon().chars().enumerate() {
                let color = if *is_active { self.ui.settings_accent } else { self.ui.settings_text_dim };
                self.text_pipeline.push_char(
                    c,
                    icon_x + i as f32 * ui_cw,
                    text_y,
                    color,
                );
            }

            // Section label
            let label_x = icon_x + ui_cw * 3.0;
            let color = if *is_active { self.ui.settings_text } else { self.ui.settings_text_dim };
            for (i, c) in section.label().chars().enumerate() {
                self.text_pipeline.push_char(
                    c,
                    label_x + i as f32 * ui_cw,
                    text_y,
                    color,
                );
            }
        }

        // ── Keyboard hints at bottom of sidebar ──
        let hints = ["↑↓ Navigate", "←→ Adjust", "Tab Section", "Esc Close"];
        let hint_start_y = window_height - (hints.len() as f32 * self.cell_height * 1.6) - self.cell_height;
        for (idx, hint) in hints.iter().enumerate() {
            let hy = hint_start_y + idx as f32 * self.cell_height * 1.6;
            let hx = panel_x + padding_x;
            for (i, c) in hint.chars().enumerate() {
                if hx + i as f32 * ui_cw * 0.85 < content_x - padding_x {
                    self.text_pipeline.push_char(
                        c,
                        hx + i as f32 * ui_cw * 0.85,
                        hy,
                        self.ui.settings_text_dim,
                    );
                }
            }
        }

        // ── Content area: section header ──
        let header_y = panel_y + self.cell_height * 1.5;
        let header_x = content_x + padding_x;
        let section_title = info.section.label();
        for (i, c) in section_title.chars().enumerate() {
            self.text_pipeline.push_char(
                c,
                header_x + i as f32 * ui_cw * 1.1,
                header_y,
                self.ui.settings_header,
            );
        }

        // Separator under header
        let sep_y = header_y + self.cell_height * 1.8;
        self.rect_pipeline.push_rect(
            content_x + padding_x, sep_y,
            content_width - padding_x * 2.0, 1.0,
            self.ui.settings_separator,
        );

        // ── Content area: setting items ──
        let items_start_y = sep_y + padding_y + self.cell_height * 0.5;

        // Dynamic value-area X: positioned after the longest label + a gap,
        // clamped so it never exceeds 65% of content width.
        let max_label_len = info.items.iter()
            .map(|item| item.label.len())
            .max()
            .unwrap_or(0);
        let label_end = padding_x + max_label_len as f32 * ui_cw;
        let min_gap = ui_cw * 2.0;
        let max_value_area = content_width * 0.65;
        let value_area_offset = (label_end + min_gap).min(max_value_area);
        let value_area_x = content_x + value_area_offset;

        // Calculate dropdown occlusion region (if dropdown is open, items below
        // the selected item may be hidden behind the dropdown overlay).
        // Since all text renders on top of all rects, we must skip rendering
        // text for occluded items to avoid text-on-text confusion.
        // (dd_x, dd_top, dd_bottom) — X is the left edge of the dropdown
        let dropdown_occludes: Option<(f32, f32, f32)> = if info.dropdown_open {
            if let Some(sel_item) = info.items.get(info.selected_item) {
                let option_count = match &sel_item.kind {
                    crate::settings::SettingKind::Dropdown { options, .. } => options.len(),
                    crate::settings::SettingKind::SteppedList { values, .. } => values.len(),
                    _ => 0,
                };
                let visible_count = option_count.min(8);
                let option_h = row_height * 0.85;
                let dd_x = value_area_x;
                let dd_top = items_start_y + info.selected_item as f32 * row_height + row_height;
                let dd_bottom = dd_top + visible_count as f32 * option_h + 8.0;
                Some((dd_x, dd_top, dd_bottom))
            } else {
                None
            }
        } else {
            None
        };

        for (idx, item) in info.items.iter().enumerate() {
            let iy = items_start_y + idx as f32 * row_height;
            let is_selected = idx == info.selected_item;

            // Check if this item's text would be occluded by the dropdown
            let is_occluded = if let Some((_dd_x, dd_top, dd_bottom)) = dropdown_occludes {
                let item_top = iy;
                let item_bottom = iy + row_height;
                // Item is occluded if it overlaps the dropdown region
                item_top < dd_bottom && item_bottom > dd_top
            } else {
                false
            };

            // Selection highlight
            if is_selected {
                self.rect_pipeline.push_rounded_rect(
                    content_x + padding_x * 0.5, iy,
                    content_width - padding_x, row_height,
                    self.ui.settings_selected,
                    4.0,
                );
                // Left accent bar for selected item
                self.rect_pipeline.push_rounded_rect(
                    content_x, iy,
                    3.0, row_height,
                    self.ui.settings_accent,
                    2.0,
                );
            }

            // If occluded by dropdown, still draw labels but skip value text below

            let text_y = iy + (row_height - self.cell_height) / 2.0;

            // Label — clip at dropdown boundary if occluded
            let label_x = content_x + padding_x;
            let label_color = if is_selected { self.ui.settings_text } else { self.ui.settings_text_dim };
            let label_clip_x = if is_occluded {
                if let Some((dd_x, _, _)) = dropdown_occludes { dd_x } else { f32::MAX }
            } else {
                f32::MAX
            };
            for (i, c) in item.label.chars().enumerate() {
                let cx = label_x + i as f32 * ui_cw;
                if cx + ui_cw > label_clip_x {
                    break; // Don't render characters that extend into dropdown
                }
                self.text_pipeline.push_char(c, cx, text_y, label_color);
            }

            // Value (right-aligned area) — skip if occluded by dropdown
            if !is_occluded {
                // value_area_x already computed above from longest label
                let value_color = if is_selected { self.ui.settings_value } else { self.ui.settings_text };
                let max_x = content_x + content_width - padding_x;

                match &item.kind {
                    crate::settings::SettingKind::Numeric { .. } => {
                        // Draw ◀ value ▶ controls for numeric
                        if is_selected {
                            self.text_pipeline.push_char(
                                '◀',
                                value_area_x,
                                text_y,
                                self.ui.settings_accent,
                            );
                        }

                        let val_x = value_area_x + ui_cw * 2.0;
                        for (i, c) in item.value.chars().enumerate() {
                            let cx = val_x + i as f32 * ui_cw;
                            if cx + ui_cw > max_x { break; }
                            self.text_pipeline.push_char(c, cx, text_y, value_color);
                        }

                        if is_selected {
                            let arrow_x = val_x + item.value.len() as f32 * ui_cw + ui_cw;
                            if arrow_x + ui_cw <= max_x {
                                self.text_pipeline.push_char('▶', arrow_x, text_y, self.ui.settings_accent);
                            }
                        }
                    }
                    crate::settings::SettingKind::Dropdown { .. } | crate::settings::SettingKind::SteppedList { .. } => {
                        // Draw value with ▼ chevron (click or Enter to open dropdown)
                        let val_x = value_area_x + ui_cw * 2.0;
                        for (i, c) in item.value.chars().enumerate() {
                            let cx = val_x + i as f32 * ui_cw;
                            if cx + ui_cw > max_x { break; }
                            self.text_pipeline.push_char(c, cx, text_y, value_color);
                        }

                        // Down chevron — only if it fits
                        let chevron_x = val_x + item.value.len() as f32 * ui_cw + ui_cw;
                        if chevron_x + ui_cw <= max_x {
                            let chevron_color = if is_selected { self.ui.settings_accent } else { self.ui.settings_text_dim };
                            self.text_pipeline.push_char('▼', chevron_x, text_y, chevron_color);
                        }
                    }
                    crate::settings::SettingKind::ReadOnly => {
                        // Just display the value, truncated with '.'
                        let val_x = value_area_x + ui_cw * 2.0;
                        for (i, c) in item.value.chars().enumerate() {
                            let cx = val_x + i as f32 * ui_cw;
                            if cx + ui_cw > max_x {
                                self.text_pipeline.push_char('.', cx, text_y, self.ui.settings_text_dim);
                                break;
                            }
                            self.text_pipeline.push_char(
                                c, cx, text_y,
                                self.ui.settings_text_dim,
                            );
                        }
                    }
                }
            }

            // Thin separator between items
            if idx + 1 < info.items.len() {
                self.rect_pipeline.push_rect(
                    content_x + padding_x, iy + row_height - 0.5,
                    content_width - padding_x * 2.0, 0.5,
                    self.ui.settings_separator,
                );
            }
        }

        // ── Footer: Save hint ──
        let footer_y = window_height - self.cell_height * 2.0;
        let footer_text = "Ctrl+S Save  |  Ctrl+R Reset";
        let footer_x = content_x + padding_x;
        // Separator above footer
        self.rect_pipeline.push_rect(
            content_x + padding_x, footer_y - padding_y,
            content_width - padding_x * 2.0, 1.0,
            self.ui.settings_separator,
        );

        // Skip footer text if occluded by dropdown
        let footer_occluded = if let Some((_, _dd_top, dd_bottom)) = dropdown_occludes {
            footer_y < dd_bottom
        } else {
            false
        };
        if !footer_occluded {
            let footer_max_x = content_x + content_width - padding_x;
            for (i, c) in footer_text.chars().enumerate() {
                let cx = footer_x + i as f32 * ui_cw * 0.9;
                if cx + ui_cw > footer_max_x { break; }
                self.text_pipeline.push_char(c, cx, footer_y, self.ui.settings_text_dim);
            }
        }

        // ── Dropdown overlay rendered separately in build_dropdown_overlay ──
    }

    /// Build dropdown overlay render data — rendered last so it appears on top of everything
    fn build_dropdown_overlay(&mut self, info: &SettingsPanelInfo, window_width: f32, _window_height: f32) {
        // ALL coordinates MUST match build_settings_panel exactly
        let ui_cw = self.cell_width.min(10.0);
        let padding_x = ui_cw * 1.5;
        let sidebar_width = padding_x + ui_cw * 3.0 + ui_cw * 10.0 + padding_x;
        let min_content_width = ui_cw * 30.0;
        let panel_width = ((sidebar_width + min_content_width).max(window_width * 0.35)).min(window_width * 0.55);
        let anim = info.animation;
        let panel_x = window_width - panel_width * anim;
        let panel_y = 0.0_f32;

        let content_x = panel_x + sidebar_width;
        let content_width = panel_width - sidebar_width;

        let row_height = (self.cell_height * 2.2).max(32.0);
        let padding_y = self.cell_height * 0.5;

        let header_y = panel_y + self.cell_height * 1.5;
        let sep_y = header_y + self.cell_height * 1.8;
        let items_start_y = sep_y + padding_y + self.cell_height * 0.5;

        if let Some(item) = info.items.get(info.selected_item) {
            let (options_text, option_count): (Vec<String>, usize) = match &item.kind {
                crate::settings::SettingKind::Dropdown { options, .. } => {
                    (options.clone(), options.len())
                }
                crate::settings::SettingKind::SteppedList { values, .. } => {
                    (values.iter().map(|v| v.to_string()).collect(), values.len())
                }
                _ => (vec![], 0),
            };

            if option_count > 0 {
                // Dynamic value-area: match build_settings_panel
                let max_label_len = info.items.iter()
                    .map(|item| item.label.len())
                    .max()
                    .unwrap_or(0);
                let padding_x = ui_cw * 1.5;
                let label_end = padding_x + max_label_len as f32 * ui_cw;
                let min_gap = ui_cw * 2.0;
                let max_value_area = content_width * 0.65;
                let value_area_offset = (label_end + min_gap).min(max_value_area);
                let dd_value_area_x = content_x + value_area_offset;
                let anchor_y = items_start_y + info.selected_item as f32 * row_height + row_height;
                let dd_x = dd_value_area_x;
                let dd_width = content_width - value_area_offset;
                let option_h = row_height * 0.85;
                let max_visible = 8usize;
                let visible_count = option_count.min(max_visible);
                let dd_height = visible_count as f32 * option_h + 8.0;

                // Shadow
                let shadow_color = [0.0, 0.0, 0.0, 0.3];
                self.rect_pipeline.push_rounded_rect(
                    dd_x + 2.0, anchor_y + 2.0,
                    dd_width, dd_height,
                    shadow_color, 6.0,
                );

                // Background
                // Use fully opaque background so content behind doesn't bleed through
                let dd_bg = [
                    self.ui.settings_bg[0],
                    self.ui.settings_bg[1],
                    self.ui.settings_bg[2],
                    1.0,
                ];
                self.rect_pipeline.push_rounded_rect(
                    dd_x, anchor_y,
                    dd_width, dd_height,
                    dd_bg, 6.0,
                );

                // Border
                let border_color = self.ui.settings_separator;
                self.rect_pipeline.push_rounded_rect(
                    dd_x, anchor_y,
                    dd_width, dd_height,
                    adjust_alpha(border_color, 0.5), 6.0,
                );
                // Inner background (slightly inset to simulate border)
                self.rect_pipeline.push_rounded_rect(
                    dd_x + 1.0, anchor_y + 1.0,
                    dd_width - 2.0, dd_height - 2.0,
                    dd_bg, 5.0,
                );

                // Render options
                for (oidx, opt_text) in options_text.iter().enumerate().take(visible_count) {
                    let oy = anchor_y + 4.0 + oidx as f32 * option_h;
                    let is_highlighted = oidx == info.dropdown_highlight;

                    if is_highlighted {
                        self.rect_pipeline.push_rounded_rect(
                            dd_x + 4.0, oy,
                            dd_width - 8.0, option_h,
                            self.ui.settings_selected, 4.0,
                        );
                        // Accent bar
                        self.rect_pipeline.push_rounded_rect(
                            dd_x + 2.0, oy,
                            3.0, option_h,
                            self.ui.settings_accent, 2.0,
                        );
                    }

                    let opt_text_y = oy + (option_h - self.cell_height) / 2.0;
                    let opt_color = if is_highlighted {
                        self.ui.settings_value
                    } else {
                        self.ui.settings_text
                    };

                    for (ci, c) in opt_text.chars().enumerate() {
                        let cx = dd_x + ui_cw * 1.5 + ci as f32 * ui_cw;
                        if cx + ui_cw > dd_x + dd_width - 8.0 {
                            self.text_pipeline.push_char('.', cx, opt_text_y, self.ui.settings_text_dim);
                            break;
                        }
                        self.text_pipeline.push_char(c, cx, opt_text_y, opt_color);
                    }
                }
            }
        }
    }
}
