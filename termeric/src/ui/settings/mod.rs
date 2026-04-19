use crate::config::{Config, SplashMode};
use crate::config::theme::Theme;
use crate::input::keybindings::ShortcutRegistry;

/// Default fallback font (always available)
const DEFAULT_FONT: &str = "monospace";

/// Geometry for the centered settings modal — single source of truth shared
/// by the renderer, hit-testing, scroll, and hover code paths.
#[derive(Debug, Clone, Copy)]
pub struct SettingsModalLayout {
    pub panel_x: f32,
    pub panel_y: f32,
    pub panel_width: f32,
    pub panel_height: f32,
    pub padding: f32,
    pub gap: f32,
    pub separator_height: f32,
    pub row_height: f32,
    pub input_height: f32,
    pub scrollbar_width: f32,
    pub footer_area: f32,
    pub list_y: f32,
    pub list_height: f32,
    pub rows_that_fit: usize,
    pub visible_count: usize,
    pub value_x: f32,
    pub cell_width: f32,
}

impl SettingsModalLayout {
    /// Compute the modal geometry. `entry_count` is the length of
    /// `SettingsPanelInfo::entries`; it controls `visible_count` and the
    /// final `panel_height` so a short list doesn't render a giant empty
    /// panel.
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        entry_count: usize,
    ) -> Self {
        let panel_width = (cell_width * 60.0).max(500.0).min(window_width * 0.7);
        let panel_x = (window_width - panel_width) / 2.0;
        let padding = cell_width * 1.5;
        let row_height = cell_height * 2.0;
        let input_height = cell_height * 2.0;
        let footer_height = cell_height * 1.6;
        let gap = 6.0;
        let separator_height = 1.0;
        let scrollbar_width = 4.0;

        let header_area =
            padding + cell_height * 1.4 + gap + input_height + gap + separator_height + gap;
        let footer_area = footer_height + padding * 0.5;

        let max_panel_height = window_height * 0.75;
        let max_list_height = (max_panel_height - header_area - footer_area).max(row_height);
        let rows_that_fit = (max_list_height / row_height).floor().max(1.0) as usize;
        let visible_count = entry_count.min(rows_that_fit);
        let list_height = visible_count as f32 * row_height;
        let panel_height = header_area + list_height + footer_area;
        let panel_y = (window_height * 0.10) + (1.0 - animation) * -30.0;
        let list_y = panel_y + header_area;
        let value_x = panel_x + panel_width * 0.50;

        Self {
            panel_x,
            panel_y,
            panel_width,
            panel_height,
            padding,
            gap,
            separator_height,
            row_height,
            input_height,
            scrollbar_width,
            footer_area,
            list_y,
            list_height,
            rows_that_fit,
            visible_count,
            value_x,
            cell_width,
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.panel_x
            && x <= self.panel_x + self.panel_width
            && y >= self.panel_y
            && y <= self.panel_y + self.panel_height
    }

    /// Map a y-coordinate to a 0-based row within the visible window, if any.
    pub fn visible_row_at(&self, y: f32) -> Option<usize> {
        if y < self.list_y || y >= self.list_y + self.list_height {
            return None;
        }
        let row = ((y - self.list_y) / self.row_height) as usize;
        if row < self.visible_count {
            Some(row)
        } else {
            None
        }
    }

    /// Geometry of the dropdown overlay anchored below the selected entry.
    /// Returns None if the selected entry is scrolled out of view.
    pub fn dropdown_rect(
        &self,
        entry_abs_idx: usize,
        scroll_offset: usize,
        option_count: usize,
        max_option_len: usize,
    ) -> Option<DropdownRect> {
        if option_count == 0 || entry_abs_idx < scroll_offset {
            return None;
        }
        let visible_row = entry_abs_idx - scroll_offset;
        if visible_row >= self.visible_count {
            return None;
        }

        let anchor_y = self.list_y + visible_row as f32 * self.row_height + self.row_height;
        let option_h = self.row_height * 0.85;
        let max_visible_options = 8usize;
        let visible_options = option_count.min(max_visible_options);
        let dd_height = visible_options as f32 * option_h + 8.0;

        let min_dd_width = (max_option_len as f32 + 4.0) * self.cell_width;
        let remaining = self.panel_x + self.panel_width - self.padding - self.value_x;
        let dd_width = min_dd_width.max(remaining).min(self.panel_width * 0.4);
        let dd_x = self.value_x;

        Some(DropdownRect {
            x: dd_x,
            y: anchor_y,
            width: dd_width,
            height: dd_height,
            option_height: option_h,
            visible_options,
        })
    }
}

/// Dropdown overlay rectangle derived from `SettingsModalLayout`.
#[derive(Debug, Clone, Copy)]
pub struct DropdownRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub option_height: f32,
    pub visible_options: usize,
}

impl DropdownRect {
    /// Which option index does a y-coordinate fall into, if any.
    pub fn option_at(&self, y: f32) -> Option<usize> {
        let inner_y = y - self.y - 4.0;
        if inner_y < 0.0 {
            return None;
        }
        let idx = (inner_y / self.option_height) as usize;
        if idx < self.visible_options {
            Some(idx)
        } else {
            None
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Sections in the settings panel — used for categorizing items
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
    Plugins,
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
            Self::Plugins => "Plugins",
        }
    }

    pub fn all() -> &'static [SettingsSection] {
        static ALL: [SettingsSection; 10] = [
            SettingsSection::Font,
            SettingsSection::Theme,
            SettingsSection::Window,
            SettingsSection::Cursor,
            SettingsSection::Bell,
            SettingsSection::Scrollback,
            SettingsSection::Startup,
            SettingsSection::Shell,
            SettingsSection::Sessions,
            SettingsSection::Plugins,
        ];
        ALL.as_slice()
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

/// A flat entry for the settings modal list (item or category header)
#[derive(Debug, Clone)]
pub struct SettingEntry {
    /// Label text (setting name or category header)
    pub label: &'static str,
    /// Current value as a display string (empty for headers)
    pub value: String,
    /// Setting kind for interaction (ReadOnly for headers)
    pub kind: SettingKind,
    /// Whether this is a category header (not selectable)
    pub is_header: bool,
    /// Which section this belongs to
    pub section: SettingsSection,
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
    /// Enable or disable a plugin
    SetPluginEnabled { plugin_id: String, enabled: bool },
    /// Change a plugin's model
    SetPluginModel { plugin_id: String, model: String },
    /// Change a plugin's approval mode
    SetPluginApprovalMode { plugin_id: String, mode: String },
    /// Save current config
    SaveConfig,
    /// Reset to default config
    ResetConfig,
    /// No action
    None,
}

/// The settings panel state — now a centered modal overlay
pub struct SettingsPanel {
    /// Animation state (overlay = centered modal with scrim)
    animation: super::animation::OverlayAnimation,
    /// Search filter text
    filter: String,
    /// Cursor position in filter text (byte offset)
    filter_cursor: usize,
    /// Currently selected entry index (selectable entries only, not headers)
    selected: usize,
    /// Scroll offset in absolute entry units (headers count)
    scroll_offset: usize,
    /// Max visible rows the list can show (updated each frame from the layout)
    max_visible: usize,
    /// Whether a dropdown overlay is currently open
    dropdown_open: bool,
    /// Which option is highlighted in the open dropdown
    dropdown_highlight: usize,
    /// Available monospace fonts (discovered from system)
    available_fonts: Vec<String>,
    /// Mouse-hovered entry index (None if not hovering any item)
    hovered_item: Option<usize>,
    /// Mouse-hovered dropdown option index (None if not hovering any dropdown option)
    dropdown_hovered: Option<usize>,
    /// Current theme metadata (name, author, description)
    theme_meta: Option<crate::config::theme::ThemeMeta>,
    /// A pending `open_to_section` request to resolve on the next frame
    /// (resolution needs access to `Config`/`ShortcutRegistry`).
    pending_section_jump: Option<SettingsSection>,
}

impl SettingsPanel {
    pub fn new() -> Self {
        Self {
            animation: super::animation::OverlayAnimation::with_duration(
                super::animation::DEFAULT_PANEL_DURATION,
            ),
            filter: String::new(),
            filter_cursor: 0,
            selected: 0,
            scroll_offset: 0,
            max_visible: 12,
            dropdown_open: false,
            dropdown_highlight: 0,
            available_fonts: vec![DEFAULT_FONT.to_string()],
            hovered_item: None,
            dropdown_hovered: None,
            theme_meta: None,
            pending_section_jump: None,
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

    /// Update the stored theme metadata (call when the theme changes)
    pub fn set_theme_meta(&mut self, meta: crate::config::theme::ThemeMeta) {
        self.theme_meta = Some(meta);
    }

    /// Toggle the panel open/closed
    pub fn toggle(&mut self) {
        if !self.animation.is_active() {
            // Reset state when opening
            self.filter.clear();
            self.filter_cursor = 0;
            self.selected = 0;
            self.scroll_offset = 0;
            self.dropdown_open = false;
            self.animation.open();
        } else {
            self.animation.close();
        }
    }

    /// Open the panel and jump selection to the first item in `section`.
    /// Does not filter — the full list is still browsable.
    pub fn open_to_section(&mut self, section: SettingsSection) {
        self.filter.clear();
        self.filter_cursor = 0;
        self.scroll_offset = 0;
        self.dropdown_open = false;
        self.animation.open();
        // Find the first selectable entry in the target section.
        self.selected = 0;
        self.pending_section_jump = Some(section);
    }

    /// Update the max visible rows (called by the renderer each frame).
    pub fn set_max_visible(&mut self, rows: usize) {
        self.max_visible = rows.max(1);
    }

    /// Close the panel
    pub fn close(&mut self) {
        self.animation.close();
    }

    /// Whether the overlay is visible (active or still animating out)
    pub fn is_visible(&self) -> bool {
        self.animation.is_visible()
    }

    /// Whether the overlay is logically active (accepting input)
    pub fn is_active(&self) -> bool {
        self.animation.is_active()
    }

    /// Update animation state. Returns true if still animating (needs redraw).
    pub fn update_animation(&mut self) -> bool {
        self.animation.update()
    }

    /// Get current animation progress with ease-out
    pub fn animation_progress(&self) -> f32 {
        self.animation.eased()
    }

    /// Whether the dropdown overlay is currently open
    pub fn is_dropdown_open(&self) -> bool {
        self.dropdown_open
    }

    /// Build the flat list of all settings grouped by section from config
    fn all_items(&self, config: &Config, _registry: &ShortcutRegistry) -> Vec<(SettingsSection, Vec<SettingItem>)> {
        let mut result = Vec::new();

        // Font
        {
            let font_options = self.available_fonts.clone();
            let font_selected = font_options.iter().position(|n| n == &config.font.family).unwrap_or(0);
            result.push((SettingsSection::Font, vec![
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
            ]));
        }

        // Theme
        {
            let options: Vec<String> = Theme::list_available().iter().map(|s| s.to_string()).collect();
            let selected = options.iter().position(|n| n == &config.theme.name).unwrap_or(0);
            let mut items = vec![
                SettingItem {
                    label: "Theme",
                    value: config.theme.name.clone(),
                    kind: SettingKind::Dropdown { options, selected },
                },
            ];
            if let Some(meta) = &self.theme_meta {
                if !meta.name.is_empty() {
                    items.push(SettingItem { label: "Name", value: meta.name.clone(), kind: SettingKind::ReadOnly });
                }
                if !meta.author.is_empty() {
                    items.push(SettingItem { label: "Author", value: meta.author.clone(), kind: SettingKind::ReadOnly });
                }
                if !meta.description.is_empty() {
                    items.push(SettingItem { label: "Description", value: meta.description.clone(), kind: SettingKind::ReadOnly });
                }
            }
            result.push((SettingsSection::Theme, items));
        }

        // Window
        result.push((SettingsSection::Window, vec![
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
        ]));

        // Cursor
        {
            use crate::config::CursorStyle;
            let style_options: Vec<String> = CursorStyle::ALL.iter().map(|s| s.label().to_string()).collect();
            let style_selected = CursorStyle::ALL.iter().position(|s| *s == config.cursor.style).unwrap_or(0);
            result.push((SettingsSection::Cursor, vec![
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
            ]));
        }

        // Bell
        result.push((SettingsSection::Bell, vec![
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
        ]));

        // Scrollback
        {
            let steps: Vec<usize> = vec![100, 500, 1_000, 2_000, 5_000, 10_000, 25_000, 50_000, 100_000];
            let selected = steps.iter().position(|&v| v >= config.scrollback.lines).unwrap_or(steps.len() - 1);
            let speed_steps: Vec<usize> = vec![1, 2, 3, 4, 6, 8, 10, 15, 20];
            let speed_selected = speed_steps.iter().position(|&v| v >= config.scrollback.scroll_speed as usize).unwrap_or(speed_steps.len() - 1);
            result.push((SettingsSection::Scrollback, vec![
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
            ]));
        }

        // Startup
        {
            let splash_options: Vec<String> = SplashMode::ALL.iter().map(|m| m.label().to_string()).collect();
            let splash_selected = SplashMode::ALL.iter().position(|m| *m == config.splash.mode).unwrap_or(0);
            result.push((SettingsSection::Startup, vec![
                SettingItem {
                    label: "Splash",
                    value: config.splash.mode.label().to_string(),
                    kind: SettingKind::Dropdown { options: splash_options, selected: splash_selected },
                },
            ]));
        }

        // Shell
        {
            let program = config.shell.program.clone().unwrap_or_else(|| "(default)".into());
            let args_str = if config.shell.args.is_empty() { "(none)".into() } else { config.shell.args.join(" ") };
            result.push((SettingsSection::Shell, vec![
                SettingItem { label: "Program", value: program, kind: SettingKind::ReadOnly },
                SettingItem { label: "Args", value: args_str, kind: SettingKind::ReadOnly },
            ]));
        }

        // Sessions
        {
            let on_off = vec!["On".into(), "Off".into()];
            result.push((SettingsSection::Sessions, vec![
                SettingItem {
                    label: "Auto Save",
                    value: if config.session.auto_save { "On".into() } else { "Off".into() },
                    kind: SettingKind::Dropdown { options: on_off.clone(), selected: if config.session.auto_save { 0 } else { 1 } },
                },
                SettingItem {
                    label: "Auto Restore",
                    value: if config.session.auto_restore { "On".into() } else { "Off".into() },
                    kind: SettingKind::Dropdown { options: on_off.clone(), selected: if config.session.auto_restore { 0 } else { 1 } },
                },
                SettingItem {
                    label: "Save Scrollback",
                    value: if config.session.save_scrollback { "On".into() } else { "Off".into() },
                    kind: SettingKind::Dropdown { options: on_off, selected: if config.session.save_scrollback { 0 } else { 1 } },
                },
            ]));
        }

        // Plugins
        {
            let en_dis: Vec<String> = vec!["Enabled".into(), "Disabled".into()];
            let claude_model_opts: Vec<String> = vec!["(default)".into(), "sonnet".into(), "opus".into(), "haiku".into()];
            let claude_model_val = config.plugins.claude_code.model.clone().unwrap_or_else(|| "(default)".into());
            let claude_model_sel = claude_model_opts.iter().position(|m| m == &claude_model_val).unwrap_or(0);
            let codex_model_opts: Vec<String> = vec!["(default)".into(), "o3-mini".into(), "o4-mini".into(), "gpt-4.1".into()];
            let codex_model_val = config.plugins.codex.model.clone().unwrap_or_else(|| "(default)".into());
            let codex_model_sel = codex_model_opts.iter().position(|m| m == &codex_model_val).unwrap_or(0);
            let codex_approval_opts: Vec<String> = vec!["suggest".into(), "auto-edit".into(), "full-auto".into()];
            let codex_approval_sel = codex_approval_opts.iter().position(|m| m == &config.plugins.codex.approval_mode).unwrap_or(0);
            let claude_approval_opts: Vec<String> = vec!["(interactive)".into(), "auto-accept".into()];
            let claude_approval_val = config.plugins.claude_code.approval_mode.clone().unwrap_or_else(|| "(interactive)".into());
            let claude_approval_sel = claude_approval_opts.iter().position(|m| m == &claude_approval_val).unwrap_or(0);

            result.push((SettingsSection::Plugins, vec![
                SettingItem {
                    label: "Claude Code",
                    value: if config.plugins.claude_code.enabled { "Enabled".into() } else { "Disabled".into() },
                    kind: SettingKind::Dropdown { options: en_dis.clone(), selected: if config.plugins.claude_code.enabled { 0 } else { 1 } },
                },
                SettingItem { label: "Claude Binary", value: config.plugins.claude_code.binary.clone(), kind: SettingKind::ReadOnly },
                SettingItem {
                    label: "Claude Model",
                    value: claude_model_val,
                    kind: SettingKind::Dropdown { options: claude_model_opts, selected: claude_model_sel },
                },
                SettingItem {
                    label: "Claude Approval",
                    value: claude_approval_val,
                    kind: SettingKind::Dropdown { options: claude_approval_opts, selected: claude_approval_sel },
                },
                SettingItem {
                    label: "Codex",
                    value: if config.plugins.codex.enabled { "Enabled".into() } else { "Disabled".into() },
                    kind: SettingKind::Dropdown { options: en_dis, selected: if config.plugins.codex.enabled { 0 } else { 1 } },
                },
                SettingItem { label: "Codex Binary", value: config.plugins.codex.binary.clone(), kind: SettingKind::ReadOnly },
                SettingItem {
                    label: "Codex Model",
                    value: codex_model_val,
                    kind: SettingKind::Dropdown { options: codex_model_opts, selected: codex_model_sel },
                },
                SettingItem {
                    label: "Codex Approval",
                    value: config.plugins.codex.approval_mode.clone(),
                    kind: SettingKind::Dropdown { options: codex_approval_opts, selected: codex_approval_sel },
                },
            ]));
        }

        result
    }

    /// Build filtered flat entries from all settings, with category headers
    pub fn build_entries(&self, config: &Config, registry: &ShortcutRegistry) -> Vec<SettingEntry> {
        let all = self.all_items(config, registry);
        let filter_lower = self.filter.to_lowercase();
        let has_filter = !filter_lower.is_empty();

        let mut entries = Vec::new();
        for (section, items) in &all {
            // Collect matching items in this section
            let matching: Vec<(usize, &SettingItem)> = items.iter().enumerate().filter(|(_, item)| {
                if !has_filter {
                    return true;
                }
                // Match against label, value, or section name
                item.label.to_lowercase().contains(&filter_lower)
                    || item.value.to_lowercase().contains(&filter_lower)
                    || section.label().to_lowercase().contains(&filter_lower)
            }).collect();

            if matching.is_empty() {
                continue;
            }

            // Add category header
            entries.push(SettingEntry {
                label: section.label(),
                value: String::new(),
                kind: SettingKind::ReadOnly,
                is_header: true,
                section: *section,
            });

            // Add matching items
            for (_, item) in matching {
                entries.push(SettingEntry {
                    label: item.label,
                    value: item.value.clone(),
                    kind: item.kind.clone(),
                    is_header: false,
                    section: *section,
                });
            }
        }

        entries
    }

    /// Count selectable (non-header) entries
    fn count_selectable(entries: &[SettingEntry]) -> usize {
        entries.iter().filter(|e| !e.is_header).count()
    }

    /// Get the nth selectable entry (0-indexed)
    fn nth_selectable(entries: &[SettingEntry], n: usize) -> Option<&SettingEntry> {
        entries.iter().filter(|e| !e.is_header).nth(n)
    }

    /// Get the nth selectable entry's absolute index in the entries vec
    fn nth_selectable_abs_index(entries: &[SettingEntry], n: usize) -> Option<usize> {
        entries.iter().enumerate()
            .filter(|(_, e)| !e.is_header)
            .nth(n)
            .map(|(i, _)| i)
    }

    /// Handle a key press. Returns a SettingsAction if a config change should be applied.
    pub fn handle_key(&mut self, key: SettingsKey, config: &Config, registry: &ShortcutRegistry) -> SettingsAction {
        // If a dropdown is open, handle keys within it
        if self.dropdown_open {
            return self.handle_dropdown_key(key, config, registry);
        }

        let entries = self.build_entries(config, registry);
        let selectable_count = Self::count_selectable(&entries);

        let action = match key {
            SettingsKey::Escape => {
                if !self.filter.is_empty() {
                    // First Escape clears filter
                    self.filter.clear();
                    self.filter_cursor = 0;
                    self.selected = 0;
                    self.scroll_offset = 0;
                } else {
                    self.close();
                }
                SettingsAction::None
            }
            SettingsKey::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                SettingsAction::None
            }
            SettingsKey::Down => {
                if selectable_count > 0 && self.selected + 1 < selectable_count {
                    self.selected += 1;
                }
                SettingsAction::None
            }
            SettingsKey::Left => self.adjust_selected_value(&entries, -1),
            SettingsKey::Right | SettingsKey::Enter => {
                if let Some(entry) = Self::nth_selectable(&entries, self.selected) {
                    match &entry.kind {
                        SettingKind::Dropdown { selected, .. } | SettingKind::SteppedList { selected, .. } => {
                            self.dropdown_open = true;
                            self.dropdown_highlight = *selected;
                            SettingsAction::None
                        }
                        _ => self.adjust_selected_value(&entries, 1),
                    }
                } else {
                    SettingsAction::None
                }
            }
            SettingsKey::Tab => {
                if let Some(current) = Self::nth_selectable(&entries, self.selected) {
                    let sections = SettingsSection::all();
                    let current_idx = sections.iter().position(|s| *s == current.section).unwrap_or(0);
                    let next_section = sections[(current_idx + 1) % sections.len()];
                    if let Some(idx) = Self::first_selectable_in_section(&entries, next_section) {
                        self.selected = idx;
                    }
                }
                SettingsAction::None
            }
            SettingsKey::Save => SettingsAction::SaveConfig,
            SettingsKey::Reset => SettingsAction::ResetConfig,
            SettingsKey::Delete => SettingsAction::None,
            SettingsKey::FilterChar(ch) => {
                self.filter.insert(self.filter_cursor, ch);
                self.filter_cursor += ch.len_utf8();
                self.selected = 0;
                self.scroll_offset = 0;
                SettingsAction::None
            }
            SettingsKey::FilterBackspace => {
                if self.filter_cursor > 0 {
                    let prev = self.filter[..self.filter_cursor]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.filter.drain(prev..self.filter_cursor);
                    self.filter_cursor = prev;
                    self.selected = 0;
                    self.scroll_offset = 0;
                }
                SettingsAction::None
            }
        };

        // Keep the selected entry visible after any navigation.
        self.ensure_selection_visible(&entries);
        action
    }

    /// Find the selectable index of the first item in `section`.
    fn first_selectable_in_section(entries: &[SettingEntry], section: SettingsSection) -> Option<usize> {
        entries
            .iter()
            .filter(|e| !e.is_header)
            .enumerate()
            .find(|(_, e)| e.section == section)
            .map(|(i, _)| i)
    }

    /// After selection changes, adjust `scroll_offset` so the selected entry
    /// (plus its section header for context) is visible within `max_visible`.
    fn ensure_selection_visible(&mut self, entries: &[SettingEntry]) {
        let Some(abs) = Self::nth_selectable_abs_index(entries, self.selected) else {
            return;
        };
        // Prefer keeping the section header visible above the selected row.
        let desired_top = if abs > 0 && entries.get(abs - 1).map_or(false, |e| e.is_header) {
            abs - 1
        } else {
            abs
        };
        if desired_top < self.scroll_offset {
            self.scroll_offset = desired_top;
        } else if abs >= self.scroll_offset + self.max_visible {
            self.scroll_offset = abs + 1 - self.max_visible;
        }
        self.clamp_scroll(entries.len());
    }

    fn clamp_scroll(&mut self, total: usize) {
        let max = total.saturating_sub(self.max_visible);
        if self.scroll_offset > max {
            self.scroll_offset = max;
        }
    }

    /// Handle keys when a dropdown overlay is open
    fn handle_dropdown_key(&mut self, key: SettingsKey, config: &Config, registry: &ShortcutRegistry) -> SettingsAction {
        let entries = self.build_entries(config, registry);
        let entry = Self::nth_selectable(&entries, self.selected);
        let Some(entry) = entry else {
            self.dropdown_open = false;
            return SettingsAction::None;
        };
        // Clone what we need before the mutable borrow
        let entry_section = entry.section;
        let entry_label = entry.label;
        let entry_kind = entry.kind.clone();

        let option_count = match &entry_kind {
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
                self.map_dropdown_action(entry_section, entry_label, &entry_kind, self.dropdown_highlight)
            }
            _ => SettingsAction::None,
        }
    }

    /// Map a dropdown option selection to a SettingsAction
    fn map_dropdown_action(&self, section: SettingsSection, label: &str, kind: &SettingKind, option_idx: usize) -> SettingsAction {
        match kind {
            SettingKind::Dropdown { options, .. } => {
                let Some(new_val) = options.get(option_idx) else { return SettingsAction::None; };
                let new_val = new_val.clone();
                match (section, label) {
                    (SettingsSection::Theme, _) => SettingsAction::SetTheme(new_val),
                    (SettingsSection::Font, "Family") => SettingsAction::SetFontFamily(new_val),
                    (SettingsSection::Window, "Decorations") => SettingsAction::SetDecorations(new_val == "On"),
                    (SettingsSection::Cursor, "Style") => SettingsAction::SetCursorStyle(new_val),
                    (SettingsSection::Cursor, "Blink") => SettingsAction::SetCursorBlink(new_val == "On"),
                    (SettingsSection::Bell, "Visual") => SettingsAction::SetVisualBell(new_val == "On"),
                    (SettingsSection::Bell, "Audio") => SettingsAction::SetAudioBell(new_val == "On"),
                    (SettingsSection::Startup, "Splash") => {
                        let mode = SplashMode::ALL.get(option_idx).copied().unwrap_or(SplashMode::FirstBoot);
                        SettingsAction::SetSplashMode(mode)
                    }
                    (SettingsSection::Sessions, "Auto Save") => SettingsAction::SetSessionAutoSave(new_val == "On"),
                    (SettingsSection::Sessions, "Auto Restore") => SettingsAction::SetSessionAutoRestore(new_val == "On"),
                    (SettingsSection::Sessions, "Save Scrollback") => SettingsAction::SetSessionSaveScrollback(new_val == "On"),
                    (SettingsSection::Plugins, "Claude Code") => SettingsAction::SetPluginEnabled { plugin_id: "claude-code".into(), enabled: new_val == "Enabled" },
                    (SettingsSection::Plugins, "Claude Model") => SettingsAction::SetPluginModel { plugin_id: "claude-code".into(), model: new_val },
                    (SettingsSection::Plugins, "Claude Approval") => SettingsAction::SetPluginApprovalMode { plugin_id: "claude-code".into(), mode: new_val },
                    (SettingsSection::Plugins, "Codex") => SettingsAction::SetPluginEnabled { plugin_id: "codex".into(), enabled: new_val == "Enabled" },
                    (SettingsSection::Plugins, "Codex Model") => SettingsAction::SetPluginModel { plugin_id: "codex".into(), model: new_val },
                    (SettingsSection::Plugins, "Codex Approval") => SettingsAction::SetPluginApprovalMode { plugin_id: "codex".into(), mode: new_val },
                    _ => SettingsAction::None,
                }
            }
            SettingKind::SteppedList { values, .. } => {
                let Some(&new_val) = values.get(option_idx) else { return SettingsAction::None; };
                match (section, label) {
                    (SettingsSection::Scrollback, "Lines") => SettingsAction::SetScrollback(new_val),
                    (SettingsSection::Scrollback, "Scroll Speed") => SettingsAction::SetScrollSpeed(new_val as f64),
                    _ => SettingsAction::None,
                }
            }
            _ => SettingsAction::None,
        }
    }

    /// Adjust the currently selected value via ←/→ using pre-built entries.
    fn adjust_selected_value(&self, entries: &[SettingEntry], direction: i32) -> SettingsAction {
        let Some(entry) = Self::nth_selectable(entries, self.selected) else {
            return SettingsAction::None;
        };

        match &entry.kind {
            SettingKind::Numeric { min, max, step } => {
                let clean_val = entry.value.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
                let current: f32 = clean_val.parse().unwrap_or(0.0);
                let new_val = (current + step * direction as f32).clamp(*min, *max);

                match (entry.section, entry.label) {
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
                if options.is_empty() { return SettingsAction::None; }
                let new_idx = if direction > 0 {
                    (selected + 1) % options.len()
                } else {
                    selected.checked_sub(1).unwrap_or(options.len() - 1)
                };
                self.map_dropdown_action(entry.section, entry.label, &entry.kind, new_idx)
            }
            SettingKind::SteppedList { values, selected } => {
                if values.is_empty() { return SettingsAction::None; }
                let new_idx = if direction > 0 {
                    (*selected + 1).min(values.len() - 1)
                } else {
                    selected.saturating_sub(1)
                };
                self.map_dropdown_action(entry.section, entry.label, &entry.kind, new_idx)
            }
            SettingKind::ReadOnly => SettingsAction::None,
        }
    }

    /// Handle a mouse click at the given window coordinates (centered modal geometry).
    pub fn handle_click(
        &mut self,
        x: f32,
        y: f32,
        config: &Config,
        registry: &ShortcutRegistry,
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> SettingsAction {
        if !self.is_active() {
            return SettingsAction::None;
        }

        let entries = self.build_entries(config, registry);
        let layout = SettingsModalLayout::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            self.animation_progress(),
            entries.len(),
        );

        // Click outside panel = close
        if !layout.contains(x, y) {
            self.close();
            return SettingsAction::None;
        }

        // If a dropdown is open, resolve clicks against it first.
        if self.dropdown_open {
            if let Some((rect, option_count)) = self.dropdown_layout(&layout, &entries) {
                if rect.contains(x, y) {
                    if let Some(opt_idx) = rect.option_at(y) {
                        self.dropdown_open = false;
                        if opt_idx < option_count {
                            if let Some(entry) = Self::nth_selectable(&entries, self.selected) {
                                return self.map_dropdown_action(
                                    entry.section,
                                    entry.label,
                                    &entry.kind,
                                    opt_idx,
                                );
                            }
                        }
                        return SettingsAction::None;
                    }
                }
            }
            // Click anywhere else inside the panel closes the dropdown.
            self.dropdown_open = false;
            return SettingsAction::None;
        }

        // Click on an entry row (respecting scroll).
        let Some(visible_row) = layout.visible_row_at(y) else {
            return SettingsAction::None;
        };
        let abs_idx = self.scroll_offset + visible_row;
        let Some(entry) = entries.get(abs_idx) else {
            return SettingsAction::None;
        };
        if entry.is_header {
            return SettingsAction::None;
        }

        // Update selection to the clicked entry.
        let sel_idx = entries[..abs_idx].iter().filter(|e| !e.is_header).count();
        self.selected = sel_idx;

        match &entry.kind {
            SettingKind::Dropdown { selected, .. } | SettingKind::SteppedList { selected, .. } => {
                self.dropdown_open = true;
                self.dropdown_highlight = *selected;
                SettingsAction::None
            }
            SettingKind::Numeric { .. } => {
                // Clicking left half = decrease, right half = increase
                let mid = layout.panel_x + layout.panel_width / 2.0;
                let direction = if x < mid { -1 } else { 1 };
                self.adjust_selected_value(&entries, direction)
            }
            SettingKind::ReadOnly => SettingsAction::None,
        }
    }

    /// Resolve the current dropdown's geometry plus its option count.
    pub fn dropdown_layout(
        &self,
        layout: &SettingsModalLayout,
        entries: &[SettingEntry],
    ) -> Option<(DropdownRect, usize)> {
        let abs_idx = Self::nth_selectable_abs_index(entries, self.selected)?;
        let entry = entries.get(abs_idx)?;
        let (option_count, max_len) = match &entry.kind {
            SettingKind::Dropdown { options, .. } => (
                options.len(),
                options.iter().map(|s| s.len()).max().unwrap_or(10),
            ),
            SettingKind::SteppedList { values, .. } => (
                values.len(),
                values.iter().map(|v| v.to_string().len()).max().unwrap_or(10),
            ),
            _ => return None,
        };
        let rect = layout.dropdown_rect(abs_idx, self.scroll_offset, option_count, max_len)?;
        Some((rect, option_count))
    }

    /// Scroll up by n rows
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down by n rows (bounded so the list still fills the viewport).
    pub fn scroll_down(&mut self, n: usize, total_entries: usize) {
        let max = total_entries.saturating_sub(self.max_visible);
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }

    /// Update hover state from mouse position
    pub fn set_hover(&mut self, item: Option<usize>) {
        self.hovered_item = item;
    }

    /// Update dropdown hover state from mouse position
    pub fn set_dropdown_hover(&mut self, index: Option<usize>) {
        self.dropdown_hovered = index;
    }
}

/// Simplified key events for the settings panel
#[derive(Debug, Clone)]
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
    /// Character typed for filter
    FilterChar(char),
    /// Backspace in filter
    FilterBackspace,
    /// Delete key
    Delete,
}

/// Display info passed from the settings panel to the renderer
pub struct SettingsPanelInfo {
    /// Animation progress (0.0 = hidden, 1.0 = fully visible)
    pub animation: f32,
    /// Flat list of entries (headers + items)
    pub entries: Vec<SettingEntry>,
    /// Index of the selected selectable entry
    pub selected: usize,
    /// Scroll offset
    pub scroll_offset: usize,
    /// Whether a dropdown overlay is currently open
    pub dropdown_open: bool,
    /// Which option is highlighted in the dropdown
    pub dropdown_highlight: usize,
    /// Mouse-hovered entry index
    pub hovered_item: Option<usize>,
    /// Mouse-hovered dropdown option index
    pub dropdown_hovered: Option<usize>,
    /// Filter text
    pub filter: String,
    /// Cursor position in filter
    pub filter_cursor: usize,
}

impl SettingsPanel {
    /// Build display info for the renderer. Resolves any pending section jump
    /// and keeps the selected entry within the visible scroll window.
    pub fn build_info(&mut self, config: &Config, registry: &ShortcutRegistry) -> SettingsPanelInfo {
        let entries = self.build_entries(config, registry);

        if let Some(section) = self.pending_section_jump.take() {
            if let Some(idx) = Self::first_selectable_in_section(&entries, section) {
                self.selected = idx;
            }
        }

        self.ensure_selection_visible(&entries);
        self.clamp_scroll(entries.len());

        SettingsPanelInfo {
            animation: self.animation_progress(),
            entries,
            selected: self.selected,
            scroll_offset: self.scroll_offset,
            dropdown_open: self.dropdown_open,
            dropdown_highlight: self.dropdown_highlight,
            hovered_item: self.hovered_item,
            dropdown_hovered: self.dropdown_hovered,
            filter: self.filter.clone(),
            filter_cursor: self.filter_cursor,
        }
    }
}

#[cfg(test)]
impl SettingsPanel {
    fn open(&mut self) {
        self.filter.clear();
        self.filter_cursor = 0;
        self.selected = 0;
        self.scroll_offset = 0;
        self.dropdown_open = false;
        self.animation.open();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn default_registry() -> ShortcutRegistry {
        ShortcutRegistry::new()
    }

    #[test]
    fn test_toggle_open_close() {
        let mut panel = SettingsPanel::new();
        assert!(!panel.is_visible());
        assert!(!panel.is_active());

        panel.toggle();
        assert!(panel.is_visible());

        // Wait for animation to complete
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();
        assert!(panel.is_active());

        panel.toggle();
        // Wait for close animation to complete
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();
        assert!(!panel.is_visible());
    }

    #[test]
    fn test_navigation() {
        let mut panel = SettingsPanel::new();
        panel.open();
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();

        let config = Config::default();
        let registry = default_registry();

        // Start at first selectable entry (Font > Size)
        assert_eq!(panel.selected, 0);

        // Move down (Font > Size -> Font > Family)
        panel.handle_key(SettingsKey::Down, &config, &registry);
        assert_eq!(panel.selected, 1);

        // Move down again (Font > Family -> Font > Line Height)
        panel.handle_key(SettingsKey::Down, &config, &registry);
        assert_eq!(panel.selected, 2);

        // Move down again should go to Theme section's first item
        panel.handle_key(SettingsKey::Down, &config, &registry);
        assert_eq!(panel.selected, 3);

        // Move up should go back
        panel.handle_key(SettingsKey::Up, &config, &registry);
        assert_eq!(panel.selected, 2);
    }

    #[test]
    fn test_value_adjustment() {
        let mut panel = SettingsPanel::new();
        panel.open();
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();

        let config = Config::default();
        let registry = default_registry();

        // Font size is first selectable entry (selected = 0), default 14.0.
        // Right on a Numeric adjusts (Right on a Dropdown would open it).
        let action = panel.handle_key(SettingsKey::Right, &config, &registry);
        match action {
            SettingsAction::SetFontSize(size) => assert_eq!(size, 15.0),
            _ => panic!("Expected SetFontSize"),
        }

        let action = panel.handle_key(SettingsKey::Left, &config, &registry);
        match action {
            SettingsAction::SetFontSize(size) => assert_eq!(size, 13.0),
            _ => panic!("Expected SetFontSize"),
        }
    }

    #[test]
    fn test_theme_dropdown() {
        let mut panel = SettingsPanel::new();
        panel.open();
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();

        let config = Config::default();
        let registry = default_registry();
        let themes = Theme::list_available();
        let expected_next = themes.get(1).expect("at least 2 themes");

        // Entries: [Font header, Size, Family, Line Height, Theme header, Theme, ...]
        // Selectables: [Size(0), Family(1), Line Height(2), Theme(3), ...]
        panel.selected = 3;

        // Right opens the dropdown for Dropdown kind
        let action = panel.handle_key(SettingsKey::Right, &config, &registry);
        assert!(matches!(action, SettingsAction::None));
        assert!(panel.dropdown_open);

        // Down moves to next option
        let action = panel.handle_key(SettingsKey::Down, &config, &registry);
        assert!(matches!(action, SettingsAction::None));
        assert_eq!(panel.dropdown_highlight, 1);

        // Enter selects and closes dropdown
        let action = panel.handle_key(SettingsKey::Enter, &config, &registry);
        assert!(!panel.dropdown_open);
        match action {
            SettingsAction::SetTheme(name) => assert_eq!(name, *expected_next),
            _ => panic!("Expected SetTheme"),
        }
    }

    #[test]
    fn test_search_filter() {
        let mut panel = SettingsPanel::new();
        panel.open();
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();

        let config = Config::default();
        let registry = default_registry();

        // Type "opacity" to filter
        for ch in "opacity".chars() {
            panel.handle_key(SettingsKey::FilterChar(ch), &config, &registry);
        }

        let entries = panel.build_entries(&config, &registry);
        // Should have Window header + Opacity item (at minimum)
        let selectable: Vec<_> = entries.iter().filter(|e| !e.is_header).collect();
        assert!(selectable.len() >= 1);
        assert!(selectable.iter().any(|e| e.label == "Opacity"));
    }

    #[test]
    fn test_escape_clears_filter_first() {
        let mut panel = SettingsPanel::new();
        panel.open();
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();

        let config = Config::default();
        let registry = default_registry();

        // Type something
        panel.handle_key(SettingsKey::FilterChar('a'), &config, &registry);
        assert!(!panel.filter.is_empty());

        // First Escape clears filter, panel stays open
        panel.handle_key(SettingsKey::Escape, &config, &registry);
        assert!(panel.filter.is_empty());
        assert!(panel.is_active());

        // Second Escape closes
        panel.handle_key(SettingsKey::Escape, &config, &registry);
        // Panel is now closing
    }

    #[test]
    fn test_category_headers_not_selectable() {
        let mut panel = SettingsPanel::new();
        panel.open();
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();

        let config = Config::default();
        let registry = default_registry();

        let entries = panel.build_entries(&config, &registry);
        let headers: Vec<_> = entries.iter().filter(|e| e.is_header).collect();
        let selectable: Vec<_> = entries.iter().filter(|e| !e.is_header).collect();

        // Should have at least 10 headers (one per section) and many selectable items
        assert!(headers.len() >= 10);
        assert!(selectable.len() > headers.len());
    }

    #[test]
    fn test_scroll_follows_selection_down() {
        let mut panel = SettingsPanel::new();
        panel.open();
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();

        // Tight viewport so scrolling actually kicks in.
        panel.set_max_visible(5);

        let config = Config::default();
        let registry = default_registry();
        let entries = panel.build_entries(&config, &registry);

        // Move selection down until it would fall outside the viewport.
        for _ in 0..10 {
            panel.handle_key(SettingsKey::Down, &config, &registry);
        }

        // Selected entry must be within the visible window.
        let abs = SettingsPanel::nth_selectable_abs_index(&entries, panel.selected).unwrap();
        assert!(abs >= panel.scroll_offset);
        assert!(abs < panel.scroll_offset + panel.max_visible);
    }

    #[test]
    fn test_open_to_section_jumps_without_filter() {
        let mut panel = SettingsPanel::new();
        panel.open_to_section(SettingsSection::Sessions);
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();

        // Filter must be empty — the full list stays browsable.
        assert!(panel.filter.is_empty());

        // build_info resolves the pending jump.
        let config = Config::default();
        let registry = default_registry();
        let info = panel.build_info(&config, &registry);

        let selected_entry = SettingsPanel::nth_selectable(&info.entries, info.selected).unwrap();
        assert_eq!(selected_entry.section, SettingsSection::Sessions);
    }
}
