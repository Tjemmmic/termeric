//! What the control protocol and plugins see of the process's windows:
//! their tabs, panes, and dividers, bounded so the inventory always fits
//! the protocol's limits.

use super::super::App;
use crate::control::protocol::ProcessInventory;
use crate::tabs::TabManager;

/// What an inventory may still list, shared across its windows.
pub(in crate::app) struct ControlInventoryBudget {
    tabs: usize,
    panes: usize,
    dividers: usize,
    metadata_bytes: usize,
}

/// A plugin's workspace query lists less than a control client may, so its
/// answer fits in one handler event.
pub(in crate::app) const PLUGIN_PROCESS_MAX_WINDOWS: usize = 8;
const PLUGIN_PROCESS_MAX_TABS: usize = 16;
const PLUGIN_PROCESS_MAX_PANES: usize = 32;
const PLUGIN_PROCESS_MAX_DIVIDERS: usize = 31;
const PLUGIN_PROCESS_MAX_METADATA_BYTES: usize = 2 * 1024;

impl ControlInventoryBudget {
    fn for_plugins() -> Self {
        Self {
            tabs: PLUGIN_PROCESS_MAX_TABS,
            panes: PLUGIN_PROCESS_MAX_PANES,
            dividers: PLUGIN_PROCESS_MAX_DIVIDERS,
            metadata_bytes: PLUGIN_PROCESS_MAX_METADATA_BYTES,
        }
    }
}

impl Default for ControlInventoryBudget {
    fn default() -> Self {
        Self {
            tabs: crate::control::protocol::MAX_TOTAL_TABS,
            panes: crate::control::protocol::MAX_TOTAL_PANES,
            dividers: crate::control::protocol::MAX_TOTAL_DIVIDERS,
            metadata_bytes: crate::control::protocol::MAX_METADATA_BYTES,
        }
    }
}

/// The inventory of `windows` (each with its control ID, in creation
/// order): the first `window_limit` of them, within `budget`.
///
/// Closing windows are left out. Every request that names a window, from a
/// control client or a plugin, refuses one that is closing
/// (`Application::live_window`), so listing it would offer an ID nothing
/// accepts; and it is gone once the current event is handled.
pub(super) fn process_inventory(
    windows: &[(u64, &App)],
    window_limit: usize,
    mut budget: ControlInventoryBudget,
) -> ProcessInventory {
    let live: Vec<_> = windows
        .iter()
        .filter(|(_, window)| !window.should_exit)
        .collect();
    let mut truncated = live.len() > window_limit;
    let mut inventories = Vec::with_capacity(live.len().min(window_limit));
    for (control_id, window) in live.into_iter().take(window_limit) {
        let inventory = window.control_inventory(*control_id, &mut budget);
        truncated |= inventory.truncated;
        inventories.push(inventory);
    }
    ProcessInventory {
        process_id: std::process::id(),
        windows: inventories,
        truncated,
    }
}

/// The inventory a plugin's workspace query answers with.
pub(in crate::app) fn plugin_inventory(windows: &[(u64, &App)]) -> ProcessInventory {
    process_inventory(
        windows,
        PLUGIN_PROCESS_MAX_WINDOWS,
        ControlInventoryBudget::for_plugins(),
    )
}

impl App {
    pub(in crate::app) fn control_inventory(
        &self,
        id: u64,
        budget: &mut ControlInventoryBudget,
    ) -> crate::control::protocol::WindowInventory {
        use crate::control::protocol::WindowInventory;

        let (pixel_width, pixel_height) = self
            .renderer
            .as_ref()
            .map(crate::renderer::Renderer::window_size)
            .unwrap_or((self.config.window.width, self.config.window.height));

        let (tabs, mut truncated) = match self.tabs.as_ref() {
            Some(tabs) => self.control_tab_inventory(tabs, budget),
            None => (Vec::new(), false),
        };
        let (workspace_id, workspace_truncated) =
            self.current_workspace_id
                .as_deref()
                .map_or((None, false), |workspace| {
                    let (workspace, was_truncated) =
                        control_metadata(workspace, &mut budget.metadata_bytes);
                    (Some(workspace), was_truncated)
                });
        truncated |= workspace_truncated;
        WindowInventory {
            id,
            focused: self.window_focused,
            visible: if self.launch.quick_access {
                self.quick_access_visible
            } else {
                self.window
                    .as_ref()
                    .and_then(|window| window.is_visible())
                    .unwrap_or(self.window.is_some())
            },
            quick_access: self.launch.quick_access,
            workspace_id,
            pixel_width,
            pixel_height,
            tabs,
            truncated,
        }
    }

    /// The window's tabs as control inventory, and whether any were left out
    /// for the budget.
    fn control_tab_inventory(
        &self,
        tabs: &TabManager,
        budget: &mut ControlInventoryBudget,
    ) -> (Vec<crate::control::protocol::TabInventory>, bool) {
        use crate::control::protocol::{
            DividerInventory, DividerKind, MAX_DIVIDERS_PER_TAB, MAX_PANES_PER_TAB,
            MAX_TABS_PER_WINDOW, PaneInventory, TabInventory, TabKind,
        };

        let active = tabs.active_tab_id();
        let all_tabs = tabs.all_tabs();
        let tab_limit = MAX_TABS_PER_WINDOW.min(budget.tabs);
        let truncated = all_tabs.len() > tab_limit;
        budget.tabs -= all_tabs.len().min(tab_limit);
        let tabs = all_tabs
            .iter()
            .take(tab_limit)
            .map(|tab| {
                let mut tab_truncated = false;
                let manager = &tab.panes;
                let focused = manager.focused_id();
                let all = manager.iter_panes();
                let pane_limit = MAX_PANES_PER_TAB.min(budget.panes);
                tab_truncated |= all.len() > pane_limit;
                budget.panes -= all.len().min(pane_limit);
                let mut panes = all
                    .into_iter()
                    .take(pane_limit)
                    .map(|pane| PaneInventory {
                        id: pane.id.0,
                        // A tab's only pane has no title of its own.
                        title: pane.title.as_deref().map(|title| {
                            let (title, title_truncated) =
                                control_metadata(title, &mut budget.metadata_bytes);
                            tab_truncated |= title_truncated;
                            title
                        }),
                        focused: pane.id == focused,
                        closed: pane.closed,
                        cols: pane.terminal.cols().min(u16::MAX as usize) as u16,
                        rows: pane.terminal.rows().min(u16::MAX as usize) as u16,
                    })
                    .collect::<Vec<_>>();
                panes.sort_by_key(|pane| pane.id);
                let all = manager.get_dividers();
                let divider_limit = MAX_DIVIDERS_PER_TAB
                    .min(budget.dividers)
                    .min(panes.len().saturating_sub(1));
                tab_truncated |= all.len() > divider_limit;
                budget.dividers -= all.len().min(divider_limit);
                let mut dividers = all
                    .into_iter()
                    .take(divider_limit)
                    .map(|divider| {
                        let (id, kind) = match divider.target {
                            crate::panes::DividerTarget::Local(id) => (id, DividerKind::Local),
                            crate::panes::DividerTarget::Domain(id) => {
                                (u64::from(id), DividerKind::LiveDomain)
                            }
                        };
                        DividerInventory {
                            id,
                            kind,
                            direction: if divider.vertical {
                                crate::panes::SplitDirection::Vertical
                            } else {
                                crate::panes::SplitDirection::Horizontal
                            },
                            ratio_millis: divider.ratio_millis,
                        }
                    })
                    .collect::<Vec<_>>();
                dividers.sort_by_key(|divider| divider.id);
                let domain_id = tab.live_domain_tab_route().map(|(domain, _)| {
                    let (domain, domain_truncated) =
                        control_metadata(domain, &mut budget.metadata_bytes);
                    tab_truncated |= domain_truncated;
                    domain
                });
                let kind = if domain_id.is_some() {
                    TabKind::LiveDomain
                } else {
                    match &tab.tab_type {
                        crate::tabs::TabType::Shell => TabKind::Shell,
                        crate::tabs::TabType::Profile { .. }
                        | crate::tabs::TabType::Serial { .. } => TabKind::Profile,
                        crate::tabs::TabType::Plugin { .. } => TabKind::Plugin,
                    }
                };
                let (title, title_truncated) =
                    control_metadata(&tab.title, &mut budget.metadata_bytes);
                tab_truncated |= title_truncated;
                TabInventory {
                    id: tab.id as u64,
                    title,
                    kind,
                    active: active == Some(tab.id),
                    pinned: tab.pinned,
                    activity: tab.has_activity,
                    closed: tab.closed || tab.closing_at.is_some(),
                    domain_id,
                    panes,
                    dividers,
                    truncated: tab_truncated,
                }
            })
            .collect();
        (tabs, truncated)
    }
}

pub(in crate::app) fn control_metadata(value: &str, remaining: &mut usize) -> (String, bool) {
    const MAX_FIELD_BYTES: usize = 1_024;
    let limit = (*remaining).min(MAX_FIELD_BYTES);
    let mut output = String::with_capacity(value.len().min(limit));
    let mut truncated = false;
    for character in value.chars() {
        if crate::safe_text::is_unsafe_display_char(character) {
            truncated = true;
            continue;
        }
        if output.len() + character.len_utf8() > limit {
            truncated = true;
            break;
        }
        output.push(character);
    }
    *remaining -= output.len();
    (output, truncated)
}

#[cfg(test)]
mod control_inventory_tests {
    use super::control_metadata;

    #[test]
    fn metadata_removes_controls_without_splitting_utf8_or_exceeding_budget() {
        let mut remaining = 5;
        let (output, truncated) = control_metadata("a\u{1b}界bc", &mut remaining);
        assert_eq!(output, "a界b");
        assert_eq!(remaining, 0);
        assert!(truncated);
    }
}
