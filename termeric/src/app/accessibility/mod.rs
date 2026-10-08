//! Incremental AccessKit tree for native screen readers. This module holds
//! the node IDs and registries, sends tree updates, dispatches actions, and
//! builds the ordinary tree; the top modal layer's tree comes from the child
//! module for its surface.

use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use accesskit::{
    Action, ActionData, ActionRequest, CustomAction, Live, Node, NodeId, Rect, Role, TextPosition,
    TextSelection, Toggled, Tree, TreeId, TreeUpdate,
};
use accesskit_winit::WindowEvent;
use termeric_core::{Cell, CellAttrs, CellFlags, Grid, Selection};

use super::modal_layer::ModalLayer;
use super::palette_input::PaletteModal;
use super::*;
use crate::ui::commands::navigation::{
    CommandControl, CommandKey as CustomCommandSemanticKey, command_index as custom_command_index,
    command_keys as custom_command_keys,
};
use crate::ui::geometry::Bounds;
use crate::ui::text_card::OverlayScroll;

mod chrome;
mod dialogs;
mod harvest;
mod overlays;
mod palette;
mod panels;
mod terminal;

use chrome::ChromeFocus;
use panels::workspace_menu_row;
use terminal::append_terminal_nodes;

const ROOT: NodeId = NodeId(1);
const TAB_LIST: NodeId = NodeId(2);
const NEW_TAB: NodeId = NodeId(3);
const WORKSPACES: NodeId = NodeId(4);
const COMMANDS: NodeId = NodeId(5);
const SETTINGS: NodeId = NodeId(6);
const PREVIOUS_TAB: NodeId = NodeId(7);
const NEXT_TAB: NodeId = NodeId(8);
const PALETTE_DIALOG: NodeId = NodeId(20);
const PALETTE_INPUT: NodeId = NodeId(21);
const PALETTE_LIST: NodeId = NodeId(22);
const PALETTE_EMPTY: NodeId = NodeId(23);
const SETTINGS_DIALOG: NodeId = NodeId(30);
const SETTINGS_INPUT: NodeId = NodeId(31);
const SETTINGS_LIST: NodeId = NodeId(32);
const SETTINGS_EMPTY: NodeId = NodeId(33);
const SETTINGS_DROPDOWN: NodeId = NodeId(34);
const SETTINGS_CLOSE: NodeId = NodeId(35);
const SETTINGS_SAVE: NodeId = NodeId(36);
const SETTINGS_PAGES: NodeId = NodeId(37);
const COMMANDS_DIALOG: NodeId = NodeId(40);
const COMMANDS_NEW: NodeId = NodeId(41);
const COMMANDS_CLOSE: NodeId = NodeId(42);
const COMMANDS_LIST: NodeId = NodeId(43);
const COMMANDS_EMPTY: NodeId = NodeId(44);
const COMMANDS_NAME: NodeId = NodeId(45);
const COMMANDS_COMMAND: NodeId = NodeId(46);
const COMMANDS_SAVE: NodeId = NodeId(47);
const COMMANDS_CANCEL: NodeId = NodeId(48);
const WORKSPACE_DIALOG: NodeId = NodeId(50);
const WORKSPACE_CLOSE: NodeId = NodeId(51);
const WORKSPACE_LIST: NodeId = NodeId(52);
const WORKSPACE_EMPTY: NodeId = NodeId(53);
const WORKSPACE_RENAME_INPUT: NodeId = NodeId(54);
const WORKSPACE_RENAME_SAVE: NodeId = NodeId(55);
const WORKSPACE_RENAME_CANCEL: NodeId = NodeId(56);
const WORKSPACE_BACK: NodeId = NodeId(57);
const WORKSPACE_SNAPSHOTS: NodeId = NodeId(58);
const SHORTCUTS_DIALOG: NodeId = NodeId(60);
const SHORTCUTS_INPUT: NodeId = NodeId(61);
const SHORTCUTS_LIST: NodeId = NodeId(62);
const SHORTCUTS_EMPTY: NodeId = NodeId(63);
const SHORTCUTS_STATUS: NodeId = NodeId(64);
const DOMAIN_DIALOG: NodeId = NodeId(70);
const DOMAIN_INPUT: NodeId = NodeId(71);
const DOMAIN_LIST: NodeId = NodeId(72);
const DOMAIN_EMPTY: NodeId = NodeId(73);
const GUARDRAIL_DIALOG: NodeId = NodeId(80);
const GUARDRAIL_COMMAND: NodeId = NodeId(81);
const GUARDRAIL_REASON: NodeId = NodeId(82);
const GUARDRAIL_CONTEXT: NodeId = NodeId(83);
const INFO_DIALOG: NodeId = NodeId(90);
const INFO_TITLE: NodeId = NodeId(91);
const INFO_STATUS: NodeId = NodeId(92);
const DIFF_DIALOG: NodeId = NodeId(100);
const DIFF_TITLE: NodeId = NodeId(101);
const DIFF_STATUS: NodeId = NodeId(102);
const CONTEXT_DIALOG: NodeId = NodeId(110);
const CONTEXT_TITLE: NodeId = NodeId(111);
const CONTEXT_STATUS: NodeId = NodeId(112);
const SEARCH_TOOLBAR: NodeId = NodeId(120);
const SEARCH_INPUT: NodeId = NodeId(121);
const SEARCH_STATUS: NodeId = NodeId(122);
const SEARCH_FILTER: NodeId = NodeId(123);
const SEARCH_CASE: NodeId = NodeId(124);
const SEARCH_REGEX: NodeId = NodeId(125);
const SEARCH_PREVIOUS: NodeId = NodeId(126);
const SEARCH_NEXT: NodeId = NodeId(127);
const SEARCH_CLOSE: NodeId = NodeId(128);
const CELLAR_DIALOG: NodeId = NodeId(130);
const CELLAR_TITLE: NodeId = NodeId(131);
const CELLAR_STATUS: NodeId = NodeId(132);
const CELLAR_LIST: NodeId = NodeId(133);
const CELLAR_CLOSE: NodeId = NodeId(134);
const SPROUT_DIALOG: NodeId = NodeId(140);
const SPROUT_MENU: NodeId = NodeId(141);
const UNICODE_DIALOG: NodeId = NodeId(150);
const UNICODE_INPUT: NodeId = NodeId(151);
const UNICODE_LIST: NodeId = NodeId(152);
const UNICODE_EMPTY: NodeId = NodeId(153);
const WORKSPACE_MENU: NodeId = NodeId(160);
const AGENTS_DIALOG: NodeId = NodeId(170);
const AGENTS_CLOSE: NodeId = NodeId(171);
const AGENTS_LIST: NodeId = NodeId(172);
const AGENTS_EMPTY: NodeId = NodeId(173);
const AGENTS_SUPERVISOR: NodeId = NodeId(174);
const AGENTS_SUPERVISOR_TOGGLE: NodeId = NodeId(175);
const AGENTS_ACTIVITY: NodeId = NodeId(176);
const AGENT_VIEW_PANE: NodeId = NodeId(177);
const SHORTCUT_CLEAR_ACTION: i32 = 1;
const TOAST_DISMISS_ACTION: i32 = 2;
const TAB_BASE: u64 = 1_000;
const PANE_BASE: u64 = 10_000;
const LINE_BASE: u64 = 1_000_000;
const WORKSPACE_ENTRY_BASE: u64 = 1 << 57;
const SHORTCUTS_ENTRY_BASE: u64 = 1 << 56;
const DOMAIN_ENTRY_BASE: u64 = 1 << 55;
const GUARDRAIL_ACTION_BASE: u64 = 1 << 54;
const INFO_ACTION_BASE: u64 = 1 << 53;
const DIFF_ACTION_BASE: u64 = 1 << 52;
const CONTEXT_ACTION_BASE: u64 = 1 << 51;
const TOAST_BASE: u64 = 1 << 50;
const HARVEST_ACTION_BASE: u64 = 1 << 49;
const UNICODE_ENTRY_BASE: u64 = 1 << 48;
const COMMANDS_ENTRY_BASE: u64 = 1 << 58;
const SETTINGS_ENTRY_BASE: u64 = 1 << 59;
const PALETTE_ENTRY_BASE: u64 = 1 << 60;
const WORKSPACE_MENU_ENTRY_BASE: u64 = 1 << 47;
const AGENTS_ENTRY_BASE: u64 = 1 << 46;
const AGENTS_ACTIVITY_BASE: u64 = 1 << 45;
const AGENT_VIEW_ENTRY_BASE: u64 = 1 << 44;
const AGENT_VIEW_TEXT_BASE: u64 = 1 << 43;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SettingsSemanticTarget {
    Page(crate::ui::settings::SettingsPage),
    Header(crate::ui::settings::SettingsSection),
    Setting {
        section: crate::ui::settings::SettingsSection,
        label: &'static str,
    },
    Option {
        section: crate::ui::settings::SettingsSection,
        label: &'static str,
        value: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum CommandsSemanticTarget {
    Group(CustomCommandSemanticKey),
    Run(CustomCommandSemanticKey),
    Delete(CustomCommandSemanticKey),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WorkspaceSemanticKey {
    id: String,
    workspaces: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum WorkspaceSemanticTarget {
    Group(WorkspaceSemanticKey),
    Load(WorkspaceSemanticKey),
    Rename(WorkspaceSemanticKey),
    Delete(WorkspaceSemanticKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ShortcutsSemanticTarget {
    Header(crate::input::keybindings::ShortcutCategory),
    Action(crate::input::keybindings::BindableAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GuardrailSemanticTarget {
    Confirm(u64),
    Cancel(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum InfoSemanticTarget {
    Content(u64),
    Input(u64),
    Copy(u64),
    Dismiss(u64),
    Choice { request_id: u64, id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DiffSemanticTarget {
    Content(u64),
    Copy(u64),
    Dismiss(u64),
    Stage(u64),
    Revert(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ContextSemanticTarget {
    List(u64),
    Source {
        request_id: u64,
        source: crate::ui::context_picker::ContextSourceId,
    },
    FileInput(u64),
    Cancel(u64),
    Primary(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum HarvestSemanticTarget {
    CellarEntry(u64),
    SproutAction {
        request_id: u64,
        action: crate::harvest::sprouts::SproutAction,
    },
}

/// Stable AccessKit nodes for one surface's semantic targets. A target keeps
/// its node while it stays listed; nodes count up from the surface's own
/// base, so no two surfaces share an ID.
struct NodeRegistry<K> {
    nodes: HashMap<K, NodeId>,
    targets: HashMap<NodeId, K>,
    next: u64,
}

impl<K: Clone + Eq + Hash> NodeRegistry<K> {
    fn new(base: u64) -> Self {
        Self {
            nodes: HashMap::new(),
            targets: HashMap::new(),
            next: base,
        }
    }

    /// The node for `target`, allocated the first time it is seen. A
    /// borrowed key (`&str` for a `String` registry) allocates only then.
    fn node<Q>(&mut self, target: &Q) -> NodeId
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized,
    {
        if let Some(node) = self.nodes.get(target) {
            return *node;
        }
        let node = NodeId(self.next);
        self.next = self.next.saturating_add(1);
        let target = target.to_owned();
        self.targets.insert(node, target.clone());
        self.nodes.insert(target, node);
        node
    }

    fn contains(&self, node: NodeId) -> bool {
        self.targets.contains_key(&node)
    }

    /// Forget every target `keep` rejects, so an action aimed at its node
    /// finds nothing. A forgotten node is never handed out again.
    fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.nodes.retain(|target, _| keep(target));
        let nodes = &self.nodes;
        self.targets.retain(|_, target| nodes.contains_key(target));
    }

    fn clear(&mut self) {
        self.nodes.clear();
        self.targets.clear();
    }
}

pub(super) struct AccessibilityState {
    active: bool,
    initialized: bool,
    previous: HashMap<NodeId, Node>,
    focus_override: Option<NodeId>,
    keyboard_focus: ChromeFocus,
    palette: NodeRegistry<String>,
    settings: NodeRegistry<SettingsSemanticTarget>,
    commands: NodeRegistry<CommandsSemanticTarget>,
    /// Agent rows, by the terminal they show, so a row keeps its node as
    /// urgency reorders the list.
    agents: NodeRegistry<crate::ui::agents_panel::AgentOrigin>,
    workspace: NodeRegistry<WorkspaceSemanticTarget>,
    shortcuts: NodeRegistry<ShortcutsSemanticTarget>,
    domain: NodeRegistry<String>,
    unicode: NodeRegistry<String>,
    guardrail: NodeRegistry<GuardrailSemanticTarget>,
    info: NodeRegistry<InfoSemanticTarget>,
    diff: NodeRegistry<DiffSemanticTarget>,
    context: NodeRegistry<ContextSemanticTarget>,
    harvest: NodeRegistry<HarvestSemanticTarget>,
    agent_view: NodeRegistry<crate::ui::agent_view::scene::Target>,
}

impl Default for AccessibilityState {
    fn default() -> Self {
        Self {
            active: false,
            initialized: false,
            previous: HashMap::new(),
            focus_override: None,
            keyboard_focus: ChromeFocus::default(),
            palette: NodeRegistry::new(PALETTE_ENTRY_BASE),
            settings: NodeRegistry::new(SETTINGS_ENTRY_BASE),
            commands: NodeRegistry::new(COMMANDS_ENTRY_BASE),
            agents: NodeRegistry::new(AGENTS_ENTRY_BASE),
            workspace: NodeRegistry::new(WORKSPACE_ENTRY_BASE),
            shortcuts: NodeRegistry::new(SHORTCUTS_ENTRY_BASE),
            domain: NodeRegistry::new(DOMAIN_ENTRY_BASE),
            unicode: NodeRegistry::new(UNICODE_ENTRY_BASE),
            guardrail: NodeRegistry::new(GUARDRAIL_ACTION_BASE),
            info: NodeRegistry::new(INFO_ACTION_BASE),
            diff: NodeRegistry::new(DIFF_ACTION_BASE),
            context: NodeRegistry::new(CONTEXT_ACTION_BASE),
            harvest: NodeRegistry::new(HARVEST_ACTION_BASE),
            agent_view: NodeRegistry::new(AGENT_VIEW_ENTRY_BASE),
        }
    }
}

struct SemanticSnapshot {
    nodes: Vec<(NodeId, Node)>,
    focus: NodeId,
}

/// A modal layer's nodes: its dialog, which the window root holds, and the
/// nodes under it.
struct ModalSnapshot {
    dialog: NodeId,
    nodes: Vec<(NodeId, Node)>,
    focus: NodeId,
}

impl AccessibilityState {
    /// The focus a modal layer reports: the node assistive technology moved
    /// to while it is still among `nodes` and `allowed`, else
    /// `default_focus`. A request that went stale is replaced by the default;
    /// with no request, focus keeps following the layer's own.
    fn modal_focus(
        &mut self,
        nodes: &[(NodeId, Node)],
        default_focus: NodeId,
        allowed: impl Fn(&Self, NodeId) -> bool,
    ) -> NodeId {
        let Some(requested) = self.focus_override else {
            return default_focus;
        };
        if allowed(self, requested) && nodes.iter().any(|(node, _)| *node == requested) {
            requested
        } else {
            self.focus_override = Some(default_focus);
            default_focus
        }
    }

    fn activate(&mut self) {
        self.active = true;
    }

    fn deactivate(&mut self) {
        self.active = false;
        self.initialized = false;
        self.previous.clear();
        self.focus_override = None;
    }

    fn is_settings_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            SETTINGS_DIALOG
                | SETTINGS_INPUT
                | SETTINGS_LIST
                | SETTINGS_EMPTY
                | SETTINGS_DROPDOWN
                | SETTINGS_CLOSE
                | SETTINGS_SAVE
                | SETTINGS_PAGES
        ) || self.settings.contains(node)
    }

    fn is_agents_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            AGENTS_DIALOG
                | AGENTS_CLOSE
                | AGENTS_LIST
                | AGENTS_EMPTY
                | AGENTS_SUPERVISOR
                | AGENTS_SUPERVISOR_TOGGLE
                | AGENTS_ACTIVITY
        ) || self.agents.contains(node)
            || (AGENTS_ACTIVITY_BASE..AGENTS_ENTRY_BASE).contains(&node.0)
    }

    fn is_commands_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            COMMANDS_DIALOG
                | COMMANDS_NEW
                | COMMANDS_CLOSE
                | COMMANDS_LIST
                | COMMANDS_EMPTY
                | COMMANDS_NAME
                | COMMANDS_COMMAND
                | COMMANDS_SAVE
                | COMMANDS_CANCEL
        ) || self.commands.contains(node)
    }

    fn is_workspace_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            WORKSPACE_DIALOG
                | WORKSPACE_CLOSE
                | WORKSPACE_LIST
                | WORKSPACE_EMPTY
                | WORKSPACE_RENAME_INPUT
                | WORKSPACE_RENAME_SAVE
                | WORKSPACE_RENAME_CANCEL
                | WORKSPACE_BACK
                | WORKSPACE_SNAPSHOTS
        ) || self.workspace.contains(node)
    }

    fn is_shortcuts_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            SHORTCUTS_DIALOG
                | SHORTCUTS_INPUT
                | SHORTCUTS_LIST
                | SHORTCUTS_EMPTY
                | SHORTCUTS_STATUS
        ) || self.shortcuts.contains(node)
    }

    fn is_domain_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            DOMAIN_DIALOG | DOMAIN_INPUT | DOMAIN_LIST | DOMAIN_EMPTY
        ) || self.domain.contains(node)
    }

    fn is_unicode_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            UNICODE_DIALOG | UNICODE_INPUT | UNICODE_LIST | UNICODE_EMPTY
        ) || self.unicode.contains(node)
    }

    fn is_guardrail_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            GUARDRAIL_DIALOG | GUARDRAIL_COMMAND | GUARDRAIL_REASON | GUARDRAIL_CONTEXT
        ) || self.guardrail.contains(node)
    }

    fn is_info_node(&self, node: NodeId) -> bool {
        matches!(node, INFO_DIALOG | INFO_TITLE | INFO_STATUS) || self.info.contains(node)
    }

    fn is_diff_node(&self, node: NodeId) -> bool {
        matches!(node, DIFF_DIALOG | DIFF_TITLE | DIFF_STATUS) || self.diff.contains(node)
    }

    fn is_context_node(&self, node: NodeId) -> bool {
        matches!(node, CONTEXT_DIALOG | CONTEXT_TITLE | CONTEXT_STATUS)
            || self.context.contains(node)
    }

    fn is_harvest_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            CELLAR_DIALOG
                | CELLAR_TITLE
                | CELLAR_STATUS
                | CELLAR_LIST
                | CELLAR_CLOSE
                | SPROUT_DIALOG
                | SPROUT_MENU
        ) || self.harvest.contains(node)
    }

    fn palette_modal(&self, modal: PaletteModal) -> &NodeRegistry<String> {
        match modal {
            PaletteModal::Commands => &self.palette,
            PaletteModal::Domains => &self.domain,
            PaletteModal::Unicode => &self.unicode,
        }
    }

    fn palette_modal_mut(&mut self, modal: PaletteModal) -> &mut NodeRegistry<String> {
        match modal {
            PaletteModal::Commands => &mut self.palette,
            PaletteModal::Domains => &mut self.domain,
            PaletteModal::Unicode => &mut self.unicode,
        }
    }

    fn update(&mut self, snapshot: SemanticSnapshot) -> TreeUpdate {
        let current: HashMap<_, _> = snapshot.nodes.iter().cloned().collect();
        let full = !self.initialized;
        let nodes = if full {
            snapshot.nodes
        } else {
            snapshot
                .nodes
                .into_iter()
                .filter(|(id, node)| self.previous.get(id) != Some(node))
                .collect()
        };
        self.previous = current;
        self.initialized = true;
        TreeUpdate {
            nodes,
            tree: full.then(|| {
                let mut tree = Tree::new(ROOT);
                tree.toolkit_name = Some("Termeric".into());
                tree.toolkit_version = Some(env!("CARGO_PKG_VERSION").into());
                tree
            }),
            tree_id: TreeId::ROOT,
            focus: self.focus_override.unwrap_or(snapshot.focus),
        }
    }
}

impl App {
    pub(super) fn handle_accesskit_event(&mut self, event: accesskit_winit::Event) {
        match event.window_event {
            WindowEvent::InitialTreeRequested => self.accessibility.activate(),
            WindowEvent::ActionRequested(request) => {
                self.accessibility.activate();
                self.handle_accessibility_action(request);
            }
            WindowEvent::AccessibilityDeactivated => {
                self.accessibility.deactivate();
                return;
            }
        }
        self.refresh_accessibility();
    }

    pub(super) fn refresh_accessibility(&mut self) {
        if !self.accessibility.active {
            return;
        }
        self.reconcile_mouse_selection_presentation();
        let snapshot = self.accessibility_snapshot();
        let snapshot = self.with_agent_view_accessibility(snapshot);
        let snapshot = self.with_toast_accessibility(snapshot);
        let update = self.accessibility.update(snapshot);
        if let Some(adapter) = &mut self.accessibility_adapter {
            adapter.update_if_active(move || update);
        }
    }

    fn handle_accessibility_action(&mut self, request: ActionRequest) {
        // Toasts sit above every modal layer, as they do for the mouse.
        if let Some(toast) = toast_id(request.target_node) {
            self.handle_toast_accessibility_action(toast, &request);
            return;
        }
        if let Some(layer) = self.top_modal_layer() {
            match layer {
                ModalLayer::Guardrail => self.handle_guardrail_accessibility_action(request),
                ModalLayer::InfoOverlay => self.handle_info_accessibility_action(request),
                ModalLayer::DiffReview => self.handle_diff_accessibility_action(request),
                ModalLayer::ContextPicker => self.handle_context_accessibility_action(request),
                ModalLayer::SproutMenu | ModalLayer::Cellar => {
                    self.handle_harvest_accessibility_action(request)
                }
                ModalLayer::DomainPicker
                | ModalLayer::UnicodePicker
                | ModalLayer::CommandPalette => {
                    if let Some(modal) = self.open_palette_modal() {
                        self.handle_palette_modal_accessibility_action(modal, request);
                    }
                }
                ModalLayer::ShortcutsPanel => self.handle_shortcuts_accessibility_action(request),
                ModalLayer::Settings => self.handle_settings_accessibility_action(request),
                ModalLayer::WorkspacePanel => self.handle_workspace_accessibility_action(request),
                ModalLayer::WorkspaceMenu => {
                    self.handle_workspace_menu_accessibility_action(request)
                }
                ModalLayer::CommandsPanel => self.handle_commands_accessibility_action(request),
                ModalLayer::AgentsPanel => self.handle_agents_accessibility_action(request),
            }
            self.request_redraw(RedrawReason::Accessibility);
            return;
        }
        if self.handle_agent_view_accessibility_action(&request) {
            self.request_redraw(RedrawReason::Accessibility);
            return;
        }
        if self.search_is_visible() && is_search_node(request.target_node) {
            self.handle_search_accessibility_action(request);
            self.request_redraw(RedrawReason::Accessibility);
            return;
        }
        if !matches!(request.action, Action::Click | Action::Focus) {
            return;
        }
        let id = request.target_node.0;
        if (PANE_BASE..LINE_BASE).contains(&id) {
            self.accessibility.focus_override = Some(request.target_node);
            self.accessibility.keyboard_focus = ChromeFocus::Terminal;
            let pane_id = PaneId((id - PANE_BASE) as u32);
            if let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut)
                && let Some(panes) = tab.split_panes_mut()
                && panes.get_pane(pane_id).is_some()
            {
                panes.focus(pane_id);
                self.refresh_focused_terminal_surfaces();
                self.accessibility.focus_override = None;
            }
        } else if let Some(focus) = self
            .chrome_focus_sequence()
            .into_iter()
            .find(|focus| focus.node() == Some(request.target_node))
        {
            // A screen reader moves the F6 focus ring, and its Click is F6 +
            // Enter on that control: one path, so both leave the same state.
            self.set_chrome_focus(focus);
            if request.action == Action::Click {
                self.activate_chrome_focus();
            }
        } else {
            self.accessibility.focus_override = Some(request.target_node);
        }
        self.request_redraw(RedrawReason::Accessibility);
    }

    fn handle_search_accessibility_action(&mut self, request: ActionRequest) {
        if self.reconcile_search_presentation() {
            return;
        }
        if request.target_node == SEARCH_INPUT {
            match request.action {
                Action::Focus => {
                    // Preserve the caret when possible. A recent insertion may
                    // have joined a grapheme across it; resume at the end then.
                    let reason = redraw_scheduler::RedrawReason::Accessibility;
                    if self.focus_search_query_at(self.search_cursor_pos, reason)
                        || self.focus_search_query_at(self.search.query().len(), reason)
                    {
                        self.accessibility.focus_override = Some(SEARCH_INPUT);
                    }
                }
                Action::SetValue if self.search.is_active() => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self
                        .with_search_and_active_grid(|search, grid| search.set_query(value, grid))
                        .unwrap_or(false)
                    {
                        self.search_cursor_pos = value.len();
                        self.ime.clear_preedit();
                        self.accessibility.focus_override = Some(SEARCH_INPUT);
                    }
                }
                _ => {}
            }
            return;
        }
        if request.action == Action::Focus {
            self.accessibility.focus_override = Some(request.target_node);
            return;
        }
        if request.action != Action::Click {
            return;
        }
        match request.target_node {
            SEARCH_FILTER => {
                if self.copy_mode.is_active() {
                    self.toasts.warning(
                        "Filter mode is unavailable inside copy mode; matches remain highlighted",
                    );
                } else {
                    self.search.toggle_filter();
                }
            }
            SEARCH_CASE => {
                self.with_search_and_active_grid(|search, grid| {
                    search.toggle_case_sensitive(grid);
                });
            }
            SEARCH_REGEX => {
                self.with_search_and_active_grid(|search, grid| search.toggle_regex(grid));
            }
            SEARCH_PREVIOUS if self.search.retained_match_count() > 0 => {
                self.search.prev();
                self.scroll_to_current_match();
            }
            SEARCH_NEXT if self.search.retained_match_count() > 0 => {
                self.search.next();
                self.scroll_to_current_match();
            }
            SEARCH_CLOSE => {
                self.search.cancel();
                self.search_cursor_pos = 0;
                self.accessibility.focus_override = None;
            }
            _ => {}
        }
    }

    pub(super) fn search_is_visible(&self) -> bool {
        self.search.is_active() || self.search.has_results() || self.search.is_searching()
    }

    fn search_accessibility_layout(
        &self,
        info: &crate::ui::search_bar::SearchBarInfo,
    ) -> crate::ui::search_bar::SearchBarLayout {
        let metrics = self.ui_metrics();
        self.renderer.as_ref().map_or_else(
            || {
                let area = self.pane_placement();
                crate::ui::search_bar::SearchBarLayout::calculate(
                    info,
                    area.left,
                    area.right - area.left,
                    area.top,
                    metrics.cell_width,
                    metrics.cell_height,
                )
            },
            |renderer| renderer.search_bar_layout(info, metrics.width),
        )
    }

    /// The window's root node, without its children.
    fn window_root_node(&self) -> Node {
        let metrics = self.ui_metrics();
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, metrics.width, metrics.height));
        root
    }

    fn accessibility_snapshot(&mut self) -> SemanticSnapshot {
        let UiMetrics {
            width,
            height,
            tab_bar_height: tab_height,
            cell_height,
            ..
        } = self.ui_metrics();

        // Search focus is stale whenever search is hidden, whatever is open.
        if !self.search_is_visible()
            && self
                .accessibility
                .focus_override
                .is_some_and(is_search_node)
        {
            self.accessibility.focus_override = None;
        }
        if !self.search.is_active() && self.accessibility.focus_override == Some(SEARCH_INPUT) {
            self.accessibility.focus_override = None;
        }
        // The top modal layer is all assistive technology sees; focus left on
        // a layer above it has closed.
        let top = self.top_modal_layer();
        for layer in ModalLayer::INPUT_PRIORITY {
            if Some(layer) == top {
                return self.modal_layer_accessibility_snapshot(layer);
            }
            self.forget_modal_layer_focus(layer);
        }

        let mut nodes = Vec::new();
        let mut root_children = vec![TAB_LIST, NEW_TAB, WORKSPACES, COMMANDS, SETTINGS];
        let mut root = self.window_root_node();

        let tab_info = self
            .tabs
            .as_ref()
            .map(TabManager::tab_info)
            .unwrap_or_default();
        let mut tab_children = Vec::new();
        let (broadcast_tab, broadcast_panes, broadcast_state) = match &self.broadcast {
            super::BroadcastState::Armed {
                tab_id,
                expires,
                targets,
            } if Instant::now() <= *expires => (
                Some(*tab_id),
                targets.as_slice(),
                Some(crate::renderer::BroadcastTargetState::Armed),
            ),
            super::BroadcastState::Active { tab_id, targets } => (
                Some(*tab_id),
                targets.as_slice(),
                Some(crate::renderer::BroadcastTargetState::Active),
            ),
            _ => (None, &[][..], None),
        };
        let active_tab_id = self.tabs.as_ref().and_then(TabManager::active_tab_id);
        for (index, tab) in tab_info.iter().enumerate() {
            let tab_id = tab_node(index);
            let close_id = tab_close_node(index);
            let mut node = Node::new(Role::Tab);
            let mut label = tab.title.clone();
            if let Some(badge) = tab.agent_badge {
                label.push_str(", ");
                label.push_str(badge.description());
            }
            if tab.pinned {
                label.push_str(", pinned");
            }
            if tab.is_active && active_tab_id == broadcast_tab {
                match broadcast_state {
                    Some(crate::renderer::BroadcastTargetState::Armed) => {
                        label.push_str(", broadcast input armed")
                    }
                    Some(crate::renderer::BroadcastTargetState::Active) => {
                        label.push_str(", broadcast input active")
                    }
                    None => {}
                }
            }
            node.set_label(label.as_str());
            node.set_selected(tab.is_active);
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.set_children([close_id]);
            if let Some(bounds) = self.renderer.as_ref().and_then(|renderer| {
                renderer.tab_bar_element_rect(
                    crate::renderer::TabBarClick::Tab(index),
                    &tab_info,
                    width,
                    height,
                )
            }) {
                node.set_bounds(bounds_rect(bounds));
            }
            nodes.push((tab_id, node));

            let mut close = Node::new(Role::Button);
            close.set_label(format!("Close {}", tab.title));
            close.add_action(Action::Click);
            close.add_action(Action::Focus);
            if let Some(bounds) = self.renderer.as_ref().and_then(|renderer| {
                renderer.tab_bar_element_rect(
                    crate::renderer::TabBarClick::CloseTab(index),
                    &tab_info,
                    width,
                    height,
                )
            }) {
                close.set_bounds(bounds_rect(bounds));
            }
            nodes.push((close_id, close));
            tab_children.push(tab_id);
        }
        let mut tab_list = Node::new(Role::TabList);
        tab_list.set_label("Terminal tabs");
        let vertical_tabs = self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.tab_bar_width() > 0.0);
        let (tab_list_orientation, tab_list_bounds) = tab_list_semantics(
            vertical_tabs,
            width,
            height,
            tab_height,
            self.renderer
                .as_ref()
                .map_or(0.0, crate::renderer::Renderer::tab_bar_width),
        );
        tab_list.set_orientation(tab_list_orientation);
        tab_list.set_children(tab_children);
        tab_list.set_bounds(tab_list_bounds);
        nodes.push((TAB_LIST, tab_list));

        for (id, label, target) in [
            (NEW_TAB, "New tab", crate::renderer::TabBarClick::NewTab),
            (
                WORKSPACES,
                "Workspaces",
                crate::renderer::TabBarClick::SessionDropdown,
            ),
            (
                COMMANDS,
                "Commands",
                crate::renderer::TabBarClick::CommandsButton,
            ),
            (SETTINGS, "Settings", crate::renderer::TabBarClick::Gear),
        ] {
            let mut node = Node::new(Role::Button);
            node.set_label(label);
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            if let Some(bounds) = self.renderer.as_ref().and_then(|renderer| {
                renderer.tab_bar_element_rect(target, &tab_info, width, height)
            }) {
                node.set_bounds(bounds_rect(bounds));
            }
            nodes.push((id, node));
        }

        for (id, label, target) in [
            (
                PREVIOUS_TAB,
                "Previous tab",
                crate::renderer::TabBarClick::PreviousTab,
            ),
            (NEXT_TAB, "Next tab", crate::renderer::TabBarClick::NextTab),
        ] {
            let Some(bounds) = self.renderer.as_ref().and_then(|renderer| {
                renderer.tab_bar_element_rect(target, &tab_info, width, height)
            }) else {
                continue;
            };
            let mut node = Node::new(Role::Button);
            node.set_label(label);
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.set_bounds(bounds_rect(bounds));
            nodes.push((id, node));
            root_children.push(id);
        }

        if self.search_is_visible() {
            let info = self.search.build_info(self.search_cursor_pos);
            let layout = self.search_accessibility_layout(&info);
            let mut search_children = vec![
                SEARCH_INPUT,
                SEARCH_STATUS,
                SEARCH_FILTER,
                SEARCH_CASE,
                SEARCH_REGEX,
            ];

            let mut input = Node::new(Role::SearchInput);
            input.set_label("Search terminal history");
            input.set_placeholder("Type to search");
            set_text_value(&mut input, SEARCH_INPUT, &info.query, Some(info.cursor_pos));
            input.set_bounds(bounds_rect(layout.input));
            input.add_action(Action::Focus);
            if info.is_active {
                input.add_action(Action::SetValue);
            } else {
                input.set_read_only();
            }
            nodes.push((SEARCH_INPUT, input));

            let mut status = Node::new(Role::Status);
            status.set_label(info.accessible_status());
            status.set_live(if info.regex_error.is_some() {
                Live::Assertive
            } else {
                Live::Polite
            });
            status.set_bounds(bounds_rect(layout.status));
            nodes.push((SEARCH_STATUS, status));

            for (id, label, rectangle, toggled) in [
                (
                    SEARCH_FILTER,
                    "Show only matching lines",
                    layout.filter,
                    info.filter_mode,
                ),
                (
                    SEARCH_CASE,
                    "Match case",
                    layout.case_sensitive,
                    info.case_sensitive,
                ),
                (
                    SEARCH_REGEX,
                    "Use regular expressions",
                    layout.regex,
                    info.use_regex,
                ),
            ] {
                let mut button = Node::new(Role::Button);
                button.set_label(label);
                button.set_toggled(if toggled {
                    Toggled::True
                } else {
                    Toggled::False
                });
                button.set_bounds(bounds_rect(rectangle));
                button.add_action(Action::Click);
                button.add_action(Action::Focus);
                nodes.push((id, button));
            }

            for (id, label, rectangle) in [
                (SEARCH_PREVIOUS, "Previous match", layout.previous),
                (SEARCH_NEXT, "Next match", layout.next),
            ] {
                if layout.navigable {
                    let mut button = Node::new(Role::Button);
                    button.set_label(label);
                    button.set_bounds(bounds_rect(rectangle));
                    button.add_action(Action::Click);
                    button.add_action(Action::Focus);
                    nodes.push((id, button));
                    search_children.push(id);
                }
            }
            let mut close = Node::new(Role::Button);
            close.set_label("Close terminal search");
            close.set_bounds(bounds_rect(layout.close));
            close.add_action(Action::Click);
            close.add_action(Action::Focus);
            nodes.push((SEARCH_CLOSE, close));
            search_children.push(SEARCH_CLOSE);

            let mut toolbar = Node::new(Role::Toolbar);
            toolbar.set_label("Terminal search");
            toolbar.set_bounds(bounds_rect(layout.bar));
            toolbar.set_children(search_children);
            nodes.push((SEARCH_TOOLBAR, toolbar));
            root_children.push(SEARCH_TOOLBAR);
        }

        let mut semantic_focus = ROOT;
        if let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) {
            let panes = &tab.panes;
            let lone = panes.pane_count() == 1;
            let placement = self.pane_placement();
            let focused = panes.focused_id();
            let mut ordered: Vec<_> = panes.iter_with_layout().collect();
            ordered.sort_by_key(|(pane, _)| pane.id.0);
            for (pane, pane_rect) in ordered {
                let id = pane_node(pane.id);
                root_children.push(id);
                if pane.id == focused {
                    semantic_focus = id;
                }
                let bounds = placement.pane_bounds(pane_rect, lone);
                append_terminal_nodes(
                    &mut nodes,
                    id,
                    pane.id,
                    &pane.terminal,
                    pane.id == focused,
                    (broadcast_tab == Some(tab.id) && broadcast_panes.contains(&pane.id))
                        .then_some(broadcast_state)
                        .flatten(),
                    pane.read_only,
                    panes.zoomed_id() == Some(pane.id),
                    rect(bounds.x, bounds.y, bounds.width, bounds.height),
                    cell_height,
                    if pane.id == focused {
                        self.search
                            .filtered_rows(pane.terminal.presentation_grid().rows())
                    } else {
                        &[]
                    },
                );
            }
        }
        if self.search.is_active() {
            semantic_focus = self
                .accessibility
                .focus_override
                .filter(|node| is_search_node(*node))
                .unwrap_or(SEARCH_INPUT);
            self.accessibility.focus_override = Some(semantic_focus);
        }
        root.set_children(root_children);
        nodes.push((ROOT, root));
        SemanticSnapshot {
            nodes,
            focus: semantic_focus,
        }
    }

    /// The top modal layer's tree: its dialog alone under the window root.
    fn modal_layer_accessibility_snapshot(&mut self, layer: ModalLayer) -> SemanticSnapshot {
        let ModalSnapshot {
            dialog,
            mut nodes,
            focus,
        } = match layer {
            ModalLayer::Guardrail => self.guardrail_accessibility_snapshot(),
            ModalLayer::InfoOverlay => self.info_accessibility_snapshot(),
            ModalLayer::DiffReview => self.diff_accessibility_snapshot(),
            ModalLayer::ContextPicker => self.context_accessibility_snapshot(),
            ModalLayer::SproutMenu => self.sprout_accessibility_snapshot(),
            ModalLayer::Cellar => self.cellar_accessibility_snapshot(),
            ModalLayer::DomainPicker => {
                self.palette_modal_accessibility_snapshot(PaletteModal::Domains)
            }
            ModalLayer::UnicodePicker => {
                self.palette_modal_accessibility_snapshot(PaletteModal::Unicode)
            }
            ModalLayer::CommandPalette => {
                self.palette_modal_accessibility_snapshot(PaletteModal::Commands)
            }
            ModalLayer::ShortcutsPanel => self.shortcuts_accessibility_snapshot(),
            ModalLayer::Settings => self.settings_accessibility_snapshot(),
            ModalLayer::WorkspacePanel => self.workspace_accessibility_snapshot(),
            ModalLayer::WorkspaceMenu => self.workspace_menu_accessibility_snapshot(),
            ModalLayer::CommandsPanel => self.commands_accessibility_snapshot(),
            ModalLayer::AgentsPanel => self.agents_accessibility_snapshot(),
        };
        let mut root = self.window_root_node();
        root.set_children([dialog]);
        nodes.push((ROOT, root));
        SemanticSnapshot { nodes, focus }
    }

    /// Drop focus left on one of `layer`'s nodes; the layer is not on top.
    fn forget_modal_layer_focus(&mut self, layer: ModalLayer) {
        if layer == ModalLayer::AgentsPanel {
            self.clear_agents_accessibility_focus();
            return;
        }
        let Some(node) = self.accessibility.focus_override else {
            return;
        };
        let stale = match layer {
            ModalLayer::Guardrail => self.accessibility.is_guardrail_node(node),
            ModalLayer::InfoOverlay => self.accessibility.is_info_node(node),
            ModalLayer::DiffReview => self.accessibility.is_diff_node(node),
            ModalLayer::ContextPicker => self.accessibility.is_context_node(node),
            // The two Harvest layers share their nodes, so only the lower one
            // forgets them, once neither is on top.
            ModalLayer::SproutMenu => false,
            ModalLayer::Cellar => self.accessibility.is_harvest_node(node),
            ModalLayer::DomainPicker => self.accessibility.is_domain_node(node),
            ModalLayer::UnicodePicker => self.accessibility.is_unicode_node(node),
            ModalLayer::CommandPalette => node == PALETTE_INPUT,
            ModalLayer::ShortcutsPanel => self.accessibility.is_shortcuts_node(node),
            ModalLayer::Settings => self.accessibility.is_settings_node(node),
            ModalLayer::WorkspacePanel => self.accessibility.is_workspace_node(node),
            ModalLayer::WorkspaceMenu => {
                node == WORKSPACE_MENU || workspace_menu_row(node).is_some()
            }
            ModalLayer::CommandsPanel => self.accessibility.is_commands_node(node),
            ModalLayer::AgentsPanel => unreachable!("handled above"),
        };
        if stale {
            self.accessibility.focus_override = None;
        }
    }
}

/// Give text node `id` its value and, with a caret, a collapsed selection
/// there. AccessKit counts the caret in characters; one off a character
/// boundary or past the end goes to the end.
fn set_text_value(node: &mut Node, id: NodeId, value: &str, cursor_byte: Option<usize>) {
    node.set_value(value);
    node.set_character_lengths(
        value
            .chars()
            .map(|character| character.len_utf8() as u8)
            .collect::<Vec<_>>(),
    );
    if let Some(cursor_byte) = cursor_byte {
        let character_index = value
            .get(..cursor_byte)
            .map_or_else(|| value.chars().count(), |before| before.chars().count());
        let position = TextPosition {
            node: id,
            character_index,
        };
        node.set_text_selection(TextSelection {
            anchor: position,
            focus: position,
        });
    }
}

fn bounds_rect(bounds: Bounds) -> Rect {
    rect(bounds.x, bounds.y, bounds.width, bounds.height)
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect::new(
        x as f64,
        y as f64,
        (x + width.max(0.0)) as f64,
        (y + height.max(0.0)) as f64,
    )
}

fn tab_list_semantics(
    vertical: bool,
    window_width: f32,
    window_height: f32,
    tab_height: f32,
    rail_width: f32,
) -> (accesskit::Orientation, Rect) {
    if vertical {
        (
            accesskit::Orientation::Vertical,
            rect(
                0.0,
                tab_height,
                rail_width,
                (window_height - tab_height).max(0.0),
            ),
        )
    } else {
        (
            accesskit::Orientation::Horizontal,
            rect(0.0, 0.0, window_width, tab_height),
        )
    }
}

fn tab_node(index: usize) -> NodeId {
    NodeId(TAB_BASE + index as u64 * 2)
}

fn tab_close_node(index: usize) -> NodeId {
    NodeId(TAB_BASE + index as u64 * 2 + 1)
}

fn pane_node(id: PaneId) -> NodeId {
    NodeId(PANE_BASE + u64::from(id.0))
}

fn line_node(pane: PaneId, row: usize) -> NodeId {
    NodeId(LINE_BASE + u64::from(pane.0) * 10_000 + row as u64)
}

fn is_search_node(node: NodeId) -> bool {
    matches!(
        node,
        SEARCH_TOOLBAR
            | SEARCH_INPUT
            | SEARCH_STATUS
            | SEARCH_FILTER
            | SEARCH_CASE
            | SEARCH_REGEX
            | SEARCH_PREVIOUS
            | SEARCH_NEXT
            | SEARCH_CLOSE
    )
}

fn toast_node(id: u64) -> NodeId {
    NodeId(TOAST_BASE.saturating_add(id))
}

/// The toast `node` stands for, when it is a toast's node.
fn toast_id(node: NodeId) -> Option<u64> {
    (TOAST_BASE..CONTEXT_ACTION_BASE)
        .contains(&node.0)
        .then(|| node.0 - TOAST_BASE)
}

#[cfg(test)]
mod tests;
