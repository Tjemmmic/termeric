use crate::config::theme::Theme;
use crate::config::{
    AccessibilityPreference, BackgroundImageFit, Config, SplashMode, TabBarPosition,
    WindowLevelConfig,
};
use crate::input::keybindings::ShortcutRegistry;
use crate::ui::text_input::{TextInputEdit, edit_grapheme};
use crate::ui::text_input_viewport::TextInputViewport;

const MAX_FILTER_CHARACTERS: usize = 256;

/// Default fallback font (always available)
const DEFAULT_FONT: &str = "monospace";
const NO_FONT_FALLBACK: &str = "None";
const FONT_FALLBACK_LABELS: [&str; crate::config::MAX_FONT_FALLBACKS] = [
    "Fallback 1",
    "Fallback 2",
    "Fallback 3",
    "Fallback 4",
    "Fallback 5",
    "Fallback 6",
    "Fallback 7",
    "Fallback 8",
    "Fallback 9",
    "Fallback 10",
    "Fallback 11",
    "Fallback 12",
    "Fallback 13",
    "Fallback 14",
    "Fallback 15",
    "Fallback 16",
];

fn font_fallback_index(label: &str) -> Option<usize> {
    FONT_FALLBACK_LABELS
        .iter()
        .position(|candidate| *candidate == label)
}

/// Geometry for the centered settings modal — single source of truth shared
/// by the renderer, hit-testing, scroll, and hover code paths.
#[derive(Debug, Clone, Copy)]
pub struct SettingsModalLayout {
    pub input: SettingsInputRect,
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

#[derive(Debug, Clone, Copy)]
pub struct SettingsInputRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl SettingsModalLayout {
    pub fn input_viewport(
        &self,
        text: &str,
        cursor: usize,
        cell_height: f32,
    ) -> Option<TextInputViewport> {
        TextInputViewport::calculate(
            text,
            cursor,
            self.input.x + self.cell_width * 2.2,
            self.input.y + (self.input.height - cell_height) / 2.0,
            self.input.x + self.input.width - self.cell_width * 0.8,
            self.cell_width,
        )
    }

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
            input: SettingsInputRect {
                x: panel_x + padding,
                y: panel_y + padding + cell_height * 1.4 + gap,
                width: panel_width - padding * 2.0,
                height: input_height,
            },
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

    /// Horizontal origin of setting values. Keeping this calculation here
    /// prevents the renderer and mouse hit-test from drifting apart.
    pub fn value_text_x(&self) -> f32 {
        self.value_x + self.cell_width * 2.0
    }

    /// Horizontal origin of the numeric decrement button.
    pub fn numeric_decrement_x(&self) -> f32 {
        self.value_x
    }

    /// Horizontal origin of the numeric increment button.
    pub fn numeric_increment_x(&self, value_char_count: usize) -> f32 {
        self.value_text_x() + value_char_count as f32 * self.cell_width + self.cell_width
    }

    /// Resolve a click against the two rendered numeric arrow buttons.
    /// The half-cell padding makes the small glyphs comfortable to click
    /// without treating the displayed value itself as an increment button.
    pub fn numeric_adjustment_at(&self, x: f32, value_char_count: usize) -> Option<i32> {
        let hit_padding = self.cell_width * 0.5;
        let contains =
            |origin: f32| x >= origin - hit_padding && x < origin + self.cell_width + hit_padding;

        if contains(self.numeric_decrement_x()) {
            Some(-1)
        } else if contains(self.numeric_increment_x(value_char_count)) {
            Some(1)
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
        highlighted_option: usize,
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
        let first_option = if option_count > max_visible_options {
            highlighted_option
                .saturating_sub(max_visible_options - 1)
                .min(option_count - max_visible_options)
        } else {
            0
        };
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
            first_option,
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
    /// Absolute option index displayed in the first visible row.
    pub first_option: usize,
}

impl DropdownRect {
    /// Which option index does a y-coordinate fall into, if any.
    pub fn option_at(&self, y: f32) -> Option<usize> {
        let inner_y = y - self.y - 4.0;
        if inner_y < 0.0 {
            return None;
        }
        let visible_index = (inner_y / self.option_height) as usize;
        if visible_index < self.visible_options {
            Some(self.first_option + visible_index)
        } else {
            None
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Sections in the settings panel — used for categorizing items
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    Harvest,
    Accessibility,
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
            Self::Harvest => "Harvest (Experimental)",
            Self::Accessibility => "Accessibility",
            Self::Plugins => "Plugins",
        }
    }

    pub fn all() -> &'static [SettingsSection] {
        static ALL: [SettingsSection; 12] = [
            SettingsSection::Font,
            SettingsSection::Theme,
            SettingsSection::Window,
            SettingsSection::Cursor,
            SettingsSection::Bell,
            SettingsSection::Scrollback,
            SettingsSection::Startup,
            SettingsSection::Shell,
            SettingsSection::Sessions,
            SettingsSection::Harvest,
            SettingsSection::Accessibility,
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
    Dropdown {
        options: Vec<String>,
        selected: usize,
    },
    /// Predefined list of numeric values (for logarithmic-style stepping)
    SteppedList { values: Vec<usize>, selected: usize },
    /// A row that invokes a bounded application action instead of changing a
    /// persisted configuration value.
    Action { action: SettingsAction },
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
    /// Change the OpenType `wght` value for ordinary terminal text.
    SetFontWeight(u16),
    /// Change the OpenType `wght` value for SGR bold terminal text.
    SetBoldFontWeight(u16),
    /// Set, append, or remove one position in the ordered fallback chain.
    SetFontFallback {
        index: usize,
        family: Option<String>,
    },
    /// Open the bounded Unicode-range font override manager.
    ManageFontRangeOverrides,
    /// Open the bounded explicit OpenType feature manager.
    ManageFontFeatures,
    /// Change font line height
    SetLineHeight(f32),
    /// Enable or disable programming ligatures.
    SetFontLigatures(bool),
    /// Enable or disable ANSI bold-to-bright compatibility.
    SetBoldIsBright(bool),
    /// Change theme to this name
    SetTheme(String),
    /// Change window padding
    SetPadding(u32),
    /// Change window opacity
    SetOpacity(f32),
    /// Change background-image alpha over the theme background.
    SetBackgroundImageOpacity(f32),
    /// Change background-image mapping within the terminal viewport.
    SetBackgroundImageFit(BackgroundImageFit),
    /// Change window decorations
    SetDecorations(bool),
    /// Change the desktop stacking policy for ordinary windows.
    SetWindowLevel(WindowLevelConfig),
    /// Place tabs in the top toolbar or a vertical left rail.
    SetTabBarPosition(TabBarPosition),
    /// Change scrollback lines
    SetScrollback(usize),
    /// Change the per-terminal scrollback memory budget in MiB
    SetScrollbackMemory(usize),
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
    /// Enable or disable the experimental Harvest beta.
    SetHarvestEnabled(bool),
    /// Show or hide Harvest growth indicators.
    SetHarvestIndicators(bool),
    /// Change the Harvest history capacity.
    SetHarvestMaxHistory(usize),
    /// Enable or disable a plugin
    SetPluginEnabled { plugin_id: String, enabled: bool },
    /// Change a plugin's model
    SetPluginModel { plugin_id: String, model: String },
    /// Change a plugin's approval mode
    SetPluginApprovalMode { plugin_id: String, mode: String },
    /// Follow the system or override non-essential UI transitions.
    SetReduceMotion(AccessibilityPreference),
    /// Follow the system or override higher-contrast application chrome.
    SetHighContrast(AccessibilityPreference),
    /// Open the installed third-party plugin manager.
    ManageRuntimePlugins,
    /// Save current config
    SaveConfig,
    /// Reset to default config
    ResetConfig,
    /// No action
    None,
}

/// State for the centered settings panel.
pub struct SettingsPanel {
    /// Panel animation releases input immediately while retaining close motion.
    animation: super::animation::PanelAnimation,
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
    /// Whether the next render should bring the keyboard selection into view.
    /// Mouse-wheel scrolling deliberately clears this so the viewport can
    /// move independently of the selected row.
    selection_visibility_pending: bool,
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
    /// Bounded third-party status summary. The adjacent action opens the
    /// request-scoped manager where full source and permission details remain
    /// visible before mutations.
    runtime_plugin_summary: String,
}

impl SettingsPanel {
    pub fn new() -> Self {
        Self {
            animation: super::animation::PanelAnimation::with_duration(
                super::animation::DEFAULT_PANEL_DURATION,
            ),
            filter: String::new(),
            filter_cursor: 0,
            selected: 0,
            scroll_offset: 0,
            max_visible: 12,
            selection_visibility_pending: true,
            dropdown_open: false,
            dropdown_highlight: 0,
            available_fonts: vec![DEFAULT_FONT.to_string()],
            hovered_item: None,
            dropdown_hovered: None,
            theme_meta: None,
            pending_section_jump: None,
            runtime_plugin_summary: "None installed (use `termeric plugin --help`)".into(),
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

    pub fn set_runtime_plugin_summary(&mut self, summary: String) {
        self.runtime_plugin_summary = summary;
    }

    /// Toggle the panel open/closed
    pub fn toggle(&mut self) {
        if !self.animation.is_open() {
            // Reset state when opening
            self.filter.clear();
            self.filter_cursor = 0;
            self.selected = 0;
            self.scroll_offset = 0;
            self.selection_visibility_pending = true;
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
        self.selection_visibility_pending = true;
        self.dropdown_open = false;
        self.animation.open();
        // Find the first selectable entry in the target section.
        self.selected = 0;
        self.pending_section_jump = Some(section);
    }

    /// Update the max visible rows (called by the renderer each frame).
    pub fn set_max_visible(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.max_visible != rows {
            self.max_visible = rows;
            self.selection_visibility_pending = true;
        }
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
        self.animation.is_open()
    }

    /// Update animation state. Returns true if still animating (needs redraw).
    pub fn update_animation(&mut self) -> bool {
        self.animation.update()
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.animation.set_reduced_motion(reduced_motion);
    }

    /// Get current animation progress with ease-out
    pub fn animation_progress(&self) -> f32 {
        self.animation.progress()
    }

    /// Whether the dropdown overlay is currently open
    pub fn is_dropdown_open(&self) -> bool {
        self.dropdown_open
    }

    /// Build the flat list of all settings grouped by section from config
    fn all_items(
        &self,
        config: &Config,
        _registry: &ShortcutRegistry,
    ) -> Vec<(SettingsSection, Vec<SettingItem>)> {
        let mut result = Vec::new();

        // Font
        {
            let mut font_options = self.available_fonts.clone();
            for configured in std::iter::once(&config.font.family).chain(&config.font.fallback) {
                if !font_options
                    .iter()
                    .any(|available| available.eq_ignore_ascii_case(configured))
                {
                    font_options.push(configured.clone());
                }
            }
            let font_selected = font_options
                .iter()
                .position(|n| n == &config.font.family)
                .unwrap_or(0);
            let mut items = vec![
                SettingItem {
                    label: "Size",
                    value: format!("{:.1}", config.font.size),
                    kind: SettingKind::Numeric {
                        min: 8.0,
                        max: 72.0,
                        step: 1.0,
                    },
                },
                SettingItem {
                    label: "Family",
                    value: config.font.family.clone(),
                    kind: SettingKind::Dropdown {
                        options: font_options.clone(),
                        selected: font_selected,
                    },
                },
                SettingItem {
                    label: "Normal Weight",
                    value: config.font.weight.to_string(),
                    kind: SettingKind::Numeric {
                        min: crate::config::MIN_FONT_WEIGHT as f32,
                        max: config.font.bold_weight as f32,
                        step: 25.0,
                    },
                },
                SettingItem {
                    label: "Bold Weight",
                    value: config.font.bold_weight.to_string(),
                    kind: SettingKind::Numeric {
                        min: config.font.weight as f32,
                        max: crate::config::MAX_FONT_WEIGHT as f32,
                        step: 25.0,
                    },
                },
            ];

            let visible_fallbacks = config
                .font
                .fallback
                .len()
                .saturating_add(1)
                .min(crate::config::MAX_FONT_FALLBACKS);
            for (index, label) in FONT_FALLBACK_LABELS
                .iter()
                .copied()
                .enumerate()
                .take(visible_fallbacks)
            {
                let current = config.font.fallback.get(index);
                let mut options = vec![NO_FONT_FALLBACK.to_string()];
                options.extend(
                    font_options
                        .iter()
                        .filter(|family| {
                            !family.eq_ignore_ascii_case(&config.font.family)
                                && !config.font.fallback.iter().enumerate().any(
                                    |(other_index, configured)| {
                                        other_index != index
                                            && configured.eq_ignore_ascii_case(family)
                                    },
                                )
                        })
                        .cloned(),
                );
                if let Some(current) = current
                    && !options
                        .iter()
                        .any(|option| option.eq_ignore_ascii_case(current))
                {
                    options.push(current.clone());
                }
                let selected = current
                    .and_then(|current| {
                        options
                            .iter()
                            .position(|option| option.eq_ignore_ascii_case(current))
                    })
                    .unwrap_or(0);
                items.push(SettingItem {
                    label,
                    value: current
                        .cloned()
                        .unwrap_or_else(|| NO_FONT_FALLBACK.to_string()),
                    kind: SettingKind::Dropdown { options, selected },
                });
            }

            items.push(SettingItem {
                label: "Range Overrides",
                value: match config.font.range_overrides.len() {
                    0 => "Manage · none configured".into(),
                    1 => "Manage · 1 configured".into(),
                    count => format!("Manage · {count} configured"),
                },
                kind: SettingKind::Action {
                    action: SettingsAction::ManageFontRangeOverrides,
                },
            });

            items.push(SettingItem {
                label: "OpenType Features",
                value: match config.font.features.len() {
                    0 => "Manage · none configured".into(),
                    1 => "Manage · 1 configured".into(),
                    count => format!("Manage · {count} configured"),
                },
                kind: SettingKind::Action {
                    action: SettingsAction::ManageFontFeatures,
                },
            });

            items.extend([
                SettingItem {
                    label: "Line Height",
                    value: format!("{:.2}", config.font.line_height),
                    kind: SettingKind::Numeric {
                        min: 0.8,
                        max: 3.0,
                        step: 0.05,
                    },
                },
                SettingItem {
                    label: "Ligatures",
                    value: if config.font.ligatures { "On" } else { "Off" }.into(),
                    kind: SettingKind::Dropdown {
                        options: vec!["On".into(), "Off".into()],
                        selected: if config.font.ligatures { 0 } else { 1 },
                    },
                },
                SettingItem {
                    label: "Bold Is Bright",
                    value: if config.font.bold_is_bright {
                        "On"
                    } else {
                        "Off"
                    }
                    .into(),
                    kind: SettingKind::Dropdown {
                        options: vec!["On".into(), "Off".into()],
                        selected: if config.font.bold_is_bright { 0 } else { 1 },
                    },
                },
            ]);
            result.push((SettingsSection::Font, items));
        }

        // Theme
        {
            let options = Theme::list_all();
            let selected = options
                .iter()
                .position(|n| n == &config.theme.name)
                .unwrap_or(0);
            let mut items = vec![SettingItem {
                label: "Theme",
                value: config.theme.name.clone(),
                kind: SettingKind::Dropdown { options, selected },
            }];
            if let Some(meta) = &self.theme_meta {
                if !meta.name.is_empty() {
                    items.push(SettingItem {
                        label: "Name",
                        value: meta.name.clone(),
                        kind: SettingKind::ReadOnly,
                    });
                }
                if !meta.author.is_empty() {
                    items.push(SettingItem {
                        label: "Author",
                        value: meta.author.clone(),
                        kind: SettingKind::ReadOnly,
                    });
                }
                if !meta.description.is_empty() {
                    items.push(SettingItem {
                        label: "Description",
                        value: meta.description.clone(),
                        kind: SettingKind::ReadOnly,
                    });
                }
            }
            result.push((SettingsSection::Theme, items));
        }

        // Window
        result.push((
            SettingsSection::Window,
            vec![
                SettingItem {
                    label: "Padding",
                    value: format!("{}", config.window.padding),
                    kind: SettingKind::Numeric {
                        min: 0.0,
                        max: 64.0,
                        step: 2.0,
                    },
                },
                SettingItem {
                    label: "Opacity",
                    value: format!("{:.2}", config.window.opacity),
                    kind: SettingKind::Numeric {
                        min: 0.1,
                        max: 1.0,
                        step: 0.05,
                    },
                },
                SettingItem {
                    label: "Background Image",
                    value: config.window.background_image.path.as_ref().map_or_else(
                        || "Off (set path in config.toml)".into(),
                        |path| path.display().to_string(),
                    ),
                    kind: SettingKind::ReadOnly,
                },
                SettingItem {
                    label: "Background Image Opacity",
                    value: format!("{:.2}", config.window.background_image.opacity),
                    kind: SettingKind::Numeric {
                        min: 0.0,
                        max: 1.0,
                        step: 0.05,
                    },
                },
                SettingItem {
                    label: "Background Image Fit",
                    value: config.window.background_image.fit.label().into(),
                    kind: SettingKind::Dropdown {
                        options: BackgroundImageFit::ALL
                            .iter()
                            .map(|fit| fit.label().to_string())
                            .collect(),
                        selected: BackgroundImageFit::ALL
                            .iter()
                            .position(|fit| *fit == config.window.background_image.fit)
                            .unwrap_or(0),
                    },
                },
                SettingItem {
                    label: "Decorations",
                    value: if config.window.decorations {
                        "On".into()
                    } else {
                        "Off".into()
                    },
                    kind: SettingKind::Dropdown {
                        options: vec!["On".into(), "Off".into()],
                        selected: if config.window.decorations { 0 } else { 1 },
                    },
                },
                SettingItem {
                    label: "Window Level",
                    value: config.window.level.label().into(),
                    kind: SettingKind::Dropdown {
                        options: WindowLevelConfig::ALL
                            .iter()
                            .map(|level| level.label().to_string())
                            .collect(),
                        selected: WindowLevelConfig::ALL
                            .iter()
                            .position(|level| *level == config.window.level)
                            .unwrap_or(0),
                    },
                },
                SettingItem {
                    label: "Tab Bar Position",
                    value: config.window.tab_bar_position.label().into(),
                    kind: SettingKind::Dropdown {
                        options: TabBarPosition::ALL
                            .iter()
                            .map(|position| position.label().to_string())
                            .collect(),
                        selected: TabBarPosition::ALL
                            .iter()
                            .position(|position| *position == config.window.tab_bar_position)
                            .unwrap_or(0),
                    },
                },
            ],
        ));

        // Cursor
        {
            use crate::config::CursorStyle;
            let style_options: Vec<String> = CursorStyle::ALL
                .iter()
                .map(|s| s.label().to_string())
                .collect();
            let style_selected = CursorStyle::ALL
                .iter()
                .position(|s| *s == config.cursor.style)
                .unwrap_or(0);
            result.push((
                SettingsSection::Cursor,
                vec![
                    SettingItem {
                        label: "Style",
                        value: config.cursor.style.label().into(),
                        kind: SettingKind::Dropdown {
                            options: style_options,
                            selected: style_selected,
                        },
                    },
                    SettingItem {
                        label: "Blink",
                        value: if config.cursor.blink {
                            "On".into()
                        } else {
                            "Off".into()
                        },
                        kind: SettingKind::Dropdown {
                            options: vec!["On".into(), "Off".into()],
                            selected: if config.cursor.blink { 0 } else { 1 },
                        },
                    },
                    SettingItem {
                        label: "Blink Rate",
                        value: format!("{}ms", config.cursor.blink_rate),
                        kind: SettingKind::Numeric {
                            min: 100.0,
                            max: 2000.0,
                            step: 50.0,
                        },
                    },
                ],
            ));
        }

        // Bell
        result.push((
            SettingsSection::Bell,
            vec![
                SettingItem {
                    label: "Visual",
                    value: if config.bell.visual {
                        "On".into()
                    } else {
                        "Off".into()
                    },
                    kind: SettingKind::Dropdown {
                        options: vec!["On".into(), "Off".into()],
                        selected: if config.bell.visual { 0 } else { 1 },
                    },
                },
                SettingItem {
                    label: "Audio",
                    value: if config.bell.audio {
                        "On".into()
                    } else {
                        "Off".into()
                    },
                    kind: SettingKind::Dropdown {
                        options: vec!["On".into(), "Off".into()],
                        selected: if config.bell.audio { 0 } else { 1 },
                    },
                },
                SettingItem {
                    label: "Flash Duration",
                    value: format!("{}ms", config.bell.visual_duration_ms),
                    kind: SettingKind::Numeric {
                        min: 50.0,
                        max: 1000.0,
                        step: 50.0,
                    },
                },
            ],
        ));

        // Scrollback
        {
            let steps: Vec<usize> = vec![
                100, 500, 1_000, 2_000, 5_000, 10_000, 25_000, 50_000, 100_000,
            ];
            let selected = steps
                .iter()
                .position(|&v| v >= config.scrollback.lines)
                .unwrap_or(steps.len() - 1);
            let speed_steps: Vec<usize> = vec![1, 2, 3, 4, 6, 8, 10, 15, 20];
            let speed_selected = speed_steps
                .iter()
                .position(|&v| v >= config.scrollback.scroll_speed as usize)
                .unwrap_or(speed_steps.len() - 1);
            let memory_steps: Vec<usize> = vec![16, 32, 64, 128, 256, 512, 1_024];
            let memory_selected = memory_steps
                .iter()
                .position(|&value| value >= config.scrollback.max_memory_mib)
                .unwrap_or(memory_steps.len() - 1);
            result.push((
                SettingsSection::Scrollback,
                vec![
                    SettingItem {
                        label: "Lines",
                        value: format!("{}", config.scrollback.lines),
                        kind: SettingKind::SteppedList {
                            values: steps,
                            selected,
                        },
                    },
                    SettingItem {
                        label: "Scroll Speed",
                        value: format!("{}", config.scrollback.scroll_speed as usize),
                        kind: SettingKind::SteppedList {
                            values: speed_steps,
                            selected: speed_selected,
                        },
                    },
                    SettingItem {
                        label: "Memory Limit",
                        value: format!("{} MiB", config.scrollback.max_memory_mib),
                        kind: SettingKind::SteppedList {
                            values: memory_steps,
                            selected: memory_selected,
                        },
                    },
                ],
            ));
        }

        // Startup
        {
            let splash_options: Vec<String> = SplashMode::ALL
                .iter()
                .map(|m| m.label().to_string())
                .collect();
            let splash_selected = SplashMode::ALL
                .iter()
                .position(|m| *m == config.splash.mode)
                .unwrap_or(0);
            result.push((
                SettingsSection::Startup,
                vec![SettingItem {
                    label: "Splash",
                    value: config.splash.mode.label().to_string(),
                    kind: SettingKind::Dropdown {
                        options: splash_options,
                        selected: splash_selected,
                    },
                }],
            ));
        }

        // Shell
        {
            let program = config
                .shell
                .program
                .clone()
                .unwrap_or_else(|| "(default)".into());
            let args_str = if config.shell.args.is_empty() {
                "(none)".into()
            } else {
                config.shell.args.join(" ")
            };
            result.push((
                SettingsSection::Shell,
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
                ],
            ));
        }

        // Sessions
        {
            let on_off = vec!["On".into(), "Off".into()];
            result.push((
                SettingsSection::Sessions,
                vec![
                    SettingItem {
                        label: "Auto Save Workspace",
                        value: if config.session.auto_save {
                            "On".into()
                        } else {
                            "Off".into()
                        },
                        kind: SettingKind::Dropdown {
                            options: on_off.clone(),
                            selected: if config.session.auto_save { 0 } else { 1 },
                        },
                    },
                    SettingItem {
                        label: "Auto Restore Workspace",
                        value: if config.session.auto_restore {
                            "On".into()
                        } else {
                            "Off".into()
                        },
                        kind: SettingKind::Dropdown {
                            options: on_off.clone(),
                            selected: if config.session.auto_restore { 0 } else { 1 },
                        },
                    },
                    SettingItem {
                        label: "Save Scrollback",
                        value: if config.session.save_scrollback {
                            "On".into()
                        } else {
                            "Off".into()
                        },
                        kind: SettingKind::Dropdown {
                            options: on_off,
                            selected: if config.session.save_scrollback { 0 } else { 1 },
                        },
                    },
                ],
            ));

            result.push((
                SettingsSection::Harvest,
                vec![
                    SettingItem {
                        label: "Status",
                        value: "Experimental beta".into(),
                        kind: SettingKind::ReadOnly,
                    },
                    SettingItem {
                        label: "Enabled",
                        value: if config.harvest.enabled { "On" } else { "Off" }.into(),
                        kind: SettingKind::Dropdown {
                            options: vec!["On".into(), "Off".into()],
                            selected: if config.harvest.enabled { 0 } else { 1 },
                        },
                    },
                    SettingItem {
                        label: "Growth Indicators",
                        value: if config.harvest.show_indicators {
                            "On"
                        } else {
                            "Off"
                        }
                        .into(),
                        kind: SettingKind::Dropdown {
                            options: vec!["On".into(), "Off".into()],
                            selected: if config.harvest.show_indicators { 0 } else { 1 },
                        },
                    },
                    SettingItem {
                        label: "History Limit",
                        value: config.harvest.max_history.to_string(),
                        kind: SettingKind::Numeric {
                            min: 1.0,
                            max: 10_000.0,
                            step: 25.0,
                        },
                    },
                ],
            ));
        }

        // Accessibility
        result.push((
            SettingsSection::Accessibility,
            vec![
                SettingItem {
                    label: "Reduce Motion",
                    value: config.accessibility.reduce_motion.label().into(),
                    kind: SettingKind::Dropdown {
                        options: AccessibilityPreference::ALL
                            .into_iter()
                            .map(|preference| preference.label().into())
                            .collect(),
                        selected: AccessibilityPreference::ALL
                            .iter()
                            .position(|preference| {
                                *preference == config.accessibility.reduce_motion
                            })
                            .unwrap_or(0),
                    },
                },
                SettingItem {
                    label: "High Contrast",
                    value: config.accessibility.high_contrast.label().into(),
                    kind: SettingKind::Dropdown {
                        options: AccessibilityPreference::ALL
                            .into_iter()
                            .map(|preference| preference.label().into())
                            .collect(),
                        selected: AccessibilityPreference::ALL
                            .iter()
                            .position(|preference| {
                                *preference == config.accessibility.high_contrast
                            })
                            .unwrap_or(0),
                    },
                },
            ],
        ));

        // Plugins
        {
            let en_dis: Vec<String> = vec!["Enabled".into(), "Disabled".into()];
            let claude_model_opts: Vec<String> = vec![
                "(default)".into(),
                "sonnet".into(),
                "opus".into(),
                "haiku".into(),
            ];
            let claude_model_val = config
                .plugins
                .claude_code
                .model
                .clone()
                .unwrap_or_else(|| "(default)".into());
            let claude_model_sel = claude_model_opts
                .iter()
                .position(|m| m == &claude_model_val)
                .unwrap_or(0);
            let codex_model_opts: Vec<String> = vec![
                "(default)".into(),
                "o3-mini".into(),
                "o4-mini".into(),
                "gpt-4.1".into(),
            ];
            let codex_model_val = config
                .plugins
                .codex
                .model
                .clone()
                .unwrap_or_else(|| "(default)".into());
            let codex_model_sel = codex_model_opts
                .iter()
                .position(|m| m == &codex_model_val)
                .unwrap_or(0);
            let codex_approval_opts: Vec<String> =
                vec!["suggest".into(), "auto-edit".into(), "full-auto".into()];
            let codex_approval_sel = codex_approval_opts
                .iter()
                .position(|m| m == &config.plugins.codex.approval_mode)
                .unwrap_or(0);
            let claude_approval_opts: Vec<String> =
                vec!["(interactive)".into(), "auto-accept".into()];
            let claude_approval_val = config
                .plugins
                .claude_code
                .approval_mode
                .clone()
                .unwrap_or_else(|| "(interactive)".into());
            let claude_approval_sel = claude_approval_opts
                .iter()
                .position(|m| m == &claude_approval_val)
                .unwrap_or(0);

            result.push((
                SettingsSection::Plugins,
                vec![
                    SettingItem {
                        label: "Claude Code",
                        value: if config.plugins.claude_code.enabled {
                            "Enabled".into()
                        } else {
                            "Disabled".into()
                        },
                        kind: SettingKind::Dropdown {
                            options: en_dis.clone(),
                            selected: if config.plugins.claude_code.enabled {
                                0
                            } else {
                                1
                            },
                        },
                    },
                    SettingItem {
                        label: "Claude Binary",
                        value: config.plugins.claude_code.binary.clone(),
                        kind: SettingKind::ReadOnly,
                    },
                    SettingItem {
                        label: "Claude Model",
                        value: claude_model_val,
                        kind: SettingKind::Dropdown {
                            options: claude_model_opts,
                            selected: claude_model_sel,
                        },
                    },
                    SettingItem {
                        label: "Claude Approval",
                        value: claude_approval_val,
                        kind: SettingKind::Dropdown {
                            options: claude_approval_opts,
                            selected: claude_approval_sel,
                        },
                    },
                    SettingItem {
                        label: "Codex",
                        value: if config.plugins.codex.enabled {
                            "Enabled".into()
                        } else {
                            "Disabled".into()
                        },
                        kind: SettingKind::Dropdown {
                            options: en_dis,
                            selected: if config.plugins.codex.enabled { 0 } else { 1 },
                        },
                    },
                    SettingItem {
                        label: "Codex Binary",
                        value: config.plugins.codex.binary.clone(),
                        kind: SettingKind::ReadOnly,
                    },
                    SettingItem {
                        label: "Codex Model",
                        value: codex_model_val,
                        kind: SettingKind::Dropdown {
                            options: codex_model_opts,
                            selected: codex_model_sel,
                        },
                    },
                    SettingItem {
                        label: "Codex Approval Policy",
                        value: config.plugins.codex.approval_mode.clone(),
                        kind: SettingKind::Dropdown {
                            options: codex_approval_opts,
                            selected: codex_approval_sel,
                        },
                    },
                    SettingItem {
                        label: "Third-party Plugins",
                        value: self.runtime_plugin_summary.clone(),
                        kind: SettingKind::ReadOnly,
                    },
                    SettingItem {
                        label: "Manage Third-party Plugins",
                        value: "Open manager".into(),
                        kind: SettingKind::Action {
                            action: SettingsAction::ManageRuntimePlugins,
                        },
                    },
                ],
            ));
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
            let matching: Vec<(usize, &SettingItem)> = items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    if !has_filter {
                        return true;
                    }
                    // Match against label, value, or section name
                    item.label.to_lowercase().contains(&filter_lower)
                        || item.value.to_lowercase().contains(&filter_lower)
                        || section.label().to_lowercase().contains(&filter_lower)
                })
                .collect();

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
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_header)
            .nth(n)
            .map(|(i, _)| i)
    }

    /// Handle a key press. Returns a SettingsAction if a config change should be applied.
    pub fn handle_key(
        &mut self,
        key: SettingsKey,
        config: &Config,
        registry: &ShortcutRegistry,
    ) -> SettingsAction {
        // If a dropdown is open, handle keys within it
        if self.dropdown_open {
            return self.handle_dropdown_key(key, config, registry);
        }

        // Text edits do not need to rebuild the settings inventory, and a
        // rejected edit must not alter result selection or scrolling.
        if matches!(key, SettingsKey::FilterBackspace) {
            self.backspace_filter();
            return SettingsAction::None;
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
                        SettingKind::Dropdown { selected, .. }
                        | SettingKind::SteppedList { selected, .. } => {
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
                    let current_idx = sections
                        .iter()
                        .position(|s| *s == current.section)
                        .unwrap_or(0);
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
            SettingsKey::FilterBackspace => unreachable!(),
        };

        // Keep the selected entry visible after any navigation.
        self.ensure_selection_visible(&entries);
        self.selection_visibility_pending = false;
        action
    }

    /// Insert one complete text event or reject it without changing any state.
    pub fn insert_filter_text(&mut self, text: &str) -> bool {
        if !self.is_active()
            || self.dropdown_open
            || text.is_empty()
            || text.len() > MAX_FILTER_CHARACTERS * 4
            || self.filter.chars().count() + text.chars().count() > MAX_FILTER_CHARACTERS
            || text
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
            || !self.filter.is_char_boundary(self.filter_cursor)
        {
            return false;
        }
        self.filter.insert_str(self.filter_cursor, text);
        self.filter_cursor += text.len();
        self.selected = 0;
        self.scroll_offset = 0;
        self.selection_visibility_pending = false;
        true
    }

    pub fn backspace_filter(&mut self) -> bool {
        if !self.is_active() || self.dropdown_open {
            return false;
        }
        if !edit_grapheme(
            &mut self.filter,
            &mut self.filter_cursor,
            TextInputEdit::Backspace,
        ) {
            return false;
        }
        self.selected = 0;
        self.scroll_offset = 0;
        self.selection_visibility_pending = false;
        true
    }

    /// Candidate geometry needs only filter state, never a rebuilt settings list.
    pub fn input_viewport(
        &self,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<TextInputViewport> {
        if !self.is_active() || self.dropdown_open {
            return None;
        }
        SettingsModalLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            self.animation_progress(),
            0,
        )
        .input_viewport(&self.filter, self.filter_cursor, cell_height)
    }

    /// Find the selectable index of the first item in `section`.
    fn first_selectable_in_section(
        entries: &[SettingEntry],
        section: SettingsSection,
    ) -> Option<usize> {
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
        let desired_top = if abs > 0 && entries.get(abs - 1).is_some_and(|e| e.is_header) {
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
    fn handle_dropdown_key(
        &mut self,
        key: SettingsKey,
        config: &Config,
        registry: &ShortcutRegistry,
    ) -> SettingsAction {
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
            _ => {
                self.dropdown_open = false;
                return SettingsAction::None;
            }
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
                self.map_dropdown_action(
                    entry_section,
                    entry_label,
                    &entry_kind,
                    self.dropdown_highlight,
                )
            }
            _ => SettingsAction::None,
        }
    }

    /// Map a dropdown option selection to a SettingsAction
    fn map_dropdown_action(
        &self,
        section: SettingsSection,
        label: &str,
        kind: &SettingKind,
        option_idx: usize,
    ) -> SettingsAction {
        match kind {
            SettingKind::Dropdown { options, .. } => {
                let Some(new_val) = options.get(option_idx) else {
                    return SettingsAction::None;
                };
                let new_val = new_val.clone();
                match (section, label) {
                    (SettingsSection::Theme, _) => SettingsAction::SetTheme(new_val),
                    (SettingsSection::Font, "Family") => SettingsAction::SetFontFamily(new_val),
                    (SettingsSection::Font, "Ligatures") => {
                        SettingsAction::SetFontLigatures(new_val == "On")
                    }
                    (SettingsSection::Font, "Bold Is Bright") => {
                        SettingsAction::SetBoldIsBright(new_val == "On")
                    }
                    (SettingsSection::Font, label) => {
                        font_fallback_index(label).map_or(SettingsAction::None, |index| {
                            SettingsAction::SetFontFallback {
                                index,
                                family: (new_val != NO_FONT_FALLBACK).then_some(new_val),
                            }
                        })
                    }
                    (SettingsSection::Window, "Decorations") => {
                        SettingsAction::SetDecorations(new_val == "On")
                    }
                    (SettingsSection::Window, "Window Level") => WindowLevelConfig::ALL
                        .iter()
                        .copied()
                        .find(|level| level.label() == new_val)
                        .map_or(SettingsAction::None, SettingsAction::SetWindowLevel),
                    (SettingsSection::Window, "Tab Bar Position") => TabBarPosition::ALL
                        .iter()
                        .copied()
                        .find(|position| position.label() == new_val)
                        .map_or(SettingsAction::None, SettingsAction::SetTabBarPosition),
                    (SettingsSection::Window, "Background Image Fit") => BackgroundImageFit::ALL
                        .iter()
                        .copied()
                        .find(|fit| fit.label() == new_val)
                        .map_or(SettingsAction::None, SettingsAction::SetBackgroundImageFit),
                    (SettingsSection::Cursor, "Style") => SettingsAction::SetCursorStyle(new_val),
                    (SettingsSection::Cursor, "Blink") => {
                        SettingsAction::SetCursorBlink(new_val == "On")
                    }
                    (SettingsSection::Bell, "Visual") => {
                        SettingsAction::SetVisualBell(new_val == "On")
                    }
                    (SettingsSection::Bell, "Audio") => {
                        SettingsAction::SetAudioBell(new_val == "On")
                    }
                    (SettingsSection::Startup, "Splash") => {
                        let mode = SplashMode::ALL
                            .get(option_idx)
                            .copied()
                            .unwrap_or(SplashMode::FirstBoot);
                        SettingsAction::SetSplashMode(mode)
                    }
                    (SettingsSection::Sessions, "Auto Save Workspace") => {
                        SettingsAction::SetSessionAutoSave(new_val == "On")
                    }
                    (SettingsSection::Sessions, "Auto Restore Workspace") => {
                        SettingsAction::SetSessionAutoRestore(new_val == "On")
                    }
                    (SettingsSection::Sessions, "Save Scrollback") => {
                        SettingsAction::SetSessionSaveScrollback(new_val == "On")
                    }
                    (SettingsSection::Harvest, "Enabled") => {
                        SettingsAction::SetHarvestEnabled(new_val == "On")
                    }
                    (SettingsSection::Harvest, "Growth Indicators") => {
                        SettingsAction::SetHarvestIndicators(new_val == "On")
                    }
                    (SettingsSection::Accessibility, "Reduce Motion") => {
                        SettingsAction::SetReduceMotion(
                            AccessibilityPreference::ALL
                                .get(option_idx)
                                .copied()
                                .unwrap_or_default(),
                        )
                    }
                    (SettingsSection::Accessibility, "High Contrast") => {
                        SettingsAction::SetHighContrast(
                            AccessibilityPreference::ALL
                                .get(option_idx)
                                .copied()
                                .unwrap_or_default(),
                        )
                    }
                    (SettingsSection::Plugins, "Claude Code") => SettingsAction::SetPluginEnabled {
                        plugin_id: "claude-code".into(),
                        enabled: new_val == "Enabled",
                    },
                    (SettingsSection::Plugins, "Claude Model") => SettingsAction::SetPluginModel {
                        plugin_id: "claude-code".into(),
                        model: new_val,
                    },
                    (SettingsSection::Plugins, "Claude Approval") => {
                        SettingsAction::SetPluginApprovalMode {
                            plugin_id: "claude-code".into(),
                            mode: new_val,
                        }
                    }
                    (SettingsSection::Plugins, "Codex") => SettingsAction::SetPluginEnabled {
                        plugin_id: "codex".into(),
                        enabled: new_val == "Enabled",
                    },
                    (SettingsSection::Plugins, "Codex Model") => SettingsAction::SetPluginModel {
                        plugin_id: "codex".into(),
                        model: new_val,
                    },
                    (SettingsSection::Plugins, "Codex Approval Policy") => {
                        SettingsAction::SetPluginApprovalMode {
                            plugin_id: "codex".into(),
                            mode: new_val,
                        }
                    }
                    _ => SettingsAction::None,
                }
            }
            SettingKind::SteppedList { values, .. } => {
                let Some(&new_val) = values.get(option_idx) else {
                    return SettingsAction::None;
                };
                match (section, label) {
                    (SettingsSection::Scrollback, "Lines") => {
                        SettingsAction::SetScrollback(new_val)
                    }
                    (SettingsSection::Scrollback, "Scroll Speed") => {
                        SettingsAction::SetScrollSpeed(new_val as f64)
                    }
                    (SettingsSection::Scrollback, "Memory Limit") => {
                        SettingsAction::SetScrollbackMemory(new_val)
                    }
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
                let clean_val = entry
                    .value
                    .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
                let current: f32 = clean_val.parse().unwrap_or(0.0);
                let new_val = (current + step * direction as f32).clamp(*min, *max);

                match (entry.section, entry.label) {
                    (SettingsSection::Font, "Size") => SettingsAction::SetFontSize(new_val),
                    (SettingsSection::Font, "Normal Weight") => {
                        SettingsAction::SetFontWeight(new_val as u16)
                    }
                    (SettingsSection::Font, "Bold Weight") => {
                        SettingsAction::SetBoldFontWeight(new_val as u16)
                    }
                    (SettingsSection::Font, "Line Height") => {
                        SettingsAction::SetLineHeight(new_val)
                    }
                    (SettingsSection::Window, "Padding") => {
                        SettingsAction::SetPadding(new_val as u32)
                    }
                    (SettingsSection::Window, "Opacity") => SettingsAction::SetOpacity(new_val),
                    (SettingsSection::Window, "Background Image Opacity") => {
                        SettingsAction::SetBackgroundImageOpacity(new_val)
                    }
                    (SettingsSection::Cursor, "Blink Rate") => {
                        SettingsAction::SetCursorBlinkRate(new_val as u32)
                    }
                    (SettingsSection::Bell, "Flash Duration") => {
                        SettingsAction::SetBellDuration(new_val as u64)
                    }
                    (SettingsSection::Harvest, "History Limit") => {
                        SettingsAction::SetHarvestMaxHistory(new_val as usize)
                    }
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
                self.map_dropdown_action(entry.section, entry.label, &entry.kind, new_idx)
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
                self.map_dropdown_action(entry.section, entry.label, &entry.kind, new_idx)
            }
            SettingKind::Action { action } => {
                if direction > 0 {
                    action.clone()
                } else {
                    SettingsAction::None
                }
            }
            SettingKind::ReadOnly => SettingsAction::None,
        }
    }

    /// Select a setting by its stable semantic identity. Returns false when
    /// the setting is not present in the current filtered list.
    pub fn accessibility_focus_setting(
        &mut self,
        section: SettingsSection,
        label: &str,
        config: &Config,
        registry: &ShortcutRegistry,
    ) -> bool {
        let entries = self.build_entries(config, registry);
        let Some(selected) = entries
            .iter()
            .filter(|entry| !entry.is_header)
            .position(|entry| entry.section == section && entry.label == label)
        else {
            return false;
        };
        self.selected = selected;
        self.ensure_selection_visible(&entries);
        self.selection_visibility_pending = false;
        true
    }

    /// Adjust a setting through the same bounded action mapping used by the
    /// keyboard and mouse paths.
    pub fn accessibility_adjust_setting(
        &mut self,
        section: SettingsSection,
        label: &str,
        direction: i32,
        config: &Config,
        registry: &ShortcutRegistry,
    ) -> SettingsAction {
        if !self.accessibility_focus_setting(section, label, config, registry) {
            return SettingsAction::None;
        }
        self.dropdown_open = false;
        let entries = self.build_entries(config, registry);
        self.adjust_selected_value(&entries, direction.signum())
    }

    /// Activate an action row through the same stable semantic identity used
    /// by keyboard and pointer input.
    pub fn accessibility_activate_setting(
        &mut self,
        section: SettingsSection,
        label: &str,
        config: &Config,
        registry: &ShortcutRegistry,
    ) -> SettingsAction {
        if !self.accessibility_focus_setting(section, label, config, registry) {
            return SettingsAction::None;
        }
        self.dropdown_open = false;
        let entries = self.build_entries(config, registry);
        let Some(entry) = Self::nth_selectable(&entries, self.selected) else {
            return SettingsAction::None;
        };
        match &entry.kind {
            SettingKind::Action { action } => action.clone(),
            _ => SettingsAction::None,
        }
    }

    /// Open or close a setting's dropdown without bypassing its current
    /// filtered identity or kind validation.
    pub fn accessibility_set_dropdown_expanded(
        &mut self,
        section: SettingsSection,
        label: &str,
        expanded: bool,
        config: &Config,
        registry: &ShortcutRegistry,
    ) -> bool {
        if !expanded {
            self.dropdown_open = false;
            return true;
        }
        if !self.accessibility_focus_setting(section, label, config, registry) {
            return false;
        }
        let entries = self.build_entries(config, registry);
        let Some(entry) = Self::nth_selectable(&entries, self.selected) else {
            return false;
        };
        let selected = match &entry.kind {
            SettingKind::Dropdown { selected, options } if !options.is_empty() => *selected,
            SettingKind::SteppedList { selected, values } if !values.is_empty() => *selected,
            _ => return false,
        };
        self.dropdown_highlight = selected;
        self.dropdown_open = true;
        true
    }

    /// Choose the option whose announced value still exists in the open
    /// dropdown. Resolving by value prevents a stale accessibility request
    /// from selecting a different option after a dynamic list is reordered.
    pub fn accessibility_choose_dropdown_option(
        &mut self,
        section: SettingsSection,
        label: &str,
        option_value: &str,
        config: &Config,
        registry: &ShortcutRegistry,
    ) -> SettingsAction {
        if !self.dropdown_open {
            return SettingsAction::None;
        }
        let entries = self.build_entries(config, registry);
        let Some(entry) = Self::nth_selectable(&entries, self.selected) else {
            self.dropdown_open = false;
            return SettingsAction::None;
        };
        if entry.section != section || entry.label != label {
            return SettingsAction::None;
        }
        let option_idx = match &entry.kind {
            SettingKind::Dropdown { options, .. } => {
                options.iter().position(|option| option == option_value)
            }
            SettingKind::SteppedList { values, .. } => values
                .iter()
                .position(|option| option.to_string() == option_value),
            _ => None,
        };
        let Some(option_idx) = option_idx else {
            return SettingsAction::None;
        };
        let action = self.map_dropdown_action(section, label, &entry.kind, option_idx);
        self.dropdown_highlight = option_idx;
        self.dropdown_open = false;
        action
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
                let value_char_count = entry.value.chars().count();
                layout
                    .numeric_adjustment_at(x, value_char_count)
                    .map_or(SettingsAction::None, |direction| {
                        self.adjust_selected_value(&entries, direction)
                    })
            }
            SettingKind::Action { action } => action.clone(),
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
                values
                    .iter()
                    .map(|v| v.to_string().len())
                    .max()
                    .unwrap_or(10),
            ),
            _ => return None,
        };
        let rect = layout.dropdown_rect(
            abs_idx,
            self.scroll_offset,
            option_count,
            max_len,
            self.dropdown_highlight,
        )?;
        Some((rect, option_count))
    }

    /// Scroll up by n rows
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.selection_visibility_pending = false;
    }

    /// Scroll down by n rows (bounded so the list still fills the viewport).
    pub fn scroll_down(&mut self, n: usize, total_entries: usize) {
        let max = total_entries.saturating_sub(self.max_visible);
        self.scroll_offset = (self.scroll_offset + n).min(max);
        self.selection_visibility_pending = false;
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
    pub fn build_info(
        &mut self,
        config: &Config,
        registry: &ShortcutRegistry,
    ) -> SettingsPanelInfo {
        let entries = self.build_entries(config, registry);

        if let Some(section) = self.pending_section_jump.take() {
            if let Some(idx) = Self::first_selectable_in_section(&entries, section) {
                self.selected = idx;
                self.selection_visibility_pending = true;
            }
        }

        if self.selection_visibility_pending {
            self.ensure_selection_visible(&entries);
            self.selection_visibility_pending = false;
        }
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
        self.selection_visibility_pending = true;
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
    fn filter_viewport_is_clipped_grapheme_safe_and_independent_of_result_count() {
        use unicode_segmentation::UnicodeSegmentation;
        let text = "界e\u{301}👩\u{200d}💻".repeat(25);
        for (width, height, cw, ch) in [(320.0, 500.0, 8.0, 16.0), (1600.0, 1000.0, 16.0, 32.0)] {
            for animation in [0.2, 0.7, 1.0] {
                let layout = SettingsModalLayout::calculate(width, height, cw, ch, animation, 100);
                let empty = SettingsModalLayout::calculate(width, height, cw, ch, animation, 0);
                for cursor in [0, "界".len(), text.len()] {
                    let view = layout.input_viewport(&text, cursor, ch).unwrap();
                    assert_eq!(Some(view.clone()), empty.input_viewport(&text, cursor, ch));
                    assert!(view.cursor_x >= view.text_x);
                    assert!(view.cursor_x + cw <= view.right + 0.001);
                    assert!(view.text_x >= layout.input.x);
                    assert!(view.right <= layout.input.x + layout.input.width);
                    assert_eq!(
                        view.text_y,
                        layout.input.y + (layout.input.height - ch) / 2.0
                    );
                    for edge in [view.text_range.start, view.text_range.end] {
                        assert!(
                            edge == text.len()
                                || text
                                    .grapheme_indices(true)
                                    .any(|(offset, _)| offset == edge)
                        );
                    }
                    if cursor == text.len() {
                        assert!(view.text_range.start > 0);
                    }
                }
                assert!(layout.input_viewport(&text, 1, ch).is_none());
            }
        }
        let mut panel = SettingsPanel::new();
        assert!(panel.input_viewport(800.0, 600.0, 8.0, 16.0).is_none());
        panel.open();
        assert!(panel.insert_filter_text(&text));
        assert!(panel.input_viewport(40.0, 600.0, 8.0, 16.0).is_none());
        assert!(panel.input_viewport(800.0, 600.0, 8.0, 16.0).is_some());
        panel.dropdown_open = true;
        assert!(panel.input_viewport(800.0, 600.0, 8.0, 16.0).is_none());
    }

    #[test]
    fn filter_text_events_are_atomic_bounded_and_dropdown_owned() {
        let mut panel = SettingsPanel::new();
        assert!(!panel.insert_filter_text("hidden"));
        panel.open();
        assert!(panel.insert_filter_text(&"🫚".repeat(255)));
        panel.selected = 2;
        panel.scroll_offset = 1;
        panel.selection_visibility_pending = true;
        for rejected in [
            "",
            "e\u{301}",
            "\n",
            "\t",
            "\u{1b}",
            "\u{2028}",
            "\u{2029}",
            &"x".repeat(1025),
        ] {
            assert!(!panel.insert_filter_text(rejected));
            assert_eq!(panel.filter.chars().count(), 255);
            assert_eq!(panel.filter_cursor, 255 * 4);
            assert_eq!((panel.selected, panel.scroll_offset), (2, 1));
            assert!(panel.selection_visibility_pending);
        }
        assert!(panel.insert_filter_text("🫚"));
        assert_eq!(panel.filter.len(), 1024);
        assert!(!panel.insert_filter_text("x"));
        assert_eq!((panel.selected, panel.scroll_offset), (0, 0));
        panel.dropdown_open = true;
        assert!(!panel.backspace_filter());
        assert!(!panel.insert_filter_text("x"));
        assert_eq!(panel.filter_cursor, 1024);
        panel.close();
        assert!(!panel.backspace_filter());
    }

    #[test]
    fn filter_backspace_removes_whole_clusters_and_insertion_honors_byte_caret() {
        let mut panel = SettingsPanel::new();
        panel.open();
        for cluster in ["e\u{301}", "👩\u{200d}💻", "🇺🇸"] {
            assert!(panel.insert_filter_text(cluster));
            assert!(panel.backspace_filter());
            assert!(panel.filter.is_empty());
            assert_eq!(panel.filter_cursor, 0);
        }
        panel.selected = 2;
        panel.scroll_offset = 1;
        assert!(!panel.backspace_filter());
        assert_eq!((panel.selected, panel.scroll_offset), (2, 1));
        assert!(panel.insert_filter_text("a界z"));
        panel.filter_cursor = 1;
        assert!(panel.insert_filter_text("e\u{301}"));
        assert_eq!(panel.filter, "ae\u{301}界z");
        assert_eq!(panel.filter_cursor, 4);
        assert!(panel.backspace_filter());
        assert_eq!(panel.filter, "a界z");
        panel.filter_cursor = 2;
        assert!(!panel.insert_filter_text("invalid caret"));
        assert_eq!(panel.filter, "a界z");
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
        assert!(!panel.is_active());
        assert!(panel.is_visible());
        // Wait for close animation to complete
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();
        assert!(!panel.is_visible());
    }

    #[test]
    fn close_releases_input_and_toggle_reopens_during_animation() {
        let mut panel = SettingsPanel::new();
        panel.toggle();
        assert!(panel.is_active());
        thread::sleep(Duration::from_millis(20));

        panel.close();
        assert!(!panel.is_active());
        assert!(panel.is_visible());

        panel.toggle();
        assert!(panel.is_active());
        assert!(panel.is_visible());
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

        // Move down again (Font > Family -> Font > Normal Weight)
        panel.handle_key(SettingsKey::Down, &config, &registry);
        assert_eq!(panel.selected, 2);

        // Move down again reaches Font > Bold Weight.
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
    fn scrollback_memory_budget_is_visible_and_adjustable() {
        let panel = SettingsPanel::new();
        let mut config = Config::default();
        config.scrollback.max_memory_mib = 64;
        let entries = panel.build_entries(&config, &default_registry());
        let memory = entries
            .iter()
            .find(|entry| entry.label == "Memory Limit")
            .expect("scrollback memory setting");
        assert_eq!(memory.value, "64 MiB");
        assert!(matches!(
            panel.map_dropdown_action(memory.section, memory.label, &memory.kind, 3),
            SettingsAction::SetScrollbackMemory(128)
        ));
    }

    #[test]
    fn ordered_font_fallback_and_rendering_controls_map_to_actions() {
        let mut panel = SettingsPanel::new();
        panel.set_available_fonts(vec![
            "monospace".into(),
            "Fira Mono".into(),
            "Noto Sans Mono".into(),
        ]);
        let mut config = Config::default();
        config.font.family = "Fira Mono".into();
        config.font.fallback = vec!["Noto Sans Mono".into()];
        config.font.range_overrides = vec![crate::config::FontRangeOverride {
            start: 0xE000,
            end: 0xF8FF,
            family: "Symbols Nerd Font Mono".into(),
        }];
        config.font.features.insert("ss01".into(), 1);
        let entries = panel.build_entries(&config, &default_registry());

        let first = entries
            .iter()
            .find(|entry| entry.label == "Fallback 1")
            .expect("configured fallback row");
        assert_eq!(first.value, "Noto Sans Mono");
        let next = entries
            .iter()
            .find(|entry| entry.label == "Fallback 2")
            .expect("empty append slot");
        assert_eq!(next.value, NO_FONT_FALLBACK);
        let ranges = entries
            .iter()
            .find(|entry| entry.label == "Range Overrides")
            .expect("range override summary");
        assert_eq!(ranges.value, "Manage · 1 configured");
        assert!(matches!(
            ranges.kind,
            SettingKind::Action {
                action: SettingsAction::ManageFontRangeOverrides
            }
        ));
        let features = entries
            .iter()
            .find(|entry| entry.label == "OpenType Features")
            .expect("OpenType feature summary");
        assert_eq!(features.value, "Manage · 1 configured");
        assert!(matches!(
            features.kind,
            SettingKind::Action {
                action: SettingsAction::ManageFontFeatures
            }
        ));
        assert!(matches!(
            panel.map_dropdown_action(first.section, first.label, &first.kind, 0),
            SettingsAction::SetFontFallback {
                index: 0,
                family: None
            }
        ));

        let off = SettingKind::Dropdown {
            options: vec!["On".into(), "Off".into()],
            selected: 0,
        };
        assert!(matches!(
            panel.map_dropdown_action(SettingsSection::Font, "Ligatures", &off, 1),
            SettingsAction::SetFontLigatures(false)
        ));
        assert!(matches!(
            panel.map_dropdown_action(SettingsSection::Font, "Bold Is Bright", &off, 1),
            SettingsAction::SetBoldIsBright(false)
        ));

        let normal = entries
            .iter()
            .find(|entry| entry.label == "Normal Weight")
            .expect("normal weight row");
        assert_eq!(normal.value, "400");
        panel.selected = entries
            .iter()
            .filter(|entry| !entry.is_header)
            .position(|entry| entry.label == "Normal Weight")
            .expect("normal weight selectable index");
        assert!(matches!(
            panel.adjust_selected_value(&entries, 1),
            SettingsAction::SetFontWeight(425)
        ));
    }

    #[test]
    fn long_dropdown_keeps_highlight_visible_and_hit_tests_absolute_option() {
        let layout = SettingsModalLayout::calculate(1_000.0, 800.0, 10.0, 20.0, 1.0, 20);
        let rect = layout
            .dropdown_rect(1, 0, 20, 12, 12)
            .expect("visible dropdown");
        assert_eq!(rect.visible_options, 8);
        assert_eq!(rect.first_option, 5);
        assert_eq!(rect.option_at(rect.y + 5.0), Some(5));
        assert_eq!(
            rect.option_at(rect.y + 4.0 + rect.option_height * 7.5),
            Some(12)
        );

        let last = layout
            .dropdown_rect(1, 0, 20, 12, 19)
            .expect("visible dropdown");
        assert_eq!(last.first_option, 12);
        assert_eq!(last.option_at(last.y + 5.0), Some(12));
    }

    #[test]
    fn window_level_setting_maps_typed_policy() {
        let panel = SettingsPanel::new();
        let mut config = Config::default();
        config.window.level = WindowLevelConfig::AlwaysOnTop;
        let entries = panel.build_entries(&config, &default_registry());
        let level = entries
            .iter()
            .find(|entry| entry.label == "Window Level")
            .expect("window level row");
        assert_eq!(level.value, "Always on top");
        assert!(matches!(
            panel.map_dropdown_action(level.section, level.label, &level.kind, 2),
            SettingsAction::SetWindowLevel(WindowLevelConfig::AlwaysOnBottom)
        ));
    }

    #[test]
    fn tab_bar_position_setting_maps_typed_policy() {
        let panel = SettingsPanel::new();
        let mut config = Config::default();
        config.window.tab_bar_position = TabBarPosition::Left;
        let entries = panel.build_entries(&config, &default_registry());
        let position = entries
            .iter()
            .find(|entry| entry.label == "Tab Bar Position")
            .expect("tab bar position row");
        assert_eq!(position.value, "Left");
        assert!(matches!(
            panel.map_dropdown_action(position.section, position.label, &position.kind, 0),
            SettingsAction::SetTabBarPosition(TabBarPosition::Top)
        ));
    }

    #[test]
    fn background_image_settings_expose_path_fit_and_opacity_without_a_file_picker() {
        let panel = SettingsPanel::new();
        let mut config = Config::default();
        config.window.background_image.path = Some("wallpaper.png".into());
        config.window.background_image.fit = BackgroundImageFit::Center;
        config.window.background_image.opacity = 0.4;
        let entries = panel.build_entries(&config, &default_registry());

        let path = entries
            .iter()
            .find(|entry| entry.label == "Background Image")
            .expect("background image path row");
        assert_eq!(path.value, "wallpaper.png");
        assert!(matches!(path.kind, SettingKind::ReadOnly));

        let fit = entries
            .iter()
            .find(|entry| entry.label == "Background Image Fit")
            .expect("background image fit row");
        assert_eq!(fit.value, "Center");
        assert!(matches!(
            panel.map_dropdown_action(fit.section, fit.label, &fit.kind, 1),
            SettingsAction::SetBackgroundImageFit(BackgroundImageFit::Contain)
        ));

        let opacity = entries
            .iter()
            .find(|entry| entry.label == "Background Image Opacity")
            .expect("background image opacity row");
        assert_eq!(opacity.value, "0.40");
        assert!(matches!(opacity.kind, SettingKind::Numeric { .. }));
    }

    #[test]
    fn third_party_permission_status_is_visible_in_plugin_settings() {
        let mut panel = SettingsPanel::new();
        panel.set_runtime_plugin_summary(
            "task-runner:on req=[process-spawn] grant=[process-spawn]".into(),
        );
        let entries = panel.build_entries(&Config::default(), &ShortcutRegistry::new());
        let status = entries
            .iter()
            .find(|entry| entry.label == "Third-party Plugins")
            .expect("third-party status row");
        assert!(status.value.contains("task-runner:on"));
        assert!(status.value.contains("req=[process-spawn]"));
        assert!(status.value.contains("grant=[process-spawn]"));
        assert!(matches!(status.kind, SettingKind::ReadOnly));

        let manager = entries
            .iter()
            .find(|entry| entry.label == "Manage Third-party Plugins")
            .expect("third-party manager action row");
        assert!(matches!(manager.kind, SettingKind::Action { .. }));
        assert!(matches!(
            panel.accessibility_activate_setting(
                SettingsSection::Plugins,
                "Manage Third-party Plugins",
                &Config::default(),
                &ShortcutRegistry::new(),
            ),
            SettingsAction::ManageRuntimePlugins
        ));
    }

    #[test]
    fn runtime_plugin_manager_action_has_shared_keyboard_and_pointer_routing() {
        let mut panel = SettingsPanel::new();
        panel.open();
        let config = Config::default();
        let registry = default_registry();
        assert!(panel.insert_filter_text("manage third-party"));
        assert!(matches!(
            panel.handle_key(SettingsKey::Enter, &config, &registry),
            SettingsAction::ManageRuntimePlugins
        ));

        let entries = panel.build_entries(&config, &registry);
        assert_eq!(entries.len(), 2, "filtered list contains header and action");
        let layout = SettingsModalLayout::calculate(
            1000.0,
            800.0,
            10.0,
            20.0,
            panel.animation_progress(),
            entries.len(),
        );
        let action = panel.handle_click(
            layout.panel_x + layout.padding,
            layout.list_y + layout.row_height * 1.5,
            &config,
            &registry,
            1000.0,
            800.0,
            10.0,
            20.0,
        );
        assert!(matches!(action, SettingsAction::ManageRuntimePlugins));
    }

    #[test]
    fn font_range_manager_action_has_shared_keyboard_pointer_and_semantic_routing() {
        let mut panel = SettingsPanel::new();
        panel.open();
        let config = Config::default();
        let registry = default_registry();
        assert!(panel.insert_filter_text("range overrides"));
        assert!(matches!(
            panel.handle_key(SettingsKey::Enter, &config, &registry),
            SettingsAction::ManageFontRangeOverrides
        ));
        assert!(matches!(
            panel.accessibility_activate_setting(
                SettingsSection::Font,
                "Range Overrides",
                &config,
                &registry,
            ),
            SettingsAction::ManageFontRangeOverrides
        ));

        let entries = panel.build_entries(&config, &registry);
        assert_eq!(entries.len(), 2, "filtered list contains header and action");
        let layout = SettingsModalLayout::calculate(
            1000.0,
            800.0,
            10.0,
            20.0,
            panel.animation_progress(),
            entries.len(),
        );
        let action = panel.handle_click(
            layout.panel_x + layout.padding,
            layout.list_y + layout.row_height * 1.5,
            &config,
            &registry,
            1000.0,
            800.0,
            10.0,
            20.0,
        );
        assert!(matches!(action, SettingsAction::ManageFontRangeOverrides));
    }

    #[test]
    fn font_feature_manager_action_has_shared_keyboard_pointer_and_semantic_routing() {
        let mut panel = SettingsPanel::new();
        panel.open();
        let config = Config::default();
        let registry = default_registry();
        assert!(panel.insert_filter_text("opentype features"));
        assert!(matches!(
            panel.handle_key(SettingsKey::Enter, &config, &registry),
            SettingsAction::ManageFontFeatures
        ));
        assert!(matches!(
            panel.accessibility_activate_setting(
                SettingsSection::Font,
                "OpenType Features",
                &config,
                &registry,
            ),
            SettingsAction::ManageFontFeatures
        ));

        let entries = panel.build_entries(&config, &registry);
        assert_eq!(entries.len(), 2, "filtered list contains header and action");
        let layout = SettingsModalLayout::calculate(
            1000.0,
            800.0,
            10.0,
            20.0,
            panel.animation_progress(),
            entries.len(),
        );
        let action = panel.handle_click(
            layout.panel_x + layout.padding,
            layout.list_y + layout.row_height * 1.5,
            &config,
            &registry,
            1000.0,
            800.0,
            10.0,
            20.0,
        );
        assert!(matches!(action, SettingsAction::ManageFontFeatures));
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

        // Default font rows are Size, Family, Normal Weight, Bold Weight, one
        // empty fallback slot, range overrides, OpenType features, Line
        // Height, Ligatures, and Bold Is Bright before Theme.
        panel.selected = 10;

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
        assert!(panel.insert_filter_text("opacity"));

        let entries = panel.build_entries(&config, &registry);
        // Should have Window header + Opacity item (at minimum)
        let selectable: Vec<_> = entries.iter().filter(|e| !e.is_header).collect();
        assert!(!selectable.is_empty());
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
        assert!(panel.insert_filter_text("a"));
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
    fn mouse_scroll_is_not_undone_by_render_info() {
        let mut panel = SettingsPanel::new();
        panel.open();
        panel.set_max_visible(5);

        let config = Config::default();
        let registry = default_registry();
        let total = panel.build_entries(&config, &registry).len();

        // Resolve the initial keyboard-selection visibility request, then
        // scroll the viewport independently with the mouse wheel.
        let _ = panel.build_info(&config, &registry);
        panel.scroll_down(4, total);
        assert_eq!(panel.scroll_offset, 4);

        let info = panel.build_info(&config, &registry);
        assert_eq!(info.scroll_offset, 4);
    }

    #[test]
    fn numeric_mouse_controls_use_rendered_arrow_geometry() {
        let mut panel = SettingsPanel::new();
        panel.open();

        let config = Config::default();
        let registry = default_registry();
        let entries = panel.build_entries(&config, &registry);
        let layout = SettingsModalLayout::calculate(
            1000.0,
            800.0,
            10.0,
            20.0,
            panel.animation_progress(),
            entries.len(),
        );
        // The first visible row is the Font header; Size is the next row.
        let y = layout.list_y + layout.row_height * 1.5;
        let value_char_count = entries[1].value.chars().count();

        let decrease = panel.handle_click(
            layout.numeric_decrement_x() + layout.cell_width * 0.5,
            y,
            &config,
            &registry,
            1000.0,
            800.0,
            10.0,
            20.0,
        );
        assert!(matches!(decrease, SettingsAction::SetFontSize(13.0)));

        let increase = panel.handle_click(
            layout.numeric_increment_x(value_char_count) + layout.cell_width * 0.5,
            y,
            &config,
            &registry,
            1000.0,
            800.0,
            10.0,
            20.0,
        );
        assert!(matches!(increase, SettingsAction::SetFontSize(15.0)));

        let value_click = panel.handle_click(
            layout.value_text_x() + layout.cell_width * 0.5,
            y,
            &config,
            &registry,
            1000.0,
            800.0,
            10.0,
            20.0,
        );
        assert!(matches!(value_click, SettingsAction::None));
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

    #[test]
    fn workspace_session_toggle_labels_map_to_real_actions() {
        let panel = SettingsPanel::new();
        let kind = SettingKind::Dropdown {
            options: vec!["On".into(), "Off".into()],
            selected: 0,
        };
        assert!(matches!(
            panel.map_dropdown_action(SettingsSection::Sessions, "Auto Save Workspace", &kind, 1,),
            SettingsAction::SetSessionAutoSave(false)
        ));
        assert!(matches!(
            panel.map_dropdown_action(
                SettingsSection::Sessions,
                "Auto Restore Workspace",
                &kind,
                1,
            ),
            SettingsAction::SetSessionAutoRestore(false)
        ));
    }

    #[test]
    fn accessibility_preferences_are_visible_and_map_to_typed_actions() {
        let panel = SettingsPanel::new();
        let mut config = Config::default();
        config.accessibility.reduce_motion = AccessibilityPreference::On;
        let entries = panel.build_entries(&config, &default_registry());
        let reduce_motion = entries
            .iter()
            .find(|entry| entry.label == "Reduce Motion")
            .unwrap();
        let high_contrast = entries
            .iter()
            .find(|entry| entry.label == "High Contrast")
            .unwrap();
        assert_eq!(reduce_motion.section, SettingsSection::Accessibility);
        assert_eq!(reduce_motion.value, "On");
        assert_eq!(high_contrast.value, "System");
        assert!(matches!(
            panel.map_dropdown_action(
                reduce_motion.section,
                reduce_motion.label,
                &reduce_motion.kind,
                2,
            ),
            SettingsAction::SetReduceMotion(AccessibilityPreference::Off)
        ));
        assert!(matches!(
            panel.map_dropdown_action(
                high_contrast.section,
                high_contrast.label,
                &high_contrast.kind,
                1,
            ),
            SettingsAction::SetHighContrast(AccessibilityPreference::On)
        ));
    }
}
