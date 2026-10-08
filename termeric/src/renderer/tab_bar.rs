//! Tab bar rendering and hit-testing

use super::Renderer;
use super::glyph_cache::UiFontMetrics;
use super::icons::draw_icon;
use super::types::*;
use crate::agents::AgentBadge;
use crate::tabs::{TabInfo, TabType};
use crate::ui::color::{high_contrast_text, mix, scale_alpha, with_alpha};
use crate::ui::constants::BUTTON_CORNER_RADIUS;
use crate::ui::geometry::Bounds;
use crate::ui::icon::Icon;

/// Logical tab widths before display scaling.
const MIN_TAB_WIDTH: f32 = 96.0;
const MAX_TAB_WIDTH: f32 = 240.0;
/// Logical gap before the first tab when the strip does not overflow.
const TAB_STRIP_INSET: f32 = 4.0;
const PREFERRED_VERTICAL_TAB_WIDTH: f32 = 220.0;
const MIN_VERTICAL_TAB_WIDTH: f32 = 160.0;
const MIN_TERMINAL_CONTENT_WIDTH: f32 = 160.0;

#[derive(Debug, Default)]
pub(super) struct TabBarViewportAnchor(Option<usize>);

impl TabBarViewportAnchor {
    pub(super) fn set_focus(&mut self, focus: Option<TabBarClick>) {
        match focus {
            Some(TabBarClick::Tab(index) | TabBarClick::CloseTab(index)) => self.0 = Some(index),
            None => self.0 = None,
            // Keep the last tab viewport while traversing chrome controls.
            // Falling back to the active tab can remove the focused overflow
            // arrow from both the layout and the keyboard focus sequence.
            Some(_) => {}
        }
    }

    fn resolve(&self, tabs: &[TabInfo]) -> usize {
        self.0
            .unwrap_or_else(|| tabs.iter().position(|tab| tab.is_active).unwrap_or(0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TabSlot {
    index: usize,
    x: f32,
    width: f32,
}

/// A top-strip overflow arrow. Its slot stays while the strip overflows, so
/// the strip never jumps; it is enabled while tabs are hidden on its side.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OverflowArrow {
    x: f32,
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct TabBarLayout {
    tabs: Vec<TabSlot>,
    add_x: f32,
    previous: Option<OverflowArrow>,
    next: Option<OverflowArrow>,
}

impl TabBarLayout {
    /// Where an overflow arrow is drawn, if the strip overflows.
    fn arrow(&self, target: TabBarClick) -> Option<OverflowArrow> {
        match target {
            TabBarClick::PreviousTab => self.previous,
            TabBarClick::NextTab => self.next,
            _ => None,
        }
    }

    /// The left edge of an overflow arrow that takes clicks and focus.
    fn enabled_arrow_x(&self, target: TabBarClick) -> Option<f32> {
        self.arrow(target)
            .filter(|arrow| arrow.enabled)
            .map(|arrow| arrow.x)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct VerticalTabSlot {
    index: usize,
    y: f32,
    height: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct VerticalTabBarLayout {
    tabs: Vec<VerticalTabSlot>,
    add_y: f32,
    previous_y: Option<f32>,
    next_y: Option<f32>,
    control_height: f32,
    inline_controls: bool,
}

impl VerticalTabBarLayout {
    fn control_rect(&self, target: TabBarClick, rail_width: f32) -> Option<Bounds> {
        if self.control_height <= 0.0 || rail_width <= 0.0 {
            return None;
        }
        let (column, y) = match target {
            TabBarClick::PreviousTab => (0.0, self.previous_y?),
            TabBarClick::NextTab => (1.0, self.next_y?),
            TabBarClick::NewTab => (2.0, self.add_y),
            _ => return None,
        };
        let width = if self.inline_controls {
            rail_width / 3.0
        } else {
            rail_width
        };
        let x = if self.inline_controls {
            column * width
        } else {
            0.0
        };
        Some(Bounds::new(x, y, width, self.control_height))
    }
}

fn vertical_tab_bar_width(window_width: f32, scale_factor: f32) -> f32 {
    let scale_factor = scale_factor.max(0.1);
    let window_width = window_width.max(0.0);
    let max_rail = (window_width - MIN_TERMINAL_CONTENT_WIDTH * scale_factor)
        .max(window_width * 0.5)
        .min(window_width);
    // Whole pixels keep the terminal grid beside the rail pixel-aligned.
    (PREFERRED_VERTICAL_TAB_WIDTH * scale_factor)
        .min(max_rail)
        .max((MIN_VERTICAL_TAB_WIDTH * scale_factor).min(max_rail))
        .floor()
}

fn vertical_tab_bar_layout(
    tabs: &[TabInfo],
    window_height: f32,
    row_height: f32,
    anchor: usize,
) -> VerticalTabBarLayout {
    let rail_height = (window_height - row_height).max(0.0);
    let tab_space = (rail_height - row_height).max(0.0);
    let overflow = !tabs.is_empty() && tabs.len() as f32 * row_height > tab_space;
    // Below five rows, separate toolbar/Previous/tab/Next/New rows overlap.
    // Keep full-size labels and share one bottom control row instead. If even
    // one full tab cannot fit, retain navigation controls, not partial labels.
    if window_height < row_height * 3.0 || (overflow && window_height < row_height * 5.0) {
        let control_height = row_height.min(rail_height);
        let add_y = window_height.max(row_height) - control_height;
        let available = (rail_height - control_height).max(0.0);
        let visible_count = ((available / row_height).floor() as usize).min(tabs.len());
        let anchor = anchor.min(tabs.len().saturating_sub(1));
        let start = anchor
            .saturating_sub(visible_count / 2)
            .min(tabs.len().saturating_sub(visible_count));
        let end = start + visible_count;
        let mut y = row_height;
        let slots = tabs[start..end]
            .iter()
            .enumerate()
            .map(|(offset, tab)| {
                let height = row_height * tab_animation_multiplier(tab);
                let slot = VerticalTabSlot {
                    index: start + offset,
                    y,
                    height,
                };
                y += height;
                slot
            })
            .collect();
        return VerticalTabBarLayout {
            tabs: slots,
            add_y,
            previous_y: (anchor > 0).then_some(add_y),
            next_y: (anchor + 1 < tabs.len()).then_some(add_y),
            control_height,
            inline_controls: true,
        };
    }
    if !overflow {
        let mut y = row_height;
        let slots = tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let height = row_height * tab_animation_multiplier(tab);
                let slot = VerticalTabSlot { index, y, height };
                y += height;
                slot
            })
            .collect();
        return VerticalTabBarLayout {
            tabs: slots,
            add_y: y.min((window_height - row_height).max(row_height)),
            previous_y: None,
            next_y: None,
            control_height: row_height,
            inline_controls: false,
        };
    }

    let viewport_y = row_height * 2.0;
    let viewport_height = (window_height - row_height * 4.0).max(row_height);
    let visible_count = ((viewport_height / row_height).floor() as usize)
        .max(1)
        .min(tabs.len());
    let anchor = anchor.min(tabs.len() - 1);
    let start = anchor
        .saturating_sub(visible_count / 2)
        .min(tabs.len() - visible_count);
    let end = start + visible_count;
    let mut y = viewport_y;
    let slots = tabs[start..end]
        .iter()
        .enumerate()
        .map(|(offset, tab)| {
            let height = row_height * tab_animation_multiplier(tab);
            let slot = VerticalTabSlot {
                index: start + offset,
                y,
                height,
            };
            y += height;
            slot
        })
        .collect();
    VerticalTabBarLayout {
        tabs: slots,
        add_y: (window_height - row_height).max(row_height),
        previous_y: (start > 0).then_some(row_height),
        next_y: (end < tabs.len()).then_some((window_height - row_height * 2.0).max(row_height)),
        control_height: row_height,
        inline_controls: false,
    }
}

fn vertical_tab_bar_hit_test(
    layout: &VerticalTabBarLayout,
    tabs: &[TabInfo],
    x: f32,
    y: f32,
    rail_width: f32,
    chrome: TabChrome,
) -> TabBarClick {
    if x < 0.0 || x >= rail_width {
        return TabBarClick::None;
    }
    for target in [
        TabBarClick::PreviousTab,
        TabBarClick::NextTab,
        TabBarClick::NewTab,
    ] {
        if layout
            .control_rect(target, rail_width)
            .is_some_and(|rect| rect.contains(x, y))
        {
            return target;
        }
    }
    for slot in &layout.tabs {
        if y < slot.y || y >= slot.y + slot.height {
            continue;
        }
        let surface = chrome.vertical_surface(slot, rail_width);
        if tab_animation_multiplier(&tabs[slot.index]) > 0.3
            && chrome.close_rect(surface).contains(x, y)
        {
            return TabBarClick::CloseTab(slot.index);
        }
        return TabBarClick::Tab(slot.index);
    }
    TabBarClick::None
}

fn tab_bar_layout(
    tabs: &[TabInfo],
    window_width: f32,
    height: f32,
    anchor: usize,
    scale: f32,
) -> TabBarLayout {
    let scale = scale.max(0.1);
    let (min_width, max_width) = (
        (MIN_TAB_WIDTH * scale).round(),
        (MAX_TAB_WIDTH * scale).round(),
    );
    let tab_strip_end = (window_width - height * 4.0).max(0.0);
    let overflow = !tabs.is_empty() && tabs.len() as f32 * min_width > tab_strip_end;
    if !overflow {
        let left = (TAB_STRIP_INSET * scale).round();
        let width = ((tab_strip_end - left) / tabs.len().max(1) as f32)
            .clamp(min_width, max_width)
            .floor();
        let mut x = left;
        let slots = tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let animated_width = width * tab_animation_multiplier(tab);
                let slot = TabSlot {
                    index,
                    x,
                    width: animated_width,
                };
                x += animated_width;
                slot
            })
            .collect();
        return TabBarLayout {
            tabs: slots,
            add_x: x.min(tab_strip_end),
            previous: None,
            next: None,
        };
    }

    // Overflow controls permanently reserve one square on each side. Keeping
    // their geometry stable prevents the tab strip from jumping as the active
    // tab reaches either end.
    let viewport_x = height;
    let viewport_width = (tab_strip_end - height * 2.0).max(1.0);
    let visible_count = ((viewport_width / min_width).floor() as usize)
        .max(1)
        .min(tabs.len());
    let anchor = anchor.min(tabs.len() - 1);
    let start = anchor
        .saturating_sub(visible_count / 2)
        .min(tabs.len() - visible_count);
    let end = start + visible_count;
    let width = (viewport_width / visible_count as f32)
        .min(max_width)
        .floor();
    let mut x = viewport_x;
    let slots = tabs[start..end]
        .iter()
        .enumerate()
        .map(|(offset, tab)| {
            let animated_width = width * tab_animation_multiplier(tab);
            let slot = TabSlot {
                index: start + offset,
                x,
                width: animated_width,
            };
            x += animated_width;
            slot
        })
        .collect();
    TabBarLayout {
        tabs: slots,
        add_x: tab_strip_end,
        previous: Some(OverflowArrow {
            x: 0.0,
            enabled: start > 0,
        }),
        next: Some(OverflowArrow {
            x: tab_strip_end - height,
            enabled: end < tabs.len(),
        }),
    }
}

/// Whole-pixel chrome geometry at one display scale. Rendering, hit testing,
/// keyboard focus, and accessibility all derive tab rectangles from here so
/// they cannot drift apart.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TabChrome {
    scale: f32,
}

impl TabChrome {
    fn new(scale: f32) -> Self {
        Self {
            scale: scale.max(0.1),
        }
    }

    /// A logical length in whole physical pixels, never below one.
    fn px(self, logical: f32) -> f32 {
        (logical * self.scale).round().max(1.0)
    }

    /// Stroke width for vector icons.
    fn stroke(self) -> f32 {
        (1.5 * self.scale).max(1.0)
    }

    fn corner_radius(self) -> f32 {
        self.px(BUTTON_CORNER_RADIUS)
    }

    /// Visible surface of a top-bar tab slot.
    fn horizontal_surface(self, slot: &TabSlot, bar_height: f32) -> Bounds {
        let (inset_x, inset_y) = (self.px(2.0), self.px(4.0));
        Bounds::new(
            slot.x + inset_x,
            inset_y,
            (slot.width - inset_x * 2.0).max(0.0),
            (bar_height - inset_y * 2.0).max(0.0),
        )
    }

    /// Visible surface of a left-rail tab slot.
    fn vertical_surface(self, slot: &VerticalTabSlot, rail_width: f32) -> Bounds {
        let (inset_x, inset_y) = (self.px(4.0), self.px(2.0));
        Bounds::new(
            inset_x,
            slot.y + inset_y,
            (rail_width - inset_x * 2.0).max(0.0),
            (slot.height - inset_y * 2.0).max(0.0),
        )
    }

    /// Close button inside a tab surface.
    fn close_rect(self, surface: Bounds) -> Bounds {
        let size = self.px(16.0).min(surface.height).min(surface.width);
        Bounds::new(
            surface.right() - self.px(6.0) - size,
            (surface.y + (surface.height - size) / 2.0).round(),
            size,
            size,
        )
    }

    /// Where `tab`'s markers and title sit in `surface`, before the close
    /// button and an unreviewed-diff badge `badge_width` pixels wide (0 for
    /// none).
    fn label(self, tab: &TabInfo, surface: Bounds, badge_width: f32) -> TabLabelLayout {
        let center_y = surface.y + surface.height / 2.0;
        let mut text_x = surface.x + self.px(10.0);
        let mut plugin_dot = None;
        if matches!(&tab.tab_type, TabType::Plugin { .. }) {
            let dot = self.px(6.0);
            plugin_dot = Some(Bounds::new(
                text_x,
                (center_y - dot / 2.0).round(),
                dot,
                dot,
            ));
            text_x += dot + self.px(6.0);
        }
        let mut status = None;
        if let Some(marker) = tab_status(tab) {
            let size = self.px(12.0);
            status = Some((
                marker,
                Bounds::new(text_x, center_y - size / 2.0, size, size),
            ));
            text_x += size + self.px(5.0);
        }
        let badge_reserve = if badge_width > 0.0 {
            badge_width + self.px(6.0)
        } else {
            0.0
        };
        TabLabelLayout {
            plugin_dot,
            status,
            text_x,
            text_right: self.close_rect(surface).x - self.px(6.0) - badge_reserve,
            badge_width,
        }
    }

    /// The caret after a title `title_width` pixels wide laid out in
    /// `label`: as tall as the UI face's `metrics`, centred in `surface`
    /// as the title is.
    fn rename_caret(
        self,
        label: &TabLabelLayout,
        title_width: f32,
        surface: Bounds,
        metrics: UiFontMetrics,
    ) -> Bounds {
        let top = metrics.centered_baseline(surface.y, surface.height) - metrics.ascent.round();
        let height = (metrics.ascent + metrics.descent)
            .round()
            .min(surface.bottom() - top)
            .max(1.0);
        Bounds::new(
            (label.text_x + title_width).round(),
            top,
            self.px(2.0),
            height,
        )
    }
}

/// Where a tab's plugin dot, status marker, and title sit in its surface.
/// Drawing the tab and placing its rename caret, which the input method
/// opens at, both start from here.
struct TabLabelLayout {
    plugin_dot: Option<Bounds>,
    status: Option<(TabStatus, Bounds)>,
    /// The title starts at `text_x` and may run up to `text_right`.
    text_x: f32,
    text_right: f32,
    badge_width: f32,
}

/// Whether `tab`, drawn in `surface`, shows its markers and title: it has
/// room, and it is not just starting to slide in or nearly closed.
fn tab_label_drawn(tab: &TabInfo, surface: Bounds) -> bool {
    surface.width > 2.0 && surface.height > 2.0 && tab_animation_multiplier(tab) > 0.3
}

/// Which edge of the active tab carries the accent indicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AccentEdge {
    Bottom,
    Left,
}

/// Commands, workspace, and settings buttons at the right of the top strip.
fn toolbar_control_rect(target: TabBarClick, window_width: f32, height: f32) -> Option<Bounds> {
    let offset = match target {
        TabBarClick::CommandsButton => 3.0,
        TabBarClick::SessionDropdown => 2.0,
        TabBarClick::Gear => 1.0,
        _ => return None,
    };
    Some(Bounds::new(
        window_width - height * offset,
        0.0,
        height,
        height,
    ))
}

fn horizontal_tab_bar_element_rect(
    layout: &TabBarLayout,
    target: TabBarClick,
    window_width: f32,
    height: f32,
    chrome: TabChrome,
) -> Option<Bounds> {
    let surface = |index: usize| {
        layout
            .tabs
            .iter()
            .find(|slot| slot.index == index)
            .map(|slot| chrome.horizontal_surface(slot, height))
    };
    match target {
        TabBarClick::Tab(index) => surface(index),
        TabBarClick::CloseTab(index) => surface(index).map(|surface| chrome.close_rect(surface)),
        TabBarClick::NewTab => Some(Bounds::new(layout.add_x, 0.0, height, height)),
        TabBarClick::PreviousTab | TabBarClick::NextTab => layout
            .enabled_arrow_x(target)
            .map(|x| Bounds::new(x, 0.0, height, height)),
        TabBarClick::CommandsButton | TabBarClick::SessionDropdown | TabBarClick::Gear => {
            toolbar_control_rect(target, window_width, height)
        }
        TabBarClick::None => None,
    }
}

/// Every top-strip button as drawn: its target, bounds, and whether it is
/// enabled. A disabled overflow arrow keeps its slot, dimmed, and takes no
/// clicks; enabled buttons are exactly the element rectangles hit testing,
/// focus, and accessibility use.
fn horizontal_toolbar_buttons(
    layout: &TabBarLayout,
    window_width: f32,
    height: f32,
    chrome: TabChrome,
) -> Vec<(TabBarClick, Bounds, bool)> {
    let arrows = [TabBarClick::PreviousTab, TabBarClick::NextTab]
        .into_iter()
        .filter_map(|target| {
            let arrow = layout.arrow(target)?;
            Some((
                target,
                Bounds::new(arrow.x, 0.0, height, height),
                arrow.enabled,
            ))
        });
    let buttons = [
        TabBarClick::NewTab,
        TabBarClick::CommandsButton,
        TabBarClick::SessionDropdown,
        TabBarClick::Gear,
    ]
    .into_iter()
    .filter_map(|target| {
        horizontal_tab_bar_element_rect(layout, target, window_width, height, chrome)
            .map(|rect| (target, rect, true))
    });
    arrows.chain(buttons).collect()
}

fn horizontal_tab_bar_hit_test(
    layout: &TabBarLayout,
    tabs: &[TabInfo],
    x: f32,
    y: f32,
    window_width: f32,
    height: f32,
    chrome: TabChrome,
) -> TabBarClick {
    if y < 0.0 || y >= height {
        return TabBarClick::None;
    }
    for target in [
        TabBarClick::Gear,
        TabBarClick::SessionDropdown,
        TabBarClick::CommandsButton,
        TabBarClick::PreviousTab,
        TabBarClick::NextTab,
        TabBarClick::NewTab,
    ] {
        if horizontal_tab_bar_element_rect(layout, target, window_width, height, chrome)
            .is_some_and(|rect| rect.contains(x, y))
        {
            return target;
        }
    }
    for slot in &layout.tabs {
        if x < slot.x || x >= slot.x + slot.width {
            continue;
        }
        let surface = chrome.horizontal_surface(slot, height);
        if tab_animation_multiplier(&tabs[slot.index]) > 0.3
            && chrome.close_rect(surface).contains(x, y)
        {
            return TabBarClick::CloseTab(slot.index);
        }
        return TabBarClick::Tab(slot.index);
    }
    TabBarClick::None
}

/// A tab's single status marker, in priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabStatus {
    BroadcastActive,
    BroadcastArmed,
    ReadOnly,
    Agent(AgentBadge),
    Pinned,
}

/// The one marker a tab has room for. Broadcast outranks everything because
/// typing there reaches other panes; an agent waiting on the person or a
/// failed turn outranks the read-only lock; progress and results outrank the
/// pin.
fn tab_status(tab: &TabInfo) -> Option<TabStatus> {
    let urgent_agent = tab
        .agent_badge
        .filter(|badge| matches!(badge, AgentBadge::NeedsYou | AgentBadge::Failed));
    if tab.broadcast_active {
        Some(TabStatus::BroadcastActive)
    } else if tab.broadcast_armed {
        Some(TabStatus::BroadcastArmed)
    } else if let Some(badge) = urgent_agent {
        Some(TabStatus::Agent(badge))
    } else if tab.read_only {
        Some(TabStatus::ReadOnly)
    } else if let Some(badge) = tab.agent_badge {
        Some(TabStatus::Agent(badge))
    } else if tab.pinned {
        Some(TabStatus::Pinned)
    } else {
        None
    }
}

impl Renderer {
    /// Width reserved by a vertical tab rail for the current viewport.
    pub fn tab_bar_width(&self) -> f32 {
        self.tab_bar_width_for(self.window_size().0 as f32)
    }

    /// Width reserved by a vertical tab rail for an explicit viewport.
    pub fn tab_bar_width_for(&self, window_width: f32) -> f32 {
        if self.tab_bar_position == crate::config::TabBarPosition::Left {
            vertical_tab_bar_width(window_width, self.scale_factor)
        } else {
            0.0
        }
    }

    fn tab_chrome(&self) -> TabChrome {
        TabChrome::new(self.scale_factor)
    }

    fn hover_progress(&self, target: TabBarClick) -> f32 {
        if self.tab_bar_hover == Some(target) {
            self.tab_bar_hover_progress
        } else {
            0.0
        }
    }

    pub(super) fn build_tab_bar(
        &mut self,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
        tab_bar_height: f32,
    ) {
        if self.tab_bar_position == crate::config::TabBarPosition::Left {
            self.build_vertical_tab_bar(tabs, window_width, window_height, tab_bar_height);
            return;
        }
        let chrome = self.tab_chrome();
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, tab_bar_height, self.ui.tab_bar_bg);
        let layout = self.tab_bar_layout(tabs, window_width);
        for (index, surface) in self.placed_tab_surfaces(tabs, window_width, window_height) {
            if self.is_dragged_tab(index) {
                self.lift_dragged_tab(surface);
            }
            self.build_tab_item(&tabs[index], index, surface, AccentEdge::Bottom);
        }
        let separator = chrome.px(1.0);
        self.rect_pipeline.push_rect(
            0.0,
            tab_bar_height - separator,
            window_width,
            separator,
            with_alpha(self.ui.tab_border, 0.6),
        );

        for (target, rect, enabled) in
            horizontal_toolbar_buttons(&layout, window_width, tab_bar_height, chrome)
        {
            self.build_toolbar_button(target, rect, enabled, false);
        }
    }

    /// Draw one tab: its surface, accent indicator, markers, label, and the
    /// close button or activity dot.
    fn build_tab_item(&mut self, tab: &TabInfo, index: usize, surface: Bounds, edge: AccentEdge) {
        let chrome = self.tab_chrome();
        let animation = tab_animation_multiplier(tab);
        if surface.width <= 2.0 || surface.height <= 2.0 {
            return;
        }
        let hovered = matches!(
            self.tab_bar_hover,
            Some(TabBarClick::Tab(hovered) | TabBarClick::CloseTab(hovered)) if hovered == index
        );
        let hover = if hovered {
            self.tab_bar_hover_progress
        } else {
            0.0
        };
        let Bounds {
            x,
            y,
            width,
            height,
        } = surface;
        let radius = chrome.corner_radius();
        if tab.is_active {
            self.rect_pipeline.push_rounded_rect(
                x,
                y,
                width,
                height,
                scale_alpha(self.ui.tab_active, animation),
                radius,
            );
            let accent = with_alpha(self.ui.tab_accent, animation);
            let thickness = chrome.px(2.0);
            let inset = chrome.px(10.0);
            match edge {
                AccentEdge::Bottom if width > inset * 2.0 => self.rect_pipeline.push_rounded_rect(
                    x + inset,
                    y + height - thickness,
                    width - inset * 2.0,
                    thickness,
                    accent,
                    thickness / 2.0,
                ),
                AccentEdge::Left if height > inset => self.rect_pipeline.push_rounded_rect(
                    x,
                    y + inset / 2.0,
                    thickness,
                    height - inset,
                    accent,
                    thickness / 2.0,
                ),
                _ => {}
            }
        } else if hover > 0.0 {
            self.rect_pipeline.push_rounded_rect(
                x,
                y,
                width,
                height,
                with_alpha(self.ui.tab_inactive, hover * animation),
                radius,
            );
        }
        if !tab_label_drawn(tab, surface) {
            return;
        }

        let text_alpha = ((animation - 0.3) / 0.7).min(1.0);
        let base_text = if tab.is_active {
            self.ui.tab_text_active
        } else {
            mix(self.ui.tab_text_inactive, self.ui.tab_text_active, hover)
        };
        let text_color = scale_alpha(base_text, text_alpha);
        let center_y = y + height / 2.0;
        let close = chrome.close_rect(surface);
        let label = self.tab_label_layout(tab, surface);

        if let Some(dot) = label.plugin_dot {
            self.rect_pipeline.push_rounded_rect(
                dot.x,
                dot.y,
                dot.width,
                dot.height,
                with_alpha(self.ui.palette_accent, text_alpha),
                dot.width / 2.0,
            );
        }
        if let Some((status, bounds)) = label.status {
            let surface_color = if tab.is_active {
                self.ui.tab_active
            } else {
                mix(self.ui.tab_bar_bg, self.ui.tab_inactive, hover)
            };
            self.build_status_icon(status, bounds, text_color, surface_color);
        }

        let size_px = self.ui_label_px();
        let text_x = label.text_x;
        let drawn = self.push_ui_label(
            &tab.title,
            text_x,
            y,
            height,
            label.text_right - text_x,
            size_px,
            text_color,
        );
        if tab.renaming {
            let caret = self.tab_rename_caret_in(tab, &label, surface);
            self.rect_pipeline.push_rect(
                caret.x,
                caret.y,
                caret.width,
                caret.height,
                with_alpha(self.ui.palette_accent, 0.9 * text_alpha),
            );
        }

        let badge_width = label.badge_width;
        let badge_text =
            (tab.unreviewed_diff_count > 0).then(|| tab.unreviewed_diff_count.to_string());
        if let Some(count) = badge_text {
            let badge_height = chrome.px(16.0).min(height);
            let badge_x = (text_x + drawn + chrome.px(6.0)).round();
            let badge_y = (center_y - badge_height / 2.0).round();
            if badge_x + badge_width <= close.x - chrome.px(4.0) {
                let fill = with_alpha(self.ui.palette_accent, 0.9 * text_alpha);
                self.rect_pipeline.push_rounded_rect(
                    badge_x,
                    badge_y,
                    badge_width,
                    badge_height,
                    fill,
                    badge_height / 2.0,
                );
                let count_size = size_px * 0.85;
                let count_color =
                    with_alpha(high_contrast_text(self.ui.palette_accent), text_alpha);
                self.push_ui_label_centered(
                    &count,
                    badge_x + badge_width / 2.0,
                    badge_y,
                    badge_height,
                    badge_width,
                    count_size,
                    count_color,
                );
            }
        }

        if tab.is_active || hovered {
            let close_hover = self.hover_progress(TabBarClick::CloseTab(index));
            if close_hover > 0.0 {
                self.rect_pipeline.push_rounded_rect(
                    close.x,
                    close.y,
                    close.width,
                    close.height,
                    with_alpha(self.ui.tab_text_active, 0.14 * close_hover * text_alpha),
                    close.width / 2.0,
                );
            }
            let color = with_alpha(
                mix(
                    self.ui.tab_text_inactive,
                    self.ui.tab_text_active,
                    close_hover,
                ),
                text_alpha,
            );
            // Smaller than a toolbar icon, to sit inside the tab.
            draw_icon(
                &mut self.rect_pipeline,
                Icon::Close,
                [close.x + close.width / 2.0, close.y + close.height / 2.0],
                12.5 * chrome.scale,
                color,
            );
        } else if tab.has_activity {
            let dot = chrome.px(6.0);
            self.rect_pipeline.push_rounded_rect(
                (close.x + (close.width - dot) / 2.0).round(),
                (close.y + (close.height - dot) / 2.0).round(),
                dot,
                dot,
                with_alpha(self.ui.warning, text_alpha),
                dot / 2.0,
            );
        }
    }

    /// Small vector marker for a tab's broadcast, read-only, or pinned state.
    fn build_status_icon(
        &mut self,
        status: TabStatus,
        rect: Bounds,
        color: [f32; 4],
        surface: [f32; 4],
    ) {
        let chrome = self.tab_chrome();
        let Bounds {
            x, y, width: size, ..
        } = rect;
        let (cx, cy) = (x + size / 2.0, y + size / 2.0);
        let stroke = chrome.stroke() * 0.8;
        match status {
            TabStatus::BroadcastActive | TabStatus::BroadcastArmed => {
                // Armed but idle: a ring rather than a filled dot.
                self.push_status_dot(
                    [cx, cy],
                    size * 0.66,
                    with_alpha(self.ui.warning, color[3]),
                    (status == TabStatus::BroadcastArmed).then_some(surface),
                );
            }
            TabStatus::ReadOnly => {
                // Padlock: a body with a shackle above it.
                let body_width = size * 0.72;
                let body_height = size * 0.5;
                let body_top = y + size - body_height;
                self.rect_pipeline.push_rounded_rect(
                    cx - body_width / 2.0,
                    body_top,
                    body_width,
                    body_height,
                    color,
                    chrome.px(1.0),
                );
                let shackle = size * 0.22;
                let top = y + stroke / 2.0;
                self.rect_pipeline.push_line(
                    cx - shackle,
                    body_top,
                    cx - shackle,
                    top + shackle,
                    stroke,
                    color,
                );
                self.rect_pipeline.push_line(
                    cx + shackle,
                    body_top,
                    cx + shackle,
                    top + shackle,
                    stroke,
                    color,
                );
                self.rect_pipeline
                    .push_line(cx - shackle, top + shackle, cx, top, stroke, color);
                self.rect_pipeline
                    .push_line(cx, top, cx + shackle, top + shackle, stroke, color);
            }
            TabStatus::Pinned => {
                // Push pin: a round head over a short needle.
                let head = (size * 0.5).round();
                self.rect_pipeline.push_rounded_rect(
                    (cx - head / 2.0).round(),
                    y,
                    head,
                    head,
                    color,
                    head / 2.0,
                );
                self.rect_pipeline
                    .push_line(cx, y + head, cx, y + size, stroke, color);
            }
            TabStatus::Agent(badge) => {
                if let Some((fill, ring)) = self.agent_status_dot(badge.into(), color) {
                    self.push_status_dot([cx, cy], size * 0.66, fill, ring.then_some(surface));
                }
            }
        }
    }

    /// Draw a toolbar button: a rounded hover surface and its vector icon.
    /// `vertical` orients overflow arrows for the left rail.
    fn build_toolbar_button(
        &mut self,
        target: TabBarClick,
        rect: Bounds,
        enabled: bool,
        vertical: bool,
    ) {
        let chrome = self.tab_chrome();
        let hover = if enabled {
            self.hover_progress(target)
        } else {
            0.0
        };
        let Bounds {
            x,
            y,
            width,
            height,
        } = rect;
        let side = (width.min(height) - chrome.px(8.0)).max(0.0);
        if hover > 0.0 && side > 0.0 {
            self.rect_pipeline.push_rounded_rect(
                (x + (width - side) / 2.0).round(),
                (y + (height - side) / 2.0).round(),
                side,
                side,
                with_alpha(self.ui.tab_inactive, hover),
                chrome.corner_radius(),
            );
        }
        let base = if enabled {
            mix(self.ui.tab_text_inactive, self.ui.tab_text_active, hover)
        } else {
            with_alpha(self.ui.tab_text_inactive, 0.25)
        };
        let icon = match target {
            TabBarClick::NewTab => Icon::Plus,
            TabBarClick::PreviousTab if vertical => Icon::ChevronUp,
            TabBarClick::NextTab if vertical => Icon::ChevronDown,
            TabBarClick::PreviousTab => Icon::Back,
            TabBarClick::NextTab => Icon::ChevronRight,
            TabBarClick::SessionDropdown => Icon::ChevronDown,
            TabBarClick::CommandsButton => Icon::Prompt,
            TabBarClick::Gear => Icon::Menu,
            TabBarClick::Tab(_) | TabBarClick::CloseTab(_) | TabBarClick::None => return,
        };
        self.push_icon(
            icon,
            (x + width / 2.0).round(),
            (y + height / 2.0).round(),
            base,
        );
    }

    fn build_vertical_tab_bar(
        &mut self,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
        row_height: f32,
    ) {
        let chrome = self.tab_chrome();
        let rail_width = self.tab_bar_width_for(window_width);
        let tab_bar_bg = self.ui.tab_bar_bg;
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, row_height, tab_bar_bg);
        self.rect_pipeline.push_rect(
            0.0,
            row_height,
            rail_width,
            (window_height - row_height).max(0.0),
            tab_bar_bg,
        );

        let layout = self.vertical_tab_bar_layout(tabs, window_height);
        for (index, surface) in self.placed_tab_surfaces(tabs, window_width, window_height) {
            if self.is_dragged_tab(index) {
                self.lift_dragged_tab(surface);
            }
            self.build_tab_item(&tabs[index], index, surface, AccentEdge::Left);
        }
        for target in [
            TabBarClick::PreviousTab,
            TabBarClick::NextTab,
            TabBarClick::NewTab,
        ] {
            if let Some(rect) = layout.control_rect(target, rail_width) {
                self.build_toolbar_button(target, rect, true, true);
            }
        }
        for target in [
            TabBarClick::CommandsButton,
            TabBarClick::SessionDropdown,
            TabBarClick::Gear,
        ] {
            if let Some(rect) = toolbar_control_rect(target, window_width, row_height) {
                self.build_toolbar_button(target, rect, true, false);
            }
        }

        let border = with_alpha(self.ui.tab_border, 0.6);
        let line = chrome.px(1.0);
        self.rect_pipeline
            .push_rect(0.0, row_height - line, window_width, line, border);
        self.rect_pipeline.push_rect(
            rail_width - line,
            row_height,
            line,
            (window_height - row_height).max(0.0),
            border,
        );
    }

    /// Draw the focus ring around a tab-bar control, inside its bounds so
    /// it never covers a neighbour.
    pub(super) fn build_tab_bar_keyboard_focus(
        &mut self,
        target: TabBarClick,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
    ) {
        let Some(bounds) = self.tab_bar_element_rect(target, tabs, window_width, window_height)
        else {
            return;
        };
        self.draw_focus_ring(bounds, self.ui.focus_ring, BUTTON_CORNER_RADIUS);
    }

    /// Exact rendered bounds for a tab-bar control. Accessibility and focus
    /// rendering share this geometry with the visible bar.
    pub fn tab_bar_element_rect(
        &self,
        target: TabBarClick,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
    ) -> Option<Bounds> {
        let height = self.tab_bar_height();
        let chrome = self.tab_chrome();
        if self.tab_bar_position == crate::config::TabBarPosition::Left {
            let rail_width = self.tab_bar_width_for(window_width);
            let layout = self.vertical_tab_bar_layout(tabs, window_height);
            let surface = |index: usize| {
                layout
                    .tabs
                    .iter()
                    .find(|slot| slot.index == index)
                    .map(|slot| chrome.vertical_surface(slot, rail_width))
            };
            return match target {
                TabBarClick::Tab(index) => surface(index),
                TabBarClick::CloseTab(index) => {
                    surface(index).map(|surface| chrome.close_rect(surface))
                }
                TabBarClick::NewTab | TabBarClick::PreviousTab | TabBarClick::NextTab => {
                    layout.control_rect(target, rail_width)
                }
                TabBarClick::CommandsButton | TabBarClick::SessionDropdown | TabBarClick::Gear => {
                    toolbar_control_rect(target, window_width, height)
                }
                TabBarClick::None => None,
            };
        }
        let layout = self.tab_bar_layout(tabs, window_width);
        horizontal_tab_bar_element_rect(&layout, target, window_width, height, chrome)
    }

    pub fn tab_bar_hit_test(
        &self,
        x: f32,
        y: f32,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
    ) -> TabBarClick {
        let tab_bar_height = self.tab_bar_height();
        let chrome = self.tab_chrome();

        if self.tab_bar_position == crate::config::TabBarPosition::Left {
            if y < tab_bar_height {
                return [
                    TabBarClick::Gear,
                    TabBarClick::SessionDropdown,
                    TabBarClick::CommandsButton,
                ]
                .into_iter()
                .find(|&target| {
                    toolbar_control_rect(target, window_width, tab_bar_height)
                        .is_some_and(|rect| rect.contains(x, y))
                })
                .unwrap_or(TabBarClick::None);
            }
            let rail_width = self.tab_bar_width_for(window_width);
            if x >= rail_width || y >= window_height {
                return TabBarClick::None;
            }
            let layout = self.vertical_tab_bar_layout(tabs, window_height);
            return vertical_tab_bar_hit_test(&layout, tabs, x, y, rail_width, chrome);
        }

        let layout = self.tab_bar_layout(tabs, window_width);
        horizontal_tab_bar_hit_test(&layout, tabs, x, y, window_width, tab_bar_height, chrome)
    }
}

impl Renderer {
    /// Where to draw each visible tab (index, start, and length along the
    /// bar) in drawing order: sliding tabs offset from their slots, and the
    /// dragged tab last, following the pointer within the strip.
    fn tab_drag_placement(&self, spans: &[TabSpan]) -> Vec<TabSpan> {
        place_tabs(spans, &self.tab_drag)
    }

    /// Each visible tab's surface where it is drawn, in drawing order: at
    /// its slot, or wherever sliding or dragging has moved it.
    fn placed_tab_surfaces(
        &self,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
    ) -> Vec<(usize, Bounds)> {
        let chrome = self.tab_chrome();
        let vertical = self.tab_bar_is_vertical();
        let rail_width = self.tab_bar_width_for(window_width);
        let bar_height = self.tab_bar_height();
        let spans = self.tab_slot_spans(tabs, window_width, window_height);
        self.tab_drag_placement(&spans)
            .into_iter()
            .map(|(index, start, length)| {
                let surface = if vertical {
                    let slot = VerticalTabSlot {
                        index,
                        y: start,
                        height: length,
                    };
                    chrome.vertical_surface(&slot, rail_width)
                } else {
                    let slot = TabSlot {
                        index,
                        x: start,
                        width: length,
                    };
                    chrome.horizontal_surface(&slot, bar_height)
                };
                (index, surface)
            })
            .collect()
    }

    /// [`TabChrome::label`] for `tab` in `surface`, measuring its badge.
    fn tab_label_layout(&mut self, tab: &TabInfo, surface: Bounds) -> TabLabelLayout {
        let chrome = self.tab_chrome();
        let badge_width = if tab.unreviewed_diff_count > 0 {
            let count = tab.unreviewed_diff_count.to_string();
            let count_px = self.ui_label_px() * 0.85;
            (self.ui_text_width(&count, count_px) + chrome.px(12.0)).max(chrome.px(18.0))
        } else {
            0.0
        };
        chrome.label(tab, surface, badge_width)
    }

    /// The caret at the end of `tab`'s title as it is drawn in `label`.
    fn tab_rename_caret_in(
        &mut self,
        tab: &TabInfo,
        label: &TabLabelLayout,
        surface: Bounds,
    ) -> Bounds {
        let size_px = self.ui_label_px();
        let (_, title_width) =
            self.fit_ui_label(&tab.title, label.text_right - label.text_x, size_px);
        let metrics = self.glyph_cache.ui_font_metrics(size_px);
        self.tab_chrome()
            .rename_caret(label, title_width, surface, metrics)
    }

    /// The caret of the tab being renamed (the one marked `renaming`) where
    /// the tab bar draws it, in a window `window_width` by `window_height`
    /// pixels; `None` while that tab is out of view or shows no title.
    pub fn tab_rename_caret(
        &mut self,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
    ) -> Option<Bounds> {
        let index = tabs.iter().position(|tab| tab.renaming)?;
        let tab = &tabs[index];
        let (_, surface) = self
            .placed_tab_surfaces(tabs, window_width, window_height)
            .into_iter()
            .find(|(placed, _)| *placed == index)?;
        if !tab_label_drawn(tab, surface) {
            return None;
        }
        let label = self.tab_label_layout(tab, surface);
        Some(self.tab_rename_caret_in(tab, &label, surface))
    }

    fn is_dragged_tab(&self, index: usize) -> bool {
        self.tab_drag
            .dragged
            .is_some_and(|(dragged, _)| dragged == index)
    }

    /// Puts the dragged tab on a layer of its own over an opaque backing,
    /// so the tab it passes over never shows through it.
    fn lift_dragged_tab(&mut self, surface: Bounds) {
        self.dragged_tab_surface = Some(surface);
        self.rect_pipeline.mark_layer();
        self.text_pipeline.mark_layer();
        let chrome = self.tab_chrome();
        self.rect_pipeline.push_rounded_rect(
            surface.x,
            surface.y,
            surface.width,
            surface.height,
            self.ui.tab_bar_bg,
            chrome.corner_radius(),
        );
    }

    /// The corner radius of a tab surface, in physical pixels.
    pub(super) fn tab_chrome_corner_radius(&self) -> f32 {
        self.tab_chrome().corner_radius()
    }

    /// Whether tabs run down the side rail rather than along the top.
    pub fn tab_bar_is_vertical(&self) -> bool {
        self.tab_bar_position == crate::config::TabBarPosition::Left
    }

    /// The visible tabs' slots as (index, start, length) along the bar.
    pub fn tab_slot_spans(
        &self,
        tabs: &[TabInfo],
        window_width: f32,
        window_height: f32,
    ) -> Vec<TabSpan> {
        if self.tab_bar_is_vertical() {
            return self
                .vertical_tab_bar_layout(tabs, window_height)
                .tabs
                .iter()
                .map(|slot| (slot.index, slot.y, slot.height))
                .collect();
        }
        self.tab_bar_layout(tabs, window_width)
            .tabs
            .iter()
            .map(|slot| (slot.index, slot.x, slot.width))
            .collect()
    }

    fn tab_bar_layout(&self, tabs: &[TabInfo], window_width: f32) -> TabBarLayout {
        let anchor = self.tab_bar_viewport_anchor.resolve(tabs);
        tab_bar_layout(
            tabs,
            window_width,
            self.tab_bar_height(),
            anchor,
            self.scale_factor,
        )
    }

    fn vertical_tab_bar_layout(
        &self,
        tabs: &[TabInfo],
        window_height: f32,
    ) -> VerticalTabBarLayout {
        let anchor = self.tab_bar_viewport_anchor.resolve(tabs);
        vertical_tab_bar_layout(tabs, window_height, self.tab_bar_height(), anchor)
    }
}

/// Where to draw each visible tab (index, start, and length along the bar)
/// in drawing order: sliding tabs offset from their slots, and the dragged
/// tab last, at the pointer but within the strip. Whole pixels throughout.
fn place_tabs(spans: &[TabSpan], drag: &TabDragVisual) -> Vec<TabSpan> {
    let dragged = drag.dragged;
    let mut placed: Vec<TabSpan> = spans
        .iter()
        .filter(|(index, _, _)| dragged.is_none_or(|(dragged, _)| dragged != *index))
        .map(|&(index, start, length)| {
            let offset = drag
                .sliding
                .iter()
                .find(|(sliding, _)| *sliding == index)
                .map_or(0.0, |(_, offset)| *offset);
            (index, (start + offset).round(), length)
        })
        .collect();
    if let Some((index, leading)) = dragged
        && let Some(&(_, _, length)) = spans.iter().find(|(slot, _, _)| *slot == index)
    {
        let first = spans.first().map_or(0.0, |(_, start, _)| *start);
        let end = spans
            .last()
            .map_or(0.0, |(_, start, length)| start + length);
        let start = leading.min(end - length).max(first).round();
        placed.push((index, start, length));
    }
    placed
}

fn tab_animation_multiplier(tab: &TabInfo) -> f32 {
    if tab.close_progress > 0.0 {
        let eased = 1.0 - (1.0 - tab.close_progress).powi(3);
        1.0 - eased
    } else {
        1.0 - (1.0 - tab.slide_progress).powi(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_tabs_to_the_rail_narrows_the_terminal_grid() {
        let geometry = |rail: f32| {
            crate::terminal::geometry::TerminalGeometry::from_window_pixels(
                1_200, 800, 8.0, rail, 36.0, 10.0, 20.0,
            )
        };
        let top = geometry(0.0);
        let left = geometry(vertical_tab_bar_width(1_200.0, 1.0));
        assert!(left.cols < top.cols);
        assert_eq!(left.rows, top.rows);
    }

    fn tabs(count: usize, active: usize) -> Vec<TabInfo> {
        (0..count)
            .map(|index| TabInfo {
                title: format!("Tab {}", index + 1),
                pinned: false,
                broadcast_armed: false,
                broadcast_active: false,
                read_only: false,
                is_active: index == active,
                has_activity: false,
                slide_progress: 1.0,
                close_progress: 0.0,
                tab_type: TabType::Shell,
                unreviewed_diff_count: 0,
                agent_badge: None,
                renaming: false,
            })
            .collect()
    }

    #[test]
    fn crowded_tab_bar_keeps_active_tab_inside_bounded_viewport() {
        let tabs = tabs(12, 10);
        let layout = tab_bar_layout(&tabs, 640.0, 32.0, 10, 1.0);

        assert!(layout.enabled_arrow_x(TabBarClick::PreviousTab).is_some());
        assert!(layout.tabs.iter().any(|slot| slot.index == 10));
        assert!(layout.tabs.iter().all(|slot| slot.x >= 32.0));
        assert!(layout.tabs.iter().all(|slot| slot.x + slot.width <= 480.0));
        assert_eq!(layout.add_x, 512.0);
    }

    #[test]
    fn crowded_tab_bar_reveals_hidden_tabs_from_each_end() {
        let tabs = tabs(12, 0);
        let first = tab_bar_layout(&tabs, 640.0, 32.0, 0, 1.0);
        assert!(first.enabled_arrow_x(TabBarClick::PreviousTab).is_none());
        assert!(first.enabled_arrow_x(TabBarClick::NextTab).is_some());
        assert_eq!(first.tabs.first().map(|slot| slot.index), Some(0));

        let last = tab_bar_layout(&tabs, 640.0, 32.0, 11, 1.0);
        assert!(last.enabled_arrow_x(TabBarClick::PreviousTab).is_some());
        assert!(last.enabled_arrow_x(TabBarClick::NextTab).is_none());
        assert_eq!(last.tabs.last().map(|slot| slot.index), Some(11));
    }

    #[test]
    fn ordinary_tab_bar_preserves_full_strip_without_overflow_controls() {
        let tabs = tabs(3, 1);
        let layout = tab_bar_layout(&tabs, 640.0, 32.0, 1, 1.0);

        assert_eq!(layout.tabs.len(), 3);
        assert!(layout.previous.is_none() && layout.next.is_none());
        assert!(layout.tabs.iter().all(|slot| slot.width > MIN_TAB_WIDTH));
        assert!(layout.add_x <= 512.0);
    }

    #[test]
    fn overflow_controls_keep_the_keyboard_viewport_when_active_tab_is_elsewhere() {
        for active in [0, 19] {
            let tabs = tabs(20, active);
            for index in 0..20 {
                let mut anchor = TabBarViewportAnchor::default();
                anchor.set_focus(Some(TabBarClick::CloseTab(index)));
                let horizontal = tab_bar_layout(&tabs, 640.0, 32.0, anchor.resolve(&tabs), 1.0);
                let vertical = vertical_tab_bar_layout(&tabs, 240.0, 32.0, anchor.resolve(&tabs));
                for target in [TabBarClick::PreviousTab, TabBarClick::NextTab] {
                    anchor.set_focus(Some(TabBarClick::CloseTab(index)));
                    anchor.set_focus(Some(target));
                    assert_eq!(
                        tab_bar_layout(&tabs, 640.0, 32.0, anchor.resolve(&tabs), 1.0),
                        horizontal,
                        "horizontal active={active} keyboard={index} control={target:?}"
                    );
                    assert_eq!(
                        vertical_tab_bar_layout(&tabs, 240.0, 32.0, anchor.resolve(&tabs)),
                        vertical,
                        "vertical active={active} keyboard={index} control={target:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn leaving_chrome_restores_active_tab_tracking_without_a_stale_anchor() {
        let tabs = tabs(20, 0);
        let mut anchor = TabBarViewportAnchor::default();
        anchor.set_focus(Some(TabBarClick::NextTab));
        assert_eq!(anchor.resolve(&tabs), 0);
        anchor.set_focus(Some(TabBarClick::Tab(19)));
        anchor.set_focus(Some(TabBarClick::Gear));
        assert_eq!(anchor.resolve(&tabs), 19);
        anchor.set_focus(None);
        assert_eq!(anchor.resolve(&tabs), 0);
        anchor.set_focus(Some(TabBarClick::Tab(7)));
        assert_eq!(anchor.resolve(&tabs), 7);
        let smaller = &tabs[..3];
        let layout = vertical_tab_bar_layout(smaller, 160.0, 32.0, anchor.resolve(smaller));
        assert!(layout.tabs.iter().any(|slot| slot.index == 2));
    }

    #[test]
    fn vertical_tab_bar_keeps_active_tab_visible_and_reserves_controls() {
        let tabs = tabs(20, 17);
        let layout = vertical_tab_bar_layout(&tabs, 600.0, 32.0, 17);

        assert!(layout.previous_y.is_some());
        assert!(layout.tabs.iter().any(|slot| slot.index == 17));
        assert!(layout.tabs.iter().all(|slot| slot.y >= 64.0));
        assert!(layout.tabs.iter().all(|slot| slot.y + slot.height <= 536.0));
        assert_eq!(layout.add_y, 568.0);
    }

    #[test]
    fn short_vertical_rail_does_not_cover_the_active_tab_with_next() {
        let tabs = tabs(20, 10);
        let layout = vertical_tab_bar_layout(&tabs, 128.0, 32.0, 10);
        let active = layout.tabs.iter().find(|slot| slot.index == 10).unwrap();
        assert_eq!(
            vertical_tab_bar_hit_test(
                &layout,
                &tabs,
                20.0,
                active.y + active.height / 2.0,
                220.0,
                TabChrome::new(1.0),
            ),
            TabBarClick::Tab(10)
        );
    }

    #[test]
    fn vertical_controls_stay_disjoint_in_bounds_and_hit_their_rendered_centers() {
        for row_height in [28.0, 32.0, 64.0, 90.0] {
            for rows in [0.0, 0.5, 1.0, 1.1, 2.0, 2.9, 3.0, 3.9, 4.0, 4.9, 5.0, 6.0] {
                let height = row_height * rows;
                for count in [0, 1, 2, 20] {
                    for anchor in 0..count.max(1) {
                        let tabs = tabs(count, anchor);
                        let layout = vertical_tab_bar_layout(&tabs, height, row_height, anchor);
                        let mut rectangles = Vec::new();
                        for target in [
                            TabBarClick::PreviousTab,
                            TabBarClick::NextTab,
                            TabBarClick::NewTab,
                        ] {
                            if let Some(
                                rect @ Bounds {
                                    x,
                                    y,
                                    width: w,
                                    height: h,
                                },
                            ) = layout.control_rect(target, 220.0)
                            {
                                assert!(x >= 0.0 && y >= row_height && w > 0.0 && h > 0.0);
                                assert!(x + w <= 220.001 && y + h <= height + 0.001);
                                assert_eq!(
                                    vertical_tab_bar_hit_test(
                                        &layout,
                                        &tabs,
                                        x + w / 2.0,
                                        y + h / 2.0,
                                        220.0,
                                        TabChrome::new(1.0),
                                    ),
                                    target
                                );
                                rectangles.push(rect);
                            }
                        }
                        if !tabs.is_empty() && rows >= 3.0 {
                            assert!(layout.tabs.iter().any(|slot| slot.index == anchor));
                        }
                        for slot in &layout.tabs {
                            assert!(slot.y >= row_height && slot.y + slot.height <= height + 0.001);
                            assert_eq!(
                                vertical_tab_bar_hit_test(
                                    &layout,
                                    &tabs,
                                    20.0,
                                    slot.y + slot.height / 2.0,
                                    220.0,
                                    TabChrome::new(1.0),
                                ),
                                TabBarClick::Tab(slot.index)
                            );
                            rectangles.push(Bounds::new(0.0, slot.y, 220.0, slot.height));
                        }
                        for (index, rect) in rectangles.iter().enumerate() {
                            for other in &rectangles[index + 1..] {
                                assert!(
                                    rect.intersect(*other).is_none_or(|overlap| {
                                        overlap.width <= 0.001 || overlap.height <= 0.001
                                    }),
                                    "rows={rows} count={count} anchor={anchor}: {rectangles:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn vertical_tab_width_preserves_a_minimum_terminal_viewport() {
        assert_eq!(vertical_tab_bar_width(800.0, 1.0), 220.0);
        assert_eq!(vertical_tab_bar_width(320.0, 1.0), 160.0);
        assert_eq!(vertical_tab_bar_width(1_600.0, 2.0), 440.0);
    }

    #[test]
    fn vertical_tab_hit_testing_uses_the_rendered_slots_and_controls() {
        let tabs = tabs(20, 17);
        let layout = vertical_tab_bar_layout(&tabs, 600.0, 32.0, 17);
        let active = layout
            .tabs
            .iter()
            .find(|slot| slot.index == 17)
            .expect("active slot");
        assert_eq!(
            vertical_tab_bar_hit_test(
                &layout,
                &tabs,
                20.0,
                active.y + 5.0,
                220.0,
                TabChrome::new(1.0)
            ),
            TabBarClick::Tab(17)
        );
        assert_eq!(
            vertical_tab_bar_hit_test(
                &layout,
                &tabs,
                205.0,
                active.y + active.height / 2.0,
                220.0,
                TabChrome::new(1.0),
            ),
            TabBarClick::CloseTab(17)
        );
        assert_eq!(
            vertical_tab_bar_hit_test(
                &layout,
                &tabs,
                20.0,
                layout.previous_y.unwrap() + 5.0,
                220.0,
                TabChrome::new(1.0),
            ),
            TabBarClick::PreviousTab
        );
        assert_eq!(
            vertical_tab_bar_hit_test(
                &layout,
                &tabs,
                20.0,
                layout.add_y + 5.0,
                220.0,
                TabChrome::new(1.0)
            ),
            TabBarClick::NewTab
        );
    }

    #[test]
    fn horizontal_hit_testing_and_element_bounds_share_one_geometry() {
        for scale in [1.0, 1.25, 2.0] {
            let chrome = TabChrome::new(scale);
            let height = (34.0_f32 * scale).round();
            for (count, width) in [(3, 900.0 * scale), (14, 900.0 * scale)] {
                let tabs = tabs(count, 1);
                let layout = tab_bar_layout(&tabs, width, height, 1, scale);
                let mut targets = vec![
                    TabBarClick::NewTab,
                    TabBarClick::CommandsButton,
                    TabBarClick::SessionDropdown,
                    TabBarClick::Gear,
                    TabBarClick::PreviousTab,
                    TabBarClick::NextTab,
                ];
                for slot in &layout.tabs {
                    targets.push(TabBarClick::Tab(slot.index));
                    targets.push(TabBarClick::CloseTab(slot.index));
                }
                for target in targets {
                    let Some(rect) =
                        horizontal_tab_bar_element_rect(&layout, target, width, height, chrome)
                    else {
                        continue;
                    };
                    // Tab bodies report the close button at their centre only
                    // when it overlaps; probe a point clear of it.
                    let probe_x = match target {
                        TabBarClick::Tab(_) => rect.x + 4.0,
                        _ => rect.x + rect.width / 2.0,
                    };
                    let probe_y = rect.y + rect.height / 2.0;
                    assert_eq!(
                        horizontal_tab_bar_hit_test(
                            &layout, &tabs, probe_x, probe_y, width, height, chrome
                        ),
                        target,
                        "scale={scale} count={count} rect={rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn overflow_arrows_are_drawn_where_they_are_hit_and_keep_their_slots() {
        for scale in [1.0, 1.25, 2.0] {
            let chrome = TabChrome::new(scale);
            let height = (34.0_f32 * scale).round();
            let width = 900.0 * scale;
            let tabs = tabs(20, 0);
            let mut slots = Vec::new();
            for anchor in [0, 10, 19] {
                let layout = tab_bar_layout(&tabs, width, height, anchor, scale);
                let buttons = horizontal_toolbar_buttons(&layout, width, height, chrome);
                for &(target, rect, enabled) in &buttons {
                    let hit = horizontal_tab_bar_hit_test(
                        &layout,
                        &tabs,
                        rect.x + rect.width / 2.0,
                        rect.y + rect.height / 2.0,
                        width,
                        height,
                        chrome,
                    );
                    if enabled {
                        assert_eq!(
                            Some(rect),
                            horizontal_tab_bar_element_rect(&layout, target, width, height, chrome)
                        );
                        assert_eq!(hit, target, "scale={scale} anchor={anchor}");
                    } else {
                        assert_ne!(hit, target, "a disabled arrow takes no clicks");
                    }
                }
                slots.push(
                    buttons
                        .iter()
                        .filter(|(target, ..)| {
                            matches!(target, TabBarClick::PreviousTab | TabBarClick::NextTab)
                        })
                        .map(|&(target, rect, _)| (target, rect))
                        .collect::<Vec<_>>(),
                );
            }
            // Both arrows show at either end of the strip, in the same slots.
            assert_eq!(slots[0].len(), 2);
            assert!(slots.iter().all(|arrows| arrows == &slots[0]));
            // The first tabs hide nothing before them, the last nothing after.
            let first = tab_bar_layout(&tabs, width, height, 0, scale);
            assert!(!first.previous.unwrap().enabled && first.next.unwrap().enabled);
        }
    }

    #[test]
    fn the_rename_caret_follows_the_markers_and_title_inside_the_tab() {
        let metrics = UiFontMetrics {
            ascent: 10.6,
            descent: 3.2,
        };
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let chrome = TabChrome::new(scale);
            let height = (34.0_f32 * scale).round();
            let mut tab = tabs(1, 0).remove(0);
            tab.tab_type = TabType::Plugin {
                plugin_id: "fake".into(),
            };
            tab.pinned = true;
            tab.renaming = true;
            let layout = tab_bar_layout(&[tab.clone()], 1000.0 * scale, height, 0, scale);
            let surface = chrome.horizontal_surface(&layout.tabs[0], height);
            let plain = chrome.label(&tabs(1, 0)[0], surface, 0.0);
            let label = chrome.label(&tab, surface, chrome.px(18.0));
            let (dot, status) = (label.plugin_dot.unwrap(), label.status.unwrap().1);
            assert_eq!(status.x, dot.right() + chrome.px(6.0));
            assert_eq!(label.text_x, status.right() + chrome.px(5.0));
            // The badge's room comes off the title's, not its start.
            assert_eq!(
                label.text_right,
                plain.text_right - chrome.px(18.0) - chrome.px(6.0)
            );

            let title_width = 40.5 * scale;
            let caret = chrome.rename_caret(&label, title_width, surface, metrics);
            assert_eq!(caret.x, (label.text_x + title_width).round());
            assert!(caret.x < label.text_right);
            assert!(caret.y >= surface.y && caret.bottom() <= surface.bottom());
            for value in [caret.x, caret.y, caret.width, caret.height] {
                assert_eq!(value.fract(), 0.0, "scale={scale} caret={caret:?}");
            }
        }
    }

    #[test]
    fn tab_chrome_rectangles_stay_on_whole_pixels_at_fractional_scales() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let chrome = TabChrome::new(scale);
            let height = (34.0_f32 * scale).round();
            let tabs = tabs(4, 0);
            let layout = tab_bar_layout(&tabs, 1000.0 * scale, height, 0, scale);
            for slot in &layout.tabs {
                let surface = chrome.horizontal_surface(slot, height);
                let close = chrome.close_rect(surface);
                for value in [
                    surface.x,
                    surface.y,
                    surface.width,
                    surface.height,
                    close.x,
                    close.y,
                    close.width,
                ] {
                    assert_eq!(
                        value.fract(),
                        0.0,
                        "scale={scale} surface={surface:?} close={close:?}"
                    );
                }
                assert!(close.x >= surface.x && close.x + close.width <= surface.x + surface.width);
                assert!(
                    close.y >= surface.y && close.y + close.height <= surface.y + surface.height
                );
            }
        }
    }

    #[test]
    fn a_dragged_tab_is_drawn_last_at_the_pointer_within_the_strip() {
        let spans = [(0, 10.0, 100.0), (1, 110.0, 100.0), (2, 210.0, 100.0)];
        let drag = TabDragVisual {
            dragged: Some((1, 147.4)),
            sliding: vec![(2, -60.6)],
            capture: false,
        };
        assert_eq!(
            place_tabs(&spans, &drag),
            [(0, 10.0, 100.0), (2, 149.0, 100.0), (1, 147.0, 100.0)]
        );
        // Dragged past either end, it stops at the strip's edge.
        let far = TabDragVisual {
            dragged: Some((0, 900.0)),
            sliding: Vec::new(),
            capture: false,
        };
        assert_eq!(place_tabs(&spans, &far).last(), Some(&(0, 210.0, 100.0)));
        let before = TabDragVisual {
            dragged: Some((2, -500.0)),
            sliding: Vec::new(),
            capture: false,
        };
        assert_eq!(place_tabs(&spans, &before).last(), Some(&(2, 10.0, 100.0)));
        assert_eq!(place_tabs(&spans, &TabDragVisual::default()), spans);
    }

    #[test]
    fn tab_status_markers_follow_priority_order() {
        let mut tab = tabs(1, 0).remove(0);
        assert_eq!(tab_status(&tab), None);
        tab.pinned = true;
        assert_eq!(tab_status(&tab), Some(TabStatus::Pinned));
        tab.read_only = true;
        assert_eq!(tab_status(&tab), Some(TabStatus::ReadOnly));
        tab.broadcast_armed = true;
        assert_eq!(tab_status(&tab), Some(TabStatus::BroadcastArmed));
        tab.broadcast_active = true;
        assert_eq!(tab_status(&tab), Some(TabStatus::BroadcastActive));

        let mut tab = tabs(1, 0).remove(0);
        tab.pinned = true;
        tab.agent_badge = Some(AgentBadge::Working);
        assert_eq!(
            tab_status(&tab),
            Some(TabStatus::Agent(AgentBadge::Working))
        );
        tab.read_only = true;
        assert_eq!(tab_status(&tab), Some(TabStatus::ReadOnly));
        tab.agent_badge = Some(AgentBadge::NeedsYou);
        assert_eq!(
            tab_status(&tab),
            Some(TabStatus::Agent(AgentBadge::NeedsYou))
        );
        tab.broadcast_armed = true;
        assert_eq!(tab_status(&tab), Some(TabStatus::BroadcastArmed));
    }
}
