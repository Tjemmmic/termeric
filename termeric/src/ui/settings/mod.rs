use crate::config::theme::Theme;
use crate::config::{
    AccessibilityPreference, BackgroundImageFit, Config, SplashMode, TabBarPosition,
    WindowLevelConfig,
};
use crate::input::keybindings::ShortcutRegistry;
use crate::ui::geometry::Bounds;
use crate::ui::text_input::{TextInputEdit, edit_grapheme};
use crate::ui::text_input_viewport::TextInputViewport;

const MAX_FILTER_CHARACTERS: usize = 256;

fn valid_filter_text(text: &str) -> bool {
    text.len() <= MAX_FILTER_CHARACTERS * 4
        && text.chars().count() <= MAX_FILTER_CHARACTERS
        && !text.chars().any(crate::safe_text::breaks_single_line)
}

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

/// Geometry for the centered settings dialog — single source of truth shared
/// by the renderer, hit-testing, scroll, hover, IME, and accessibility.
///
/// A header holds the title, filter field, and close button; a sidebar lists
/// pages; the content column shows the page (or search results) as rows; a
/// footer holds the save status and Save button.
#[derive(Debug, Clone, Copy)]
pub struct SettingsModalLayout {
    /// The filter field in the header.
    pub input: Bounds,
    pub panel_x: f32,
    pub panel_y: f32,
    pub panel_width: f32,
    pub panel_height: f32,
    pub padding: f32,
    pub header_height: f32,
    pub close_button: Bounds,
    pub sidebar: Bounds,
    pub page_height: f32,
    /// Left edge and width of the rows' text column.
    pub content_x: f32,
    pub content_width: f32,
    /// Top of the page heading above the rows.
    pub content_title_y: f32,
    pub row_height: f32,
    pub scrollbar_width: f32,
    pub list_y: f32,
    pub list_height: f32,
    pub rows_that_fit: usize,
    pub visible_count: usize,
    /// Left edge of the value column.
    pub value_x: f32,
    pub footer_y: f32,
    pub footer_height: f32,
    pub save_button: Bounds,
    pub cell_width: f32,
}

impl SettingsModalLayout {
    pub fn input_cursor_at(
        &self,
        text: &str,
        cursor: usize,
        position: (f32, f32),
        cell_height: f32,
    ) -> Option<usize> {
        let (x, y) = position;
        if !self.input.contains(x, y) {
            return None;
        }
        self.input_viewport(text, cursor, cell_height)?
            .cursor_at(text, x, self.cell_width)
    }

    pub fn input_viewport(
        &self,
        text: &str,
        cursor: usize,
        cell_height: f32,
    ) -> Option<TextInputViewport> {
        TextInputViewport::calculate(
            text,
            cursor,
            self.input.x + self.cell_width * crate::ui::constants::FILTER_TEXT_INSET_CELLS,
            self.input.y + (self.input.height - cell_height) / 2.0,
            self.input.x + self.input.width - self.cell_width * 0.8,
            self.cell_width,
        )
    }

    /// Compute the dialog geometry. The dialog keeps one size while pages
    /// and results change; `entry_count` only limits `visible_count`.
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        entry_count: usize,
    ) -> Self {
        let panel_width = (cell_width * 100.0)
            .clamp(560.0, 900.0)
            .min((window_width - cell_width * 4.0).max(0.0));
        let panel_height = (window_height * 0.82)
            .clamp(320.0, 640.0)
            .min((window_height - cell_height * 2.0).max(0.0));
        let panel_x = (window_width - panel_width) / 2.0;
        let panel_y = (window_height - panel_height) * 0.4 + (1.0 - animation) * -30.0;
        let padding = cell_width * 1.5;
        let control = cell_height * 1.5;

        let header_height = cell_height * 3.0;
        let close_button = Bounds {
            x: panel_x + panel_width - padding * 0.5 - control,
            y: panel_y + (header_height - control) / 2.0,
            width: control,
            height: control,
        };
        let input_height = cell_height * 1.7;
        let input_width = (panel_width * 0.36).min(cell_width * 36.0);
        let input = Bounds {
            x: close_button.x - cell_width * 0.75 - input_width,
            y: panel_y + (header_height - input_height) / 2.0,
            width: input_width,
            height: input_height,
        };

        let footer_height = cell_height * 2.8;
        let footer_y = panel_y + panel_height - footer_height;
        let body_y = panel_y + header_height;
        let sidebar = Bounds {
            x: panel_x,
            y: body_y,
            width: (cell_width * 22.0).clamp(150.0, 220.0),
            height: (footer_y - body_y).max(0.0),
        };
        let content_x = sidebar.x + sidebar.width + padding;
        let content_width = (panel_x + panel_width - padding - content_x).max(0.0);
        let content_title_y = body_y + padding * 0.5;
        let row_height = cell_height * 2.0;
        let list_y = content_title_y + cell_height * 2.2;
        let list_space = (footer_y - padding * 0.5 - list_y).max(0.0);
        let rows_that_fit = (list_space / row_height).floor().max(1.0) as usize;
        let visible_count = entry_count.min(rows_that_fit);
        let save_height = cell_height * 1.6;
        let save_width = cell_width * 9.0;

        Self {
            input,
            panel_x,
            panel_y,
            panel_width,
            panel_height,
            padding,
            header_height,
            close_button,
            sidebar,
            page_height: cell_height * 2.0,
            content_x,
            content_width,
            content_title_y,
            row_height,
            scrollbar_width: 3.0,
            list_y,
            list_height: visible_count as f32 * row_height,
            rows_that_fit,
            visible_count,
            value_x: content_x + content_width * 0.5,
            footer_y,
            footer_height,
            save_button: Bounds {
                x: panel_x + panel_width - padding - save_width,
                y: footer_y + (footer_height - save_height) / 2.0,
                width: save_width,
                height: save_height,
            },
            cell_width,
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        Bounds::new(
            self.panel_x,
            self.panel_y,
            self.panel_width,
            self.panel_height,
        )
        .contains(x, y)
    }

    /// Sidebar row for the `index`th page.
    pub fn page_rect(&self, index: usize) -> Bounds {
        Bounds {
            x: self.sidebar.x + self.padding * 0.5,
            y: self.sidebar.y + self.padding * 0.5 + index as f32 * self.page_height,
            width: self.sidebar.width - self.padding,
            height: self.page_height,
        }
    }

    /// The sidebar page under a point.
    pub fn page_at(&self, x: f32, y: f32) -> Option<SettingsPage> {
        SettingsPage::all()
            .iter()
            .enumerate()
            .find(|(index, _)| self.page_rect(*index).contains(x, y))
            .map(|(_, page)| *page)
    }

    /// Bounds of the `row`th visible entry, spanning the content column.
    pub fn row_rect(&self, row: usize) -> Bounds {
        Bounds {
            x: self.content_x - self.padding * 0.5,
            y: self.list_y + row as f32 * self.row_height,
            width: self.content_width + self.padding,
            height: self.row_height,
        }
    }

    /// Map a point to a 0-based row within the visible window, if any.
    pub fn visible_row_at(&self, x: f32, y: f32) -> Option<usize> {
        let first = self.row_rect(0);
        if x < first.x || x > first.x + first.width {
            return None;
        }
        if y < self.list_y || y >= self.list_y + self.list_height {
            return None;
        }
        let row = ((y - self.list_y) / self.row_height) as usize;
        (row < self.visible_count).then_some(row)
    }

    /// Width of a numeric stepper's − and + buttons.
    fn stepper_button_width(&self) -> f32 {
        self.cell_width * 2.5
    }

    /// Width of the value box between a stepper's buttons.
    pub fn stepper_value_width(&self) -> f32 {
        self.cell_width * 8.0
    }

    /// Left edge of a numeric value box, between the stepper buttons.
    pub fn value_text_x(&self) -> f32 {
        self.value_x + self.stepper_button_width()
    }

    /// Left edge of the numeric decrement button.
    pub fn numeric_decrement_x(&self) -> f32 {
        self.value_x
    }

    /// Left edge of the numeric increment button.
    pub fn numeric_increment_x(&self) -> f32 {
        self.value_text_x() + self.stepper_value_width()
    }

    /// Resolve a click against the stepper's − and + buttons. Values keep a
    /// fixed box so the buttons never move as the number changes.
    pub fn numeric_adjustment_at(&self, x: f32) -> Option<i32> {
        let width = self.stepper_button_width();
        let contains = |origin: f32| x >= origin && x < origin + width;
        if contains(self.numeric_decrement_x()) {
            Some(-1)
        } else if contains(self.numeric_increment_x()) {
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
        let remaining = self.content_x + self.content_width - self.value_x;
        let dd_width = min_dd_width.max(remaining).min(self.content_width * 0.7);

        Some(DropdownRect {
            bounds: Bounds::new(self.value_x, anchor_y, dd_width, dd_height),
            option_height: option_h,
            visible_options,
            first_option,
        })
    }
}

/// Dropdown overlay rectangle derived from `SettingsModalLayout`.
#[derive(Debug, Clone, Copy)]
pub struct DropdownRect {
    pub bounds: Bounds,
    pub option_height: f32,
    pub visible_options: usize,
    /// Absolute option index displayed in the first visible row.
    pub first_option: usize,
}

impl DropdownRect {
    /// Which option index does a y-coordinate fall into, if any.
    pub fn option_at(&self, y: f32) -> Option<usize> {
        let inner_y = y - self.bounds.y - 4.0;
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
}

/// Pages in the settings sidebar. Each page shows one or more sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    Appearance,
    Font,
    Terminal,
    Keyboard,
    Agents,
    Sessions,
    Accessibility,
    Plugins,
    Experimental,
}

impl SettingsPage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Font => "Font",
            Self::Terminal => "Terminal",
            Self::Keyboard => "Keyboard",
            Self::Agents => "Agents",
            Self::Sessions => "Sessions",
            Self::Accessibility => "Accessibility",
            Self::Plugins => "Plugins",
            Self::Experimental => "Experimental",
        }
    }

    /// Sidebar order.
    pub fn all() -> &'static [SettingsPage] {
        &[
            Self::Appearance,
            Self::Font,
            Self::Terminal,
            Self::Keyboard,
            Self::Agents,
            Self::Sessions,
            Self::Accessibility,
            Self::Plugins,
            Self::Experimental,
        ]
    }

    fn index(self) -> usize {
        Self::all()
            .iter()
            .position(|page| *page == self)
            .unwrap_or(0)
    }

    /// The page `step` places after this one in sidebar order, wrapping.
    fn cycle(self, step: isize) -> Self {
        let pages = Self::all();
        let count = pages.len() as isize;
        pages[(self.index() as isize + step).rem_euclid(count) as usize]
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
    Keyboard,
    Agents,
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
            Self::Keyboard => "Keyboard",
            Self::Agents => "Agents",
            Self::Harvest => "Harvest",
            Self::Accessibility => "Accessibility",
            Self::Plugins => "Plugins",
        }
    }

    /// The sidebar page that shows this section.
    pub fn page(self) -> SettingsPage {
        match self {
            Self::Theme | Self::Window => SettingsPage::Appearance,
            Self::Font => SettingsPage::Font,
            Self::Cursor | Self::Scrollback | Self::Bell | Self::Shell => SettingsPage::Terminal,
            Self::Keyboard => SettingsPage::Keyboard,
            Self::Agents => SettingsPage::Agents,
            Self::Startup | Self::Sessions => SettingsPage::Sessions,
            Self::Accessibility => SettingsPage::Accessibility,
            Self::Plugins => SettingsPage::Plugins,
            Self::Harvest => SettingsPage::Experimental,
        }
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
    /// A number stepped within its range; `set` makes the change.
    Numeric {
        value: f32,
        min: f32,
        max: f32,
        step: f32,
        set: fn(f32) -> SettingsAction,
    },
    /// Dropdown with a list of options, the current index, and what picking
    /// an option does.
    Dropdown {
        options: Vec<String>,
        selected: usize,
        choose: Choose,
    },
    /// Predefined list of numeric values (for logarithmic-style stepping)
    SteppedList {
        values: Vec<usize>,
        selected: usize,
        set: fn(usize) -> SettingsAction,
    },
    /// A row that invokes a bounded application action instead of changing a
    /// persisted configuration value.
    Action { action: SettingsAction },
    /// Read-only display value
    ReadOnly,
}

/// How a picked dropdown option becomes an action.
#[derive(Debug, Clone, Copy)]
pub enum Choose {
    /// The option's text is the value: a theme, font family, or cursor style.
    Text(fn(String) -> SettingsAction),
    /// The options are On and Off, in that order.
    Toggle(fn(bool) -> SettingsAction),
    /// The options list a fixed set of values in order; the position picks one.
    Index(fn(usize) -> SettingsAction),
    /// One position in the font fallback chain; the "None" option removes it.
    FontFallback(usize),
    /// Offer or hide this built-in agent; the options are On and Off.
    Agent(&'static str),
}

impl Choose {
    fn action(self, options: &[String], index: usize) -> SettingsAction {
        let Some(option) = options.get(index) else {
            return SettingsAction::None;
        };
        match self {
            Self::Text(set) => set(option.clone()),
            Self::Toggle(set) => set(index == 0),
            Self::Index(set) => set(index),
            Self::FontFallback(index) => SettingsAction::SetFontFallback {
                index,
                family: (option != NO_FONT_FALLBACK).then(|| option.clone()),
            },
            Self::Agent(agent_id) => SettingsAction::SetAgentEnabled {
                agent_id: agent_id.into(),
                enabled: index == 0,
            },
        }
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

fn on_off_options() -> Vec<String> {
    vec!["On".into(), "Off".into()]
}

/// An On/Off setting.
fn toggle(label: &'static str, on: bool, set: fn(bool) -> SettingsAction) -> SettingItem {
    SettingItem {
        label,
        value: on_off(on).into(),
        kind: SettingKind::Dropdown {
            options: on_off_options(),
            selected: usize::from(!on),
            choose: Choose::Toggle(set),
        },
    }
}

/// The `index`th of `values`, as `set` turns it into an action.
fn pick<T: Copy>(values: &[T], index: usize, set: fn(T) -> SettingsAction) -> SettingsAction {
    values.get(index).copied().map_or(SettingsAction::None, set)
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
    /// Revalidated filter-only caret placement; never a configuration change.
    PlaceFilterCaret(usize),
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
    /// Offer or hide one built-in coding agent.
    SetAgentEnabled { agent_id: String, enabled: bool },
    /// Show what setting up agent status hooks would change.
    SetUpAgentStatus,
    /// Notify when an agent the person is not looking at needs them.
    SetAgentNotifications(bool),
    /// Offer agents' modes that run every action without asking.
    SetAllowBypassPermissions(bool),
    /// Where New Agent… opens agents the agent view can drive.
    SetOpenAgentsIn(crate::agents::config::OpenAgentsIn),
    /// Choose which agent supervises.
    SetSupervisorAgent(String),
    /// Choose how much the supervisor may do.
    SetSupervisorMode(crate::agents::supervisor::SupervisorMode),
    /// Follow the system or override non-essential UI transitions.
    SetReduceMotion(AccessibilityPreference),
    /// Follow the system or override higher-contrast application chrome.
    SetHighContrast(AccessibilityPreference),
    /// Open the installed third-party plugin manager.
    ManageRuntimePlugins,
    /// Leave Settings for the Keyboard Shortcuts dialog.
    OpenKeyboardShortcuts,
    /// Save current config
    SaveConfig,
    /// Reset to default config
    ResetConfig,
    /// No action
    None,
}

/// Header, footer, and sidebar controls that show pointer hover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsControl {
    Close,
    Save,
    Page(SettingsPage),
}

/// State for the centered settings panel.
pub struct SettingsPanel {
    /// Panel animation releases input immediately while retaining close motion.
    animation: super::animation::PanelAnimation,
    /// Sidebar page shown while the filter is empty. Kept across reopenings.
    page: SettingsPage,
    /// Header, footer, or sidebar control under the pointer.
    hovered_control: Option<SettingsControl>,
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
    /// Built-in agents with their installed and enabled state, supplied by
    /// the application because finding executables touches the file system.
    agent_rows: Vec<AgentSettingsRow>,
    custom_agent_summary: String,
    /// Whether agent status hooks are set up, supplied by the application.
    agent_status_summary: String,
}

/// One built-in agent row on the Agents page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSettingsRow {
    pub id: &'static str,
    pub label: &'static str,
    pub installed: bool,
    pub enabled: bool,
}

impl SettingsPanel {
    pub fn new() -> Self {
        Self {
            animation: super::animation::PanelAnimation::with_duration(
                super::animation::DEFAULT_PANEL_DURATION,
            ),
            page: SettingsPage::Appearance,
            hovered_control: None,
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
            agent_rows: Vec::new(),
            custom_agent_summary: "None".into(),
            agent_status_summary: "Not set up".into(),
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

    /// Built-in agent rows, a one-line summary of custom agents, and whether
    /// agent status hooks are set up.
    pub fn set_agents(
        &mut self,
        rows: Vec<AgentSettingsRow>,
        custom_summary: String,
        status_summary: String,
    ) {
        self.agent_rows = rows;
        self.custom_agent_summary = custom_summary;
        self.agent_status_summary = status_summary;
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
            self.hovered_item = None;
            self.hovered_control = None;
            self.animation.open();
        } else {
            self.animation.close();
        }
    }

    /// Show `page`, leaving any search. Selection starts at its first row.
    pub fn select_page(&mut self, page: SettingsPage) {
        self.page = page;
        self.filter.clear();
        self.filter_cursor = 0;
        self.selected = 0;
        self.scroll_offset = 0;
        self.dropdown_open = false;
        self.hovered_item = None;
        self.selection_visibility_pending = true;
    }

    /// Open the panel on `section`'s page with its first item selected.
    /// Does not filter — the rest of the page stays browsable.
    pub fn open_to_section(&mut self, section: SettingsSection) {
        self.page = section.page();
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
        registry: &ShortcutRegistry,
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
                        value: config.font.size,
                        set: SettingsAction::SetFontSize,
                        min: 8.0,
                        max: 72.0,
                        step: 1.0,
                    },
                },
                SettingItem {
                    label: "Family",
                    value: config.font.family.clone(),
                    kind: SettingKind::Dropdown {
                        choose: Choose::Text(SettingsAction::SetFontFamily),
                        options: font_options.clone(),
                        selected: font_selected,
                    },
                },
                SettingItem {
                    label: "Normal Weight",
                    value: config.font.weight.to_string(),
                    kind: SettingKind::Numeric {
                        value: f32::from(config.font.weight),
                        set: |weight| SettingsAction::SetFontWeight(weight as u16),
                        min: crate::config::MIN_FONT_WEIGHT as f32,
                        max: config.font.bold_weight as f32,
                        step: 25.0,
                    },
                },
                SettingItem {
                    label: "Bold Weight",
                    value: config.font.bold_weight.to_string(),
                    kind: SettingKind::Numeric {
                        value: f32::from(config.font.bold_weight),
                        set: |weight| SettingsAction::SetBoldFontWeight(weight as u16),
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
                    kind: SettingKind::Dropdown {
                        options,
                        selected,
                        choose: Choose::FontFallback(index),
                    },
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
                        value: config.font.line_height,
                        set: SettingsAction::SetLineHeight,
                        min: 0.8,
                        max: 3.0,
                        step: 0.05,
                    },
                },
                toggle(
                    "Ligatures",
                    config.font.ligatures,
                    SettingsAction::SetFontLigatures,
                ),
                toggle(
                    "Bold Is Bright",
                    config.font.bold_is_bright,
                    SettingsAction::SetBoldIsBright,
                ),
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
                kind: SettingKind::Dropdown {
                    options,
                    selected,
                    choose: Choose::Text(SettingsAction::SetTheme),
                },
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
                        value: config.window.padding as f32,
                        set: |padding| SettingsAction::SetPadding(padding as u32),
                        min: 0.0,
                        max: 64.0,
                        step: 2.0,
                    },
                },
                SettingItem {
                    label: "Opacity",
                    value: format!("{:.2}", config.window.opacity),
                    kind: SettingKind::Numeric {
                        value: config.window.opacity,
                        set: SettingsAction::SetOpacity,
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
                        value: config.window.background_image.opacity,
                        set: SettingsAction::SetBackgroundImageOpacity,
                        min: 0.0,
                        max: 1.0,
                        step: 0.05,
                    },
                },
                SettingItem {
                    label: "Background Image Fit",
                    value: config.window.background_image.fit.label().into(),
                    kind: SettingKind::Dropdown {
                        choose: Choose::Index(|index| {
                            pick(
                                BackgroundImageFit::ALL,
                                index,
                                SettingsAction::SetBackgroundImageFit,
                            )
                        }),
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
                toggle(
                    "Decorations",
                    config.window.decorations,
                    SettingsAction::SetDecorations,
                ),
                SettingItem {
                    label: "Window Level",
                    value: config.window.level.label().into(),
                    kind: SettingKind::Dropdown {
                        choose: Choose::Index(|index| {
                            pick(
                                WindowLevelConfig::ALL,
                                index,
                                SettingsAction::SetWindowLevel,
                            )
                        }),
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
                        choose: Choose::Index(|index| {
                            pick(
                                TabBarPosition::ALL,
                                index,
                                SettingsAction::SetTabBarPosition,
                            )
                        }),
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
                            choose: Choose::Text(SettingsAction::SetCursorStyle),
                            options: style_options,
                            selected: style_selected,
                        },
                    },
                    toggle("Blink", config.cursor.blink, SettingsAction::SetCursorBlink),
                    SettingItem {
                        label: "Blink Rate",
                        value: format!("{}ms", config.cursor.blink_rate),
                        kind: SettingKind::Numeric {
                            value: config.cursor.blink_rate as f32,
                            set: |rate| SettingsAction::SetCursorBlinkRate(rate as u32),
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
                toggle("Visual", config.bell.visual, SettingsAction::SetVisualBell),
                toggle("Audio", config.bell.audio, SettingsAction::SetAudioBell),
                SettingItem {
                    label: "Flash Duration",
                    value: format!("{}ms", config.bell.visual_duration_ms),
                    kind: SettingKind::Numeric {
                        value: config.bell.visual_duration_ms as f32,
                        set: |duration| SettingsAction::SetBellDuration(duration as u64),
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
                            set: SettingsAction::SetScrollback,
                            values: steps,
                            selected,
                        },
                    },
                    SettingItem {
                        label: "Scroll Speed",
                        value: format!("{}", config.scrollback.scroll_speed as usize),
                        kind: SettingKind::SteppedList {
                            set: |speed| SettingsAction::SetScrollSpeed(speed as f64),
                            values: speed_steps,
                            selected: speed_selected,
                        },
                    },
                    SettingItem {
                        label: "Memory Limit",
                        value: format!("{} MiB", config.scrollback.max_memory_mib),
                        kind: SettingKind::SteppedList {
                            set: SettingsAction::SetScrollbackMemory,
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
                        choose: Choose::Index(|index| {
                            pick(SplashMode::ALL, index, SettingsAction::SetSplashMode)
                        }),
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

        // Keyboard: the shortcuts editor is its own dialog.
        {
            let customized = registry.user_overrides().len();
            result.push((
                SettingsSection::Keyboard,
                vec![SettingItem {
                    label: "Keyboard Shortcuts",
                    value: match customized {
                        0 => "Defaults".to_string(),
                        1 => "1 changed".to_string(),
                        count => format!("{count} changed"),
                    },
                    kind: SettingKind::Action {
                        action: SettingsAction::OpenKeyboardShortcuts,
                    },
                }],
            ));
        }

        // Sessions
        {
            result.push((
                SettingsSection::Sessions,
                vec![
                    toggle(
                        "Auto Save Workspace",
                        config.session.auto_save,
                        SettingsAction::SetSessionAutoSave,
                    ),
                    toggle(
                        "Auto Restore Workspace",
                        config.session.auto_restore,
                        SettingsAction::SetSessionAutoRestore,
                    ),
                    toggle(
                        "Save Scrollback",
                        config.session.save_scrollback,
                        SettingsAction::SetSessionSaveScrollback,
                    ),
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
                    toggle(
                        "Enabled",
                        config.harvest.enabled,
                        SettingsAction::SetHarvestEnabled,
                    ),
                    toggle(
                        "Growth Indicators",
                        config.harvest.show_indicators,
                        SettingsAction::SetHarvestIndicators,
                    ),
                    SettingItem {
                        label: "History Limit",
                        value: config.harvest.max_history.to_string(),
                        kind: SettingKind::Numeric {
                            value: config.harvest.max_history as f32,
                            set: |limit| SettingsAction::SetHarvestMaxHistory(limit as usize),
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
                        choose: Choose::Index(|index| {
                            pick(
                                &AccessibilityPreference::ALL,
                                index,
                                SettingsAction::SetReduceMotion,
                            )
                        }),
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
                        choose: Choose::Index(|index| {
                            pick(
                                &AccessibilityPreference::ALL,
                                index,
                                SettingsAction::SetHighContrast,
                            )
                        }),
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

        // Agents: built-in agents by name, then a summary of custom ones.
        {
            let mut items: Vec<SettingItem> = self
                .agent_rows
                .iter()
                .map(|row| {
                    if !row.installed {
                        return SettingItem {
                            label: row.label,
                            value: "Not installed".into(),
                            kind: SettingKind::ReadOnly,
                        };
                    }
                    SettingItem {
                        label: row.label,
                        value: on_off(row.enabled).into(),
                        kind: SettingKind::Dropdown {
                            options: on_off_options(),
                            selected: usize::from(!row.enabled),
                            choose: Choose::Agent(row.id),
                        },
                    }
                })
                .collect();
            items.push(SettingItem {
                label: "Custom Agents",
                value: self.custom_agent_summary.clone(),
                kind: SettingKind::ReadOnly,
            });
            items.push(SettingItem {
                label: "Agent Status",
                value: self.agent_status_summary.clone(),
                kind: SettingKind::Action {
                    action: SettingsAction::SetUpAgentStatus,
                },
            });
            let supervisor_agents = ["Claude Code", "Codex"];
            let supervisor_index = crate::agents::supervisor::SUPERVISOR_AGENTS
                .iter()
                .position(|id| *id == config.supervisor.agent)
                .unwrap_or(0);
            items.push(SettingItem {
                label: "Supervisor",
                value: supervisor_agents[supervisor_index].into(),
                kind: SettingKind::Dropdown {
                    choose: Choose::Index(|index| {
                        pick(
                            &crate::agents::supervisor::SUPERVISOR_AGENTS,
                            index,
                            |agent| SettingsAction::SetSupervisorAgent(agent.into()),
                        )
                    }),
                    options: supervisor_agents
                        .iter()
                        .map(|name| (*name).into())
                        .collect(),
                    selected: supervisor_index,
                },
            });
            let modes = crate::agents::supervisor::SupervisorMode::ALL;
            items.push(SettingItem {
                label: "Supervisor Mode",
                value: config.supervisor.mode.label().into(),
                kind: SettingKind::Dropdown {
                    choose: Choose::Index(|index| {
                        pick(
                            &crate::agents::supervisor::SupervisorMode::ALL,
                            index,
                            SettingsAction::SetSupervisorMode,
                        )
                    }),
                    options: modes.iter().map(|mode| mode.label().into()).collect(),
                    selected: modes
                        .iter()
                        .position(|mode| *mode == config.supervisor.mode)
                        .unwrap_or(0),
                },
            });
            let places = crate::agents::config::OpenAgentsIn::ALL;
            items.push(SettingItem {
                label: "Open Agents In",
                value: config.agent_view.open_agents_in.label().into(),
                kind: SettingKind::Dropdown {
                    choose: Choose::Index(|index| {
                        pick(
                            &crate::agents::config::OpenAgentsIn::ALL,
                            index,
                            SettingsAction::SetOpenAgentsIn,
                        )
                    }),
                    options: places.iter().map(|place| place.label().into()).collect(),
                    selected: places
                        .iter()
                        .position(|place| *place == config.agent_view.open_agents_in)
                        .unwrap_or(0),
                },
            });
            let bypass = config.agent_view.allow_bypass_permissions;
            items.push(toggle(
                "Allow Bypass Permissions",
                bypass,
                SettingsAction::SetAllowBypassPermissions,
            ));
            items.push(toggle(
                "Notify When Agents Need You",
                config.agent_status.notify,
                SettingsAction::SetAgentNotifications,
            ));
            result.push((SettingsSection::Agents, items));
        }

        // Plugins
        {
            result.push((
                SettingsSection::Plugins,
                vec![
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

    /// Rows to show: the current page's settings, or while searching every
    /// page's matches. Section headings appear when a list spans sections.
    pub fn build_entries(&self, config: &Config, registry: &ShortcutRegistry) -> Vec<SettingEntry> {
        let all = self.all_items(config, registry);
        let filter_lower = self.filter.to_lowercase();
        let has_filter = !filter_lower.is_empty();
        // Without a search only the current page shows, and a page made of
        // one section needs no heading beneath the page title.
        let sections_on_page = all
            .iter()
            .filter(|(section, _)| section.page() == self.page)
            .count();
        let show_headers = has_filter || sections_on_page > 1;

        let mut entries = Vec::new();
        for (section, items) in &all {
            if !has_filter && section.page() != self.page {
                continue;
            }
            // Collect matching items in this section
            let matching: Vec<(usize, &SettingItem)> = items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    if !has_filter {
                        return true;
                    }
                    // Match against label, value, section, or page name
                    item.label.to_lowercase().contains(&filter_lower)
                        || item.value.to_lowercase().contains(&filter_lower)
                        || section.label().to_lowercase().contains(&filter_lower)
                        || section
                            .page()
                            .label()
                            .to_lowercase()
                            .contains(&filter_lower)
                })
                .collect();

            if matching.is_empty() {
                continue;
            }

            if show_headers {
                entries.push(SettingEntry {
                    label: section.label(),
                    value: String::new(),
                    kind: SettingKind::ReadOnly,
                    is_header: true,
                    section: *section,
                });
            }

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
            SettingsKey::Tab | SettingsKey::BackTab => {
                let step = if matches!(key, SettingsKey::Tab) {
                    1
                } else {
                    -1
                };
                self.select_page(self.shown_page(&entries).cycle(step));
                // The page's own rows replace the ones built above.
                return SettingsAction::None;
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
            || !valid_filter_text(text)
            || self.filter.chars().count() + text.chars().count() > MAX_FILTER_CHARACTERS
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

    /// Replace the filter atomically using the same policy as keyboard/IME/paste.
    /// Empty replacement clears the query; the accepted caret ends at the text.
    pub fn accessibility_set_filter(&mut self, text: &str) -> bool {
        if !self.is_active() || self.dropdown_open || !valid_filter_text(text) {
            return false;
        }
        self.filter.clear();
        self.filter.push_str(text);
        self.filter_cursor = text.len();
        self.selected = 0;
        self.scroll_offset = 0;
        self.selection_visibility_pending = false;
        true
    }

    pub fn backspace_filter(&mut self) -> bool {
        self.edit_filter(TextInputEdit::Backspace)
    }

    /// Edit without rebuilding the inventory or resetting results on caret moves.
    /// Hidden and dropdown-owned fields refuse both text and caret changes.
    pub fn edit_filter(&mut self, edit: TextInputEdit) -> bool {
        if !self.is_active() || self.dropdown_open {
            return false;
        }
        let previous_cursor = self.filter_cursor;
        let changed = edit_grapheme(&mut self.filter, &mut self.filter_cursor, edit);
        if changed {
            self.selected = 0;
            self.scroll_offset = 0;
            self.selection_visibility_pending = false;
        }
        changed || self.filter_cursor != previous_cursor
    }

    /// Reject hidden/dropdown owners and non-grapheme boundaries before mutation.
    pub fn place_filter_caret(&mut self, cursor: usize) -> bool {
        use unicode_segmentation::UnicodeSegmentation;
        if !self.is_active()
            || self.dropdown_open
            || (cursor != self.filter.len()
                && !self
                    .filter
                    .grapheme_indices(true)
                    .any(|(offset, _)| offset == cursor))
        {
            return false;
        }
        self.filter_cursor = cursor;
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
        self.scroll_offset = if desired_top < self.scroll_offset {
            desired_top
        } else {
            crate::ui::scroll::reveal_row(self.scroll_offset, abs, self.max_visible)
        };
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
                Self::option_action(&entry_kind, self.dropdown_highlight)
            }
            _ => SettingsAction::None,
        }
    }

    /// Map a dropdown option selection to a SettingsAction
    /// The action picking option `index` of a dropdown or stepped list takes.
    fn option_action(kind: &SettingKind, index: usize) -> SettingsAction {
        match kind {
            SettingKind::Dropdown {
                options, choose, ..
            } => choose.action(options, index),
            SettingKind::SteppedList { values, set, .. } => values
                .get(index)
                .copied()
                .map_or(SettingsAction::None, *set),
            _ => SettingsAction::None,
        }
    }

    /// Adjust the currently selected value via ←/→ using pre-built entries.
    fn adjust_selected_value(&self, entries: &[SettingEntry], direction: i32) -> SettingsAction {
        let Some(entry) = Self::nth_selectable(entries, self.selected) else {
            return SettingsAction::None;
        };

        match &entry.kind {
            SettingKind::Numeric {
                value,
                min,
                max,
                step,
                set,
            } => {
                // Settle the float noise repeated steps leave (0.05 has no
                // exact binary form) at a thousandth, finer than any step.
                let stepped = ((value + step * direction as f32) * 1000.0).round() / 1000.0;
                set(stepped.clamp(*min, *max))
            }
            SettingKind::Dropdown {
                options, selected, ..
            } => {
                if options.is_empty() {
                    return SettingsAction::None;
                }
                let new_idx = if direction > 0 {
                    (selected + 1) % options.len()
                } else {
                    selected.checked_sub(1).unwrap_or(options.len() - 1)
                };
                Self::option_action(&entry.kind, new_idx)
            }
            SettingKind::SteppedList {
                values, selected, ..
            } => {
                if values.is_empty() {
                    return SettingsAction::None;
                }
                let new_idx = if direction > 0 {
                    (*selected + 1).min(values.len() - 1)
                } else {
                    selected.saturating_sub(1)
                };
                Self::option_action(&entry.kind, new_idx)
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
            SettingKind::Dropdown {
                selected, options, ..
            } if !options.is_empty() => *selected,
            SettingKind::SteppedList {
                selected, values, ..
            } if !values.is_empty() => *selected,
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
        let action = Self::option_action(&entry.kind, option_idx);
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
                if rect.bounds.contains(x, y) {
                    if let Some(opt_idx) = rect.option_at(y) {
                        self.dropdown_open = false;
                        if opt_idx < option_count {
                            if let Some(entry) = Self::nth_selectable(&entries, self.selected) {
                                return Self::option_action(&entry.kind, opt_idx);
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

        if layout.close_button.contains(x, y) {
            self.close();
            return SettingsAction::None;
        }
        if layout.save_button.contains(x, y) {
            return SettingsAction::SaveConfig;
        }
        if let Some(page) = layout.page_at(x, y) {
            self.select_page(page);
            return SettingsAction::None;
        }

        // Input padding maps to visible slice edges. A collapsed input still
        // consumes the click, and dropdown dismissal above never moves a caret.
        if layout.input.contains(x, y) {
            return layout
                .input_cursor_at(&self.filter, self.filter_cursor, (x, y), cell_height)
                .map_or(SettingsAction::None, SettingsAction::PlaceFilterCaret);
        }

        // Click on an entry row (respecting scroll).
        let Some(visible_row) = layout.visible_row_at(x, y) else {
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
            SettingKind::Numeric { .. } => layout
                .numeric_adjustment_at(x)
                .map_or(SettingsAction::None, |direction| {
                    self.adjust_selected_value(&entries, direction)
                }),
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

    /// Update header, footer, and sidebar hover from the pointer.
    pub fn set_control_hover(&mut self, control: Option<SettingsControl>) {
        self.hovered_control = control;
    }

    /// The page the sidebar marks: the browsed page, or while searching the
    /// page of the selected result.
    fn shown_page(&self, entries: &[SettingEntry]) -> SettingsPage {
        if self.filter.is_empty() {
            return self.page;
        }
        Self::nth_selectable(entries, self.selected).map_or(self.page, |entry| entry.section.page())
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
    /// Next sidebar page.
    Tab,
    /// Previous sidebar page.
    BackTab,
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
    /// Page marked in the sidebar.
    pub page: SettingsPage,
    /// Header, footer, or sidebar control under the pointer.
    pub hovered_control: Option<SettingsControl>,
    /// Whether the live configuration differs from the saved file. The
    /// application fills this in; the panel cannot see the file.
    pub unsaved: bool,
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
            selected: self.selected,
            scroll_offset: self.scroll_offset,
            dropdown_open: self.dropdown_open,
            dropdown_highlight: self.dropdown_highlight,
            hovered_item: self.hovered_item,
            dropdown_hovered: self.dropdown_hovered,
            filter: self.filter.clone(),
            filter_cursor: self.filter_cursor,
            page: self.shown_page(&entries),
            hovered_control: self.hovered_control,
            unsaved: false,
            entries,
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
    fn semantic_filter_replacement_is_atomic_bounded_and_owner_checked() {
        let mut panel = SettingsPanel::new();
        assert!(!panel.accessibility_set_filter("hidden"));
        panel.open();
        assert!(panel.accessibility_set_filter(&"🫚".repeat(256)));
        assert_eq!((panel.filter.len(), panel.filter_cursor), (1024, 1024));
        panel.selected = 3;
        panel.scroll_offset = 2;
        panel.selection_visibility_pending = true;
        for value in [
            "\n",
            "\r",
            "\t",
            "\0",
            "\u{7f}",
            "\u{85}",
            "\u{2028}",
            "\u{2029}",
            &"x".repeat(257),
            &"🫚".repeat(257),
        ] {
            assert!(!panel.accessibility_set_filter(value));
            assert_eq!(panel.filter, "🫚".repeat(256));
            assert_eq!(panel.filter_cursor, 1024);
            assert_eq!((panel.selected, panel.scroll_offset), (3, 2));
            assert!(panel.selection_visibility_pending);
        }
        panel.dropdown_open = true;
        assert!(!panel.accessibility_set_filter(""));
        assert!(!panel.accessibility_set_filter("size"));
        assert_eq!(panel.filter_cursor, 1024);
        panel.dropdown_open = false;
        assert!(panel.accessibility_set_filter("e\u{301}👩\u{200d}💻"));
        assert_eq!((panel.selected, panel.scroll_offset), (0, 0));
        assert!(!panel.selection_visibility_pending);
        assert_eq!(panel.filter_cursor, panel.filter.len());
        assert!(panel.accessibility_set_filter(""));
        assert!(panel.filter.is_empty());
        assert_eq!(panel.filter_cursor, 0);
        panel.close();
        assert!(!panel.accessibility_set_filter("closed"));
    }

    #[test]
    fn filter_edits_preserve_results_on_caret_moves_and_refuse_inactive_owners() {
        let mut panel = SettingsPanel::new();
        assert!(!panel.edit_filter(TextInputEdit::Home));
        panel.open();
        assert!(panel.insert_filter_text("e\u{301}界👩\u{200d}💻"));
        panel.selected = 3;
        panel.scroll_offset = 2;
        panel.selection_visibility_pending = true;
        assert!(panel.edit_filter(TextInputEdit::Home));
        assert_eq!(panel.filter_cursor, 0);
        assert_eq!((panel.selected, panel.scroll_offset), (3, 2));
        assert!(panel.selection_visibility_pending);
        assert!(!panel.edit_filter(TextInputEdit::Home));
        for expected in ["界👩\u{200d}💻", "👩\u{200d}💻", ""] {
            assert!(panel.edit_filter(TextInputEdit::Delete));
            assert_eq!(panel.filter, expected);
            assert_eq!(panel.filter_cursor, 0);
            assert_eq!((panel.selected, panel.scroll_offset), (0, 0));
            assert!(!panel.selection_visibility_pending);
        }
        assert!(!panel.edit_filter(TextInputEdit::Delete));
        assert!(panel.insert_filter_text("draft size"));
        panel.dropdown_open = true;
        for edit in [
            TextInputEdit::Home,
            TextInputEdit::WordLeft,
            TextInputEdit::BackspaceWord,
        ] {
            assert!(!panel.edit_filter(edit));
        }
        assert_eq!((&*panel.filter, panel.filter_cursor), ("draft size", 10));
        panel.dropdown_open = false;
        panel.close();
        assert!(!panel.edit_filter(TextInputEdit::Home));
        assert!(!panel.edit_filter(TextInputEdit::BackspaceWord));
        assert_eq!((&*panel.filter, panel.filter_cursor), ("draft size", 10));
    }

    #[test]
    fn filter_pointer_geometry_uses_visible_grapheme_halves_and_padding_edges() {
        use unicode_segmentation::UnicodeSegmentation;
        for (width, cw, ch) in [(1000.0, 8.0, 16.0), (500.0, 15.0, 30.0)] {
            for animation in [0.2, 0.7, 1.0] {
                let layout = SettingsModalLayout::calculate(width, 700.0, cw, ch, animation, 8);
                for text in [
                    "e\u{301}界👩\u{200d}💻🇺🇸".to_string(),
                    "a界e\u{301}👩\u{200d}💻".repeat(20),
                ] {
                    let cursor = text.len();
                    let view = layout.input_viewport(&text, cursor, ch).unwrap();
                    let mut x = view.text_x;
                    for (offset, grapheme) in text[view.text_range.clone()].grapheme_indices(true) {
                        let pixels = termeric_core::unicode::grapheme_width(grapheme) as f32 * cw;
                        for (fraction, expected) in [
                            (0.25, view.text_range.start + offset),
                            (0.75, view.text_range.start + offset + grapheme.len()),
                        ] {
                            assert_eq!(
                                layout.input_cursor_at(
                                    &text,
                                    cursor,
                                    (x + fraction * pixels, view.text_y),
                                    ch
                                ),
                                Some(expected)
                            );
                        }
                        x += pixels;
                    }
                    assert_eq!(
                        layout.input_cursor_at(&text, cursor, (layout.input.x, view.text_y), ch),
                        Some(view.text_range.start)
                    );
                    assert_eq!(
                        layout.input_cursor_at(
                            &text,
                            cursor,
                            (layout.input.right() - 0.5, view.text_y),
                            ch
                        ),
                        Some(view.text_range.end)
                    );
                    for position in [
                        (view.text_x, layout.input.y - 1.0),
                        (f32::NAN, view.text_y),
                        (view.text_x, f32::INFINITY),
                    ] {
                        assert_eq!(layout.input_cursor_at(&text, cursor, position, ch), None);
                    }
                }
            }
        }
    }

    #[test]
    fn filter_pointer_action_preserves_results_and_dropdown_dismissal_never_moves_caret() {
        let mut panel = SettingsPanel::new();
        let config = Config::default();
        let registry = default_registry();
        panel.set_reduced_motion(true);
        assert!(!panel.place_filter_caret(0));
        panel.open();
        let text = "e\u{301}界👩\u{200d}💻";
        assert!(panel.insert_filter_text(text));
        panel.selected = 2;
        panel.scroll_offset = 1;
        let layout = SettingsModalLayout::calculate(1000.0, 700.0, 8.0, 16.0, 1.0, 0);
        let (x, y) = (
            layout.input.x + crate::ui::constants::FILTER_TEXT_INSET_CELLS * 8.0 + 6.0,
            layout.input.y,
        );
        let action = panel.handle_click(x, y, &config, &registry, 1000.0, 700.0, 8.0, 16.0);
        let SettingsAction::PlaceFilterCaret(cursor) = action else {
            panic!("expected filter caret");
        };
        assert_eq!(cursor, "e\u{301}".len());
        assert_eq!(
            panel.filter_cursor,
            text.len(),
            "hit test only proposes an action"
        );
        assert!(panel.place_filter_caret(cursor));
        assert_eq!((panel.selected, panel.scroll_offset), (2, 1));
        for invalid in [1, 2, 4, text.len() + 1] {
            assert!(!panel.place_filter_caret(invalid));
            assert_eq!(panel.filter_cursor, cursor);
        }
        panel.dropdown_open = true;
        assert!(!panel.place_filter_caret(0));
        assert!(matches!(
            panel.handle_click(x, y, &config, &registry, 1000.0, 700.0, 8.0, 16.0),
            SettingsAction::None
        ));
        assert!(!panel.dropdown_open);
        assert_eq!(panel.filter_cursor, cursor);
        assert!(panel.insert_filter_text("@"));
        assert_eq!(panel.filter, "e\u{301}@界👩\u{200d}💻");
        let collapsed = SettingsModalLayout::calculate(60.0, 700.0, 8.0, 16.0, 1.0, 0);
        assert!(matches!(
            panel.handle_click(
                collapsed.input.x,
                collapsed.input.y,
                &config,
                &registry,
                60.0,
                700.0,
                8.0,
                16.0
            ),
            SettingsAction::None
        ));
        assert_eq!(panel.filter_cursor, cursor + 1);
        panel.close();
        assert!(!panel.place_filter_caret(0));
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
        panel.select_page(SettingsPage::Font);

        // Font size is the Font page's first entry (selected = 0), default 12.0.
        // Right on a Numeric adjusts (Right on a Dropdown would open it).
        let action = panel.handle_key(SettingsKey::Right, &config, &registry);
        match action {
            SettingsAction::SetFontSize(size) => assert_eq!(size, 13.0),
            _ => panic!("Expected SetFontSize"),
        }

        let action = panel.handle_key(SettingsKey::Left, &config, &registry);
        match action {
            SettingsAction::SetFontSize(size) => assert_eq!(size, 11.0),
            _ => panic!("Expected SetFontSize"),
        }
    }

    #[test]
    fn scrollback_memory_budget_is_visible_and_adjustable() {
        let mut panel = SettingsPanel::new();
        panel.select_page(SettingsPage::Terminal);
        let mut config = Config::default();
        config.scrollback.max_memory_mib = 64;
        let entries = panel.build_entries(&config, &default_registry());
        let memory = entries
            .iter()
            .find(|entry| entry.label == "Memory Limit")
            .expect("scrollback memory setting");
        assert_eq!(memory.value, "64 MiB");
        assert!(matches!(
            SettingsPanel::option_action(&memory.kind, 3),
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
        panel.select_page(SettingsPage::Font);
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
            SettingsPanel::option_action(&first.kind, 0),
            SettingsAction::SetFontFallback {
                index: 0,
                family: None
            }
        ));

        let kind = |label: &str| {
            &entries
                .iter()
                .find(|entry| entry.label == label)
                .expect("font toggle row")
                .kind
        };
        assert!(matches!(
            SettingsPanel::option_action(kind("Ligatures"), 1),
            SettingsAction::SetFontLigatures(false)
        ));
        assert!(matches!(
            SettingsPanel::option_action(kind("Bold Is Bright"), 1),
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
    fn numeric_steps_start_from_the_configured_value_not_its_display() {
        let step = |config: &Config, page, label: &str, direction| {
            let mut panel = SettingsPanel::new();
            panel.select_page(page);
            let entries = panel.build_entries(config, &default_registry());
            panel.selected = entries
                .iter()
                .filter(|entry| !entry.is_header)
                .position(|entry| entry.label == label)
                .expect("numeric row");
            panel.adjust_selected_value(&entries, direction)
        };
        let mut config = Config::default();
        // Shown as "0.33", but the step is from 0.333.
        config.window.opacity = 0.333;
        assert!(matches!(
            step(&config, SettingsPage::Appearance, "Opacity", -1),
            SettingsAction::SetOpacity(value) if value == 0.283
        ));
        // Repeated binary steps don't drift.
        config.font.line_height = 1.1;
        assert!(matches!(
            step(&config, SettingsPage::Font, "Line Height", 1),
            SettingsAction::SetLineHeight(value) if value == 1.15
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
        assert_eq!(rect.option_at(rect.bounds.y + 5.0), Some(5));
        assert_eq!(
            rect.option_at(rect.bounds.y + 4.0 + rect.option_height * 7.5),
            Some(12)
        );

        let last = layout
            .dropdown_rect(1, 0, 20, 12, 19)
            .expect("visible dropdown");
        assert_eq!(last.first_option, 12);
        assert_eq!(last.option_at(last.bounds.y + 5.0), Some(12));
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
            SettingsPanel::option_action(&level.kind, 2),
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
            SettingsPanel::option_action(&position.kind, 0),
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
            SettingsPanel::option_action(&fit.kind, 1),
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
        panel.select_page(SettingsPage::Plugins);
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
        let row = layout.row_rect(1);
        let action = panel.handle_click(
            row.x + row.width * 0.25,
            row.y + row.height / 2.0,
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
        let row = layout.row_rect(1);
        let action = panel.handle_click(
            row.x + row.width * 0.25,
            row.y + row.height / 2.0,
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
        let row = layout.row_rect(1);
        let action = panel.handle_click(
            row.x + row.width * 0.25,
            row.y + row.height / 2.0,
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

        // Appearance opens first, and Theme is its first row.
        assert_eq!(
            panel.build_entries(&config, &registry)[1].label,
            "Theme",
            "below the Theme heading"
        );
        panel.selected = 0;

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
    fn pages_scope_the_list_and_search_spans_every_page() {
        let mut panel = SettingsPanel::new();
        panel.open();
        let config = Config::default();
        let registry = default_registry();
        let sections = |panel: &SettingsPanel| {
            let mut sections: Vec<_> = panel
                .build_entries(&config, &registry)
                .iter()
                .map(|entry| entry.section)
                .collect();
            sections.dedup();
            sections
        };
        assert_eq!(
            sections(&panel),
            [SettingsSection::Theme, SettingsSection::Window]
        );

        panel.handle_key(SettingsKey::Tab, &config, &registry);
        assert_eq!(sections(&panel), [SettingsSection::Font]);
        panel.handle_key(SettingsKey::BackTab, &config, &registry);
        panel.handle_key(SettingsKey::BackTab, &config, &registry);
        assert_eq!(sections(&panel), [SettingsSection::Harvest], "wraps");

        // Search reaches settings on every page and marks the result's page.
        assert!(panel.insert_filter_text("blink"));
        let info = panel.build_info(&config, &registry);
        assert!(
            info.entries
                .iter()
                .all(|entry| entry.section == SettingsSection::Cursor
                    || entry.label.to_lowercase().contains("blink")
                    || entry.value.to_lowercase().contains("blink"))
        );
        assert_eq!(info.page, SettingsPage::Terminal);
        assert!(panel.accessibility_set_filter("appearance"));
        let found = sections(&panel);
        assert!(
            found.contains(&SettingsSection::Theme) && found.contains(&SettingsSection::Window)
        );

        // Choosing a page leaves the search.
        panel.select_page(SettingsPage::Keyboard);
        let entries = panel.build_entries(&config, &registry);
        assert_eq!(panel.build_info(&config, &registry).filter, "");
        assert!(matches!(
            entries[0].kind,
            SettingKind::Action {
                action: SettingsAction::OpenKeyboardShortcuts
            }
        ));
    }

    #[test]
    fn sidebar_close_and_save_are_hit_in_the_shared_layout() {
        let mut panel = SettingsPanel::new();
        panel.open();
        let config = Config::default();
        let registry = default_registry();
        let layout = SettingsModalLayout::calculate(
            1000.0,
            800.0,
            10.0,
            20.0,
            panel.animation_progress(),
            0,
        );
        let centre = |rect: Bounds| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let click = |panel: &mut SettingsPanel, (x, y): (f32, f32)| {
            panel.handle_click(x, y, &config, &registry, 1000.0, 800.0, 10.0, 20.0)
        };

        let plugins = SettingsPage::all()
            .iter()
            .position(|page| *page == SettingsPage::Plugins)
            .unwrap();
        assert!(matches!(
            click(&mut panel, centre(layout.page_rect(plugins))),
            SettingsAction::None
        ));
        assert_eq!(
            panel.build_info(&config, &registry).page,
            SettingsPage::Plugins
        );
        assert!(matches!(
            click(&mut panel, centre(layout.save_button)),
            SettingsAction::SaveConfig
        ));
        // Rows never extend into the sidebar.
        let sidebar_point = centre(layout.page_rect(0));
        assert_eq!(
            layout.visible_row_at(sidebar_point.0, layout.list_y + 1.0),
            None
        );
        assert!(layout.sidebar.x + layout.sidebar.width < layout.row_rect(0).x);
        click(&mut panel, centre(layout.close_button));
        assert!(!panel.is_active());
    }

    #[test]
    fn test_category_headers_not_selectable() {
        let mut panel = SettingsPanel::new();
        panel.open();
        thread::sleep(Duration::from_millis(250));
        panel.update_animation();

        let config = Config::default();
        let registry = default_registry();

        // Appearance holds two sections, so each gets a heading; Font is one
        // section under its page title and needs none.
        let entries = panel.build_entries(&config, &registry);
        let headers: Vec<_> = entries.iter().filter(|e| e.is_header).collect();
        assert_eq!(headers.len(), 2);
        assert!(entries.iter().filter(|e| !e.is_header).count() > headers.len());
        panel.handle_key(SettingsKey::Down, &config, &registry);
        let second = SettingsPanel::nth_selectable(&entries, panel.selected).unwrap();
        assert!(!second.is_header);
        panel.select_page(SettingsPage::Font);
        assert!(
            !panel
                .build_entries(&config, &registry)
                .iter()
                .any(|e| e.is_header)
        );
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
        panel.select_page(SettingsPage::Font);

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
        // The Font page is one section, so Size is the first row.
        assert_eq!(entries[0].label, "Size");
        let y = layout.list_y + layout.row_height * 0.5;

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
        assert!(matches!(decrease, SettingsAction::SetFontSize(11.0)));

        let increase = panel.handle_click(
            layout.numeric_increment_x() + layout.cell_width * 0.5,
            y,
            &config,
            &registry,
            1000.0,
            800.0,
            10.0,
            20.0,
        );
        assert!(matches!(increase, SettingsAction::SetFontSize(13.0)));

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

        // Filter must be empty — the rest of the page stays browsable.
        assert!(panel.filter.is_empty());

        // build_info resolves the pending jump.
        let config = Config::default();
        let registry = default_registry();
        let info = panel.build_info(&config, &registry);

        let selected_entry = SettingsPanel::nth_selectable(&info.entries, info.selected).unwrap();
        assert_eq!(selected_entry.section, SettingsSection::Sessions);
        assert_eq!(info.page, SettingsPage::Sessions);
    }

    #[test]
    fn workspace_session_toggles_map_to_real_actions() {
        let mut panel = SettingsPanel::new();
        panel.select_page(SettingsPage::Sessions);
        let entries = panel.build_entries(&Config::default(), &default_registry());
        let kind = |label: &str| {
            &entries
                .iter()
                .find(|entry| entry.label == label)
                .expect("session toggle row")
                .kind
        };
        assert!(matches!(
            SettingsPanel::option_action(kind("Auto Save Workspace"), 1),
            SettingsAction::SetSessionAutoSave(false)
        ));
        assert!(matches!(
            SettingsPanel::option_action(kind("Auto Restore Workspace"), 1),
            SettingsAction::SetSessionAutoRestore(false)
        ));
    }

    #[test]
    fn accessibility_preferences_are_visible_and_map_to_typed_actions() {
        let mut panel = SettingsPanel::new();
        panel.select_page(SettingsPage::Accessibility);
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
            SettingsPanel::option_action(&reduce_motion.kind, 2),
            SettingsAction::SetReduceMotion(AccessibilityPreference::Off)
        ));
        assert!(matches!(
            SettingsPanel::option_action(&high_contrast.kind, 1),
            SettingsAction::SetHighContrast(AccessibilityPreference::On)
        ));
    }
}
