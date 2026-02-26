use std::time::Instant;

use crate::config::{Config, SplashMode};
use crate::config::theme::Theme;

/// Layout geometry for the settings panel.
/// This struct ensures consistent layout between rendering and hit testing.
#[derive(Debug, Clone, Copy)]
pub struct SettingsPanelLayout {
    /// Panel x position (left edge)
    pub panel_x: f32,
    /// Total panel width
    pub panel_width: f32,
    /// Sidebar width (section list)
    pub sidebar_width: f32,
    /// Content area x position
    pub content_x: f32,
    /// Content area width
    pub content_width: f32,
    /// Height of each row/item
    pub row_height: f32,
    /// Horizontal padding
    pub padding_x: f32,
    /// Vertical padding
    pub padding_y: f32,
    /// Y position of the title/header
    pub header_y: f32,
    /// Y position of the separator line below header
    pub separator_y: f32,
    /// Y position where items start
    pub items_start_y: f32,
    /// Y position where sidebar sections start
    pub sections_start_y: f32,
    /// X position where value controls start (for dropdowns, numeric values)
    pub value_area_x: f32,
    /// Cell width used for calculations
    pub cell_width: f32,
    /// Cell height used for calculations
    pub cell_height: f32,
}

impl SettingsPanelLayout {
    /// Calculate layout for the settings panel.
    ///
    /// This is the single source of truth for all layout calculations.
    /// Both the renderer and click handler must use this.
    pub fn calculate(
        window_width: f32,
        _window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
    ) -> Self {
        // UI character width — use actual cell width from font metrics
        let ui_cw = cell_width;

        // Padding derived from cell size
        let padding_x = ui_cw * 1.5;
        let padding_y = cell_height * 0.5;

        // Sidebar: padding + longest label("Scrollback"=10) + padding
        let sidebar_width = padding_x + ui_cw * 10.0 + padding_x;

        // Content needs room for labels + values
        let min_content_width = ui_cw * 30.0;

        // Panel width: at least sidebar + content, between 35% and 55% of window
        let panel_width = ((sidebar_width + min_content_width).max(window_width * 0.35)).min(window_width * 0.55);

        // Panel slides in from right edge
        let panel_x = window_width - panel_width * animation;

        // Content area is whatever remains after sidebar
        let content_x = panel_x + sidebar_width;
        let content_width = panel_width - sidebar_width;

        // Row height derived from cell height
        let row_height = (cell_height * 2.2).max(32.0);

        // Vertical positions
        let header_y = cell_height * 1.5;
        let separator_y = header_y + cell_height * 1.8;
        let items_start_y = separator_y + padding_y + cell_height * 0.5;
        let sections_start_y = header_y + cell_height * 2.5;

        // Value area X is calculated dynamically in handle_click based on actual label lengths
        // Set a placeholder here; the actual value is computed per-item
        let value_area_x = content_x + content_width * 0.45;

        Self {
            panel_x,
            panel_width,
            sidebar_width,
            content_x,
            content_width,
            row_height,
            padding_x,
            padding_y,
            header_y,
            separator_y,
            items_start_y,
            sections_start_y,
            value_area_x,
            cell_width,
            cell_height,
        }
    }

    /// Get the Y position for a specific item index
    pub fn item_y(&self, index: usize) -> f32 {
        self.items_start_y + index as f32 * self.row_height
    }

    /// Get the Y position for a specific sidebar section index
    pub fn section_y(&self, index: usize) -> f32 {
        self.sections_start_y + index as f32 * self.row_height
    }

    /// Check if a point is within the panel bounds
    pub fn is_in_panel(&self, x: f32, _y: f32) -> bool {
        x >= self.panel_x
    }

    /// Check if a point is in the sidebar
    pub fn is_in_sidebar(&self, x: f32, _y: f32) -> bool {
        x >= self.panel_x && x < self.content_x
    }

    /// Check if a point is in the content area
    pub fn is_in_content(&self, x: f32, _y: f32) -> bool {
        x >= self.content_x
    }

    /// Find which item index a y coordinate falls into, if any
    pub fn item_at_y(&self, y: f32, item_count: usize) -> Option<usize> {
        if y < self.items_start_y {
            return None;
        }
        let idx = ((y - self.items_start_y) / self.row_height) as usize;
        if idx < item_count {
            Some(idx)
        } else {
            None
        }
    }

    /// Find which section index a y coordinate falls into, if any
    pub fn section_at_y(&self, y: f32, section_count: usize) -> Option<usize> {
        if y < self.sections_start_y {
            return None;
        }
        let idx = ((y - self.sections_start_y) / self.row_height) as usize;
        if idx < section_count {
            Some(idx)
        } else {
            None
        }
    }
}

/// Default fallback font (always available)
const DEFAULT_FONT: &str = "monospace";

/// Sections in the settings panel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Font,
    Theme,
    Window,
    Cursor,
    Bell,
    Scrollback,
    Startup,
    Shell,
    Sessions,
}

impl SettingsSection {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Font => "Font",
            Self::Theme => "Theme",
            Self::Window => "Window",
            Self::Cursor => "Cursor",
            Self::Bell => "Bell",
            Self::Scrollback => "Scrollback",
            Self::Startup => "Startup",
            Self::Shell => "Shell",
            Self::Sessions => "Sessions",
        }
    }

    pub const ALL: &[SettingsSection] = &[
        Self::Font,
        Self::Theme,
        Self::Window,
        Self::Cursor,
        Self::Bell,
        Self::Scrollback,
        Self::Startup,
        Self::Shell,
        Self::Sessions,
    ];

    pub fn index(&self) -> usize {
        Self::ALL.iter().position(|s| s == self).unwrap_or(0)
    }
}

/// A single setting item for display
#[derive(Debug, Clone)]
pub struct SettingItem {
    pub label: &'static str,
    pub value: String,
    pub kind: SettingKind,
}

/// The type of a setting value, determines how ←→ adjusts it
#[derive(Debug, Clone)]
pub enum SettingKind {
    /// Numeric value with min, max, step
    Numeric { min: f32, max: f32, step: f32 },
    /// Dropdown with a list of options and current index
    Dropdown { options: Vec<String>, selected: usize },
    /// Predefined list of numeric values (for logarithmic-style stepping)
    SteppedList { values: Vec<usize>, selected: usize },
    /// Read-only display value
    ReadOnly,
}

/// Result of a user action in the settings panel
#[derive(Debug, Clone)]
pub enum SettingsAction {
    /// Change font size to this value
    SetFontSize(f32),
    /// Change font family to this value
    SetFontFamily(String),
    /// Change font line height
    SetLineHeight(f32),
    /// Change theme to this name
    SetTheme(String),
    /// Change window padding
    SetPadding(u32),
    /// Change window opacity
    SetOpacity(f32),
    /// Change window decorations
    SetDecorations(bool),
    /// Change scrollback lines
    SetScrollback(usize),
    /// Change scroll speed
    SetScrollSpeed(f64),
    /// Change cursor style
    SetCursorStyle(String),
    /// Change cursor blink
    SetCursorBlink(bool),
    /// Change cursor blink rate
    SetCursorBlinkRate(u32),
    /// Change visual bell
    SetVisualBell(bool),
    /// Change audio bell
    SetAudioBell(bool),
    /// Change bell flash duration
    SetBellDuration(u64),
    /// Change splash screen mode
    SetSplashMode(SplashMode),
    /// Toggle session auto-save
    SetSessionAutoSave(bool),
    /// Toggle session auto-restore
    SetSessionAutoRestore(bool),
    /// Toggle session save scrollback
    SetSessionSaveScrollback(bool),
    /// Save current config
    SaveConfig,
    /// Reset to default config
    ResetConfig,
    /// No action
    None,
}

/// Animation duration in seconds
const ANIMATION_DURATION: f32 = 0.2; // 200ms

/// The settings panel state
pub struct SettingsPanel {
    /// Whether the panel is currently visible
    open: bool,
    /// Currently active section
    section: SettingsSection,
    /// Currently selected item index within the section
    selected_item: usize,
    /// Scroll offset for sections that don't fit
    scroll_offset: usize,
    /// When the current animation started
    animation_start: Instant,
    /// Whether we are animating open (true) or closed (false)
    animating_open: bool,
    /// The animation value at the time the direction changed (for seamless reversal)
    animation_base: f32,
    /// Whether a dropdown overlay is currently open
    dropdown_open: bool,
    /// Which option is highlighted in the open dropdown
    dropdown_highlight: usize,
    /// Available monospace fonts (discovered from system)
    available_fonts: Vec<String>,
    /// Mouse-hovered content item index (None if not hovering any item)
    hovered_item: Option<usize>,
    /// Mouse-hovered sidebar section index (None if not hovering any section)
    hovered_section: Option<usize>,
    /// Mouse-hovered dropdown option index (None if not hovering any dropdown option)
    dropdown_hovered: Option<usize>,
}

impl SettingsPanel {
    pub fn new() -> Self {
        Self {
            open: false,
            section: SettingsSection::Font,
            selected_item: 0,
            scroll_offset: 0,
            animation_start: Instant::now(),
            animating_open: false,
            animation_base: 0.0,
            dropdown_open: false,
            dropdown_highlight: 0,
            available_fonts: vec![DEFAULT_FONT.to_string()],
            hovered_item: None,
            hovered_section: None,
            dropdown_hovered: None,
        }
    }

    /// Set the list of available monospace fonts (discovered from system)
    pub fn set_available_fonts(&mut self, fonts: Vec<String>) {
        self.available_fonts = if fonts.is_empty() {
            vec![DEFAULT_FONT.to_string()]
        } else {
            fonts
        };
    }

    /// Toggle the panel open/closed
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Open the panel
    pub fn open(&mut self) {
        self.animation_base = self.raw_animation();
        self.open = true;
        self.animating_open = true;
        self.animation_start = Instant::now();
        self.section = SettingsSection::Font;
        self.selected_item = 0;
        self.scroll_offset = 0;
    }

    /// Open the panel directly to a specific section
    pub fn open_to_section(&mut self, section: SettingsSection) {
        self.animation_base = self.raw_animation();
        self.open = true;
        self.animating_open = true;
        self.animation_start = Instant::now();
        self.section = section;
        self.selected_item = 0;
        self.scroll_offset = 0;
    }

    /// Close the panel
    pub fn close(&mut self) {
        self.animation_base = self.raw_animation();
        self.animating_open = false;
        self.animation_start = Instant::now();
    }

    /// Raw linear animation value (0.0 = closed, 1.0 = open)
    fn raw_animation(&self) -> f32 {
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        let t = (elapsed / ANIMATION_DURATION).min(1.0);

        if self.animating_open {
            // Animate from base toward 1.0
            self.animation_base + (1.0 - self.animation_base) * t
        } else {
            // Animate from base toward 0.0
            self.animation_base * (1.0 - t)
        }
    }

    /// Whether the panel is open or animating
    pub fn is_visible(&self) -> bool {
        self.open || self.raw_animation() > 0.001
    }

    /// Whether the panel is fully open and accepting input
    pub fn is_active(&self) -> bool {
        self.open && self.raw_animation() > 0.9
    }

    /// Check if animation is still in progress and update open state. Returns true if redraw needed.
    pub fn update_animation(&mut self) -> bool {
        let anim = self.raw_animation();

        // Close is complete
        if !self.animating_open && anim < 0.001 {
            self.open = false;
            return false;
        }

        // Check if still animating
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        elapsed < ANIMATION_DURATION
    }

    /// Get current animation progress with ease-out (0.0 = closed, 1.0 = fully open)
    pub fn animation_progress(&self) -> f32 {
        let t = self.raw_animation();
        // Ease-out for smooth deceleration
        1.0 - (1.0 - t) * (1.0 - t)
    }

    /// Get the current section
    pub fn section(&self) -> SettingsSection {
        self.section
    }

    /// Get the currently selected item index
    pub fn selected_item(&self) -> usize {
        self.selected_item
    }

    /// Whether the dropdown overlay is currently open
    pub fn is_dropdown_open(&self) -> bool {
        self.dropdown_open
    }

    /// Alias for selected_item used by hover detection
    pub fn selected_item_index(&self) -> usize {
        self.selected_item
    }

    /// Build the list of items for the current section from config
    pub fn items_for_section(&self, config: &Config) -> Vec<SettingItem> {
        match self.section {
            SettingsSection::Font => {
                let font_options = self.available_fonts.clone();
                let font_selected = font_options.iter().position(|n| n == &config.font.family).unwrap_or(0);
                vec![
                    SettingItem {
                        label: "Size",
                        value: format!("{:.1}", config.font.size),
                        kind: SettingKind::Numeric { min: 8.0, max: 72.0, step: 1.0 },
                    },
                    SettingItem {
                        label: "Family",
                        value: config.font.family.clone(),
                        kind: SettingKind::Dropdown { options: font_options, selected: font_selected },
                    },
                    SettingItem {
                        label: "Line Height",
                        value: format!("{:.2}", config.font.line_height),
                        kind: SettingKind::Numeric { min: 0.8, max: 2.0, step: 0.05 },
                    },
                ]
            },
            SettingsSection::Theme => {
                let options: Vec<String> = Theme::list_available().iter().map(|s| s.to_string()).collect();
                let selected = options.iter().position(|n| n == &config.theme.name).unwrap_or(0);
                vec![
                    SettingItem {
                        label: "Theme",
                        value: config.theme.name.clone(),
                        kind: SettingKind::Dropdown { options, selected },
                    },
                ]
            }
            SettingsSection::Window => vec![
                SettingItem {
                    label: "Padding",
                    value: format!("{}", config.window.padding),
                    kind: SettingKind::Numeric { min: 0.0, max: 64.0, step: 2.0 },
                },
                SettingItem {
                    label: "Opacity",
                    value: format!("{:.2}", config.window.opacity),
                    kind: SettingKind::Numeric { min: 0.1, max: 1.0, step: 0.05 },
                },
                SettingItem {
                    label: "Decorations",
                    value: if config.window.decorations { "On".into() } else { "Off".into() },
                    kind: SettingKind::Dropdown {
                        options: vec!["On".into(), "Off".into()],
                        selected: if config.window.decorations { 0 } else { 1 },
                    },
                },
            ],
            SettingsSection::Cursor => {
                use crate::config::CursorStyle;
                let style_options: Vec<String> = CursorStyle::ALL.iter().map(|s| s.label().to_string()).collect();
                let style_selected = CursorStyle::ALL.iter().position(|s| *s == config.cursor.style).unwrap_or(0);
                vec![
                    SettingItem {
                        label: "Style",
                        value: config.cursor.style.label().into(),
                        kind: SettingKind::Dropdown { options: style_options, selected: style_selected },
                    },
                    SettingItem {
                        label: "Blink",
                        value: if config.cursor.blink { "On".into() } else { "Off".into() },
                        kind: SettingKind::Dropdown {
                            options: vec!["On".into(), "Off".into()],
                            selected: if config.cursor.blink { 0 } else { 1 },
                        },
                    },
                    SettingItem {
                        label: "Blink Rate",
                        value: format!("{}ms", config.cursor.blink_rate),
                        kind: SettingKind::Numeric { min: 100.0, max: 2000.0, step: 50.0 },
                    },
                ]
            }
            SettingsSection::Bell => vec![
                SettingItem {
                    label: "Visual",
                    value: if config.bell.visual { "On".into() } else { "Off".into() },
                    kind: SettingKind::Dropdown {
                        options: vec!["On".into(), "Off".into()],
                        selected: if config.bell.visual { 0 } else { 1 },
                    },
                },
                SettingItem {
                    label: "Audio",
                    value: if config.bell.audio { "On".into() } else { "Off".into() },
                    kind: SettingKind::Dropdown {
                        options: vec!["On".into(), "Off".into()],
                        selected: if config.bell.audio { 0 } else { 1 },
                    },
                },
                SettingItem {
                    label: "Flash Duration",
                    value: format!("{}ms", config.bell.visual_duration_ms),
                    kind: SettingKind::Numeric { min: 50.0, max: 1000.0, step: 50.0 },
                },
            ],
            SettingsSection::Scrollback => {
                let steps: Vec<usize> = vec![
                    100, 500, 1_000, 2_000, 5_000,
                    10_000, 25_000, 50_000, 100_000,
                ];
                let selected = steps.iter().position(|&v| v >= config.scrollback.lines)
                    .unwrap_or(steps.len() - 1);

                let speed_steps: Vec<usize> = vec![1, 2, 3, 4, 6, 8, 10, 15, 20];
                let speed_selected = speed_steps.iter().position(|&v| v >= config.scrollback.scroll_speed as usize)
                    .unwrap_or(speed_steps.len() - 1);
                vec![
                    SettingItem {
                        label: "Lines",
                        value: format!("{}", config.scrollback.lines),
                        kind: SettingKind::SteppedList { values: steps, selected },
                    },
                    SettingItem {
                        label: "Scroll Speed",
                        value: format!("{}", config.scrollback.scroll_speed as usize),
                        kind: SettingKind::SteppedList { values: speed_steps, selected: speed_selected },
                    },
                ]
            }
            SettingsSection::Startup => {
                let splash_options: Vec<String> = SplashMode::ALL.iter().map(|m| m.label().to_string()).collect();
                let splash_selected = SplashMode::ALL.iter().position(|m| *m == config.splash.mode).unwrap_or(0);
                vec![
                    SettingItem {
                        label: "Splash",
                        value: config.splash.mode.label().to_string(),
                        kind: SettingKind::Dropdown { options: splash_options, selected: splash_selected },
                    },
                ]
            }
            SettingsSection::Shell => {
                let program = config.shell.program.clone().unwrap_or_else(|| "(default)".into());
                let args_str = if config.shell.args.is_empty() {
                    "(none)".into()
                } else {
                    config.shell.args.join(" ")
                };
                vec![
                    SettingItem {
                        label: "Program",
                        value: program,
                        kind: SettingKind::ReadOnly,
                    },
                    SettingItem {
                        label: "Args",
                        value: args_str,
                        kind: SettingKind::ReadOnly,
                    },
                ]
            }
            SettingsSection::Sessions => {
                let on_off = vec!["On".into(), "Off".into()];
                vec![
                    SettingItem {
                        label: "Auto Save",
                        value: if config.session.auto_save { "On".into() } else { "Off".into() },
                        kind: SettingKind::Dropdown {
                            options: on_off.clone(),
                            selected: if config.session.auto_save { 0 } else { 1 },
                        },
                    },
                    SettingItem {
                        label: "Auto Restore",
                        value: if config.session.auto_restore { "On".into() } else { "Off".into() },
                        kind: SettingKind::Dropdown {
                            options: on_off.clone(),
                            selected: if config.session.auto_restore { 0 } else { 1 },
                        },
                    },
                    SettingItem {
                        label: "Save Scrollback",
                        value: if config.session.save_scrollback { "On".into() } else { "Off".into() },
                        kind: SettingKind::Dropdown {
                            options: on_off,
                            selected: if config.session.save_scrollback { 0 } else { 1 },
                        },
                    },
                ]
            }
        }
    }

    /// Handle a key press. Returns a SettingsAction if a config change should be applied.
    pub fn handle_key(&mut self, key: SettingsKey, config: &Config) -> SettingsAction {
        // If a dropdown is open, handle keys within it
        if self.dropdown_open {
            return self.handle_dropdown_key(key, config);
        }

        match key {
            SettingsKey::Escape => {
                self.close();
                SettingsAction::None
            }
            SettingsKey::Up => {
                if self.selected_item > 0 {
                    self.selected_item -= 1;
                } else {
                    // Move to previous section
                    let idx = self.section.index();
                    if idx > 0 {
                        self.section = SettingsSection::ALL[idx - 1];
                        let items = self.items_for_section(config);
                        self.selected_item = items.len().saturating_sub(1);
                    }
                }
                SettingsAction::None
            }
            SettingsKey::Down => {
                let items = self.items_for_section(config);
                if self.selected_item + 1 < items.len() {
                    self.selected_item += 1;
                } else {
                    // Move to next section
                    let idx = self.section.index();
                    if idx + 1 < SettingsSection::ALL.len() {
                        self.section = SettingsSection::ALL[idx + 1];
                        self.selected_item = 0;
                    }
                }
                SettingsAction::None
            }
            SettingsKey::Left => self.adjust_value(config, -1),
            SettingsKey::Right | SettingsKey::Enter => {
                // For dropdown items, open the dropdown overlay
                let items = self.items_for_section(config);
                if let Some(item) = items.get(self.selected_item) {
                    match &item.kind {
                        SettingKind::Dropdown { selected, .. } | SettingKind::SteppedList { selected, .. } => {
                            self.dropdown_open = true;
                            self.dropdown_highlight = *selected;
                            return SettingsAction::None;
                        }
                        _ => {}
                    }
                }
                self.adjust_value(config, 1)
            }
            SettingsKey::Tab => {
                // Cycle to next section
                self.dropdown_open = false;
                let idx = self.section.index();
                self.section = SettingsSection::ALL[(idx + 1) % SettingsSection::ALL.len()];
                self.selected_item = 0;
                SettingsAction::None
            }
            SettingsKey::Save => SettingsAction::SaveConfig,
            SettingsKey::Reset => SettingsAction::ResetConfig,
        }
    }

    /// Handle keys when a dropdown overlay is open
    fn handle_dropdown_key(&mut self, key: SettingsKey, config: &Config) -> SettingsAction {
        let items = self.items_for_section(config);
        let Some(item) = items.get(self.selected_item) else {
            self.dropdown_open = false;
            return SettingsAction::None;
        };

        let option_count = match &item.kind {
            SettingKind::Dropdown { options, .. } => options.len(),
            SettingKind::SteppedList { values, .. } => values.len(),
            _ => { self.dropdown_open = false; return SettingsAction::None; }
        };

        match key {
            SettingsKey::Escape | SettingsKey::Left => {
                self.dropdown_open = false;
                SettingsAction::None
            }
            SettingsKey::Up => {
                if self.dropdown_highlight > 0 {
                    self.dropdown_highlight -= 1;
                }
                SettingsAction::None
            }
            SettingsKey::Down => {
                if self.dropdown_highlight + 1 < option_count {
                    self.dropdown_highlight += 1;
                }
                SettingsAction::None
            }
            SettingsKey::Enter | SettingsKey::Right => {
                self.dropdown_open = false;
                self.select_dropdown_option(config, self.dropdown_highlight)
            }
            _ => SettingsAction::None,
        }
    }

    /// Select a specific option index from the current dropdown
    fn select_dropdown_option(&self, config: &Config, option_idx: usize) -> SettingsAction {
        let items = self.items_for_section(config);
        let Some(item) = items.get(self.selected_item) else {
            return SettingsAction::None;
        };

        match &item.kind {
            SettingKind::Dropdown { options, .. } => {
                let Some(new_val) = options.get(option_idx) else {
                    return SettingsAction::None;
                };
                let new_val = new_val.clone();
                match (self.section, item.label) {
                    (SettingsSection::Theme, _) => SettingsAction::SetTheme(new_val),
                    (SettingsSection::Font, "Family") => SettingsAction::SetFontFamily(new_val),
                    (SettingsSection::Window, "Decorations") => SettingsAction::SetDecorations(new_val == "On"),
                    (SettingsSection::Cursor, "Style") => SettingsAction::SetCursorStyle(new_val),
                    (SettingsSection::Cursor, "Blink") => SettingsAction::SetCursorBlink(new_val == "On"),
                    (SettingsSection::Bell, "Visual") => SettingsAction::SetVisualBell(new_val == "On"),
                    (SettingsSection::Bell, "Audio") => SettingsAction::SetAudioBell(new_val == "On"),
                    (SettingsSection::Startup, "Splash") => {
                        // Convert index to SplashMode
                        let mode = SplashMode::ALL.get(option_idx).copied().unwrap_or(SplashMode::FirstBoot);
                        SettingsAction::SetSplashMode(mode)
                    }
                    (SettingsSection::Sessions, "Auto Save") => SettingsAction::SetSessionAutoSave(new_val == "On"),
                    (SettingsSection::Sessions, "Auto Restore") => SettingsAction::SetSessionAutoRestore(new_val == "On"),
                    (SettingsSection::Sessions, "Save Scrollback") => SettingsAction::SetSessionSaveScrollback(new_val == "On"),
                    _ => SettingsAction::None,
                }
            }
            SettingKind::SteppedList { values, .. } => {
                let Some(&new_val) = values.get(option_idx) else {
                    return SettingsAction::None;
                };
                match (self.section, item.label) {
                    (SettingsSection::Scrollback, "Lines") => SettingsAction::SetScrollback(new_val),
                    (SettingsSection::Scrollback, "Scroll Speed") => SettingsAction::SetScrollSpeed(new_val as f64),
                    _ => SettingsAction::None,
                }
            }
            _ => SettingsAction::None,
        }
    }

    /// Adjust the currently selected value
    fn adjust_value(&self, config: &Config, direction: i32) -> SettingsAction {
        let items = self.items_for_section(config);
        let Some(item) = items.get(self.selected_item) else {
            return SettingsAction::None;
        };

        match &item.kind {
            SettingKind::Numeric { min, max, step } => {
                let clean_val = item.value.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
                let current: f32 = clean_val.parse().unwrap_or(0.0);
                let new_val = (current + step * direction as f32).clamp(*min, *max);

                match (self.section, item.label) {
                    (SettingsSection::Font, "Size") => SettingsAction::SetFontSize(new_val),
                    (SettingsSection::Font, "Line Height") => SettingsAction::SetLineHeight(new_val),
                    (SettingsSection::Window, "Padding") => SettingsAction::SetPadding(new_val as u32),
                    (SettingsSection::Window, "Opacity") => SettingsAction::SetOpacity(new_val),
                    (SettingsSection::Cursor, "Blink Rate") => SettingsAction::SetCursorBlinkRate(new_val as u32),
                    (SettingsSection::Bell, "Flash Duration") => SettingsAction::SetBellDuration(new_val as u64),
                    _ => SettingsAction::None,
                }
            }
            SettingKind::Dropdown { options, selected } => {
                if options.is_empty() {
                    return SettingsAction::None;
                }
                let new_idx = if direction > 0 {
                    (selected + 1) % options.len()
                } else {
                    selected.checked_sub(1).unwrap_or(options.len() - 1)
                };
                let new_val = options[new_idx].clone();

                match (self.section, item.label) {
                    (SettingsSection::Theme, _) => SettingsAction::SetTheme(new_val),
                    (SettingsSection::Font, "Family") => SettingsAction::SetFontFamily(new_val),
                    (SettingsSection::Window, "Decorations") => SettingsAction::SetDecorations(new_val == "On"),
                    (SettingsSection::Cursor, "Style") => SettingsAction::SetCursorStyle(new_val),
                    (SettingsSection::Cursor, "Blink") => SettingsAction::SetCursorBlink(new_val == "On"),
                    (SettingsSection::Bell, "Visual") => SettingsAction::SetVisualBell(new_val == "On"),
                    (SettingsSection::Bell, "Audio") => SettingsAction::SetAudioBell(new_val == "On"),
                    (SettingsSection::Startup, "Splash") => {
                        let mode = SplashMode::ALL.get(new_idx).copied().unwrap_or(SplashMode::FirstBoot);
                        SettingsAction::SetSplashMode(mode)
                    }
                    (SettingsSection::Sessions, "Auto Save") => SettingsAction::SetSessionAutoSave(new_val == "On"),
                    (SettingsSection::Sessions, "Auto Restore") => SettingsAction::SetSessionAutoRestore(new_val == "On"),
                    (SettingsSection::Sessions, "Save Scrollback") => SettingsAction::SetSessionSaveScrollback(new_val == "On"),
                    _ => SettingsAction::None,
                }
            }
            SettingKind::SteppedList { values, selected } => {
                if values.is_empty() {
                    return SettingsAction::None;
                }
                let new_idx = if direction > 0 {
                    (*selected + 1).min(values.len() - 1)
                } else {
                    selected.saturating_sub(1)
                };
                let new_val = values[new_idx];

                match (self.section, item.label) {
                    (SettingsSection::Scrollback, "Lines") => SettingsAction::SetScrollback(new_val),
                    (SettingsSection::Scrollback, "Scroll Speed") => SettingsAction::SetScrollSpeed(new_val as f64),
                    _ => SettingsAction::None,
                }
            }
            SettingKind::ReadOnly => SettingsAction::None,
        }
    }

    /// Handle a mouse click at the given window coordinates.
    /// Uses SettingsPanelLayout to ensure consistent hit testing with rendering.
    pub fn handle_click(
        &mut self,
        x: f32,
        y: f32,
        config: &Config,
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> SettingsAction {
        if !self.is_active() {
            return SettingsAction::None;
        }

        // Use the centralized layout calculation
        let layout = SettingsPanelLayout::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            self.animation_progress(),
        );

        // Click outside panel = close
        if !layout.is_in_panel(x, y) {
            self.close();
            return SettingsAction::None;
        }

        // ── Sidebar section click ──
        if layout.is_in_sidebar(x, y) {
            self.dropdown_open = false;
            if let Some(idx) = layout.section_at_y(y, SettingsSection::ALL.len()) {
                if let Some(section) = SettingsSection::ALL.get(idx) {
                    self.section = *section;
                    self.selected_item = 0;
                }
            }
            return SettingsAction::None;
        }

        // ── Dropdown overlay click ──
        if self.dropdown_open {
            let items = self.items_for_section(config);
            if let Some(item) = items.get(self.selected_item) {
                let (options_text, option_count) = match &item.kind {
                    SettingKind::Dropdown { options, .. } => (options.clone(), options.len()),
                    SettingKind::SteppedList { values, .. } => {
                        (values.iter().map(|v| v.to_string()).collect::<Vec<_>>(), values.len())
                    }
                    _ => (vec![], 0),
                };

                if option_count > 0 {
                    // Calculate value_area_x the same way as the renderer:
                    // Based on actual label lengths, not a fixed percentage
                    let max_label_len = items.iter().map(|i| i.label.len()).max().unwrap_or(0);
                    let label_end = layout.padding_x + max_label_len as f32 * layout.cell_width;
                    let min_gap = layout.cell_width * 2.0;
                    let max_value_area = layout.content_width * 0.65;
                    let value_area_offset = (label_end + min_gap).min(max_value_area);
                    let actual_value_area_x = layout.content_x + value_area_offset;

                    // Match the exact dropdown position calculation from renderer
                    let max_option_len = options_text.iter().map(|s| s.len()).max().unwrap_or(10);
                    let min_dd_width = (max_option_len as f32 + 4.0) * layout.cell_width;
                    let remaining_space = layout.content_width - value_area_offset;
                    let dd_width = min_dd_width.max(remaining_space).min(layout.content_width - layout.cell_width * 2.0);

                    // Shift dropdown left if it would exceed panel bounds
                    let dd_x = if actual_value_area_x + dd_width > layout.panel_x + layout.panel_width - layout.cell_width {
                        (layout.panel_x + layout.panel_width - dd_width - layout.cell_width).max(layout.content_x + layout.cell_width)
                    } else {
                        actual_value_area_x
                    };

                    // Dropdown appears below the selected item
                    let anchor_y = layout.item_y(self.selected_item) + layout.row_height;
                    let option_h = layout.row_height * 0.85;
                    let max_visible = 8usize;
                    let visible_count = option_count.min(max_visible);
                    let dd_height = visible_count as f32 * option_h + 8.0;

                    // Click inside dropdown?
                    if x >= dd_x && x < dd_x + dd_width && y >= anchor_y && y < anchor_y + dd_height {
                        let clicked_idx = ((y - anchor_y - 4.0) / option_h) as usize;
                        if clicked_idx < option_count {
                            self.dropdown_open = false;
                            return self.select_dropdown_option(config, clicked_idx);
                        }
                    }
                }
            }
            // Click outside dropdown closes it
            self.dropdown_open = false;
            return SettingsAction::None;
        }

        // ── Content area click ──
        let items = self.items_for_section(config);

        // Calculate value_area_x the same way as the renderer
        let max_label_len = items.iter().map(|i| i.label.len()).max().unwrap_or(0);
        let label_end = layout.padding_x + max_label_len as f32 * layout.cell_width;
        let min_gap = layout.cell_width * 2.0;
        let max_value_area = layout.content_width * 0.65;
        let value_area_offset = (label_end + min_gap).min(max_value_area);
        let actual_value_area_x = layout.content_x + value_area_offset;

        if let Some(idx) = layout.item_at_y(y, items.len()) {
            self.selected_item = idx;

            if let Some(item) = items.get(idx) {
                // For dropdown/stepped items, clicking the value area opens the dropdown
                match &item.kind {
                    SettingKind::Dropdown { selected, .. } => {
                        if x >= actual_value_area_x {
                            self.dropdown_open = true;
                            self.dropdown_highlight = *selected;
                            return SettingsAction::None;
                        }
                    }
                    SettingKind::SteppedList { selected, .. } => {
                        if x >= actual_value_area_x {
                            self.dropdown_open = true;
                            self.dropdown_highlight = *selected;
                            return SettingsAction::None;
                        }
                    }
                    SettingKind::Numeric { .. } => {
                        // ◀ button area (left of value)
                        let left_arrow_x = actual_value_area_x;
                        if x >= left_arrow_x && x < left_arrow_x + layout.cell_width * 2.0 {
                            return self.adjust_value(config, -1);
                        }
                        // ▶ button area (right of value)
                        let val_x = actual_value_area_x + layout.cell_width * 2.0;
                        let val_end = val_x + item.value.len() as f32 * layout.cell_width;
                        let right_arrow_x = val_end + layout.cell_width;
                        if x >= right_arrow_x {
                            return self.adjust_value(config, 1);
                        }
                    }
                    _ => {}
                }
            }
        }

        SettingsAction::None
    }
}

/// Simplified key events for the settings panel
#[derive(Debug, Clone, Copy)]
pub enum SettingsKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Tab,
    Save,
    Reset,
}

/// Display info passed from the settings panel to the renderer
pub struct SettingsPanelInfo {
    /// Animation progress (0.0 = hidden, 1.0 = fully visible)
    pub animation: f32,
    /// Current section
    pub section: SettingsSection,
    /// All sections for the sidebar
    pub sections: Vec<(SettingsSection, bool)>, // (section, is_active)
    /// Items in the current section
    pub items: Vec<SettingItem>,
    /// Index of the selected item
    pub selected_item: usize,
    /// Whether a dropdown overlay is currently open
    pub dropdown_open: bool,
    /// Which option is highlighted in the dropdown
    pub dropdown_highlight: usize,
    /// Mouse-hovered content item index
    pub hovered_item: Option<usize>,
    /// Mouse-hovered sidebar section index
    pub hovered_section: Option<usize>,
    /// Mouse-hovered dropdown option index
    pub dropdown_hovered: Option<usize>,
}

impl SettingsPanel {
    /// Build display info for the renderer
    pub fn build_info(&self, config: &Config) -> SettingsPanelInfo {
        let items = self.items_for_section(config);
        let sections = SettingsSection::ALL
            .iter()
            .map(|s| (*s, *s == self.section))
            .collect();

        SettingsPanelInfo {
            animation: self.animation_progress(),
            section: self.section,
            sections,
            items,
            selected_item: self.selected_item,
            dropdown_open: self.dropdown_open,
            dropdown_highlight: self.dropdown_highlight,
            hovered_item: self.hovered_item,
            hovered_section: self.hovered_section,
            dropdown_hovered: self.dropdown_hovered,
        }
    }

    /// Update hover state from mouse position
    pub fn set_hover(&mut self, item: Option<usize>, section: Option<usize>) {
        self.hovered_item = item;
        self.hovered_section = section;
    }

    /// Update dropdown hover state from mouse position
    pub fn set_dropdown_hover(&mut self, index: Option<usize>) {
        self.dropdown_hovered = index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toggle_open_close() {
        let mut panel = SettingsPanel::new();
        assert!(!panel.is_visible());
        assert!(!panel.is_active());

        panel.toggle();
        assert!(panel.open);
        assert!(panel.animating_open);

        // Simulate animation completing by backdating the start time
        panel.animation_start = Instant::now() - std::time::Duration::from_millis(300);
        panel.update_animation();
        assert!(panel.is_active());

        panel.toggle();
        assert!(!panel.animating_open);
        // Simulate close animation completing
        panel.animation_start = Instant::now() - std::time::Duration::from_millis(300);
        panel.update_animation();
        assert!(!panel.is_visible());
    }

    #[test]
    fn test_navigation() {
        let mut panel = SettingsPanel::new();
        panel.open();
        panel.animation_start = Instant::now() - std::time::Duration::from_millis(300);

        let config = Config::default();

        // Start at Font section, item 0
        assert_eq!(panel.section, SettingsSection::Font);
        assert_eq!(panel.selected_item, 0);

        // Move down within Font section (Size -> Family)
        panel.handle_key(SettingsKey::Down, &config);
        assert_eq!(panel.selected_item, 1);

        // Move down again within Font section (Family -> Line Height)
        panel.handle_key(SettingsKey::Down, &config);
        assert_eq!(panel.section, SettingsSection::Font);
        assert_eq!(panel.selected_item, 2);

        // Move down again should go to Theme section
        panel.handle_key(SettingsKey::Down, &config);
        assert_eq!(panel.section, SettingsSection::Theme);
        assert_eq!(panel.selected_item, 0);

        // Move up should go back to Font, last item (Line Height)
        panel.handle_key(SettingsKey::Up, &config);
        assert_eq!(panel.section, SettingsSection::Font);
        assert_eq!(panel.selected_item, 2);

        // Tab should cycle sections
        panel.handle_key(SettingsKey::Tab, &config);
        assert_eq!(panel.section, SettingsSection::Theme);
        assert_eq!(panel.selected_item, 0);
    }

    #[test]
    fn test_value_adjustment() {
        let mut panel = SettingsPanel::new();
        panel.open();
        panel.animation_start = Instant::now() - std::time::Duration::from_millis(300);

        let config = Config::default();

        // Font size is item 0 in Font section, default is 14.0
        let action = panel.handle_key(SettingsKey::Right, &config);
        match action {
            SettingsAction::SetFontSize(size) => assert_eq!(size, 15.0),
            _ => panic!("Expected SetFontSize"),
        }

        let action = panel.handle_key(SettingsKey::Left, &config);
        match action {
            SettingsAction::SetFontSize(size) => assert_eq!(size, 13.0),
            _ => panic!("Expected SetFontSize"),
        }
    }

    #[test]
    fn test_theme_dropdown() {
        let mut panel = SettingsPanel::new();
        panel.open();
        panel.animation_start = Instant::now() - std::time::Duration::from_millis(300);
        panel.section = SettingsSection::Theme;
        panel.selected_item = 0;

        let config = Config::default();
        let themes = Theme::list_available();
        let expected_next = themes.get(1).expect("at least 2 themes");

        // Right opens the dropdown (does NOT cycle)
        let action = panel.handle_key(SettingsKey::Right, &config);
        assert!(matches!(action, SettingsAction::None));
        assert!(panel.dropdown_open);

        // Down moves to next option
        let action = panel.handle_key(SettingsKey::Down, &config);
        assert!(matches!(action, SettingsAction::None));
        assert_eq!(panel.dropdown_highlight, 1);

        // Enter selects and closes dropdown
        let action = panel.handle_key(SettingsKey::Enter, &config);
        assert!(!panel.dropdown_open);
        match action {
            SettingsAction::SetTheme(name) => assert_eq!(name, *expected_next),
            _ => panic!("Expected SetTheme"),
        }
    }
}
