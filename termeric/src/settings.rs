use std::time::Instant;

use crate::config::Config;
use crate::theme::Theme;


/// Common monospace font families
pub const MONOSPACE_FONTS: &[&str] = &[
    "monospace",
    "JetBrains Mono",
    "Fira Code",
    "Cascadia Code",
    "Source Code Pro",
    "Hack",
    "Inconsolata",
    "IBM Plex Mono",
    "Ubuntu Mono",
    "Roboto Mono",
    "Victor Mono",
    "Iosevka",
];

/// Sections in the settings panel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Font,
    Theme,
    Window,
    Cursor,
    Bell,
    Scrollback,
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
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Font => "Aa",
            Self::Theme => "◈",
            Self::Window => "□",
            Self::Cursor => "⌖",
            Self::Bell => "🔔",
            Self::Scrollback => "↕",
        }
    }

    const ALL: &[SettingsSection] = &[
        Self::Font,
        Self::Theme,
        Self::Window,
        Self::Scrollback,
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
        }
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

    /// Build the list of items for the current section from config
    pub fn items_for_section(&self, config: &Config) -> Vec<SettingItem> {
        match self.section {
            SettingsSection::Font => {
                let font_options: Vec<String> = MONOSPACE_FONTS.iter().map(|s| s.to_string()).collect();
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
            ],
            SettingsSection::Scrollback => {
                let steps: Vec<usize> = vec![
                    100, 500, 1_000, 2_000, 5_000,
                    10_000, 25_000, 50_000, 100_000,
                ];
                let selected = steps.iter().position(|&v| v >= config.scrollback.lines)
                    .unwrap_or(steps.len() - 1);
                vec![
                    SettingItem {
                        label: "Lines",
                        value: format!("{}", config.scrollback.lines),
                        kind: SettingKind::SteppedList { values: steps, selected },
                    },
                ]
            }
        }
    }

    /// Handle a key press. Returns a SettingsAction if a config change should be applied.
    pub fn handle_key(&mut self, key: SettingsKey, config: &Config) -> SettingsAction {
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
            SettingsKey::Right => self.adjust_value(config, 1),
            SettingsKey::Tab => {
                // Cycle to next section
                let idx = self.section.index();
                self.section = SettingsSection::ALL[(idx + 1) % SettingsSection::ALL.len()];
                self.selected_item = 0;
                SettingsAction::None
            }
            SettingsKey::Save => SettingsAction::SaveConfig,
            SettingsKey::Reset => SettingsAction::ResetConfig,
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
                    (SettingsSection::Window, "Padding") => SettingsAction::SetPadding(new_val as u32),
                    (SettingsSection::Window, "Opacity") => SettingsAction::SetOpacity(new_val),
                    (SettingsSection::Cursor, "Blink Rate") => SettingsAction::SetCursorBlinkRate(new_val as u32),
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
                    _ => SettingsAction::None,
                }
            }
            SettingKind::ReadOnly => SettingsAction::None,
        }
    }

    /// Handle a mouse click at the given window coordinates.
    /// Layout parameters must match the renderer's settings panel layout.
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

        let anim = self.animation_progress();
        let panel_width = (window_width * 0.45).min(520.0).max(360.0);
        let panel_x = window_width - panel_width * anim;
        let sidebar_width = (cell_width * 14.0).max(120.0).min(160.0);
        let content_x = panel_x + sidebar_width;
        let content_width = panel_width - sidebar_width;
        let row_height = (cell_height * 2.2).max(32.0);
        let padding_x = cell_width * 1.5;
        let padding_y = cell_height * 0.5;

        // Click outside panel = close
        if x < panel_x {
            self.close();
            return SettingsAction::None;
        }

        // ── Sidebar section click ──
        let title_y = cell_height * 1.5;
        let section_start_y = title_y + cell_height * 2.5;
        if x >= panel_x && x < content_x {
            for (idx, section) in SettingsSection::ALL.iter().enumerate() {
                let sy = section_start_y + idx as f32 * row_height;
                if y >= sy && y < sy + row_height {
                    self.section = *section;
                    self.selected_item = 0;
                    return SettingsAction::None;
                }
            }
            return SettingsAction::None;
        }

        // ── Content area click ──
        let header_y = cell_height * 1.5;
        let sep_y = header_y + cell_height * 1.8;
        let items_start_y = sep_y + padding_y + cell_height * 0.5;
        let items = self.items_for_section(config);

        for (idx, _item) in items.iter().enumerate() {
            let iy = items_start_y + idx as f32 * row_height;
            if y >= iy && y < iy + row_height {
                // Clicked on this item row
                self.selected_item = idx;

                // Check if clicked on left arrow area (◀)
                let value_area_x = content_x + content_width * 0.45;
                if x < value_area_x + cell_width * 1.5 && x >= value_area_x - cell_width {
                    return self.adjust_value(config, -1);
                }

                // Check if clicked on right arrow area (▶)
                let val_x = value_area_x + cell_width * 2.0;
                let val_end = val_x + _item.value.len() as f32 * cell_width;
                if x > val_end {
                    return self.adjust_value(config, 1);
                }

                return SettingsAction::None;
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
        }
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

        // Move down within Font section
        panel.handle_key(SettingsKey::Down, &config);
        assert_eq!(panel.selected_item, 1);

        // Move down again should go to Theme section
        panel.handle_key(SettingsKey::Down, &config);
        assert_eq!(panel.section, SettingsSection::Theme);
        assert_eq!(panel.selected_item, 0);

        // Move up should go back to Font, last item
        panel.handle_key(SettingsKey::Up, &config);
        assert_eq!(panel.section, SettingsSection::Font);
        assert_eq!(panel.selected_item, 1);

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

        // Cycle theme forward
        let action = panel.handle_key(SettingsKey::Right, &config);
        match action {
            SettingsAction::SetTheme(name) => assert_eq!(name, "solarized-dark"),
            _ => panic!("Expected SetTheme"),
        }
    }
}
