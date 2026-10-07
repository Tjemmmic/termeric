//! Incremental AccessKit tree for native screen readers.

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
const SESSION_DIALOG: NodeId = NodeId(50);
const SESSION_CLOSE: NodeId = NodeId(51);
const SESSION_LIST: NodeId = NodeId(52);
const SESSION_EMPTY: NodeId = NodeId(53);
const SESSION_RENAME_INPUT: NodeId = NodeId(54);
const SESSION_RENAME_SAVE: NodeId = NodeId(55);
const SESSION_RENAME_CANCEL: NodeId = NodeId(56);
const SESSION_BACK: NodeId = NodeId(57);
const SESSION_SNAPSHOTS: NodeId = NodeId(58);
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
const SESSION_ENTRY_BASE: u64 = 1 << 57;
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
struct SessionSemanticKey {
    id: String,
    workspaces: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SessionSemanticTarget {
    Group(SessionSemanticKey),
    Load(SessionSemanticKey),
    Rename(SessionSemanticKey),
    Delete(SessionSemanticKey),
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
    session: NodeRegistry<SessionSemanticTarget>,
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
            session: NodeRegistry::new(SESSION_ENTRY_BASE),
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ChromeFocus {
    #[default]
    Terminal,
    Tab(usize),
    CloseTab(usize),
    PreviousTab,
    NextTab,
    NewTab,
    Workspaces,
    Commands,
    Settings,
}

impl ChromeFocus {
    fn node(self) -> Option<NodeId> {
        match self {
            Self::Terminal => None,
            Self::Tab(index) => Some(tab_node(index)),
            Self::CloseTab(index) => Some(tab_close_node(index)),
            Self::PreviousTab => Some(PREVIOUS_TAB),
            Self::NextTab => Some(NEXT_TAB),
            Self::NewTab => Some(NEW_TAB),
            Self::Workspaces => Some(WORKSPACES),
            Self::Commands => Some(COMMANDS),
            Self::Settings => Some(SETTINGS),
        }
    }

    fn render_target(self) -> Option<crate::renderer::TabBarClick> {
        use crate::renderer::TabBarClick;
        match self {
            Self::Terminal => None,
            Self::Tab(index) => Some(TabBarClick::Tab(index)),
            Self::CloseTab(index) => Some(TabBarClick::CloseTab(index)),
            Self::PreviousTab => Some(TabBarClick::PreviousTab),
            Self::NextTab => Some(TabBarClick::NextTab),
            Self::NewTab => Some(TabBarClick::NewTab),
            Self::Workspaces => Some(TabBarClick::SessionDropdown),
            Self::Commands => Some(TabBarClick::CommandsButton),
            Self::Settings => Some(TabBarClick::Gear),
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

    fn is_session_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            SESSION_DIALOG
                | SESSION_CLOSE
                | SESSION_LIST
                | SESSION_EMPTY
                | SESSION_RENAME_INPUT
                | SESSION_RENAME_SAVE
                | SESSION_RENAME_CANCEL
                | SESSION_BACK
                | SESSION_SNAPSHOTS
        ) || self.session.contains(node)
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
    pub(super) fn chrome_focus_render_target(&self) -> Option<crate::renderer::TabBarClick> {
        self.accessibility.keyboard_focus.render_target()
    }

    pub(super) fn handle_chrome_keyboard(
        &mut self,
        key: &winit::keyboard::Key,
        state: ElementState,
    ) -> bool {
        use winit::keyboard::{Key, NamedKey};

        let is_f6 = matches!(key, Key::Named(NamedKey::F6));
        if state != ElementState::Pressed {
            return self.accessibility.keyboard_focus != ChromeFocus::Terminal && !is_f6;
        }

        let sequence = self.chrome_focus_sequence();
        if is_f6 {
            let next = if self.modifiers.shift_key() {
                sequence.last().copied().unwrap_or(ChromeFocus::Terminal)
            } else {
                sequence.first().copied().unwrap_or(ChromeFocus::Terminal)
            };
            if self.accessibility.keyboard_focus == ChromeFocus::Terminal {
                self.set_chrome_focus(next);
            } else {
                self.move_chrome_focus(&sequence, self.modifiers.shift_key());
            }
            return true;
        }
        if self.accessibility.keyboard_focus == ChromeFocus::Terminal {
            return false;
        }

        match key {
            Key::Named(NamedKey::Escape) => self.set_chrome_focus(ChromeFocus::Terminal),
            Key::Named(NamedKey::Tab) => {
                self.move_chrome_focus(&sequence, self.modifiers.shift_key())
            }
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
                self.move_chrome_focus(&sequence, true)
            }
            Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => {
                self.move_chrome_focus(&sequence, false)
            }
            Key::Named(NamedKey::Home) => {
                if let Some(first) = sequence.first().copied() {
                    self.set_chrome_focus(first);
                }
            }
            Key::Named(NamedKey::End) => {
                if let Some(last) = sequence.last().copied() {
                    self.set_chrome_focus(last);
                }
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => self.activate_chrome_focus(),
            _ => {}
        }
        true
    }

    fn chrome_focus_sequence(&self) -> Vec<ChromeFocus> {
        let count = self.tabs.as_ref().map(TabManager::count).unwrap_or(0);
        let mut sequence = Vec::with_capacity(count * 2 + 4);
        for index in 0..count {
            sequence.push(ChromeFocus::Tab(index));
            sequence.push(ChromeFocus::CloseTab(index));
        }
        let tabs = self
            .tabs
            .as_ref()
            .map(TabManager::tab_info)
            .unwrap_or_default();
        let (width, height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (width as f32, height as f32)
            })
            .unwrap_or((0.0, 0.0));
        if self.renderer.as_ref().is_some_and(|renderer| {
            renderer
                .tab_bar_element_rect(
                    crate::renderer::TabBarClick::PreviousTab,
                    &tabs,
                    width,
                    height,
                )
                .is_some()
        }) {
            sequence.push(ChromeFocus::PreviousTab);
        }
        if self.renderer.as_ref().is_some_and(|renderer| {
            renderer
                .tab_bar_element_rect(crate::renderer::TabBarClick::NextTab, &tabs, width, height)
                .is_some()
        }) {
            sequence.push(ChromeFocus::NextTab);
        }
        sequence.extend([
            ChromeFocus::NewTab,
            ChromeFocus::Commands,
            ChromeFocus::Workspaces,
            ChromeFocus::Settings,
        ]);
        sequence
    }

    fn move_chrome_focus(&mut self, sequence: &[ChromeFocus], backwards: bool) {
        let Some(current) = sequence
            .iter()
            .position(|focus| *focus == self.accessibility.keyboard_focus)
        else {
            self.set_chrome_focus(ChromeFocus::Terminal);
            return;
        };
        let next = if backwards {
            current.checked_sub(1).unwrap_or(sequence.len() - 1)
        } else {
            (current + 1) % sequence.len()
        };
        self.set_chrome_focus(sequence[next]);
    }

    fn set_chrome_focus(&mut self, focus: ChromeFocus) {
        self.accessibility.keyboard_focus = focus;
        self.accessibility.focus_override = focus.node();
        self.ime.clear_preedit();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_tab_bar_keyboard_focus(focus.render_target());
        }
        self.request_redraw(RedrawReason::Accessibility);
        self.refresh_accessibility();
    }

    /// Call after normal pointer dispatch: the keyboard-anchored tab viewport
    /// must remain intact until hit testing has resolved the clicked target.
    pub(super) fn finish_pointer_chrome_focus(&mut self, button: MouseButton, state: ElementState) {
        if button == MouseButton::Left
            && state == ElementState::Pressed
            && self.accessibility.keyboard_focus != ChromeFocus::Terminal
        {
            self.set_chrome_focus(ChromeFocus::Terminal);
        }
    }

    fn activate_chrome_focus(&mut self) {
        match self.accessibility.keyboard_focus {
            ChromeFocus::Terminal => {}
            ChromeFocus::Tab(index) => {
                self.handle_action(crate::input::Action::SwitchToTab(index));
            }
            ChromeFocus::CloseTab(index) => {
                self.handle_action(crate::input::Action::SwitchToTab(index));
                self.handle_action(crate::input::Action::CloseTab);
                self.set_chrome_focus(ChromeFocus::Terminal);
                return;
            }
            ChromeFocus::PreviousTab => {
                self.handle_action(crate::input::Action::PrevTab);
                self.set_chrome_focus(ChromeFocus::Terminal);
                return;
            }
            ChromeFocus::NextTab => {
                self.handle_action(crate::input::Action::NextTab);
                self.set_chrome_focus(ChromeFocus::Terminal);
                return;
            }
            ChromeFocus::NewTab => {
                self.handle_action(crate::input::Action::NewTab);
                self.set_chrome_focus(ChromeFocus::Terminal);
                return;
            }
            ChromeFocus::Workspaces => self.toggle_workspace_menu(true),
            ChromeFocus::Commands => self.commands_panel.toggle(),
            ChromeFocus::Settings => self.toggle_settings_surface(),
        }
        if !matches!(self.accessibility.keyboard_focus, ChromeFocus::Tab(_)) {
            self.set_chrome_focus(ChromeFocus::Terminal);
        } else {
            self.request_redraw(RedrawReason::Accessibility);
        }
    }

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
                ModalLayer::SessionPanel => self.handle_session_accessibility_action(request),
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
                        self.redraw_scheduler
                            .request(redraw_scheduler::RedrawReason::Accessibility);
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

    fn handle_harvest_accessibility_action(&mut self, request: ActionRequest) {
        if let Some((request_id, action)) = self
            .accessibility
            .harvest
            .targets
            .get(&request.target_node)
            .and_then(|target| match target {
                HarvestSemanticTarget::SproutAction { request_id, action } => {
                    Some((*request_id, *action))
                }
                HarvestSemanticTarget::CellarEntry(_) => None,
            })
        {
            let current = self
                .harvest
                .as_ref()
                .and_then(|harvest| harvest.sprout_menu.as_ref())
                .is_some_and(|menu| {
                    menu.request_id == request_id
                        && menu.actions.contains(&action)
                        && menu.action_enabled(action)
                });
            if !current {
                return;
            }
            match request.action {
                Action::Focus => {
                    if let Some(menu) = self
                        .harvest
                        .as_mut()
                        .and_then(|harvest| harvest.sprout_menu.as_mut())
                    {
                        menu.select(action);
                    }
                    self.accessibility.focus_override = Some(request.target_node);
                }
                Action::Click => {
                    self.execute_sprout_request(request_id, action);
                    self.accessibility.focus_override = None;
                }
                _ => {}
            }
            return;
        }

        if self
            .harvest
            .as_ref()
            .is_some_and(|harvest| harvest.sprout_menu.is_some())
        {
            // The context menu is modal. Background and retired actions fail
            // closed instead of falling through to application controls.
            return;
        }

        if request.target_node == CELLAR_CLOSE {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(CELLAR_CLOSE),
                Action::Click => {
                    self.apply_cellar_action(crate::harvest::CellarAction::Close);
                    self.accessibility.focus_override = None;
                }
                _ => {}
            }
            return;
        }
        if request.target_node == CELLAR_LIST {
            match request.action {
                Action::ScrollUp => {
                    if let Some(harvest) = &mut self.harvest {
                        harvest.cellar.select_page(-1);
                    }
                }
                Action::ScrollDown => {
                    if let Some(harvest) = &mut self.harvest {
                        harvest.cellar.select_page(1);
                    }
                }
                _ => {}
            }
            return;
        }
        let Some(id) = self
            .accessibility
            .harvest
            .targets
            .get(&request.target_node)
            .and_then(|target| match target {
                HarvestSemanticTarget::CellarEntry(id) => Some(*id),
                HarvestSemanticTarget::SproutAction { .. } => None,
            })
        else {
            return;
        };
        let visible = self
            .cellar_layout()
            .is_some_and(|layout| layout.rows.iter().any(|(entry_id, _)| *entry_id == id));
        if !visible {
            return;
        }
        match request.action {
            Action::Focus => {
                self.apply_cellar_action(crate::harvest::CellarAction::Select(id));
                self.accessibility.focus_override = Some(request.target_node);
            }
            Action::Click => {
                self.apply_cellar_action(crate::harvest::CellarAction::Activate(id));
                self.accessibility.focus_override = None;
            }
            _ => {}
        }
    }

    /// The query takes text and focus; a visible entry takes focus or a
    /// click. The modal hides everything behind it, so a stale request for
    /// anything else is dropped.
    fn handle_palette_modal_accessibility_action(
        &mut self,
        modal: PaletteModal,
        request: ActionRequest,
    ) {
        let ids = palette_modal_node_ids(modal);
        if request.target_node == ids.input {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(ids.input),
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.palette_owner_mut(modal).set_query(value) {
                        self.ime.clear_preedit();
                        self.accessibility.focus_override = Some(ids.input);
                    }
                }
                _ => {}
            }
            return;
        }
        if !matches!(request.action, Action::Click | Action::Focus) {
            return;
        }
        let Some(command) = self
            .accessibility
            .palette_modal(modal)
            .targets
            .get(&request.target_node)
            .cloned()
        else {
            return;
        };
        let info = self.palette_owner(modal).build_info();
        let layout = self.command_palette_accessibility_layout(&info);
        if !info
            .entries
            .iter()
            .take(layout.visible_count)
            .any(|entry| entry.id == command)
            || !self.palette_entry_actionable(modal, &command)
        {
            return;
        }
        if request.action == Action::Focus {
            if self
                .palette_owner_mut(modal)
                .palette_mut()
                .select_command(&command)
            {
                self.accessibility.focus_override = Some(ids.input);
            }
        } else if let Some(choice) = self.execute_palette_modal_command(modal, &command) {
            // A staged picker moves on with its query still focused; the
            // others close.
            self.accessibility.focus_override =
                (modal == PaletteModal::Domains).then_some(ids.input);
            self.finish_palette_input(Some(choice));
        }
    }

    fn handle_guardrail_accessibility_action(&mut self, request: ActionRequest) {
        let Some(target) = self
            .accessibility
            .guardrail
            .targets
            .get(&request.target_node)
            .copied()
        else {
            // Confirmation is modal and safety-critical. Any stale or
            // background request is consumed without side effects.
            return;
        };
        let (request_id, action) = match target {
            GuardrailSemanticTarget::Confirm(request_id) => (
                request_id,
                crate::terminal::guardrails::GuardrailAction::Confirm { request_id },
            ),
            GuardrailSemanticTarget::Cancel(request_id) => (
                request_id,
                crate::terminal::guardrails::GuardrailAction::Cancel { request_id },
            ),
        };
        if !self.guardrail.is_awaiting(request_id) {
            return;
        }
        match request.action {
            Action::Focus => {
                self.accessibility.focus_override = Some(request.target_node);
            }
            Action::Click => {
                self.apply_guardrail_action(action);
                self.accessibility.focus_override = Some(GUARDRAIL_DIALOG);
            }
            _ => {}
        }
    }

    fn handle_info_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::info_overlay::InfoOverlayAction;

        let Some(target) = self
            .accessibility
            .info
            .targets
            .get(&request.target_node)
            .cloned()
        else {
            // The notice is modal. Background and retired request nodes are
            // consumed instead of falling through to terminal chrome.
            return;
        };
        let request_id = match &target {
            InfoSemanticTarget::Content(request_id)
            | InfoSemanticTarget::Input(request_id)
            | InfoSemanticTarget::Copy(request_id)
            | InfoSemanticTarget::Dismiss(request_id) => *request_id,
            InfoSemanticTarget::Choice { request_id, .. } => *request_id,
        };
        if !self.info_overlay.is_awaiting(request_id) {
            return;
        }
        let action = match (request.action, target) {
            (Action::Focus, target) => {
                use crate::ui::info_overlay::InfoOverlayFocus;
                let focus = match target {
                    InfoSemanticTarget::Content(_) => None,
                    InfoSemanticTarget::Input(_) => Some(InfoOverlayFocus::Input),
                    InfoSemanticTarget::Choice { id, .. } => Some(InfoOverlayFocus::Choice(id)),
                    InfoSemanticTarget::Copy(_) => Some(InfoOverlayFocus::Copy),
                    InfoSemanticTarget::Dismiss(_) => Some(InfoOverlayFocus::Continue),
                };
                self.info_overlay.set_focus(request_id, focus);
                self.ime.clear_preedit();
                self.accessibility.focus_override = Some(request.target_node);
                InfoOverlayAction::None
            }
            (Action::Click, InfoSemanticTarget::Copy(request_id)) => {
                InfoOverlayAction::Copy { request_id }
            }
            (Action::Click, InfoSemanticTarget::Dismiss(request_id)) => {
                if self.info_overlay.has_input() {
                    InfoOverlayAction::SubmitInput { request_id }
                } else {
                    InfoOverlayAction::Dismiss { request_id }
                }
            }
            (Action::SetValue, InfoSemanticTarget::Input(request_id)) => {
                let Some(ActionData::Value(value)) = request.data.as_ref() else {
                    return;
                };
                if self.info_overlay.replace_input_text(request_id, value) {
                    self.info_overlay.set_focus(
                        request_id,
                        Some(crate::ui::info_overlay::InfoOverlayFocus::Input),
                    );
                    self.accessibility.focus_override = Some(request.target_node);
                }
                InfoOverlayAction::None
            }
            (
                Action::Click,
                InfoSemanticTarget::Choice {
                    request_id,
                    id: choice_id,
                },
            ) => InfoOverlayAction::Choose {
                request_id,
                choice_id,
            },
            (Action::ScrollUp, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::Scroll {
                    request_id,
                    by: OverlayScroll::Rows(-(self.info_overlay.viewport().rows() as isize)),
                }
            }
            (Action::ScrollDown, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::Scroll {
                    request_id,
                    by: OverlayScroll::Rows(self.info_overlay.viewport().rows() as isize),
                }
            }
            (Action::ScrollLeft, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::Scroll {
                    request_id,
                    by: OverlayScroll::Columns(-(self.info_overlay.viewport().columns() as isize)),
                }
            }
            (Action::ScrollRight, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::Scroll {
                    request_id,
                    by: OverlayScroll::Columns(self.info_overlay.viewport().columns() as isize),
                }
            }
            _ => InfoOverlayAction::None,
        };
        self.apply_info_overlay_action(action);
    }

    fn handle_diff_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::diff_review::DiffReviewAction;

        let Some(target) = self
            .accessibility
            .diff
            .targets
            .get(&request.target_node)
            .copied()
        else {
            // Diff review is modal. Stale nodes and background application
            // controls are consumed without falling through.
            return;
        };
        let request_id = match target {
            DiffSemanticTarget::Content(request_id)
            | DiffSemanticTarget::Copy(request_id)
            | DiffSemanticTarget::Dismiss(request_id)
            | DiffSemanticTarget::Stage(request_id)
            | DiffSemanticTarget::Revert(request_id) => request_id,
        };
        if !self.diff_review.is_awaiting(request_id) {
            return;
        }
        let action = match (request.action, target) {
            (Action::Focus, _) => {
                self.accessibility.focus_override = Some(request.target_node);
                DiffReviewAction::None
            }
            (Action::Click, DiffSemanticTarget::Copy(request_id)) => {
                DiffReviewAction::Copy { request_id }
            }
            (Action::Click, DiffSemanticTarget::Dismiss(request_id)) => {
                DiffReviewAction::Dismiss { request_id }
            }
            (Action::Click, DiffSemanticTarget::Stage(request_id)) => {
                DiffReviewAction::Stage { request_id }
            }
            (Action::Click, DiffSemanticTarget::Revert(request_id)) => {
                DiffReviewAction::Revert { request_id }
            }
            (Action::ScrollUp, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: OverlayScroll::Rows(-(self.diff_review.viewport().rows() as isize)),
                }
            }
            (Action::ScrollDown, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: OverlayScroll::Rows(self.diff_review.viewport().rows() as isize),
                }
            }
            (Action::ScrollLeft, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: OverlayScroll::Columns(-(self.diff_review.viewport().columns() as isize)),
                }
            }
            (Action::ScrollRight, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: OverlayScroll::Columns(self.diff_review.viewport().columns() as isize),
                }
            }
            _ => DiffReviewAction::None,
        };
        self.apply_diff_review_action(action);
    }

    fn handle_context_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::context_picker::ContextPickerAction;

        let Some(target) = self
            .accessibility
            .context
            .targets
            .get(&request.target_node)
            .copied()
        else {
            return;
        };
        let request_id = match target {
            ContextSemanticTarget::List(request_id)
            | ContextSemanticTarget::FileInput(request_id)
            | ContextSemanticTarget::Cancel(request_id)
            | ContextSemanticTarget::Primary(request_id) => request_id,
            ContextSemanticTarget::Source { request_id, .. } => request_id,
        };
        if !self.context_picker.is_awaiting(request_id) {
            return;
        }
        let action = match (request.action, target) {
            (Action::Focus, ContextSemanticTarget::Source { source, .. }) => {
                self.context_picker.select_source(request_id, source);
                self.accessibility.focus_override = Some(request.target_node);
                ContextPickerAction::None
            }
            (Action::Focus, _) => {
                self.accessibility.focus_override = Some(request.target_node);
                ContextPickerAction::None
            }
            (Action::Click, ContextSemanticTarget::Source { source, .. }) => {
                ContextPickerAction::Toggle { request_id, source }
            }
            (Action::Click, ContextSemanticTarget::Cancel(_)) => {
                ContextPickerAction::Cancel { request_id }
            }
            (Action::Click, ContextSemanticTarget::Primary(_)) => {
                ContextPickerAction::Primary { request_id }
            }
            (Action::SetValue, ContextSemanticTarget::FileInput(_)) => {
                let Some(ActionData::Value(value)) = request.data else {
                    return;
                };
                ContextPickerAction::SetFileInput {
                    request_id,
                    value: value.into(),
                }
            }
            (Action::ScrollUp, ContextSemanticTarget::List(_)) => ContextPickerAction::Scroll {
                request_id,
                delta: -(self.context_picker.viewport_rows() as isize),
            },
            (Action::ScrollDown, ContextSemanticTarget::List(_)) => ContextPickerAction::Scroll {
                request_id,
                delta: self.context_picker.viewport_rows() as isize,
            },
            _ => ContextPickerAction::None,
        };
        self.apply_context_picker_action(action);
    }

    fn handle_settings_accessibility_action(&mut self, request: ActionRequest) {
        if request.target_node == SETTINGS_INPUT {
            // A retained node must not edit or take focus from a live dropdown.
            if self.settings_panel.is_dropdown_open() {
                return;
            }
            match request.action {
                Action::Focus => {
                    self.accessibility.focus_override = Some(SETTINGS_INPUT);
                }
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.settings_panel.accessibility_set_filter(value) {
                        self.ime.clear_preedit();
                        self.accessibility.focus_override = Some(SETTINGS_INPUT);
                        self.redraw_scheduler
                            .request(redraw_scheduler::RedrawReason::Accessibility);
                    }
                }
                _ => {}
            }
            return;
        }
        if matches!(request.target_node, SETTINGS_CLOSE | SETTINGS_SAVE) {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(request.target_node),
                Action::Click if request.target_node == SETTINGS_CLOSE => {
                    self.settings_panel.close();
                    self.accessibility.focus_override = None;
                }
                Action::Click => {
                    self.apply_settings_action(crate::ui::settings::SettingsAction::SaveConfig);
                }
                _ => {}
            }
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::Accessibility);
            return;
        }
        let Some(target) = self
            .accessibility
            .settings
            .targets
            .get(&request.target_node)
            .cloned()
        else {
            // Settings is modal. Stale requests for controls behind it must
            // never escape into the normal application action path.
            return;
        };

        use crate::ui::settings::SettingsAction;
        match target {
            SettingsSemanticTarget::Page(page) => match request.action {
                Action::Click => {
                    self.settings_panel.select_page(page);
                    self.accessibility.focus_override = Some(request.target_node);
                    self.redraw_scheduler
                        .request(redraw_scheduler::RedrawReason::Accessibility);
                }
                Action::Focus => self.accessibility.focus_override = Some(request.target_node),
                _ => {}
            },
            SettingsSemanticTarget::Header(_) => {}
            SettingsSemanticTarget::Setting { section, label } => match request.action {
                Action::Focus => {
                    if self.settings_panel.accessibility_focus_setting(
                        section,
                        label,
                        &self.config,
                        &self.shortcut_registry,
                    ) {
                        self.accessibility.focus_override = Some(request.target_node);
                    }
                }
                Action::Increment | Action::Decrement => {
                    let direction = if request.action == Action::Increment {
                        1
                    } else {
                        -1
                    };
                    let action = self.settings_panel.accessibility_adjust_setting(
                        section,
                        label,
                        direction,
                        &self.config,
                        &self.shortcut_registry,
                    );
                    self.accessibility.focus_override = Some(request.target_node);
                    self.apply_settings_action(action);
                }
                Action::Click => {
                    let action = self.settings_panel.accessibility_activate_setting(
                        section,
                        label,
                        &self.config,
                        &self.shortcut_registry,
                    );
                    if matches!(action, SettingsAction::None) {
                        if self.settings_panel.accessibility_set_dropdown_expanded(
                            section,
                            label,
                            true,
                            &self.config,
                            &self.shortcut_registry,
                        ) {
                            self.accessibility.focus_override = Some(request.target_node);
                        }
                    } else {
                        self.apply_settings_action(action);
                    }
                }
                Action::Expand => {
                    if self.settings_panel.accessibility_set_dropdown_expanded(
                        section,
                        label,
                        true,
                        &self.config,
                        &self.shortcut_registry,
                    ) {
                        self.accessibility.focus_override = Some(request.target_node);
                    }
                }
                Action::Collapse => {
                    self.settings_panel.accessibility_set_dropdown_expanded(
                        section,
                        label,
                        false,
                        &self.config,
                        &self.shortcut_registry,
                    );
                    self.accessibility.focus_override = Some(request.target_node);
                }
                _ => {}
            },
            SettingsSemanticTarget::Option {
                section,
                label,
                value,
            } => {
                if request.action != Action::Click {
                    return;
                }
                let action = self.settings_panel.accessibility_choose_dropdown_option(
                    section,
                    label,
                    &value,
                    &self.config,
                    &self.shortcut_registry,
                );
                if !matches!(action, SettingsAction::None) {
                    self.apply_settings_action(action);
                }
                if let Some(node) = self
                    .accessibility
                    .settings
                    .nodes
                    .get(&SettingsSemanticTarget::Setting { section, label })
                {
                    self.accessibility.focus_override = Some(*node);
                }
            }
        }
    }

    fn handle_shortcuts_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::shortcuts_panel::ShortcutsAction;

        if request.target_node == SHORTCUTS_INPUT {
            if self.shortcuts_panel.is_recording() {
                return;
            }
            match request.action {
                Action::Focus => {
                    self.accessibility.focus_override = Some(SHORTCUTS_INPUT);
                }
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.shortcuts_panel.accessibility_set_filter(value) {
                        self.ime.clear_preedit();
                        self.accessibility.focus_override = Some(SHORTCUTS_INPUT);
                        self.redraw_scheduler
                            .request(redraw_scheduler::RedrawReason::Accessibility);
                    }
                }
                _ => {}
            }
            return;
        }

        let Some(ShortcutsSemanticTarget::Action(action)) = self
            .accessibility
            .shortcuts
            .targets
            .get(&request.target_node)
            .copied()
        else {
            // The editor is modal. Stale requests for hidden application
            // controls and non-action headings stop here.
            return;
        };
        if !self
            .shortcuts_panel
            .is_visible_action(action, &self.shortcut_registry)
        {
            return;
        }
        match request.action {
            Action::Focus => {
                if self
                    .shortcuts_panel
                    .select_action(action, &self.shortcut_registry)
                {
                    self.accessibility.focus_override = Some(request.target_node);
                }
            }
            Action::Click => {
                self.apply_shortcuts_action(ShortcutsAction::StartRecording(action));
                self.accessibility.focus_override = Some(request.target_node);
            }
            Action::CustomAction
                if matches!(
                    request.data,
                    Some(ActionData::CustomAction(SHORTCUT_CLEAR_ACTION))
                ) =>
            {
                self.apply_shortcuts_action(ShortcutsAction::ClearShortcut(action));
                self.accessibility.focus_override = Some(request.target_node);
            }
            _ => {}
        }
    }

    fn shortcuts_accessibility_layout(
        &self,
        info: &crate::ui::shortcuts_panel::ShortcutsPanelInfo,
    ) -> crate::ui::shortcuts_panel::ShortcutsPanelLayout {
        let UiMetrics {
            width,
            height,
            cell_width,
            cell_height,
            ..
        } = self.ui_metrics();
        crate::ui::shortcuts_panel::ShortcutsPanelLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            info.animation,
            info.entries.len(),
        )
    }

    fn settings_accessibility_layout(
        &self,
        info: &crate::ui::settings::SettingsPanelInfo,
    ) -> crate::ui::settings::SettingsModalLayout {
        let UiMetrics {
            width,
            height,
            cell_width,
            cell_height,
            ..
        } = self.ui_metrics();
        crate::ui::settings::SettingsModalLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            info.animation,
            info.entries.len(),
        )
    }

    pub(super) fn clear_agents_accessibility_focus(&mut self) {
        if self
            .accessibility
            .focus_override
            .is_some_and(|id| self.accessibility.is_agents_node(id))
        {
            self.accessibility.focus_override = None;
        }
    }

    fn handle_agents_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::agents_panel::{AgentsControl, AgentsPanelAction};

        let control = match request.target_node {
            AGENTS_LIST => {
                let delta = match request.action {
                    Action::ScrollUp => -1,
                    Action::ScrollDown => 1,
                    _ => return,
                };
                let layout = self.agents_panel_layout();
                self.agents_panel.scroll_rows(delta, layout);
                return;
            }
            AGENTS_CLOSE => AgentsControl::Close,
            AGENTS_SUPERVISOR_TOGGLE if self.supervisor_running() => AgentsControl::Supervisor,
            node => {
                // The panel is modal: a request for anything else, including
                // a row whose agent has gone, stops here.
                let Some(index) = self
                    .accessibility
                    .agents
                    .targets
                    .get(&node)
                    .and_then(|identity| self.agents_panel.shown_index(*identity))
                else {
                    return;
                };
                AgentsControl::Agent(index)
            }
        };
        match request.action {
            Action::Click => {
                let action = match control {
                    AgentsControl::Close => AgentsPanelAction::Close,
                    AgentsControl::Supervisor => AgentsPanelAction::ToggleSupervisor,
                    AgentsControl::Agent(index) => AgentsPanelAction::Show(index),
                };
                self.apply_agents_panel_action(action);
                self.accessibility.focus_override =
                    (control == AgentsControl::Supervisor).then_some(AGENTS_SUPERVISOR_TOGGLE);
            }
            Action::Focus => {
                let layout = self.agents_panel_layout();
                if self.agents_panel.focus_control_at(control, layout) {
                    self.accessibility.focus_override = Some(request.target_node);
                }
            }
            _ => {}
        }
    }

    pub(super) fn clear_commands_accessibility_focus(&mut self) {
        if self
            .accessibility
            .focus_override
            .is_some_and(|id| self.accessibility.is_commands_node(id))
        {
            self.accessibility.focus_override = None;
        }
    }

    /// A deletion changes duplicate occurrence identities. Retire all published
    /// row actions before the next frame; never reuse their node IDs for rows
    /// that shifted into a deleted occurrence. Fixed form/footer IDs stay live.
    pub(super) fn retire_commands_accessibility_rows(&mut self) {
        self.clear_commands_accessibility_focus();
        self.accessibility.commands.clear();
    }

    pub(super) fn retain_commands_accessibility_rows(
        &mut self,
        keys: &HashSet<CustomCommandSemanticKey>,
    ) {
        let retained = |target: &CommandsSemanticTarget| {
            let (CommandsSemanticTarget::Group(key)
            | CommandsSemanticTarget::Run(key)
            | CommandsSemanticTarget::Delete(key)) = target;
            keys.contains(key)
        };
        if self.accessibility.focus_override.is_some_and(|id| {
            self.accessibility
                .commands
                .targets
                .get(&id)
                .is_some_and(|target| !retained(target))
        }) {
            self.accessibility.focus_override = None;
        }
        self.accessibility.commands.retain(retained);
    }

    fn handle_commands_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::commands::panel::CommandsPanelAction;

        match request.target_node {
            COMMANDS_LIST => match request.action {
                Action::ScrollUp => self.scroll_commands_panel(-1),
                Action::ScrollDown => self.scroll_commands_panel(1),
                _ => {}
            },
            COMMANDS_NEW => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::StartAdd);
                    self.accessibility.focus_override = Some(COMMANDS_NAME);
                }
                Action::Focus => {
                    let layout = self.commands_panel_layout();
                    self.commands_panel.focus_browse_control(
                        CommandControl::New,
                        &self.config.commands,
                        layout,
                    );
                    self.accessibility.focus_override = Some(COMMANDS_NEW);
                }
                _ => {}
            },
            COMMANDS_CLOSE => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::Close);
                    self.accessibility.focus_override = None;
                }
                Action::Focus => {
                    let layout = self.commands_panel_layout();
                    self.commands_panel.focus_browse_control(
                        CommandControl::Close,
                        &self.config.commands,
                        layout,
                    );
                    self.accessibility.focus_override = Some(COMMANDS_CLOSE);
                }
                _ => {}
            },
            COMMANDS_NAME | COMMANDS_COMMAND if self.commands_panel.is_editing() => {
                let is_name = request.target_node == COMMANDS_NAME;
                match request.action {
                    Action::Focus => {
                        self.apply_commands_panel_action(if is_name {
                            CommandsPanelAction::ClickName
                        } else {
                            CommandsPanelAction::ClickCommand
                        });
                        self.accessibility.focus_override = Some(request.target_node);
                    }
                    Action::SetValue => {
                        let Some(ActionData::Value(value)) = request.data.as_ref() else {
                            return;
                        };
                        let accepted = if is_name {
                            self.commands_panel.accessibility_set_name(value)
                        } else {
                            self.commands_panel.accessibility_set_command(value)
                        };
                        if accepted {
                            self.ime.clear_preedit();
                            self.redraw_scheduler
                                .request(redraw_scheduler::RedrawReason::Accessibility);
                            self.accessibility.focus_override = Some(request.target_node);
                        }
                    }
                    _ => {}
                }
            }
            COMMANDS_SAVE if self.commands_panel.is_editing() => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::Submit);
                    self.accessibility.focus_override = Some(if self.commands_panel.is_editing() {
                        COMMANDS_SAVE
                    } else {
                        COMMANDS_NEW
                    });
                }
                Action::Focus => self.accessibility.focus_override = Some(COMMANDS_SAVE),
                _ => {}
            },
            COMMANDS_CANCEL if self.commands_panel.is_editing() => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::CancelEdit);
                    self.accessibility.focus_override = Some(COMMANDS_NEW);
                }
                Action::Focus => self.accessibility.focus_override = Some(COMMANDS_CANCEL),
                _ => {}
            },
            _ => {
                let Some(target) = self
                    .accessibility
                    .commands
                    .targets
                    .get(&request.target_node)
                    .cloned()
                else {
                    // The commands panel is modal; stale requests for hidden
                    // application controls must stop here.
                    return;
                };
                let key = match &target {
                    CommandsSemanticTarget::Group(key)
                    | CommandsSemanticTarget::Run(key)
                    | CommandsSemanticTarget::Delete(key) => key,
                };
                let Some(index) = custom_command_index(&self.config.commands, key) else {
                    return;
                };
                match (target, request.action) {
                    (CommandsSemanticTarget::Run(_), Action::Click) => {
                        self.apply_commands_panel_action(CommandsPanelAction::Run(index));
                        self.accessibility.focus_override = None;
                    }
                    (CommandsSemanticTarget::Delete(_), Action::Click) => {
                        self.apply_commands_panel_action(CommandsPanelAction::Delete(index));
                    }
                    (
                        target @ (CommandsSemanticTarget::Run(_)
                        | CommandsSemanticTarget::Delete(_)),
                        Action::Focus,
                    ) => {
                        let layout = self.commands_panel_layout();
                        let control = if matches!(target, CommandsSemanticTarget::Run(_)) {
                            CommandControl::Run(index)
                        } else {
                            CommandControl::Delete(index)
                        };
                        if self.commands_panel.focus_browse_control(
                            control,
                            &self.config.commands,
                            layout,
                        ) {
                            self.accessibility.focus_override = Some(request.target_node);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn handle_session_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::session_panel::SessionPanelAction;

        match request.target_node {
            SESSION_BACK | SESSION_SNAPSHOTS => match request.action {
                Action::Click => {
                    self.apply_session_panel_action(if request.target_node == SESSION_BACK {
                        SessionPanelAction::ShowWorkspaces
                    } else {
                        SessionPanelAction::ShowSnapshots
                    });
                    self.accessibility.focus_override = None;
                }
                Action::Focus => self.accessibility.focus_override = Some(request.target_node),
                _ => {}
            },
            SESSION_CLOSE => match request.action {
                Action::Click => {
                    self.apply_session_panel_action(SessionPanelAction::Close);
                    self.accessibility.focus_override = None;
                }
                Action::Focus => self.accessibility.focus_override = Some(SESSION_CLOSE),
                _ => {}
            },
            SESSION_RENAME_INPUT if self.session_panel.is_renaming() => match request.action {
                Action::Focus => {
                    self.accessibility.focus_override = Some(SESSION_RENAME_INPUT);
                }
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.session_panel.accessibility_set_rename(value) {
                        self.ime.clear_preedit();
                        self.accessibility.focus_override = Some(SESSION_RENAME_INPUT);
                        self.redraw_scheduler
                            .request(redraw_scheduler::RedrawReason::Accessibility);
                    }
                }
                _ => {}
            },
            SESSION_RENAME_SAVE if self.session_panel.is_renaming() => match request.action {
                Action::Click => {
                    self.apply_session_panel_action(SessionPanelAction::SubmitRename);
                    self.accessibility.focus_override = None;
                }
                Action::Focus => self.accessibility.focus_override = Some(SESSION_RENAME_SAVE),
                _ => {}
            },
            SESSION_RENAME_CANCEL if self.session_panel.is_renaming() => match request.action {
                Action::Click => {
                    self.apply_session_panel_action(SessionPanelAction::CancelRename);
                    self.accessibility.focus_override = None;
                }
                Action::Focus => self.accessibility.focus_override = Some(SESSION_RENAME_CANCEL),
                _ => {}
            },
            _ => {
                if self.session_panel.is_renaming() {
                    return;
                }
                let Some(target) = self
                    .accessibility
                    .session
                    .targets
                    .get(&request.target_node)
                    .cloned()
                else {
                    return;
                };
                let key = match &target {
                    SessionSemanticTarget::Group(key)
                    | SessionSemanticTarget::Load(key)
                    | SessionSemanticTarget::Rename(key)
                    | SessionSemanticTarget::Delete(key) => key,
                };
                if key.workspaces != self.session_panel.is_workspace_mode()
                    || !self.session_panel.is_visible_id(&key.id)
                {
                    return;
                }
                match (target, request.action) {
                    (
                        SessionSemanticTarget::Load(key)
                        | SessionSemanticTarget::Rename(key)
                        | SessionSemanticTarget::Delete(key),
                        Action::Focus,
                    ) => {
                        if self.session_panel.select_id(&key.id) {
                            self.accessibility.focus_override = Some(request.target_node);
                        }
                    }
                    (SessionSemanticTarget::Load(key), Action::Click) => {
                        self.apply_session_panel_action(SessionPanelAction::Load(key.id));
                        self.accessibility.focus_override = None;
                    }
                    (SessionSemanticTarget::Rename(key), Action::Click) => {
                        self.apply_session_panel_action(SessionPanelAction::Rename(key.id));
                        if self.session_panel.is_renaming() {
                            self.accessibility.focus_override = Some(SESSION_RENAME_INPUT);
                        }
                    }
                    (SessionSemanticTarget::Delete(key), Action::Click) => {
                        self.apply_session_panel_action(SessionPanelAction::Delete(key.id));
                        self.accessibility.focus_override = None;
                    }
                    _ => {}
                }
            }
        }
    }

    fn session_accessibility_layout(
        &self,
        info: &crate::ui::session_panel::SessionPanelInfo,
    ) -> crate::ui::session_panel::SessionPanelLayout {
        let UiMetrics {
            width,
            height,
            tab_bar_height: top,
            cell_width,
            cell_height,
        } = self.ui_metrics();
        crate::ui::session_panel::SessionPanelLayout::calculate(
            width,
            height,
            top,
            cell_width,
            cell_height,
            info.animation,
            info.rename_text.is_some(),
            info.workspaces,
            info.sessions.len(),
        )
    }

    fn command_palette_accessibility_layout(
        &self,
        info: &crate::ui::command_palette::PaletteInfo,
    ) -> crate::ui::command_palette::PaletteLayout {
        let UiMetrics {
            width,
            height,
            cell_width,
            cell_height,
            ..
        } = self.ui_metrics();
        crate::ui::command_palette::PaletteLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            info.animation,
            info.entries.len(),
        )
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

    /// Adds the docked agent view beside the terminals. Modal surfaces
    /// replace the tree, so the panel joins only the ordinary one.
    fn with_agent_view_accessibility(
        &mut self,
        mut snapshot: SemanticSnapshot,
    ) -> SemanticSnapshot {
        use crate::ui::agent_view::scene::{HitRole, Node as SceneNode, Target};

        let ordinary = snapshot
            .nodes
            .iter()
            .any(|(id, node)| *id == ROOT && node.children().contains(&TAB_LIST));
        let Some(scene) = self
            .agent_view
            .scene
            .clone()
            .filter(|_| ordinary && self.agent_view.state.is_open())
        else {
            return snapshot;
        };
        let shown: HashSet<&Target> = scene.hits.iter().map(|hit| &hit.target).collect();
        self.accessibility
            .agent_view
            .retain(|target| shown.contains(target));
        let composer = self
            .agent_view
            .state
            .composer()
            .map(|composer| composer.text().to_owned())
            .unwrap_or_default();
        let bounds = bounds_rect;
        let mut children = Vec::with_capacity(scene.nodes.len());
        let mut text_index = 0;
        for node in &scene.nodes {
            match node {
                SceneNode::Text(text) => {
                    let id = NodeId(AGENT_VIEW_TEXT_BASE + text_index);
                    text_index += 1;
                    let mut label = Node::new(if text.heading {
                        Role::Heading
                    } else {
                        Role::Label
                    });
                    label.set_label(text.text.clone());
                    let visible = match text.clip {
                        Some(clip) => text.bounds.intersect(clip),
                        None => Some(text.bounds),
                    };
                    if let Some(visible) = visible {
                        label.set_bounds(bounds(visible));
                    }
                    snapshot.nodes.push((id, label));
                    children.push(id);
                }
                SceneNode::Hit(index) => {
                    let hit = &scene.hits[*index];
                    if hit.target == Target::DismissMenu {
                        continue;
                    }
                    let id = self.accessibility.agent_view.node(&hit.target);
                    let mut control = match hit.role {
                        HitRole::Button => Node::new(Role::Button),
                        HitRole::Toggle { expanded } => {
                            let mut node = Node::new(Role::Button);
                            node.set_expanded(expanded);
                            node
                        }
                        HitRole::Option { selected } => {
                            let mut node = Node::new(Role::RadioButton);
                            node.set_toggled(accesskit::Toggled::from(selected));
                            node
                        }
                        HitRole::TextInput => {
                            let mut node = Node::new(Role::MultilineTextInput);
                            node.set_value(composer.clone());
                            node
                        }
                        HitRole::ListItem => Node::new(Role::ListItem),
                        HitRole::MenuItem { checked } => {
                            let mut node = Node::new(Role::MenuItemCheckBox);
                            node.set_toggled(accesskit::Toggled::from(checked));
                            node
                        }
                    };
                    control.set_label(hit.label.clone());
                    control.add_action(Action::Click);
                    control.add_action(Action::Focus);
                    if let Some(visible) = hit.visible_bounds() {
                        control.set_bounds(bounds(visible));
                    }
                    snapshot.nodes.push((id, control));
                    children.push(id);
                }
            }
        }
        let mut pane = Node::new(Role::Pane);
        pane.set_label("Agent view");
        pane.set_bounds(bounds(scene.panel));
        pane.set_children(children);
        snapshot.nodes.push((AGENT_VIEW_PANE, pane));
        if let Some((_, root)) = snapshot.nodes.iter_mut().find(|(id, _)| *id == ROOT) {
            let mut children = root.children().to_vec();
            children.push(AGENT_VIEW_PANE);
            root.set_children(children);
        }
        if self.agent_view.state.has_keyboard() {
            let focused = match &self.agent_view.state.focus {
                crate::ui::agent_view::Focus::Composer => Target::Composer,
                crate::ui::agent_view::Focus::Target(target) => target.clone(),
            };
            if let Some(node) = self.accessibility.agent_view.nodes.get(&focused) {
                snapshot.focus = *node;
            }
        }
        snapshot
    }

    /// Clicks and focus requests on the agent view. Returns whether the
    /// node was the panel's.
    fn handle_agent_view_accessibility_action(&mut self, request: &ActionRequest) -> bool {
        use crate::ui::agent_view::scene::Target;
        let Some(target) = self
            .accessibility
            .agent_view
            .targets
            .get(&request.target_node)
            .cloned()
        else {
            return request.target_node == AGENT_VIEW_PANE;
        };
        let focus = if target == Target::Composer {
            crate::ui::agent_view::Focus::Composer
        } else {
            crate::ui::agent_view::Focus::Target(target.clone())
        };
        match request.action {
            Action::Click => {
                self.set_agent_view_keyboard(true);
                self.agent_view.state.focus = focus;
                self.activate_agent_view_target(target);
            }
            Action::Focus => {
                self.set_agent_view_keyboard(true);
                self.agent_view.state.focus = focus;
                self.agent_view.state.focus_visible = true;
                self.reveal_agent_view_target(&target);
                self.request_redraw(RedrawReason::Action);
            }
            _ => {}
        }
        true
    }

    /// Clicking a toast or its Dismiss action dismisses it, as a mouse press
    /// on it does, so a persistent notice never needs the mouse.
    fn handle_toast_accessibility_action(&mut self, toast: u64, request: &ActionRequest) {
        let dismiss = match request.action {
            Action::Click => true,
            Action::CustomAction => matches!(
                request.data,
                Some(ActionData::CustomAction(TOAST_DISMISS_ACTION))
            ),
            _ => false,
        };
        if dismiss && self.toasts.dismiss(toast) {
            self.request_redraw(RedrawReason::Accessibility);
        }
    }

    /// The window's root node, without its children.
    fn window_root_node(&self) -> Node {
        let metrics = self.ui_metrics();
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, metrics.width, metrics.height));
        root
    }

    fn with_toast_accessibility(&self, mut snapshot: SemanticSnapshot) -> SemanticSnapshot {
        let toasts = self.toasts.build_accessibility_info();
        if toasts.is_empty() {
            return snapshot;
        }
        // Toasts sit in the terminal area's corner, clear of the dock.
        let UiMetrics {
            height: window_height,
            cell_width,
            cell_height,
            ..
        } = self.ui_metrics();
        let window_width = self.pane_placement().right;
        let placements = crate::ui::toast::toast_layout(
            &toasts,
            window_width,
            window_height,
            cell_width,
            cell_height,
        );
        let mut toast_ids = Vec::with_capacity(placements.len());
        for placement in placements {
            let toast = &toasts[placement.toast_index];
            let id = toast_node(toast.id);
            let severity = match toast.severity {
                crate::ui::toast::ToastSeverity::Info => "Information",
                crate::ui::toast::ToastSeverity::Success => "Success",
                crate::ui::toast::ToastSeverity::Warning => "Warning",
                crate::ui::toast::ToastSeverity::Error => "Error",
            };
            let mut node = Node::new(Role::Status);
            node.set_label(format!("{severity}: {}", toast.message));
            node.set_live(
                if toast.severity == crate::ui::toast::ToastSeverity::Error {
                    Live::Assertive
                } else {
                    Live::Polite
                },
            );
            node.add_action(Action::Click);
            node.add_action(Action::CustomAction);
            node.push_custom_action(CustomAction {
                id: TOAST_DISMISS_ACTION,
                description: "Dismiss".into(),
            });
            node.set_bounds(bounds_rect(placement.rect));
            snapshot.nodes.push((id, node));
            toast_ids.push(id);
        }
        if let Some((_, root)) = snapshot.nodes.iter_mut().find(|(id, _)| *id == ROOT) {
            let mut children = root.children().to_vec();
            children.extend(toast_ids);
            root.set_children(children);
        }
        snapshot
    }

    fn accessibility_snapshot(&mut self) -> SemanticSnapshot {
        let UiMetrics {
            width,
            height,
            tab_bar_height: tab_height,
            cell_width,
            cell_height,
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
                    cell_width,
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

    fn cellar_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self
            .cellar_layout()
            .expect("the Cellar layer is open only while Harvest is on");
        let (entries, selected, total, evicted, scroll, healthy) = self
            .harvest
            .as_ref()
            .map(|harvest| {
                (
                    harvest
                        .cellar
                        .visible_entries()
                        .cloned()
                        .collect::<Vec<_>>(),
                    harvest.cellar.selected_id(),
                    harvest.cellar.len(),
                    harvest.cellar.evicted_commands(),
                    harvest.cellar.scroll,
                    (harvest.cellar.success_rate() * 100.0).round() as usize,
                )
            })
            .expect("the Cellar layer is open only while Harvest is on");
        let targets = entries
            .iter()
            .map(|entry| HarvestSemanticTarget::CellarEntry(entry.id))
            .collect::<HashSet<_>>();
        self.accessibility
            .harvest
            .retain(|target| targets.contains(target));
        let entry_nodes = entries
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    self.accessibility
                        .harvest
                        .node(&HarvestSemanticTarget::CellarEntry(entry.id)),
                )
            })
            .collect::<Vec<_>>();

        let mut nodes = Vec::with_capacity(entries.len() + 7);

        let mut title = Node::new(Role::Heading);
        title.set_label("Root Cellar");
        nodes.push((CELLAR_TITLE, title));

        let mut status = Node::new(Role::Status);
        status.set_label(if total == 0 {
            "No harvested commands".to_string()
        } else if evicted == 0 {
            format!("{total} retained harvested commands, {healthy}% healthy")
        } else {
            format!(
                "{total} retained harvested commands, {evicted} older commands evicted, {healthy}% healthy"
            )
        });
        status.set_live(Live::Polite);
        status.set_bounds(bounds_rect(layout.summary));
        nodes.push((CELLAR_STATUS, status));

        for (entry, (id, node_id)) in entries.iter().zip(&entry_nodes) {
            let mut node = Node::new(Role::ListBoxOption);
            node.set_label(entry.command.clone());
            let result = match entry.exit_code {
                Some(0) => "successful".to_string(),
                Some(code) => format!("failed with exit code {code}"),
                None => "still running".to_string(),
            };
            let owner = format!(
                "tab {}, pane {}",
                entry.source.tab_id,
                u64::from(entry.source.pane_id) + 1,
            );
            let retention = if entry.output_truncated {
                "; only the last 64 KiB of output is retained"
            } else {
                ""
            };
            node.set_description(format!(
                "{result}; {owner}; terminal line {}{retention}. Activate for command actions",
                entry.row
            ));
            node.set_selected(selected == Some(*id));
            node.add_action(Action::Focus);
            node.add_action(Action::Click);
            if let Some((_, row)) = layout.rows.iter().find(|(row_id, _)| row_id == id) {
                node.set_bounds(bounds_rect(*row));
            }
            nodes.push((*node_id, node));
        }

        let mut list = Node::new(Role::ListBox);
        list.set_label("Harvested command history");
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_children(
            entry_nodes
                .iter()
                .map(|(_, node)| *node)
                .collect::<Vec<_>>(),
        );
        list.set_bounds(bounds_rect(layout.list));
        if scroll > 0 {
            list.add_action(Action::ScrollUp);
        }
        if scroll.saturating_add(layout.max_visible) < total {
            list.add_action(Action::ScrollDown);
        }
        nodes.push((CELLAR_LIST, list));

        let mut close = Node::new(Role::Button);
        close.set_label("Close Root Cellar");
        close.set_bounds(bounds_rect(layout.close));
        close.add_action(Action::Focus);
        close.add_action(Action::Click);
        nodes.push((CELLAR_CLOSE, close));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Root Cellar command history");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children([CELLAR_TITLE, CELLAR_STATUS, CELLAR_LIST, CELLAR_CLOSE]);
        nodes.push((CELLAR_DIALOG, dialog));

        let default_focus = selected
            .and_then(|selected| {
                entry_nodes
                    .iter()
                    .find(|(id, _)| *id == selected)
                    .map(|(_, node)| *node)
            })
            .unwrap_or(CELLAR_CLOSE);
        let focus = self
            .accessibility
            .focus_override
            .filter(|focus| *focus == CELLAR_CLOSE || *focus == default_focus)
            .unwrap_or(default_focus);
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: CELLAR_DIALOG,
            nodes,
            focus,
        }
    }

    /// The sprout menu's nodes, placed where the menu is drawn.
    fn sprout_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self
            .sprout_menu_layout()
            .expect("the sprout menu layer is open only while its menu is");
        let (request_id, actions, selected, command) = self
            .harvest
            .as_ref()
            .and_then(|harvest| harvest.sprout_menu.as_ref())
            .map(|menu| {
                (
                    menu.request_id,
                    menu.actions
                        .iter()
                        .map(|action| {
                            (
                                *action,
                                menu.action_label(*action),
                                menu.action_enabled(*action),
                                menu.action_description(*action),
                            )
                        })
                        .collect::<Vec<_>>(),
                    menu.selected,
                    menu.command
                        .as_ref()
                        .map(|entry| entry.command.clone())
                        .unwrap_or_else(|| "selected command".to_string()),
                )
            })
            .unwrap();
        let targets = actions
            .iter()
            .map(|(action, _, _, _)| HarvestSemanticTarget::SproutAction {
                request_id,
                action: *action,
            })
            .collect::<HashSet<_>>();
        self.accessibility
            .harvest
            .retain(|target| targets.contains(target));
        let action_nodes = actions
            .iter()
            .map(|(action, _, _, _)| {
                self.accessibility
                    .harvest
                    .node(&HarvestSemanticTarget::SproutAction {
                        request_id,
                        action: *action,
                    })
            })
            .collect::<Vec<_>>();

        let mut nodes = Vec::with_capacity(actions.len() + 4);

        for (index, (((_action, label, enabled, description), node_id), row)) in actions
            .iter()
            .zip(action_nodes.iter().copied())
            .zip(&layout.rows)
            .enumerate()
        {
            let mut node = Node::new(Role::MenuItem);
            node.set_label(label.as_str());
            if let Some(description) = description {
                node.set_description(*description);
            }
            node.set_selected(index == selected);
            node.set_bounds(bounds_rect(*row));
            if *enabled {
                node.add_action(Action::Focus);
                node.add_action(Action::Click);
            } else {
                node.set_disabled();
            }
            nodes.push((node_id, node));
        }

        let mut menu = Node::new(Role::Menu);
        menu.set_label(format!("Actions for {command}"));
        menu.set_orientation(accesskit::Orientation::Vertical);
        menu.set_bounds(bounds_rect(layout.panel));
        menu.set_children(action_nodes.clone());
        nodes.push((SPROUT_MENU, menu));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(format!("Harvest actions for {command}"));
        dialog.set_description("Press Escape to close without running an action");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children([SPROUT_MENU]);
        nodes.push((SPROUT_DIALOG, dialog));

        let default_focus = action_nodes
            .get(selected.min(action_nodes.len().saturating_sub(1)))
            .copied()
            .unwrap_or(SPROUT_MENU);
        let focus = self
            .accessibility
            .focus_override
            .filter(|focus| *focus == default_focus)
            .unwrap_or(default_focus);
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: SPROUT_DIALOG,
            nodes,
            focus,
        }
    }

    /// The open Workspaces menu as a menu of items sharing the drawn geometry.
    fn workspace_menu_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self.workspace_menu_layout();
        let menu = self
            .workspace_menu
            .as_ref()
            .expect("the Workspaces menu layer is open only while its menu is");
        let mut nodes = Vec::new();
        let mut items = Vec::new();
        for (row, entry) in menu.rows().iter().enumerate() {
            let crate::ui::popup_menu::MenuRow::Item(item) = entry else {
                continue;
            };
            let id = workspace_menu_node(row);
            let mut node = Node::new(Role::MenuItem);
            node.set_label(item.label.as_str());
            let description = match (&item.detail, item.checked) {
                (Some(detail), true) => Some(format!("{detail}, current workspace")),
                (Some(detail), false) => Some(detail.clone()),
                (None, true) => Some("Current workspace".into()),
                (None, false) => None,
            };
            if let Some(description) = description {
                node.set_description(description);
            }
            node.set_selected(menu.highlighted() == Some(row));
            if item.enabled {
                node.add_action(Action::Focus);
                node.add_action(Action::Click);
            } else {
                node.set_disabled();
            }
            if let Some(&bounds) = layout.as_ref().and_then(|layout| layout.rows.get(row)) {
                node.set_bounds(bounds_rect(bounds));
            }
            items.push(id);
            nodes.push((id, node));
        }
        let mut menu_node = Node::new(Role::Menu);
        menu_node.set_label("Workspaces");
        menu_node.set_orientation(accesskit::Orientation::Vertical);
        if let Some(layout) = &layout {
            menu_node.set_bounds(bounds_rect(layout.panel));
        }
        menu_node.set_children(items);
        nodes.push((WORKSPACE_MENU, menu_node));

        let focus = menu
            .highlighted()
            .map_or(WORKSPACE_MENU, workspace_menu_node);
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: WORKSPACE_MENU,
            nodes,
            focus,
        }
    }

    fn handle_workspace_menu_accessibility_action(&mut self, request: ActionRequest) {
        let Some(row) = workspace_menu_row(request.target_node) else {
            return;
        };
        match request.action {
            Action::Focus => {
                if let Some(menu) = self.workspace_menu.as_mut() {
                    menu.set_highlight(Some(row));
                }
                self.accessibility.focus_override = Some(request.target_node);
            }
            Action::Click => {
                let action = self
                    .workspace_menu
                    .as_ref()
                    .and_then(|menu| menu.action_at(row));
                if let Some(action) = action {
                    self.activate_workspace_menu(action);
                }
                self.accessibility.focus_override = None;
            }
            _ => {}
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
            ModalLayer::SessionPanel => self.session_accessibility_snapshot(),
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
            ModalLayer::SessionPanel => self.accessibility.is_session_node(node),
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

    fn palette_modal_accessibility_snapshot(&mut self, modal: PaletteModal) -> ModalSnapshot {
        let ids = palette_modal_node_ids(modal);
        let mut info = self.palette_owner(modal).build_info();
        let mut layout = self.command_palette_accessibility_layout(&info);
        self.palette_owner_mut(modal)
            .palette_mut()
            .set_max_visible(layout.rows_that_fit);
        info = self.palette_owner(modal).build_info();
        layout = self.command_palette_accessibility_layout(&info);
        let words = self.palette_modal_wording(modal, &info);
        let editable = self.palette_owner(modal).query_editable();

        let registered = self
            .palette_owner(modal)
            .palette()
            .command_ids()
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        self.accessibility
            .palette_modal_mut(modal)
            .retain(|id| registered.contains(id));
        let visible_entries = info
            .entries
            .iter()
            .take(layout.visible_count)
            .cloned()
            .collect::<Vec<_>>();
        let entry_nodes = visible_entries
            .iter()
            .map(|entry| self.accessibility.palette_modal_mut(modal).node(&entry.id))
            .collect::<Vec<_>>();
        let actionable = visible_entries
            .iter()
            .map(|entry| self.palette_entry_actionable(modal, &entry.id))
            .collect::<Vec<_>>();
        let selected_action = entry_nodes
            .get(info.selected)
            .filter(|_| actionable[info.selected])
            .copied();

        let mut nodes = Vec::with_capacity(entry_nodes.len() + 5);

        let mut input = Node::new(Role::SearchInput);
        input.set_label(words.input);
        input.set_placeholder(info.placeholder.clone());
        set_text_value(&mut input, ids.input, &info.input, Some(info.cursor));
        input.set_bounds(rect(
            layout.panel_x + layout.padding,
            layout.panel_y + layout.padding,
            layout.panel_width - layout.padding * 2.0,
            layout.input_height,
        ));
        input.set_controls([ids.list]);
        input.add_action(Action::Focus);
        if editable {
            input.add_action(Action::SetValue);
        } else {
            input.set_read_only();
        }
        if let Some(selected) = selected_action {
            input.set_active_descendant(selected);
        }
        nodes.push((ids.input, input));

        for (row, ((entry, node_id), actionable)) in visible_entries
            .iter()
            .zip(entry_nodes.iter().copied())
            .zip(actionable.iter().copied())
            .enumerate()
        {
            let mut node = Node::new(if actionable {
                Role::ListBoxOption
            } else {
                Role::Status
            });
            node.set_label(entry.title.clone());
            node.set_description(if modal == PaletteModal::Commands {
                let mut description = format!("{} {}", entry.category, info.item_noun);
                if entry.destructive {
                    description.push_str(", destructive");
                }
                description
            } else {
                entry.category.clone()
            });
            if let Some(shortcut) = &entry.shortcut {
                node.set_keyboard_shortcut(shortcut.clone());
            }
            node.set_bounds(rect(
                layout.panel_x + layout.padding * 0.5,
                layout.list_y + row as f32 * layout.row_height,
                layout.panel_width - layout.padding,
                layout.row_height,
            ));
            if actionable {
                node.set_selected(row == info.selected);
                node.add_action(Action::Click);
                node.add_action(Action::Focus);
            } else {
                node.set_live(Live::Polite);
            }
            nodes.push((node_id, node));
        }

        let mut list = Node::new(if actionable.contains(&true) {
            Role::ListBox
        } else {
            Role::List
        });
        list.set_label(words.list);
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel_x + layout.padding * 0.5,
            layout.list_y,
            layout.panel_width - layout.padding,
            layout.list_height,
        ));
        if let Some(selected) = selected_action {
            list.set_active_descendant(selected);
        }
        list.set_children(entry_nodes);
        nodes.push((ids.list, list));

        let mut dialog_children = vec![ids.input, ids.list];
        if visible_entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label(words.empty);
            empty.set_live(Live::Polite);
            nodes.push((ids.empty, empty));
            dialog_children.push(ids.empty);
        }
        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(words.dialog);
        if let Some(description) = words.description {
            dialog.set_description(description);
        }
        dialog.set_modal();
        dialog.set_bounds(rect(
            layout.panel_x,
            layout.panel_y,
            layout.panel_width,
            layout.panel_height,
        ));
        dialog.set_children(dialog_children);
        nodes.push((ids.dialog, dialog));

        self.accessibility.focus_override = Some(ids.input);
        ModalSnapshot {
            dialog: ids.dialog,
            nodes,
            focus: ids.input,
        }
    }

    /// Whether choosing an entry does anything. Only a live-domain picker
    /// lists status lines among its choices.
    fn palette_entry_actionable(&self, modal: PaletteModal, command: &str) -> bool {
        modal != PaletteModal::Domains || self.domain_picker.is_actionable(command)
    }

    fn palette_modal_wording(
        &self,
        modal: PaletteModal,
        info: &crate::ui::command_palette::PaletteInfo,
    ) -> PaletteWording {
        match modal {
            PaletteModal::Commands => PaletteWording {
                dialog: info.dialog_label.to_owned(),
                description: None,
                input: "Search commands",
                list: format!("{} results", info.item_noun),
                empty: format!("No matching {}s", info.item_noun),
            },
            PaletteModal::Domains => PaletteWording {
                dialog: "Attach live domain".into(),
                description: Some(if self.domain_picker.is_busy() {
                    "Waiting for authoritative domain state; Escape returns to hosts"
                } else if !self.domain_picker.can_filter() {
                    "Domain discovery status; Escape returns to hosts"
                } else {
                    "Choose a local or saved SSH host, then choose a live domain"
                }),
                input: "Search live-domain hosts or domains",
                list: format!("Live-domain {} results", info.item_noun),
                empty: "No matching live-domain results".into(),
            },
            PaletteModal::Unicode => PaletteWording {
                dialog: "Unicode character picker".into(),
                description: Some(
                    "Search common characters or enter U+ followed by a non-control Unicode scalar",
                ),
                input: "Search Unicode characters or enter a code point",
                list: "Unicode character results".into(),
                empty: info
                    .status
                    .unwrap_or("No matching Unicode character")
                    .into(),
            },
        }
    }

    fn info_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self.info_overlay_layout();
        self.info_overlay
            .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
        let request_id = self.info_overlay.request_id();
        let awaiting = self.info_overlay.is_awaiting_current();
        let scroll_offset = self.info_overlay.viewport().row();
        let horizontal_offset = self.info_overlay.viewport().column();
        let total_lines = self.info_overlay.lines().len();
        let input_info = self.info_overlay.input_info(request_id);
        let input_visible = input_info.as_ref().is_some_and(|input| {
            layout
                .source_line_rect(input.source_line, scroll_offset, total_lines)
                .is_some()
        });

        let content_target = InfoSemanticTarget::Content(request_id);
        let copy_target = InfoSemanticTarget::Copy(request_id);
        let dismiss_target = InfoSemanticTarget::Dismiss(request_id);
        let visible_choices: Vec<_> = self
            .info_overlay
            .choices()
            .iter()
            .filter(|choice| {
                choice.source_line >= scroll_offset
                    && choice.source_line < scroll_offset.saturating_add(layout.rows_that_fit)
            })
            .cloned()
            .collect();
        let mut targets = HashSet::from([
            content_target.clone(),
            copy_target.clone(),
            dismiss_target.clone(),
        ]);
        if input_visible {
            targets.insert(InfoSemanticTarget::Input(request_id));
        }
        targets.extend(
            visible_choices
                .iter()
                .map(|choice| InfoSemanticTarget::Choice {
                    request_id,
                    id: choice.id.clone(),
                }),
        );
        self.accessibility
            .info
            .retain(|target| targets.contains(target));
        let content_id = self.accessibility.info.node(&content_target);
        let copy_id = self.accessibility.info.node(&copy_target);
        let dismiss_id = self.accessibility.info.node(&dismiss_target);
        let input_id = input_visible.then(|| {
            self.accessibility
                .info
                .node(&InfoSemanticTarget::Input(request_id))
        });
        let choice_nodes: Vec<_> = visible_choices
            .iter()
            .map(|choice| {
                let target = InfoSemanticTarget::Choice {
                    request_id,
                    id: choice.id.clone(),
                };
                (choice.clone(), self.accessibility.info.node(&target))
            })
            .collect();

        let visible_lines: Vec<_> = self
            .info_overlay
            .visible_lines()
            .map(|(_, line)| {
                line.chars()
                    .skip(horizontal_offset)
                    .take(layout.columns_that_fit)
                    .collect::<String>()
            })
            .collect();
        let first_line = scroll_offset.saturating_add(1).min(total_lines.max(1));
        let last_line = scroll_offset
            .saturating_add(layout.rows_that_fit)
            .min(total_lines);
        let status_text = format!(
            "Showing lines {first_line} through {last_line} of {total_lines}, starting at column {}",
            horizontal_offset + 1
        );

        let mut nodes = Vec::with_capacity(7 + choice_nodes.len());

        let mut title = Node::new(Role::Heading);
        title.set_label(self.info_overlay.title());
        title.set_bounds(rect(
            layout.card.x + layout.padding,
            layout.card.y + layout.padding,
            layout.card.width - layout.padding * 2.0,
            layout.title_height,
        ));
        nodes.push((INFO_TITLE, title));

        let mut content = Node::new(Role::Document);
        content.set_label("Notice content");
        content.set_value(visible_lines.join("\n"));
        content.set_description(status_text.clone());
        content.set_bounds(bounds_rect(layout.content));
        let mut content_children = input_id.into_iter().collect::<Vec<_>>();
        content_children.extend(choice_nodes.iter().map(|(_, node)| *node));
        content.set_children(content_children);
        if awaiting {
            content.add_action(Action::Focus);
            if self.info_overlay.viewport().row() > 0 {
                content.add_action(Action::ScrollUp);
            }
            if self.info_overlay.viewport().row() < self.info_overlay.viewport().max_row() {
                content.add_action(Action::ScrollDown);
            }
            if self.info_overlay.viewport().column() > 0 {
                content.add_action(Action::ScrollLeft);
            }
            if self.info_overlay.viewport().column() < self.info_overlay.viewport().max_column() {
                content.add_action(Action::ScrollRight);
            }
        } else {
            content.set_disabled();
        }
        nodes.push((content_id, content));

        if let (Some(input), Some(input_id)) = (input_info.as_ref(), input_id)
            && let Some(input_rect) =
                layout.source_line_rect(input.source_line, scroll_offset, total_lines)
        {
            let mut node = Node::new(Role::TextInput);
            node.set_label(input.label.trim_end_matches([' ', ':']));
            node.set_description(format!("Single-line value, at most {}", input.limit));
            set_text_value(&mut node, input_id, &input.value, Some(input.cursor_byte));
            node.set_bounds(bounds_rect(input_rect));
            if awaiting {
                node.add_action(Action::Focus);
                node.add_action(Action::SetValue);
            } else {
                node.set_disabled();
            }
            nodes.push((input_id, node));
        }

        for (choice, node_id) in &choice_nodes {
            let visible_row = choice.source_line.saturating_sub(scroll_offset);
            let mut node = Node::new(Role::Button);
            node.set_label(choice.label.as_str());
            node.set_description("Select this notice option");
            if let Some(shortcut) = choice.shortcut {
                node.set_keyboard_shortcut(shortcut.to_string());
            }
            node.set_bounds(rect(
                layout.content.x,
                layout.content.y + visible_row as f32 * layout.line_height,
                layout.content.width,
                layout.line_height,
            ));
            if awaiting {
                node.add_action(Action::Click);
                node.add_action(Action::Focus);
            } else {
                node.set_disabled();
            }
            nodes.push((*node_id, node));
        }

        let mut status = Node::new(Role::Status);
        status.set_label(status_text);
        status.set_live(Live::Polite);
        nodes.push((INFO_STATUS, status));

        let mut copy = Node::new(Role::Button);
        copy.set_label("Copy notice content");
        if self.info_overlay.copy_key_free() {
            copy.set_keyboard_shortcut("C");
        }
        copy.set_bounds(bounds_rect(layout.copy_button));
        if awaiting {
            copy.add_action(Action::Click);
            copy.add_action(Action::Focus);
        } else {
            copy.set_disabled();
        }
        nodes.push((copy_id, copy));

        let mut dismiss = Node::new(Role::DefaultButton);
        if input_info.is_some() {
            dismiss.set_label("Submit input");
            dismiss.set_keyboard_shortcut("Enter");
        } else {
            dismiss.set_label("Continue and close notice");
            dismiss.set_keyboard_shortcut("Enter or Escape");
        }
        dismiss.set_bounds(bounds_rect(layout.continue_button));
        if awaiting {
            dismiss.add_action(Action::Click);
            dismiss.add_action(Action::Focus);
        } else {
            dismiss.set_disabled();
        }
        nodes.push((dismiss_id, dismiss));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(self.info_overlay.title());
        dialog.set_description("Application notice with bounded scrollable content");
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        dialog.set_bounds(bounds_rect(layout.card));
        let mut children = vec![INFO_TITLE, content_id];
        children.extend([INFO_STATUS, copy_id, dismiss_id]);
        dialog.set_children(children);
        nodes.push((INFO_DIALOG, dialog));

        let model_focus = self.info_overlay.focus().and_then(|focus| {
            use crate::ui::info_overlay::InfoOverlayFocus;
            match focus {
                InfoOverlayFocus::Input => input_id,
                InfoOverlayFocus::Choice(id) => choice_nodes
                    .iter()
                    .find(|(choice, _)| &choice.id == id)
                    .map(|(_, node)| *node),
                InfoOverlayFocus::Copy => Some(copy_id),
                InfoOverlayFocus::Continue => Some(dismiss_id),
            }
        });
        let focus = model_focus.unwrap_or(if awaiting { content_id } else { INFO_DIALOG });
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: INFO_DIALOG,
            nodes,
            focus,
        }
    }

    fn context_accessibility_snapshot(&mut self) -> ModalSnapshot {
        use crate::ui::context_picker::{ContextPickerPhase, ContextSourceId};

        let layout = self.context_picker_layout();
        self.context_picker.set_viewport(layout.visible_count);
        let snapshot = self.context_picker.snapshot();
        let request_id = snapshot.request_id;
        let list_target = ContextSemanticTarget::List(request_id);
        let input_target = ContextSemanticTarget::FileInput(request_id);
        let cancel_target = ContextSemanticTarget::Cancel(request_id);
        let primary_target = ContextSemanticTarget::Primary(request_id);
        let visible_sources: Vec<_> = snapshot
            .sources
            .iter()
            .skip(snapshot.scroll_offset)
            .take(layout.visible_count)
            .cloned()
            .collect();

        let mut targets = HashSet::from([cancel_target, primary_target]);
        match snapshot.phase {
            ContextPickerPhase::Sources => {
                targets.insert(list_target);
                targets.extend(visible_sources.iter().map(|source| {
                    ContextSemanticTarget::Source {
                        request_id,
                        source: source.id,
                    }
                }));
            }
            ContextPickerPhase::FilePaths => {
                targets.insert(input_target);
            }
            ContextPickerPhase::Gathering => {}
        }
        self.accessibility
            .context
            .retain(|target| targets.contains(target));
        let cancel_id = self.accessibility.context.node(&cancel_target);
        let primary_id = self.accessibility.context.node(&primary_target);
        let list_id = (snapshot.phase == ContextPickerPhase::Sources)
            .then(|| self.accessibility.context.node(&list_target));
        let input_id = (snapshot.phase == ContextPickerPhase::FilePaths)
            .then(|| self.accessibility.context.node(&input_target));
        let source_nodes: Vec<_> = if snapshot.phase == ContextPickerPhase::Sources {
            visible_sources
                .iter()
                .map(|source| {
                    let id = self
                        .accessibility
                        .context
                        .node(&ContextSemanticTarget::Source {
                            request_id,
                            source: source.id,
                        });
                    (source.clone(), id)
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut nodes = Vec::with_capacity(8 + source_nodes.len());

        let title_text = match snapshot.phase {
            ContextPickerPhase::Sources => "Insert Context",
            ContextPickerPhase::FilePaths => "Choose Workspace Files",
            ContextPickerPhase::Gathering => "Gathering Context",
        };
        let mut title = Node::new(Role::Heading);
        title.set_label(title_text);
        title.set_bounds(rect(
            layout.card.x + layout.padding,
            layout.card.y + layout.padding,
            layout.card.width - layout.padding * 2.0,
            layout.title_height,
        ));
        nodes.push((CONTEXT_TITLE, title));

        if let Some(list_id) = list_id {
            let mut list = Node::new(Role::List);
            list.set_label("Available context sources");
            list.set_bounds(bounds_rect(layout.content));
            list.set_children(source_nodes.iter().map(|(_, id)| *id).collect::<Vec<_>>());
            if snapshot.awaiting {
                list.add_action(Action::Focus);
                if snapshot.scroll_offset > 0 {
                    list.add_action(Action::ScrollUp);
                }
                if snapshot.scroll_offset
                    < snapshot.sources.len().saturating_sub(layout.visible_count)
                {
                    list.add_action(Action::ScrollDown);
                }
            } else {
                list.set_disabled();
            }
            nodes.push((list_id, list));

            for (visible_index, (source, node_id)) in source_nodes.iter().enumerate() {
                let source_index = snapshot.scroll_offset + visible_index;
                let mut node = Node::new(Role::CheckBox);
                node.set_label(source.label.as_str());
                node.set_description(if source.id == ContextSourceId::SpecificFiles {
                    "Opens a workspace-bounded comma-separated file path editor"
                } else if source.available {
                    "Toggle this source in the context bundle"
                } else {
                    "Unavailable for the current working directory"
                });
                node.set_toggled(if source.selected {
                    Toggled::True
                } else {
                    Toggled::False
                });
                node.set_selected(source_index == snapshot.selected_index);
                node.set_bounds(rect(
                    layout.content.x,
                    layout.content.y + visible_index as f32 * layout.row_height,
                    layout.content.width,
                    layout.row_height,
                ));
                if snapshot.awaiting && source.available {
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                } else {
                    node.set_disabled();
                }
                nodes.push((*node_id, node));
            }
        }

        if let Some(input_id) = input_id {
            let mut input = Node::new(Role::TextInput);
            input.set_label("Workspace file paths");
            input.set_description(
                "Comma-separated paths; files must remain inside the context working directory",
            );
            input.set_value(snapshot.file_input.as_str());
            input.set_bounds(bounds_rect(layout.file_input));
            if snapshot.awaiting {
                input.add_action(Action::Focus);
                input.add_action(Action::SetValue);
            } else {
                input.set_disabled();
            }
            nodes.push((input_id, input));
        }

        let status_text = snapshot
            .status
            .clone()
            .unwrap_or_else(|| match snapshot.phase {
                ContextPickerPhase::Sources => format!(
                    "{} of {} sources selected",
                    snapshot
                        .sources
                        .iter()
                        .filter(|source| source.selected)
                        .count(),
                    snapshot.sources.len()
                ),
                ContextPickerPhase::FilePaths => {
                    "Enter comma-separated workspace-relative file paths".into()
                }
                ContextPickerPhase::Gathering => {
                    "Gathering bounded context in the background".into()
                }
            });
        let mut status = Node::new(Role::Status);
        status.set_label(status_text);
        status.set_live(Live::Polite);
        nodes.push((CONTEXT_STATUS, status));

        let mut cancel = Node::new(Role::Button);
        cancel.set_label(snapshot.cancel_label());
        cancel.set_keyboard_shortcut("Escape");
        cancel.set_bounds(bounds_rect(layout.cancel_button));
        if snapshot.awaiting {
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
        } else {
            cancel.set_disabled();
        }
        nodes.push((cancel_id, cancel));

        let primary_enabled = snapshot.awaiting && snapshot.phase != ContextPickerPhase::Gathering;
        let mut primary = Node::new(Role::DefaultButton);
        primary.set_label(snapshot.primary_label());
        primary.set_keyboard_shortcut("Enter");
        primary.set_bounds(bounds_rect(layout.primary_button));
        if primary_enabled {
            primary.add_action(Action::Click);
            primary.add_action(Action::Focus);
        } else {
            primary.set_disabled();
        }
        nodes.push((primary_id, primary));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(title_text);
        dialog.set_description(
            "Choose bounded local context and insert it into one exact terminal destination",
        );
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        dialog.set_bounds(bounds_rect(layout.card));
        let mut children = vec![CONTEXT_TITLE];
        if let Some(main) = list_id.or(input_id) {
            children.push(main);
        }
        children.extend([CONTEXT_STATUS, cancel_id, primary_id]);
        dialog.set_children(children);
        nodes.push((CONTEXT_DIALOG, dialog));

        let default_focus = match snapshot.phase {
            ContextPickerPhase::Sources => source_nodes
                .iter()
                .enumerate()
                .find(|(visible_index, (source, _))| {
                    source.available
                        && snapshot.scroll_offset + visible_index == snapshot.selected_index
                })
                .map(|(_, (_, id))| *id)
                .or_else(|| {
                    source_nodes
                        .iter()
                        .find(|(source, _)| source.available)
                        .map(|(_, id)| *id)
                })
                .unwrap_or(primary_id),
            ContextPickerPhase::FilePaths => input_id.unwrap_or(cancel_id),
            ContextPickerPhase::Gathering => cancel_id,
        };
        let focus = self.accessibility.modal_focus(
            &nodes,
            if snapshot.awaiting {
                default_focus
            } else {
                CONTEXT_DIALOG
            },
            |accessibility, node| {
                accessibility.is_context_node(node) && (snapshot.awaiting || node == CONTEXT_DIALOG)
            },
        );
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: CONTEXT_DIALOG,
            nodes,
            focus,
        }
    }

    fn diff_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self.diff_review_layout();
        self.diff_review
            .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
        let request_id = self.diff_review.request_id();
        let awaiting = self.diff_review.is_awaiting_current();
        let scroll_offset = self.diff_review.viewport().row();
        let horizontal_offset = self.diff_review.viewport().column();
        let total_rows = self.diff_review.rows().len();

        let content_target = DiffSemanticTarget::Content(request_id);
        let copy_target = DiffSemanticTarget::Copy(request_id);
        let dismiss_target = DiffSemanticTarget::Dismiss(request_id);
        let change_actions = [
            (DiffSemanticTarget::Stage(request_id), layout.stage_button),
            (DiffSemanticTarget::Revert(request_id), layout.revert_button),
        ];
        let mut targets = HashSet::from([content_target, copy_target, dismiss_target]);
        targets.extend(
            change_actions
                .iter()
                .filter(|(_, button)| button.is_some())
                .map(|(target, _)| *target),
        );
        self.accessibility
            .diff
            .retain(|target| targets.contains(target));
        let content_id = self.accessibility.diff.node(&content_target);
        let copy_id = self.accessibility.diff.node(&copy_target);
        let dismiss_id = self.accessibility.diff.node(&dismiss_target);

        let visible_rows: Vec<_> = self
            .diff_review
            .visible_rows()
            .map(|(_, row)| {
                row.display_text()
                    .chars()
                    .skip(horizontal_offset)
                    .take(layout.columns_that_fit)
                    .collect::<String>()
            })
            .collect();
        let first_row = scroll_offset.saturating_add(1).min(total_rows.max(1));
        let last_row = scroll_offset
            .saturating_add(layout.rows_that_fit)
            .min(total_rows);
        let status_text = format!(
            "Showing diff rows {first_row} through {last_row} of {total_rows}, starting at column {}",
            horizontal_offset + 1
        );

        let mut nodes = Vec::with_capacity(7);

        let title_text = format!("Diff: {}", self.diff_review.filename());
        let mut title = Node::new(Role::Heading);
        title.set_label(title_text.as_str());
        title.set_bounds(rect(
            layout.card.x + layout.padding,
            layout.card.y + layout.padding,
            layout.card.width - layout.padding * 2.0,
            layout.title_height,
        ));
        nodes.push((DIFF_TITLE, title));

        let mut content = Node::new(Role::Document);
        content.set_label(if self.diff_review.has_change_actions() {
            "Changes to this file since the agent started"
        } else {
            "Detected unified diff"
        });
        content.set_value(visible_rows.join("\n"));
        content.set_description(status_text.clone());
        content.set_bounds(bounds_rect(layout.content));
        if awaiting {
            content.add_action(Action::Focus);
            if self.diff_review.viewport().row() > 0 {
                content.add_action(Action::ScrollUp);
            }
            if self.diff_review.viewport().row() < self.diff_review.viewport().max_row() {
                content.add_action(Action::ScrollDown);
            }
            if self.diff_review.viewport().column() > 0 {
                content.add_action(Action::ScrollLeft);
            }
            if self.diff_review.viewport().column() < self.diff_review.viewport().max_column() {
                content.add_action(Action::ScrollRight);
            }
        } else {
            content.set_disabled();
        }
        nodes.push((content_id, content));

        let mut status = Node::new(Role::Status);
        status.set_label(status_text);
        status.set_live(Live::Polite);
        nodes.push((DIFF_STATUS, status));

        let mut change_ids = Vec::new();
        for (target, button) in change_actions {
            let Some(button) = button else {
                continue;
            };
            let (label, shortcut) = match target {
                DiffSemanticTarget::Stage(_) => ("Stage this file", "S"),
                _ => ("Revert this file to how it was when the agent started", "R"),
            };
            let id = self.accessibility.diff.node(&target);
            let mut node = Node::new(Role::Button);
            node.set_label(label);
            node.set_keyboard_shortcut(shortcut);
            node.set_bounds(bounds_rect(button));
            if awaiting {
                node.add_action(Action::Click);
                node.add_action(Action::Focus);
            } else {
                node.set_disabled();
            }
            nodes.push((id, node));
            change_ids.push(id);
        }

        let mut copy = Node::new(Role::Button);
        copy.set_label("Copy complete raw diff");
        copy.set_keyboard_shortcut("C");
        copy.set_bounds(bounds_rect(layout.copy_button));
        if awaiting {
            copy.add_action(Action::Click);
            copy.add_action(Action::Focus);
        } else {
            copy.set_disabled();
        }
        nodes.push((copy_id, copy));

        let mut dismiss = Node::new(Role::DefaultButton);
        dismiss.set_label("Mark this diff reviewed and close");
        dismiss.set_keyboard_shortcut("Enter or Escape");
        dismiss.set_bounds(bounds_rect(layout.done_button));
        if awaiting {
            dismiss.add_action(Action::Click);
            dismiss.add_action(Action::Focus);
        } else {
            dismiss.set_disabled();
        }
        nodes.push((dismiss_id, dismiss));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(title_text);
        dialog.set_description(if self.diff_review.has_change_actions() {
            "Scrollable review of one file an agent changed, which can be staged or reverted"
        } else {
            "Scrollable review of one detected unified diff"
        });
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        dialog.set_bounds(bounds_rect(layout.card));
        let mut children = vec![DIFF_TITLE, content_id, DIFF_STATUS];
        children.extend(change_ids);
        children.extend([copy_id, dismiss_id]);
        dialog.set_children(children);
        nodes.push((DIFF_DIALOG, dialog));

        let focus = self.accessibility.modal_focus(
            &nodes,
            if awaiting { dismiss_id } else { DIFF_DIALOG },
            |accessibility, node| {
                accessibility.is_diff_node(node) && (awaiting || node == DIFF_DIALOG)
            },
        );
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: DIFF_DIALOG,
            nodes,
            focus,
        }
    }

    fn guardrail_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self.guardrail_layout();
        let request_id = self.guardrail.request_id();
        let awaiting = self.guardrail.is_awaiting_current();
        let matched = self.guardrail.matched.clone();
        let confirm_target = GuardrailSemanticTarget::Confirm(request_id);
        let cancel_target = GuardrailSemanticTarget::Cancel(request_id);
        self.accessibility
            .guardrail
            .retain(|target| *target == confirm_target || *target == cancel_target);
        let confirm_id = self.accessibility.guardrail.node(&confirm_target);
        let cancel_id = self.accessibility.guardrail.node(&cancel_target);

        let mut nodes = Vec::with_capacity(7);

        let (command_value, reason_value, context_value, critical) = matched.as_ref().map_or_else(
            || {
                (
                    "Guarded command unavailable".to_string(),
                    "The confirmation request is closing".to_string(),
                    "No command can be released".to_string(),
                    true,
                )
            },
            |matched| {
                (
                    crate::safe_text::truncate_with_ellipsis(&matched.command, 1024),
                    crate::safe_text::truncate_with_ellipsis(&matched.reason, 512),
                    format!(
                        "Where: {}; As: {}",
                        crate::safe_text::truncate_with_ellipsis(&matched.target, 256),
                        crate::safe_text::truncate_with_ellipsis(&matched.user, 128)
                    ),
                    matched.severity == crate::terminal::guardrails::Severity::Critical,
                )
            },
        );

        let title_y = layout.card.y + layout.padding;
        let command_y = title_y + layout.title_height;
        let reason_y = command_y + layout.command_height;
        let context_y = reason_y + layout.reason_height;
        let content_x = layout.card.x + layout.padding;
        let content_width = layout.card.width - layout.padding * 2.0;

        let mut command = Node::new(Role::Code);
        command.set_label("Guarded command");
        command.set_value(command_value);
        command.set_bounds(rect(
            content_x,
            command_y,
            content_width,
            layout.command_height,
        ));
        nodes.push((GUARDRAIL_COMMAND, command));

        let mut reason = Node::new(Role::Alert);
        reason.set_label(reason_value.clone());
        reason.set_description(if critical {
            "Critical destructive-command warning"
        } else {
            "Potentially destructive-command warning"
        });
        reason.set_live(if critical {
            Live::Assertive
        } else {
            Live::Polite
        });
        reason.set_bounds(rect(
            content_x,
            reason_y,
            content_width,
            layout.reason_height,
        ));
        nodes.push((GUARDRAIL_REASON, reason));

        let mut context = Node::new(Role::Label);
        context.set_label(context_value.clone());
        context.set_bounds(rect(
            content_x,
            context_y,
            content_width,
            layout.context_height,
        ));
        nodes.push((GUARDRAIL_CONTEXT, context));

        let mut cancel = Node::new(Role::Button);
        cancel.set_label("Cancel guarded command");
        cancel.set_description("Discard all buffered guarded input; recommended safe action");
        cancel.set_keyboard_shortcut("Enter or Escape");
        cancel.set_bounds(bounds_rect(layout.cancel_button));
        if awaiting {
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
        } else {
            cancel.set_disabled();
        }
        nodes.push((cancel_id, cancel));

        let mut confirm = Node::new(Role::Button);
        confirm.set_label(if critical {
            "Run dangerous command anyway"
        } else {
            "Confirm guarded command"
        });
        confirm.set_description(format!(
            "Release all buffered input to the exact captured terminal destination or destination set; {reason_value}; {context_value}"
        ));
        confirm.set_keyboard_shortcut("Ctrl+Enter");
        confirm.set_bounds(bounds_rect(layout.confirm_button));
        if awaiting {
            confirm.add_action(Action::Click);
            confirm.add_action(Action::Focus);
        } else {
            confirm.set_disabled();
        }
        nodes.push((confirm_id, confirm));

        let mut dialog = Node::new(Role::AlertDialog);
        dialog.set_label(if critical {
            "Dangerous command blocked"
        } else {
            "Command requires confirmation"
        });
        dialog.set_description("Review the exact command, target, and user before choosing");
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        dialog.set_bounds(bounds_rect(layout.card));
        dialog.set_children([
            GUARDRAIL_COMMAND,
            GUARDRAIL_REASON,
            GUARDRAIL_CONTEXT,
            cancel_id,
            confirm_id,
        ]);
        nodes.push((GUARDRAIL_DIALOG, dialog));

        let default_focus = if awaiting {
            cancel_id
        } else {
            GUARDRAIL_DIALOG
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                accessibility.is_guardrail_node(node) && (awaiting || node == GUARDRAIL_DIALOG)
            });
        ModalSnapshot {
            dialog: GUARDRAIL_DIALOG,
            nodes,
            focus,
        }
    }

    fn shortcuts_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let mut info = self.shortcuts_panel.build_info(&self.shortcut_registry);
        let mut layout = self.shortcuts_accessibility_layout(&info);
        self.shortcuts_panel.set_max_visible(layout.rows_that_fit);
        info = self.shortcuts_panel.build_info(&self.shortcut_registry);
        layout = self.shortcuts_accessibility_layout(&info);

        let mut nodes = Vec::with_capacity(info.entries.len() + 8);

        let mut input = Node::new(Role::SearchInput);
        input.set_label("Filter keyboard shortcuts");
        input.set_placeholder("Filter shortcuts...");
        set_text_value(&mut input, SHORTCUTS_INPUT, &info.filter, Some(info.cursor));
        input.set_bounds(bounds_rect(layout.input));
        input.set_controls([SHORTCUTS_LIST]);
        if info.recording {
            input.set_read_only();
        } else {
            input.add_action(Action::Focus);
            input.add_action(Action::SetValue);
        }

        let mut list_children = Vec::with_capacity(info.entries.len());
        let mut selected_node = None;
        for (row, entry) in info.entries.iter().enumerate() {
            let target = if entry.is_header {
                ShortcutsSemanticTarget::Header(entry.action.meta().category)
            } else {
                ShortcutsSemanticTarget::Action(entry.action)
            };
            let node_id = self.accessibility.shortcuts.node(&target);
            list_children.push(node_id);
            let bounds = bounds_rect(layout.row(row));
            if entry.is_header {
                let mut heading = Node::new(Role::Heading);
                heading.set_label(entry.label);
                heading.set_bounds(bounds);
                nodes.push((node_id, heading));
                continue;
            }

            let mut node = Node::new(Role::Button);
            node.set_label(format!("Change {} shortcut", entry.label));
            node.set_description(format!(
                "{} shortcut; activate, then press a key; use the Clear binding action to unbind",
                entry.action.meta().category.label()
            ));
            node.set_value(entry.binding.clone());
            if entry.binding != "Unset" {
                node.set_keyboard_shortcut(entry.binding.clone());
            }
            node.set_selected(entry.selected);
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.add_action(Action::CustomAction);
            node.push_custom_action(CustomAction {
                id: SHORTCUT_CLEAR_ACTION,
                description: "Clear binding".into(),
            });
            node.set_bounds(bounds);
            if entry.selected {
                selected_node = Some(node_id);
            }
            nodes.push((node_id, node));
        }
        if let Some(selected) = selected_node {
            input.set_active_descendant(selected);
        }
        nodes.push((SHORTCUTS_INPUT, input));

        let mut list = Node::new(Role::List);
        list.set_label(format!("{} keyboard shortcuts", info.total_entries));
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(bounds_rect(layout.list));
        if let Some(selected) = selected_node {
            list.set_active_descendant(selected);
        }
        list.set_children(list_children);
        nodes.push((SHORTCUTS_LIST, list));

        let mut dialog_children = vec![SHORTCUTS_INPUT, SHORTCUTS_LIST];
        if info.entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No matching keyboard shortcuts");
            empty.set_live(Live::Polite);
            nodes.push((SHORTCUTS_EMPTY, empty));
            dialog_children.push(SHORTCUTS_EMPTY);
        }
        if info.recording || info.notice.is_some() {
            let mut status = Node::new(Role::Status);
            status.set_label(if info.recording {
                "Recording shortcut. Press a key to bind, Delete to clear, or Escape to cancel."
                    .to_string()
            } else {
                info.notice
                    .as_ref()
                    .map(crate::ui::shortcuts_panel::ShortcutNotice::message)
                    .unwrap_or_default()
            });
            status.set_live(Live::Polite);
            nodes.push((SHORTCUTS_STATUS, status));
            dialog_children.push(SHORTCUTS_STATUS);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Keyboard shortcuts");
        dialog.set_description("Search, change, or clear application keyboard shortcuts");
        dialog.set_modal();
        dialog.set_bounds(rect(
            layout.panel_x,
            layout.panel_y,
            layout.panel_width,
            layout.panel_height,
        ));
        dialog.set_children(dialog_children);
        nodes.push((SHORTCUTS_DIALOG, dialog));

        let default_focus = selected_node.unwrap_or(if info.recording {
            SHORTCUTS_DIALOG
        } else {
            SHORTCUTS_INPUT
        });
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                (!info.recording || node != SHORTCUTS_INPUT)
                    && accessibility.is_shortcuts_node(node)
            });
        ModalSnapshot {
            dialog: SHORTCUTS_DIALOG,
            nodes,
            focus,
        }
    }

    fn settings_accessibility_snapshot(&mut self) -> ModalSnapshot {
        use crate::ui::settings::SettingKind;

        let mut info = self
            .settings_panel
            .build_info(&self.config, &self.shortcut_registry);
        let mut layout = self.settings_accessibility_layout(&info);
        self.settings_panel.set_max_visible(layout.rows_that_fit);
        info = self
            .settings_panel
            .build_info(&self.config, &self.shortcut_registry);
        info.unsaved = self.config_has_unsaved_changes();
        layout = self.settings_accessibility_layout(&info);
        let end = (info.scroll_offset + layout.visible_count).min(info.entries.len());
        let visible_entries = &info.entries[info.scroll_offset..end];

        let mut nodes = Vec::with_capacity(visible_entries.len() + 16);

        let mut input = Node::new(Role::SearchInput);
        input.set_label("Search settings");
        input.set_placeholder("Search settings");
        set_text_value(
            &mut input,
            SETTINGS_INPUT,
            &info.filter,
            Some(info.filter_cursor),
        );
        input.set_bounds(bounds_rect(layout.input));
        input.set_controls([SETTINGS_LIST]);
        if info.dropdown_open {
            input.set_read_only();
        } else {
            input.add_action(Action::Focus);
            input.add_action(Action::SetValue);
        }

        let selectables_before_offset = info.entries[..info.scroll_offset]
            .iter()
            .filter(|entry| !entry.is_header)
            .count();
        let mut selectable_index = selectables_before_offset;
        let mut list_children = Vec::with_capacity(visible_entries.len());
        let mut selected_setting = None;
        let mut open_dropdown = None;

        for (row, entry) in visible_entries.iter().enumerate() {
            let target = if entry.is_header {
                SettingsSemanticTarget::Header(entry.section)
            } else {
                SettingsSemanticTarget::Setting {
                    section: entry.section,
                    label: entry.label,
                }
            };
            let node_id = self.accessibility.settings.node(&target);
            list_children.push(node_id);
            let row_rect = layout.row_rect(row);
            let bounds = bounds_rect(row_rect);

            if entry.is_header {
                let mut node = Node::new(Role::Heading);
                node.set_label(entry.label);
                node.set_bounds(bounds);
                nodes.push((node_id, node));
                continue;
            }

            let is_selected = selectable_index == info.selected;
            if is_selected {
                selected_setting = Some(node_id);
            }
            let mut node = match &entry.kind {
                SettingKind::Numeric {
                    value,
                    min,
                    max,
                    step,
                    ..
                } => {
                    let mut node = Node::new(Role::SpinButton);
                    node.set_label(entry.label);
                    node.set_description(format!("{} setting", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.set_numeric_value(f64::from(*value));
                    node.set_min_numeric_value(f64::from(*min));
                    node.set_max_numeric_value(f64::from(*max));
                    node.set_numeric_value_step(f64::from(*step));
                    node.add_action(Action::Increment);
                    node.add_action(Action::Decrement);
                    node.add_action(Action::Focus);
                    node
                }
                SettingKind::Dropdown { options, .. } => {
                    let mut node = Node::new(Role::ComboBox);
                    node.set_label(entry.label);
                    node.set_description(format!("{} setting", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.set_expanded(is_selected && info.dropdown_open);
                    node.add_action(Action::Click);
                    node.add_action(Action::Expand);
                    node.add_action(Action::Collapse);
                    node.add_action(Action::Focus);
                    if is_selected && info.dropdown_open {
                        node.set_controls([SETTINGS_DROPDOWN]);
                        open_dropdown = Some((entry, options.clone()));
                    }
                    node
                }
                SettingKind::SteppedList { values, .. } => {
                    let mut node = Node::new(Role::ComboBox);
                    node.set_label(entry.label);
                    node.set_description(format!("{} setting", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.set_expanded(is_selected && info.dropdown_open);
                    node.add_action(Action::Click);
                    node.add_action(Action::Expand);
                    node.add_action(Action::Collapse);
                    node.add_action(Action::Focus);
                    if is_selected && info.dropdown_open {
                        node.set_controls([SETTINGS_DROPDOWN]);
                        open_dropdown =
                            Some((entry, values.iter().map(ToString::to_string).collect()));
                    }
                    node
                }
                SettingKind::Action { .. } => {
                    let mut node = Node::new(Role::Button);
                    node.set_label(entry.label);
                    node.set_description(format!("{} action", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                    node
                }
                SettingKind::ReadOnly => {
                    let mut node = Node::new(Role::Label);
                    node.set_label(entry.label);
                    node.set_description(format!("{} setting", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.set_read_only();
                    node.add_action(Action::Focus);
                    node
                }
            };
            node.set_bounds(bounds);
            nodes.push((node_id, node));
            selectable_index += 1;
        }

        if let Some(selected) = selected_setting {
            input.set_active_descendant(selected);
        }
        nodes.push((SETTINGS_INPUT, input));

        let mut list = Node::new(Role::List);
        list.set_label(if info.filter.is_empty() {
            info.page.label()
        } else {
            "Search results"
        });
        list.set_orientation(accesskit::Orientation::Vertical);
        let first_row = layout.row_rect(0);
        list.set_bounds(rect(
            first_row.x,
            layout.list_y,
            first_row.width,
            layout.list_height,
        ));
        list.set_children(list_children);
        nodes.push((SETTINGS_LIST, list));

        // Sidebar pages are tabs selecting what the list shows.
        let mut page_ids = Vec::new();
        for (index, page) in crate::ui::settings::SettingsPage::all().iter().enumerate() {
            let id = self
                .accessibility
                .settings
                .node(&SettingsSemanticTarget::Page(*page));
            let bounds = layout.page_rect(index);
            let mut tab = Node::new(Role::Tab);
            tab.set_label(page.label());
            tab.set_selected(info.filter.is_empty() && *page == info.page);
            tab.set_controls([SETTINGS_LIST]);
            tab.set_bounds(bounds_rect(bounds));
            tab.add_action(Action::Click);
            tab.add_action(Action::Focus);
            nodes.push((id, tab));
            page_ids.push(id);
        }
        let mut pages = Node::new(Role::TabList);
        pages.set_label("Settings pages");
        pages.set_orientation(accesskit::Orientation::Vertical);
        pages.set_bounds(bounds_rect(layout.sidebar));
        pages.set_children(page_ids);
        nodes.push((SETTINGS_PAGES, pages));

        for (id, label, description, bounds) in [
            (SETTINGS_CLOSE, "Close settings", None, layout.close_button),
            (
                SETTINGS_SAVE,
                "Save settings",
                Some(if info.unsaved {
                    "There are unsaved changes"
                } else {
                    "Settings match the saved file"
                }),
                layout.save_button,
            ),
        ] {
            let mut button = Node::new(Role::Button);
            button.set_label(label);
            if let Some(description) = description {
                button.set_description(description);
            }
            button.set_bounds(bounds_rect(bounds));
            button.add_action(Action::Click);
            button.add_action(Action::Focus);
            nodes.push((id, button));
        }

        let mut dialog_children = vec![
            SETTINGS_INPUT,
            SETTINGS_CLOSE,
            SETTINGS_PAGES,
            SETTINGS_LIST,
            SETTINGS_SAVE,
        ];
        if info.entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No matching settings");
            empty.set_live(Live::Polite);
            nodes.push((SETTINGS_EMPTY, empty));
            dialog_children.push(SETTINGS_EMPTY);
        }

        if let Some((entry, options)) = open_dropdown {
            if let Some(dropdown_rect) = self
                .settings_panel
                .dropdown_layout(&layout, &info.entries)
                .map(|dropdown| dropdown.rect)
            {
                let mut option_nodes = Vec::new();
                for (visible_index, (index, value)) in options
                    .iter()
                    .enumerate()
                    .skip(dropdown_rect.first_option)
                    .take(dropdown_rect.visible_options)
                    .enumerate()
                {
                    let option_id =
                        self.accessibility
                            .settings
                            .node(&SettingsSemanticTarget::Option {
                                section: entry.section,
                                label: entry.label,
                                value: value.clone(),
                            });
                    let mut option = Node::new(Role::ListBoxOption);
                    option.set_label(value.clone());
                    option.set_selected(index == info.dropdown_highlight);
                    option.add_action(Action::Click);
                    option.set_bounds(bounds_rect(dropdown_rect.option_bounds(visible_index)));
                    nodes.push((option_id, option));
                    option_nodes.push(option_id);
                }
                let mut dropdown = Node::new(Role::ListBox);
                dropdown.set_label(format!("{} options", entry.label));
                dropdown.set_orientation(accesskit::Orientation::Vertical);
                dropdown.set_bounds(bounds_rect(dropdown_rect.bounds));
                if let Some(highlighted) = option_nodes
                    .get(
                        info.dropdown_highlight
                            .saturating_sub(dropdown_rect.first_option),
                    )
                    .copied()
                {
                    dropdown.set_active_descendant(highlighted);
                }
                dropdown.set_children(option_nodes);
                nodes.push((SETTINGS_DROPDOWN, dropdown));
                dialog_children.push(SETTINGS_DROPDOWN);
            }
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Settings");
        dialog.set_modal();
        dialog.set_bounds(rect(
            layout.panel_x,
            layout.panel_y,
            layout.panel_width,
            layout.panel_height,
        ));
        dialog.set_children(dialog_children);
        nodes.push((SETTINGS_DIALOG, dialog));

        let default_focus = if info.dropdown_open {
            selected_setting.unwrap_or(SETTINGS_DIALOG)
        } else {
            SETTINGS_INPUT
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                (!info.dropdown_open || node != SETTINGS_INPUT)
                    && accessibility.is_settings_node(node)
            });
        ModalSnapshot {
            dialog: SETTINGS_DIALOG,
            nodes,
            focus,
        }
    }

    fn commands_accessibility_snapshot(&mut self) -> ModalSnapshot {
        use crate::ui::commands::panel::EditMode;

        let info = self.commands_panel.build_info(&self.config.commands);
        let layout = self.commands_panel_layout();
        let keys = custom_command_keys(&info.commands);
        let retained = keys
            .iter()
            .flat_map(|key| {
                [
                    CommandsSemanticTarget::Group(key.clone()),
                    CommandsSemanticTarget::Run(key.clone()),
                    CommandsSemanticTarget::Delete(key.clone()),
                ]
            })
            .collect::<HashSet<_>>();
        self.accessibility
            .commands
            .retain(|target| retained.contains(target));

        let mut nodes = Vec::with_capacity(info.commands.len() * 3 + 12);

        let mut close = Node::new(Role::Button);
        close.set_label("Close custom commands");
        close.add_action(Action::Click);
        close.add_action(Action::Focus);
        close.set_bounds(bounds_rect(layout.close_button));
        nodes.push((COMMANDS_CLOSE, close));

        let mut new_command = Node::new(Role::Button);
        new_command.set_label("New command");
        new_command.add_action(Action::Click);
        new_command.add_action(Action::Focus);
        new_command.set_bounds(bounds_rect(layout.new_button));
        nodes.push((COMMANDS_NEW, new_command));

        let mut dialog_children = vec![COMMANDS_CLOSE, COMMANDS_NEW];
        if let Some(form) = layout.form {
            let name_active = info.edit_mode == EditMode::EditingName;
            let command_active = info.edit_mode == EditMode::EditingCommand;
            let mut name = command_form_input(
                COMMANDS_NAME,
                "Command name",
                crate::ui::commands::panel::NAME_PLACEHOLDER,
                &info.form_name,
                name_active.then_some(info.form_cursor),
                form.name_input,
            );
            name.add_action(Action::SetValue);
            nodes.push((COMMANDS_NAME, name));

            let mut command = command_form_input(
                COMMANDS_COMMAND,
                "Shell command",
                crate::ui::commands::panel::COMMAND_PLACEHOLDER,
                &info.form_command,
                command_active.then_some(info.form_cursor),
                form.command_input,
            );
            command.add_action(Action::SetValue);
            nodes.push((COMMANDS_COMMAND, command));

            let mut save = Node::new(Role::Button);
            save.set_label("Save command");
            save.set_description("Both name and shell command are required");
            save.add_action(Action::Click);
            save.add_action(Action::Focus);
            save.set_bounds(bounds_rect(form.save_button));
            nodes.push((COMMANDS_SAVE, save));

            let mut cancel = Node::new(Role::Button);
            cancel.set_label("Cancel new command");
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
            cancel.set_bounds(bounds_rect(form.cancel_button));
            nodes.push((COMMANDS_CANCEL, cancel));
            dialog_children.extend([
                COMMANDS_NAME,
                COMMANDS_COMMAND,
                COMMANDS_SAVE,
                COMMANDS_CANCEL,
            ]);
        }

        let mut command_groups = Vec::with_capacity(keys.len());
        for (index, (command, key)) in info.commands.iter().zip(keys.iter()).enumerate() {
            let group_id = self
                .accessibility
                .commands
                .node(&CommandsSemanticTarget::Group(key.clone()));
            let run_id = self
                .accessibility
                .commands
                .node(&CommandsSemanticTarget::Run(key.clone()));
            let delete_id = self
                .accessibility
                .commands
                .node(&CommandsSemanticTarget::Delete(key.clone()));
            let mut run = Node::new(Role::Button);
            run.set_label(format!("Run {}", command.name));
            let mut description = command.command.clone();
            if let Some(alias) = &command.alias {
                description.push_str(&format!(", alias {alias}"));
            }
            run.set_description(description);
            run.add_action(Action::Click);
            run.add_action(Action::Focus);
            if layout.visible_range().contains(&index) {
                run.set_bounds(bounds_rect(layout.command_run(index)));
            }
            nodes.push((run_id, run));

            let mut delete = Node::new(Role::Button);
            delete.set_label(format!("Delete {}", command.name));
            delete.set_description("Permanently removes this custom command");
            delete.add_action(Action::Click);
            delete.add_action(Action::Focus);
            if layout.visible_range().contains(&index) {
                delete.set_bounds(bounds_rect(layout.command_delete(index)));
            }
            nodes.push((delete_id, delete));

            let mut group = Node::new(Role::Group);
            group.set_label(command.name.clone());
            if layout.visible_range().contains(&index) {
                group.set_bounds(bounds_rect(layout.command_row(index)));
            }
            group.set_children([run_id, delete_id]);
            nodes.push((group_id, group));
            command_groups.push(group_id);
        }

        let mut list = Node::new(Role::List);
        list.set_label(format!("{} custom commands", info.commands.len()));
        if layout.scroll_offset > 0 {
            list.add_action(Action::ScrollUp);
        }
        if layout.visible_count > 0 && layout.visible_range().end < info.commands.len() {
            list.add_action(Action::ScrollDown);
        }
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel.x,
            layout.list_start_y,
            layout.panel.width,
            (layout.panel.height - layout.list_start_y).max(0.0),
        ));
        list.set_children(command_groups);
        nodes.push((COMMANDS_LIST, list));
        dialog_children.push(COMMANDS_LIST);

        if info.commands.is_empty() && info.edit_mode == EditMode::Browse {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No saved commands");
            empty.set_live(Live::Polite);
            nodes.push((COMMANDS_EMPTY, empty));
            dialog_children.push(COMMANDS_EMPTY);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Custom commands");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((COMMANDS_DIALOG, dialog));

        let default_focus = match info.edit_mode {
            EditMode::EditingName => COMMANDS_NAME,
            EditMode::EditingCommand => COMMANDS_COMMAND,
            EditMode::Browse => match self.commands_panel.browse_focus(&info.commands) {
                Some(CommandControl::Close) => COMMANDS_CLOSE,
                Some(CommandControl::Run(index)) => {
                    self.accessibility.commands.nodes
                        [&CommandsSemanticTarget::Run(keys[index].clone())]
                }
                Some(CommandControl::Delete(index)) => {
                    self.accessibility.commands.nodes
                        [&CommandsSemanticTarget::Delete(keys[index].clone())]
                }
                _ => COMMANDS_NEW,
            },
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                accessibility.is_commands_node(node)
            });
        ModalSnapshot {
            dialog: COMMANDS_DIALOG,
            nodes,
            focus,
        }
    }

    fn agents_accessibility_snapshot(&mut self) -> ModalSnapshot {
        use crate::ui::agents_panel::AgentsControl;

        let info = self.agents_panel_info();
        let layout = self.agents_panel_layout();
        let shown: HashSet<_> = info.rows.iter().map(|row| row.identity).collect();
        self.accessibility
            .agents
            .retain(|target| shown.contains(target));

        let mut nodes = Vec::with_capacity(info.rows.len() + 12);

        let mut close = Node::new(Role::Button);
        close.set_label("Close agents");
        close.add_action(Action::Click);
        close.add_action(Action::Focus);
        close.set_bounds(bounds_rect(layout.close_button));
        nodes.push((AGENTS_CLOSE, close));
        let mut dialog_children = vec![AGENTS_CLOSE];

        let mut row_ids = Vec::with_capacity(info.rows.len());
        for (index, row) in info.rows.iter().enumerate() {
            let id = self.accessibility.agents.node(&row.identity);
            let mut button = Node::new(Role::Button);
            button.set_label(format!("{}, {}", row.name, row.place));
            button.set_description(row.summary.clone());
            button.add_action(Action::Click);
            button.add_action(Action::Focus);
            if layout.visible_range().contains(&index) {
                button.set_bounds(bounds_rect(layout.row(index)));
            }
            nodes.push((id, button));
            row_ids.push(id);
        }
        let mut list = Node::new(Role::List);
        list.set_label(format!("{} agents", info.rows.len()));
        if layout.scroll_offset > 0 {
            list.add_action(Action::ScrollUp);
        }
        if layout.visible_count > 0 && layout.visible_range().end < info.rows.len() {
            list.add_action(Action::ScrollDown);
        }
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel.x,
            layout.list_start_y,
            layout.panel.width,
            layout.visible_count as f32 * layout.row_height,
        ));
        list.set_children(row_ids.clone());
        nodes.push((AGENTS_LIST, list));
        dialog_children.push(AGENTS_LIST);

        if info.rows.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No agents in this window");
            empty.set_live(Live::Polite);
            nodes.push((AGENTS_EMPTY, empty));
            dialog_children.push(AGENTS_EMPTY);
        }

        if let (Some(supervisor), Some(section)) = (&info.supervisor, layout.supervisor) {
            let mut toggle = Node::new(Role::Button);
            toggle.set_label(if supervisor.held {
                "Hand back to supervisor"
            } else {
                "Take over from supervisor"
            });
            toggle.add_action(Action::Click);
            toggle.add_action(Action::Focus);
            toggle.set_bounds(bounds_rect(section.button));
            nodes.push((AGENTS_SUPERVISOR_TOGGLE, toggle));

            let mut entry_ids = Vec::with_capacity(section.entry_count);
            for (index, entry) in supervisor
                .entries
                .iter()
                .take(section.entry_count)
                .enumerate()
            {
                let id = NodeId(AGENTS_ACTIVITY_BASE + index as u64);
                let mut item = Node::new(Role::ListItem);
                item.set_label(entry.clone());
                item.set_bounds(bounds_rect(section.entry(index)));
                nodes.push((id, item));
                entry_ids.push(id);
            }
            let mut activity = Node::new(Role::List);
            activity.set_label("Supervisor activity");
            activity.set_children(entry_ids);
            nodes.push((AGENTS_ACTIVITY, activity));

            let mut group = Node::new(Role::Group);
            group.set_label(format!("Supervisor, {}", supervisor.mode));
            group.set_bounds(bounds_rect(section.section));
            group.set_children([AGENTS_SUPERVISOR_TOGGLE, AGENTS_ACTIVITY]);
            nodes.push((AGENTS_SUPERVISOR, group));
            dialog_children.push(AGENTS_SUPERVISOR);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Agents");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((AGENTS_DIALOG, dialog));

        let default_focus = match info.focus {
            Some(AgentsControl::Agent(index)) => {
                row_ids.get(index).copied().unwrap_or(AGENTS_CLOSE)
            }
            Some(AgentsControl::Supervisor) if info.supervisor.is_some() => {
                AGENTS_SUPERVISOR_TOGGLE
            }
            _ => AGENTS_CLOSE,
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                accessibility.is_agents_node(node)
            });
        ModalSnapshot {
            dialog: AGENTS_DIALOG,
            nodes,
            focus,
        }
    }

    fn session_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let mut info = self.session_panel.build_info();
        let mut layout = self.session_accessibility_layout(&info);
        self.session_panel.set_max_visible(layout.rows_that_fit);
        info = self.session_panel.build_info();
        layout = self.session_accessibility_layout(&info);

        let keys = info
            .sessions
            .iter()
            .map(|entry| SessionSemanticKey {
                id: entry.id.clone(),
                workspaces: info.workspaces,
            })
            .collect::<Vec<_>>();
        let retained = keys
            .iter()
            .flat_map(|key| {
                let mut targets = vec![
                    SessionSemanticTarget::Group(key.clone()),
                    SessionSemanticTarget::Load(key.clone()),
                    SessionSemanticTarget::Delete(key.clone()),
                ];
                if info.workspaces {
                    targets.push(SessionSemanticTarget::Rename(key.clone()));
                }
                targets
            })
            .collect::<HashSet<_>>();
        self.accessibility
            .session
            .retain(|target| retained.contains(target));

        let mut nodes = Vec::with_capacity(layout.visible_count * 4 + 10);

        let mut close = Node::new(Role::Button);
        close.set_label(if info.workspaces {
            "Close workspaces"
        } else {
            "Close tab snapshots"
        });
        close.add_action(Action::Click);
        close.add_action(Action::Focus);
        close.set_bounds(bounds_rect(layout.close_button));
        nodes.push((SESSION_CLOSE, close));

        let mut dialog_children = vec![SESSION_CLOSE];
        if let Some(back) = layout.back_button {
            let mut node = Node::new(Role::Button);
            node.set_label("Back to workspaces");
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.set_bounds(bounds_rect(back));
            nodes.push((SESSION_BACK, node));
            dialog_children.push(SESSION_BACK);
        }
        if info.snapshot_count > 0
            && let Some(link) = layout.snapshots_link
        {
            let mut node = Node::new(Role::Button);
            node.set_label(format!("Tab snapshots ({})", info.snapshot_count));
            node.set_description("Show snapshots saved when tabs closed");
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.set_bounds(bounds_rect(link));
            nodes.push((SESSION_SNAPSHOTS, node));
            dialog_children.push(SESSION_SNAPSHOTS);
        }
        if let (Some(value), Some(rename)) = (&info.rename_text, layout.rename) {
            let mut input = Node::new(Role::TextInput);
            input.set_label("Workspace name");
            set_text_value(
                &mut input,
                SESSION_RENAME_INPUT,
                value,
                Some(info.rename_cursor),
            );
            input.add_action(Action::Focus);
            input.add_action(Action::SetValue);
            input.set_bounds(bounds_rect(rename.input));
            nodes.push((SESSION_RENAME_INPUT, input));

            let mut save = Node::new(Role::Button);
            save.set_label("Save workspace name");
            save.add_action(Action::Click);
            save.add_action(Action::Focus);
            save.set_bounds(bounds_rect(rename.save_button));
            nodes.push((SESSION_RENAME_SAVE, save));

            let mut cancel = Node::new(Role::Button);
            cancel.set_label("Cancel workspace rename");
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
            cancel.set_bounds(bounds_rect(rename.cancel_button));
            nodes.push((SESSION_RENAME_CANCEL, cancel));
            dialog_children.extend([
                SESSION_RENAME_INPUT,
                SESSION_RENAME_SAVE,
                SESSION_RENAME_CANCEL,
            ]);
        }

        let end = (info.scroll_offset + layout.visible_count).min(info.sessions.len());
        let rename_active = info.rename_text.is_some();
        let mut group_nodes = Vec::with_capacity(layout.visible_count);
        let mut selected_group = None;
        let mut selected_load = None;
        for (visible_index, absolute_index) in (info.scroll_offset..end).enumerate() {
            let entry = &info.sessions[absolute_index];
            let key = keys[absolute_index].clone();
            let geometry = layout.entry(visible_index);
            let group_id = self
                .accessibility
                .session
                .node(&SessionSemanticTarget::Group(key.clone()));
            let load_id = self
                .accessibility
                .session
                .node(&SessionSemanticTarget::Load(key.clone()));
            let delete_id = self
                .accessibility
                .session
                .node(&SessionSemanticTarget::Delete(key.clone()));
            let mut load = Node::new(Role::Button);
            load.set_label(format!("Open {}", entry.display_name));
            load.set_description(if info.workspaces {
                "Relaunch this workspace's tabs and panes"
            } else {
                "Open this tab snapshot in a new shell"
            });
            if rename_active {
                load.set_disabled();
            } else {
                load.add_action(Action::Click);
                load.add_action(Action::Focus);
            }
            load.set_bounds(bounds_rect(geometry.load_button));
            nodes.push((load_id, load));

            let mut children = vec![load_id];
            if info.workspaces {
                let rename_id = self
                    .accessibility
                    .session
                    .node(&SessionSemanticTarget::Rename(key.clone()));
                let mut rename = Node::new(Role::Button);
                rename.set_label(format!("Rename {}", entry.display_name));
                if rename_active {
                    rename.set_disabled();
                } else {
                    rename.add_action(Action::Click);
                    rename.add_action(Action::Focus);
                }
                if let Some(bounds) = geometry.rename_button {
                    rename.set_bounds(bounds_rect(bounds));
                }
                nodes.push((rename_id, rename));
                children.push(rename_id);
            }

            let mut delete = Node::new(Role::Button);
            delete.set_label(format!("Delete {}", entry.display_name));
            delete.set_description(if info.workspaces {
                "Permanently deletes this saved workspace"
            } else {
                "Permanently deletes this tab snapshot"
            });
            if rename_active {
                delete.set_disabled();
            } else {
                delete.add_action(Action::Click);
                delete.add_action(Action::Focus);
            }
            delete.set_bounds(bounds_rect(geometry.delete_button));
            nodes.push((delete_id, delete));
            children.push(delete_id);

            let mut group = Node::new(Role::Group);
            group.set_label(entry.display_name.clone());
            let description = if entry.subtitle.is_empty() {
                entry.last_modified.clone()
            } else {
                format!("{}; {}", entry.subtitle, entry.last_modified)
            };
            if !description.is_empty() {
                group.set_description(description);
            }
            group.set_bounds(bounds_rect(geometry.row));
            group.set_children(children);
            nodes.push((group_id, group));
            group_nodes.push(group_id);
            if absolute_index == info.selected_index {
                selected_group = Some(group_id);
                selected_load = Some(load_id);
            }
        }

        let mut list = Node::new(Role::List);
        list.set_label(if info.workspaces {
            format!("{} saved workspaces", info.sessions.len())
        } else {
            format!("{} tab snapshots", info.sessions.len())
        });
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel.x,
            layout.list_start_y,
            layout.panel.width,
            (layout.panel.height - layout.list_start_y).max(0.0),
        ));
        if let Some(selected) = selected_group {
            list.set_active_descendant(selected);
        }
        list.set_children(group_nodes);
        nodes.push((SESSION_LIST, list));
        dialog_children.push(SESSION_LIST);

        if info.sessions.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label(if info.workspaces {
                "No saved workspaces"
            } else {
                "No tab snapshots"
            });
            empty.set_live(Live::Polite);
            nodes.push((SESSION_EMPTY, empty));
            dialog_children.push(SESSION_EMPTY);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(if info.workspaces {
            "Workspaces"
        } else {
            "Tab snapshots"
        });
        dialog.set_description(if info.workspaces {
            "Opening relaunches the saved tabs and panes"
        } else {
            "Saved when tabs close. Opening starts a new shell"
        });
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((SESSION_DIALOG, dialog));

        let default_focus = if info.rename_text.is_some() {
            SESSION_RENAME_INPUT
        } else {
            selected_load.unwrap_or(SESSION_CLOSE)
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                accessibility.is_session_node(node)
            });
        ModalSnapshot {
            dialog: SESSION_DIALOG,
            nodes,
            focus,
        }
    }
}

fn command_form_input(
    id: NodeId,
    label: &str,
    placeholder: &str,
    value: &str,
    cursor: Option<usize>,
    bounds: Bounds,
) -> Node {
    let mut input = Node::new(Role::TextInput);
    input.set_label(label);
    input.set_placeholder(placeholder);
    set_text_value(&mut input, id, value, cursor);
    input.add_action(Action::Focus);
    input.set_bounds(bounds_rect(bounds));
    input
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

fn append_terminal_nodes(
    nodes: &mut Vec<(NodeId, Node)>,
    terminal_id: NodeId,
    pane_id: PaneId,
    terminal: &termeric_core::Terminal,
    focused: bool,
    broadcast_target: Option<crate::renderer::BroadcastTargetState>,
    read_only: bool,
    zoomed: bool,
    bounds: Rect,
    cell_width: f32,
    cell_height: f32,
    filtered_rows: &[i32],
) {
    let grid = terminal.presentation_grid();
    let mut line_ids = Vec::with_capacity(grid.rows());
    let mut lines = Vec::with_capacity(grid.rows());
    let row_count = if filtered_rows.is_empty() {
        grid.rows()
    } else {
        filtered_rows.len()
    };
    for row in 0..row_count {
        let line_id = line_node(pane_id, row);
        let line = if filtered_rows.is_empty() {
            accessible_line(grid, row)
        } else {
            accessible_source_line(grid, filtered_rows[row])
        };
        let mut node = Node::new(Role::TextRun);
        node.set_value(line.value.clone());
        node.set_character_lengths(line.character_lengths.clone());
        node.set_bounds(rect(
            bounds.x0 as f32,
            bounds.y0 as f32 + row as f32 * cell_height,
            bounds.width() as f32,
            cell_height,
        ));
        nodes.push((line_id, node));
        line_ids.push(line_id);
        lines.push(line);
    }

    let mut node = Node::new(Role::Terminal);
    let mut label = format!("Terminal pane {}", pane_id.0 + 1);
    if zoomed {
        label.push_str(", zoomed");
    }
    if read_only {
        label.push_str(", read-only input lock active");
    }
    match broadcast_target {
        Some(crate::renderer::BroadcastTargetState::Armed) => {
            label.push_str(", selected broadcast target, not active")
        }
        Some(crate::renderer::BroadcastTargetState::Active) => {
            label.push_str(", active broadcast target")
        }
        None => {}
    }
    node.set_label(label);
    node.set_description("Visible terminal text; off-screen rows are not exposed");
    node.set_bounds(bounds);
    node.set_children(line_ids);
    node.set_live(Live::Polite);
    node.set_read_only();
    node.add_action(Action::Focus);
    node.add_action(Action::Click);
    if focused && !lines.is_empty() {
        let selection = terminal
            .selection()
            .and_then(|selection| {
                if filtered_rows.is_empty() {
                    accessible_selection(selection, pane_id, &lines, grid.scroll_offset())
                } else {
                    accessible_filtered_selection(selection, pane_id, &lines, filtered_rows)
                }
            })
            .or_else(|| {
                if grid.scroll_offset() != 0 || !filtered_rows.is_empty() {
                    return None;
                }
                let cursor = grid.cursor();
                let row = cursor.row.min(lines.len() - 1);
                let index = lines[row].character_index(cursor.col);
                let position = TextPosition {
                    node: line_node(pane_id, row),
                    character_index: index,
                };
                Some(TextSelection {
                    anchor: position,
                    focus: position,
                })
            });
        if let Some(selection) = selection {
            node.set_text_selection(selection);
        }
    }
    let _ = cell_width; // retained in the signature for future character geometry.
    nodes.push((terminal_id, node));
}

#[derive(Debug, PartialEq, Eq)]
struct AccessibleLine {
    value: String,
    character_lengths: Vec<u8>,
    column_to_character: Vec<usize>,
}

impl AccessibleLine {
    fn character_index(&self, column: usize) -> usize {
        self.column_to_character
            .get(column)
            .copied()
            .unwrap_or(self.character_lengths.len())
            .min(self.character_lengths.len())
    }
}

fn accessible_line(grid: &Grid, row: usize) -> AccessibleLine {
    let source =
        (row as i32).saturating_sub(i32::try_from(grid.scroll_offset()).unwrap_or(i32::MAX));
    accessible_source_line(grid, source)
}

fn accessible_source_line(grid: &Grid, row: i32) -> AccessibleLine {
    let mut value = String::new();
    let mut character_lengths = Vec::new();
    let mut column_to_character = Vec::with_capacity(grid.cols() + 1);
    let source = grid.row_at_offset(row);
    for col in 0..grid.cols() {
        column_to_character.push(character_lengths.len());
        if let Some(cell) = source.as_ref().and_then(|row| row.get(col)) {
            append_accessible_cell(&cell, &mut value, &mut character_lengths);
        } else {
            value.push(' ');
            character_lengths.push(1);
        }
    }
    column_to_character.push(character_lengths.len());
    while value.ends_with(' ') {
        value.pop();
        character_lengths.pop();
    }
    AccessibleLine {
        value,
        character_lengths,
        column_to_character,
    }
}

fn append_accessible_cell(cell: &Cell, value: &mut String, lengths: &mut Vec<u8>) {
    if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER)
        || (cell.multicell.is_present() && !cell.multicell.is_leader())
    {
        return;
    }
    if cell.attrs.contains(CellAttrs::HIDDEN) || cell.content.is_empty() {
        value.push(' ');
        lengths.push(1);
        return;
    }
    let mut content = String::new();
    cell.content.push_to(&mut content);
    for character in content.chars() {
        value.push(character);
        lengths.push(character.len_utf8() as u8);
    }
}

fn accessible_filtered_selection(
    selection: &Selection,
    pane_id: PaneId,
    lines: &[AccessibleLine],
    rows: &[i32],
) -> Option<TextSelection> {
    let (start, end) = selection.bounds();
    let first = rows.partition_point(|row| *row < start.row);
    let after_last = rows.partition_point(|row| *row <= end.row);
    if first >= after_last {
        return None;
    }
    let last = after_last - 1;
    let start_col = if rows[first] == start.row {
        start.col
    } else {
        0
    };
    let end_col = if rows[last] == end.row {
        end.col
    } else {
        lines.get(last)?.column_to_character.len().saturating_sub(2)
    };
    let start_point = termeric_core::Point::new(first as i32, start_col);
    let end_point = termeric_core::Point::new(last as i32, end_col);
    let mut projected = selection.clone();
    let forward = selection.start <= selection.end;
    projected.start = if forward { start_point } else { end_point };
    projected.end = if forward { end_point } else { start_point };
    accessible_selection(&projected, pane_id, lines, 0)
}

fn accessible_selection(
    selection: &Selection,
    pane_id: PaneId,
    lines: &[AccessibleLine],
    scroll_offset: usize,
) -> Option<TextSelection> {
    let last_row = i64::try_from(lines.len().checked_sub(1)?).ok()?;
    let offset = i64::try_from(scroll_offset).unwrap_or(i64::MAX);
    let (first, last) = selection.bounds();
    let first_visible = i64::from(first.row).saturating_add(offset);
    let last_visible = i64::from(last.row).saturating_add(offset);
    if last_visible < 0 || first_visible > last_row {
        return None;
    }
    let position = |point: termeric_core::Point, after_cell: bool| {
        let row = i64::from(point.row)
            .saturating_add(offset)
            .clamp(0, last_row) as usize;
        Some(TextPosition {
            node: line_node(pane_id, row),
            character_index: lines[row]
                .character_index(point.col.saturating_add(usize::from(after_cell))),
        })
    };
    let forward = selection.start <= selection.end;
    Some(TextSelection {
        anchor: position(selection.start, !forward)?,
        focus: position(selection.end, forward)?,
    })
}

fn workspace_menu_node(row: usize) -> NodeId {
    NodeId(WORKSPACE_MENU_ENTRY_BASE + row as u64)
}

/// Menu rows are bounded, so the range never reaches the next base.
fn workspace_menu_row(node: NodeId) -> Option<usize> {
    node.0
        .checked_sub(WORKSPACE_MENU_ENTRY_BASE)
        .filter(|row| *row < 64)
        .map(|row| row as usize)
}

/// The fixed nodes of a palette-style modal's dialog.
struct PaletteNodeIds {
    dialog: NodeId,
    input: NodeId,
    list: NodeId,
    empty: NodeId,
}

fn palette_modal_node_ids(modal: PaletteModal) -> PaletteNodeIds {
    match modal {
        PaletteModal::Commands => PaletteNodeIds {
            dialog: PALETTE_DIALOG,
            input: PALETTE_INPUT,
            list: PALETTE_LIST,
            empty: PALETTE_EMPTY,
        },
        PaletteModal::Domains => PaletteNodeIds {
            dialog: DOMAIN_DIALOG,
            input: DOMAIN_INPUT,
            list: DOMAIN_LIST,
            empty: DOMAIN_EMPTY,
        },
        PaletteModal::Unicode => PaletteNodeIds {
            dialog: UNICODE_DIALOG,
            input: UNICODE_INPUT,
            list: UNICODE_LIST,
            empty: UNICODE_EMPTY,
        },
    }
}

/// What assistive technology hears a palette-style modal called.
struct PaletteWording {
    dialog: String,
    description: Option<&'static str>,
    input: &'static str,
    list: String,
    empty: String,
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
