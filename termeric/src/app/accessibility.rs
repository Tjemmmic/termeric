//! Incremental AccessKit tree for native screen readers.

use std::collections::{HashMap, HashSet};

use accesskit::{
    Action, ActionData, ActionRequest, CustomAction, Live, Node, NodeId, Rect, Role, TextPosition,
    TextSelection, Toggled, Tree, TreeId, TreeUpdate,
};
use accesskit_winit::WindowEvent;
use termeric_core::{Cell, CellAttrs, CellFlags, Grid, Selection};

use super::*;

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
const SHORTCUT_CLEAR_ACTION: i32 = 1;
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SettingsSemanticTarget {
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
struct CustomCommandSemanticKey {
    name: String,
    command: String,
    alias: Option<String>,
    occurrence: usize,
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

pub(super) struct AccessibilityState {
    active: bool,
    initialized: bool,
    previous: HashMap<NodeId, Node>,
    focus_override: Option<NodeId>,
    keyboard_focus: ChromeFocus,
    palette_nodes: HashMap<String, NodeId>,
    palette_commands: HashMap<NodeId, String>,
    next_palette_node: u64,
    settings_nodes: HashMap<SettingsSemanticTarget, NodeId>,
    settings_targets: HashMap<NodeId, SettingsSemanticTarget>,
    next_settings_node: u64,
    commands_nodes: HashMap<CommandsSemanticTarget, NodeId>,
    commands_targets: HashMap<NodeId, CommandsSemanticTarget>,
    next_commands_node: u64,
    session_nodes: HashMap<SessionSemanticTarget, NodeId>,
    session_targets: HashMap<NodeId, SessionSemanticTarget>,
    next_session_node: u64,
    shortcuts_nodes: HashMap<ShortcutsSemanticTarget, NodeId>,
    shortcuts_targets: HashMap<NodeId, ShortcutsSemanticTarget>,
    next_shortcuts_node: u64,
    domain_nodes: HashMap<String, NodeId>,
    domain_commands: HashMap<NodeId, String>,
    next_domain_node: u64,
    unicode_nodes: HashMap<String, NodeId>,
    unicode_commands: HashMap<NodeId, String>,
    next_unicode_node: u64,
    guardrail_nodes: HashMap<GuardrailSemanticTarget, NodeId>,
    guardrail_targets: HashMap<NodeId, GuardrailSemanticTarget>,
    next_guardrail_node: u64,
    info_nodes: HashMap<InfoSemanticTarget, NodeId>,
    info_targets: HashMap<NodeId, InfoSemanticTarget>,
    next_info_node: u64,
    diff_nodes: HashMap<DiffSemanticTarget, NodeId>,
    diff_targets: HashMap<NodeId, DiffSemanticTarget>,
    next_diff_node: u64,
    context_nodes: HashMap<ContextSemanticTarget, NodeId>,
    context_targets: HashMap<NodeId, ContextSemanticTarget>,
    next_context_node: u64,
    harvest_nodes: HashMap<HarvestSemanticTarget, NodeId>,
    harvest_targets: HashMap<NodeId, HarvestSemanticTarget>,
    next_harvest_node: u64,
}

impl Default for AccessibilityState {
    fn default() -> Self {
        Self {
            active: false,
            initialized: false,
            previous: HashMap::new(),
            focus_override: None,
            keyboard_focus: ChromeFocus::default(),
            palette_nodes: HashMap::new(),
            palette_commands: HashMap::new(),
            next_palette_node: PALETTE_ENTRY_BASE,
            settings_nodes: HashMap::new(),
            settings_targets: HashMap::new(),
            next_settings_node: SETTINGS_ENTRY_BASE,
            commands_nodes: HashMap::new(),
            commands_targets: HashMap::new(),
            next_commands_node: COMMANDS_ENTRY_BASE,
            session_nodes: HashMap::new(),
            session_targets: HashMap::new(),
            next_session_node: SESSION_ENTRY_BASE,
            shortcuts_nodes: HashMap::new(),
            shortcuts_targets: HashMap::new(),
            next_shortcuts_node: SHORTCUTS_ENTRY_BASE,
            domain_nodes: HashMap::new(),
            domain_commands: HashMap::new(),
            next_domain_node: DOMAIN_ENTRY_BASE,
            unicode_nodes: HashMap::new(),
            unicode_commands: HashMap::new(),
            next_unicode_node: UNICODE_ENTRY_BASE,
            guardrail_nodes: HashMap::new(),
            guardrail_targets: HashMap::new(),
            next_guardrail_node: GUARDRAIL_ACTION_BASE,
            info_nodes: HashMap::new(),
            info_targets: HashMap::new(),
            next_info_node: INFO_ACTION_BASE,
            diff_nodes: HashMap::new(),
            diff_targets: HashMap::new(),
            next_diff_node: DIFF_ACTION_BASE,
            context_nodes: HashMap::new(),
            context_targets: HashMap::new(),
            next_context_node: CONTEXT_ACTION_BASE,
            harvest_nodes: HashMap::new(),
            harvest_targets: HashMap::new(),
            next_harvest_node: HARVEST_ACTION_BASE,
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

impl AccessibilityState {
    fn activate(&mut self) {
        self.active = true;
    }

    fn deactivate(&mut self) {
        self.active = false;
        self.initialized = false;
        self.previous.clear();
        self.focus_override = None;
    }

    fn palette_node(&mut self, command: &str) -> NodeId {
        if let Some(node) = self.palette_nodes.get(command) {
            return *node;
        }
        let node = NodeId(self.next_palette_node);
        self.next_palette_node = self.next_palette_node.saturating_add(1);
        self.palette_nodes.insert(command.to_string(), node);
        self.palette_commands.insert(node, command.to_string());
        node
    }

    fn retain_palette_commands(&mut self, commands: &HashSet<&str>) {
        self.palette_nodes
            .retain(|command, _| commands.contains(command.as_str()));
        self.palette_commands
            .retain(|_, command| commands.contains(command.as_str()));
    }

    fn settings_node(&mut self, target: SettingsSemanticTarget) -> NodeId {
        if let Some(node) = self.settings_nodes.get(&target) {
            return *node;
        }
        let node = NodeId(self.next_settings_node);
        self.next_settings_node = self.next_settings_node.saturating_add(1);
        self.settings_nodes.insert(target.clone(), node);
        self.settings_targets.insert(node, target);
        node
    }

    fn is_settings_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            SETTINGS_DIALOG | SETTINGS_INPUT | SETTINGS_LIST | SETTINGS_EMPTY | SETTINGS_DROPDOWN
        ) || self.settings_targets.contains_key(&node)
    }

    fn commands_node(&mut self, target: CommandsSemanticTarget) -> NodeId {
        if let Some(node) = self.commands_nodes.get(&target) {
            return *node;
        }
        let node = NodeId(self.next_commands_node);
        self.next_commands_node = self.next_commands_node.saturating_add(1);
        self.commands_nodes.insert(target.clone(), node);
        self.commands_targets.insert(node, target);
        node
    }

    fn retain_commands(&mut self, targets: &HashSet<CommandsSemanticTarget>) {
        self.commands_nodes
            .retain(|target, _| targets.contains(target));
        self.commands_targets
            .retain(|_, target| targets.contains(target));
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
        ) || self.commands_targets.contains_key(&node)
    }

    fn session_node(&mut self, target: SessionSemanticTarget) -> NodeId {
        if let Some(node) = self.session_nodes.get(&target) {
            return *node;
        }
        let node = NodeId(self.next_session_node);
        self.next_session_node = self.next_session_node.saturating_add(1);
        self.session_nodes.insert(target.clone(), node);
        self.session_targets.insert(node, target);
        node
    }

    fn retain_sessions(&mut self, targets: &HashSet<SessionSemanticTarget>) {
        self.session_nodes
            .retain(|target, _| targets.contains(target));
        self.session_targets
            .retain(|_, target| targets.contains(target));
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
        ) || self.session_targets.contains_key(&node)
    }

    fn shortcuts_node(&mut self, target: ShortcutsSemanticTarget) -> NodeId {
        if let Some(node) = self.shortcuts_nodes.get(&target) {
            return *node;
        }
        let node = NodeId(self.next_shortcuts_node);
        self.next_shortcuts_node = self.next_shortcuts_node.saturating_add(1);
        self.shortcuts_nodes.insert(target, node);
        self.shortcuts_targets.insert(node, target);
        node
    }

    fn is_shortcuts_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            SHORTCUTS_DIALOG
                | SHORTCUTS_INPUT
                | SHORTCUTS_LIST
                | SHORTCUTS_EMPTY
                | SHORTCUTS_STATUS
        ) || self.shortcuts_targets.contains_key(&node)
    }

    fn domain_node(&mut self, command: &str) -> NodeId {
        if let Some(node) = self.domain_nodes.get(command) {
            return *node;
        }
        let node = NodeId(self.next_domain_node);
        self.next_domain_node = self.next_domain_node.saturating_add(1);
        self.domain_nodes.insert(command.to_string(), node);
        self.domain_commands.insert(node, command.to_string());
        node
    }

    fn retain_domain_commands(&mut self, commands: &HashSet<&str>) {
        self.domain_nodes
            .retain(|command, _| commands.contains(command.as_str()));
        self.domain_commands
            .retain(|_, command| commands.contains(command.as_str()));
    }

    fn is_domain_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            DOMAIN_DIALOG | DOMAIN_INPUT | DOMAIN_LIST | DOMAIN_EMPTY
        ) || self.domain_commands.contains_key(&node)
    }

    fn unicode_node(&mut self, command: &str) -> NodeId {
        if let Some(node) = self.unicode_nodes.get(command) {
            return *node;
        }
        let node = NodeId(self.next_unicode_node);
        self.next_unicode_node = self.next_unicode_node.saturating_add(1);
        self.unicode_nodes.insert(command.to_string(), node);
        self.unicode_commands.insert(node, command.to_string());
        node
    }

    fn retain_unicode_commands(&mut self, commands: &HashSet<&str>) {
        self.unicode_nodes
            .retain(|command, _| commands.contains(command.as_str()));
        self.unicode_commands
            .retain(|_, command| commands.contains(command.as_str()));
    }

    fn is_unicode_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            UNICODE_DIALOG | UNICODE_INPUT | UNICODE_LIST | UNICODE_EMPTY
        ) || self.unicode_commands.contains_key(&node)
    }

    fn guardrail_node(&mut self, target: GuardrailSemanticTarget) -> NodeId {
        if let Some(node) = self.guardrail_nodes.get(&target) {
            return *node;
        }
        let node = NodeId(self.next_guardrail_node);
        self.next_guardrail_node = self.next_guardrail_node.saturating_add(1);
        self.guardrail_nodes.insert(target, node);
        self.guardrail_targets.insert(node, target);
        node
    }

    fn retain_guardrail_targets(&mut self, targets: &HashSet<GuardrailSemanticTarget>) {
        self.guardrail_nodes
            .retain(|target, _| targets.contains(target));
        self.guardrail_targets
            .retain(|_, target| targets.contains(target));
    }

    fn is_guardrail_node(&self, node: NodeId) -> bool {
        matches!(
            node,
            GUARDRAIL_DIALOG | GUARDRAIL_COMMAND | GUARDRAIL_REASON | GUARDRAIL_CONTEXT
        ) || self.guardrail_targets.contains_key(&node)
    }

    fn info_node(&mut self, target: InfoSemanticTarget) -> NodeId {
        if let Some(node) = self.info_nodes.get(&target) {
            return *node;
        }
        let node = NodeId(self.next_info_node);
        self.next_info_node = self.next_info_node.saturating_add(1);
        self.info_nodes.insert(target.clone(), node);
        self.info_targets.insert(node, target);
        node
    }

    fn retain_info_targets(&mut self, targets: &HashSet<InfoSemanticTarget>) {
        self.info_nodes.retain(|target, _| targets.contains(target));
        self.info_targets
            .retain(|_, target| targets.contains(target));
    }

    fn is_info_node(&self, node: NodeId) -> bool {
        matches!(node, INFO_DIALOG | INFO_TITLE | INFO_STATUS)
            || self.info_targets.contains_key(&node)
    }

    fn diff_node(&mut self, target: DiffSemanticTarget) -> NodeId {
        if let Some(node) = self.diff_nodes.get(&target) {
            return *node;
        }
        let node = NodeId(self.next_diff_node);
        self.next_diff_node = self.next_diff_node.saturating_add(1);
        self.diff_nodes.insert(target, node);
        self.diff_targets.insert(node, target);
        node
    }

    fn retain_diff_targets(&mut self, targets: &HashSet<DiffSemanticTarget>) {
        self.diff_nodes.retain(|target, _| targets.contains(target));
        self.diff_targets
            .retain(|_, target| targets.contains(target));
    }

    fn is_diff_node(&self, node: NodeId) -> bool {
        matches!(node, DIFF_DIALOG | DIFF_TITLE | DIFF_STATUS)
            || self.diff_targets.contains_key(&node)
    }

    fn context_node(&mut self, target: ContextSemanticTarget) -> NodeId {
        if let Some(node) = self.context_nodes.get(&target) {
            return *node;
        }
        let node = NodeId(self.next_context_node);
        self.next_context_node = self.next_context_node.saturating_add(1);
        self.context_nodes.insert(target, node);
        self.context_targets.insert(node, target);
        node
    }

    fn retain_context_targets(&mut self, targets: &HashSet<ContextSemanticTarget>) {
        self.context_nodes
            .retain(|target, _| targets.contains(target));
        self.context_targets
            .retain(|_, target| targets.contains(target));
    }

    fn is_context_node(&self, node: NodeId) -> bool {
        matches!(node, CONTEXT_DIALOG | CONTEXT_TITLE | CONTEXT_STATUS)
            || self.context_targets.contains_key(&node)
    }

    fn harvest_node(&mut self, target: HarvestSemanticTarget) -> NodeId {
        if let Some(node) = self.harvest_nodes.get(&target) {
            return *node;
        }
        let node = NodeId(self.next_harvest_node);
        self.next_harvest_node = self.next_harvest_node.saturating_add(1);
        self.harvest_nodes.insert(target, node);
        self.harvest_targets.insert(node, target);
        node
    }

    fn retain_harvest_targets(&mut self, targets: &HashSet<HarvestSemanticTarget>) {
        self.harvest_nodes
            .retain(|target, _| targets.contains(target));
        self.harvest_targets
            .retain(|_, target| targets.contains(target));
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
        ) || self.harvest_targets.contains_key(&node)
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

    pub(super) fn handle_chrome_keyboard(&mut self, event: &winit::event::KeyEvent) -> bool {
        use winit::event::ElementState;
        use winit::keyboard::{Key, NamedKey};

        let is_f6 = matches!(event.logical_key, Key::Named(NamedKey::F6));
        if event.state != ElementState::Pressed {
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

        match &event.logical_key {
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
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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
            ChromeFocus::Workspaces => {
                self.session_dropdown_open = !self.session_dropdown_open;
                if self.session_dropdown_open {
                    self.session_dropdown_items = crate::session::list_sessions_with_titles();
                }
            }
            ChromeFocus::Commands => self.commands_panel.toggle(),
            ChromeFocus::Settings => self.toggle_settings_surface(),
        }
        if !matches!(self.accessibility.keyboard_focus, ChromeFocus::Tab(_)) {
            self.set_chrome_focus(ChromeFocus::Terminal);
        } else if let Some(window) = &self.window {
            window.request_redraw();
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
        let snapshot = self.accessibility_snapshot();
        let snapshot = self.with_toast_accessibility(snapshot);
        let update = self.accessibility.update(snapshot);
        if let Some(adapter) = &mut self.accessibility_adapter {
            adapter.update_if_active(move || update);
        }
    }

    fn handle_accessibility_action(&mut self, request: ActionRequest) {
        if self.guardrail.is_active() {
            self.handle_guardrail_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.info_overlay.is_active() {
            self.handle_info_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.diff_review.is_active() {
            self.handle_diff_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.context_picker.is_active() {
            self.handle_context_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self
            .harvest
            .as_ref()
            .is_some_and(|harvest| harvest.sprout_menu.is_some() || harvest.cellar.is_expanded())
        {
            self.handle_harvest_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.domain_picker.is_active() {
            self.handle_domain_picker_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.unicode_picker.is_active() {
            self.handle_unicode_picker_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.command_palette.is_active() {
            self.handle_command_palette_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.shortcuts_panel.is_active() {
            self.handle_shortcuts_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.settings_panel.is_active() {
            self.handle_settings_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.commands_panel.is_open() {
            self.handle_commands_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.session_panel.is_open() {
            self.handle_session_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.search_is_visible() && is_search_node(request.target_node) {
            self.handle_search_accessibility_action(request);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if !matches!(request.action, Action::Click | Action::Focus) {
            return;
        }
        self.accessibility.focus_override = Some(request.target_node);
        let id = request.target_node.0;
        if (TAB_BASE..PANE_BASE).contains(&id) {
            let offset = id - TAB_BASE;
            let index = (offset / 2) as usize;
            self.accessibility.keyboard_focus = if offset.is_multiple_of(2) {
                ChromeFocus::Tab(index)
            } else {
                ChromeFocus::CloseTab(index)
            };
            if offset.is_multiple_of(2) {
                let from = self
                    .tabs
                    .as_ref()
                    .map(TabManager::active_index)
                    .unwrap_or(0);
                if let Some(tabs) = &mut self.tabs {
                    tabs.switch_to(index);
                }
                if from != index {
                    self.refresh_focused_terminal_surfaces();
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to: index });
                }
            } else if request.action == Action::Click {
                self.handle_action(crate::input::Action::SwitchToTab(index));
                self.handle_action(crate::input::Action::CloseTab);
                self.accessibility.focus_override = None;
            }
        } else if (PANE_BASE..LINE_BASE).contains(&id) {
            self.accessibility.keyboard_focus = ChromeFocus::Terminal;
            let pane_id = PaneId((id - PANE_BASE) as u32);
            if let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut)
                && let Some(panes) = &mut tab.pane_manager
                && panes.get_pane(pane_id).is_some()
            {
                panes.focus(pane_id);
                self.refresh_focused_terminal_surfaces();
                self.accessibility.focus_override = None;
            }
        } else {
            let focus = match request.target_node {
                NEW_TAB => ChromeFocus::NewTab,
                WORKSPACES => ChromeFocus::Workspaces,
                COMMANDS => ChromeFocus::Commands,
                SETTINGS => ChromeFocus::Settings,
                PREVIOUS_TAB => ChromeFocus::PreviousTab,
                NEXT_TAB => ChromeFocus::NextTab,
                _ => self.accessibility.keyboard_focus,
            };
            self.accessibility.keyboard_focus = focus;
            if let Some(renderer) = &mut self.renderer {
                renderer.set_tab_bar_keyboard_focus(focus.render_target());
            }
            if request.action != Action::Click {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
            match request.target_node {
                NEW_TAB => {
                    self.handle_action(crate::input::Action::NewTab);
                    self.accessibility.focus_override = None;
                    self.accessibility.keyboard_focus = ChromeFocus::Terminal;
                }
                WORKSPACES => {
                    self.session_dropdown_open = !self.session_dropdown_open;
                    if self.session_dropdown_open {
                        self.session_dropdown_items = crate::session::list_sessions_with_titles();
                    }
                }
                COMMANDS => self.commands_panel.toggle(),
                SETTINGS => self.toggle_settings_surface(),
                PREVIOUS_TAB => {
                    self.handle_action(crate::input::Action::PrevTab);
                    self.accessibility.focus_override = None;
                    self.accessibility.keyboard_focus = ChromeFocus::Terminal;
                }
                NEXT_TAB => {
                    self.handle_action(crate::input::Action::NextTab);
                    self.accessibility.focus_override = None;
                    self.accessibility.keyboard_focus = ChromeFocus::Terminal;
                }
                _ => {}
            }
            if self.accessibility.keyboard_focus == ChromeFocus::Terminal
                && let Some(renderer) = &mut self.renderer
            {
                renderer.set_tab_bar_keyboard_focus(None);
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn handle_search_accessibility_action(&mut self, request: ActionRequest) {
        if request.target_node == SEARCH_INPUT {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(SEARCH_INPUT),
                Action::SetValue if self.search.is_active() => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self
                        .with_search_and_active_grid(|search, grid| search.set_query(value, grid))
                        .unwrap_or(false)
                    {
                        self.search_cursor_pos = value.len();
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

    fn handle_harvest_accessibility_action(&mut self, request: ActionRequest) {
        if let Some((request_id, action)) = self
            .accessibility
            .harvest_targets
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
            .harvest_targets
            .get(&request.target_node)
            .and_then(|target| match target {
                HarvestSemanticTarget::CellarEntry(id) => Some(*id),
                HarvestSemanticTarget::SproutAction { .. } => None,
            })
        else {
            return;
        };
        let visible = self
            .cellar_accessibility_layout()
            .rows
            .iter()
            .any(|(entry_id, _)| *entry_id == id);
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

    fn handle_command_palette_accessibility_action(&mut self, request: ActionRequest) {
        if request.target_node == PALETTE_INPUT {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(PALETTE_INPUT),
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.command_palette.accessibility_set_input(value) {
                        self.accessibility.focus_override = Some(PALETTE_INPUT);
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
            .palette_commands
            .get(&request.target_node)
            .cloned()
        else {
            // The palette is modal. Never let a stale request reach controls
            // hidden behind it.
            return;
        };
        let info = self.command_palette.build_info();
        let layout = self.command_palette_accessibility_layout(&info);
        if !info
            .entries
            .iter()
            .take(layout.visible_count)
            .any(|entry| entry.id == command)
        {
            return;
        }
        if request.action == Action::Focus {
            self.command_palette.select_command(&command);
            self.accessibility.focus_override = Some(PALETTE_INPUT);
        } else if let Some(result) = self.command_palette.execute_command(&command) {
            self.accessibility.focus_override = None;
            self.handle_palette_result(result);
        }
    }

    fn handle_domain_picker_accessibility_action(&mut self, request: ActionRequest) {
        if request.target_node == DOMAIN_INPUT {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(DOMAIN_INPUT),
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.domain_picker.accessibility_set_filter(value) {
                        self.accessibility.focus_override = Some(DOMAIN_INPUT);
                    }
                }
                _ => {}
            }
            return;
        }
        let Some(command) = self
            .accessibility
            .domain_commands
            .get(&request.target_node)
            .cloned()
        else {
            // The picker is modal; requests for the hidden application and
            // retired staged results never reach the normal action path.
            return;
        };
        let info = self.domain_picker.build_info();
        let layout = self.domain_picker_accessibility_layout(&info);
        if !info
            .entries
            .iter()
            .take(layout.visible_count)
            .any(|entry| entry.id == command)
        {
            return;
        }
        match request.action {
            Action::Focus if self.domain_picker.is_actionable(&command) => {
                if self.domain_picker.select_command(&command) {
                    self.accessibility.focus_override = Some(DOMAIN_INPUT);
                }
            }
            Action::Click if self.domain_picker.is_actionable(&command) => {
                if let Some(action) = self.domain_picker.execute_command(&command) {
                    self.accessibility.focus_override = Some(DOMAIN_INPUT);
                    self.handle_domain_picker_command(action);
                }
            }
            _ => {}
        }
    }

    fn handle_unicode_picker_accessibility_action(&mut self, request: ActionRequest) {
        if request.target_node == UNICODE_INPUT {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(UNICODE_INPUT),
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.unicode_picker.accessibility_set_filter(value) {
                        self.accessibility.focus_override = Some(UNICODE_INPUT);
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
            .unicode_commands
            .get(&request.target_node)
            .cloned()
        else {
            // The picker is modal. Hidden application controls and retired
            // dynamic code-point results never receive stale actions.
            return;
        };
        let info = self.unicode_picker.build_info();
        let layout = self.unicode_picker_accessibility_layout(&info);
        if !info
            .entries
            .iter()
            .take(layout.visible_count)
            .any(|entry| entry.id == command)
        {
            return;
        }
        if request.action == Action::Focus {
            if self.unicode_picker.select_command(&command) {
                self.accessibility.focus_override = Some(UNICODE_INPUT);
            }
        } else if let Some(selection) = self.unicode_picker.execute_command(&command) {
            self.accessibility.focus_override = None;
            self.apply_unicode_picker_selection(selection);
        }
    }

    fn handle_guardrail_accessibility_action(&mut self, request: ActionRequest) {
        let Some(target) = self
            .accessibility
            .guardrail_targets
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
            .info_targets
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
                InfoOverlayAction::ScrollLines {
                    request_id,
                    delta: -(self.info_overlay.viewport_rows() as isize),
                }
            }
            (Action::ScrollDown, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::ScrollLines {
                    request_id,
                    delta: self.info_overlay.viewport_rows() as isize,
                }
            }
            (Action::ScrollLeft, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::ScrollColumns {
                    request_id,
                    delta: -(self.info_overlay.viewport_columns() as isize),
                }
            }
            (Action::ScrollRight, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::ScrollColumns {
                    request_id,
                    delta: self.info_overlay.viewport_columns() as isize,
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
            .diff_targets
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
            | DiffSemanticTarget::Dismiss(request_id) => request_id,
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
            (Action::ScrollUp, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::ScrollRows {
                    request_id,
                    delta: -(self.diff_review.viewport_rows() as isize),
                }
            }
            (Action::ScrollDown, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::ScrollRows {
                    request_id,
                    delta: self.diff_review.viewport_rows() as isize,
                }
            }
            (Action::ScrollLeft, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::ScrollColumns {
                    request_id,
                    delta: -(self.diff_review.viewport_columns() as isize),
                }
            }
            (Action::ScrollRight, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::ScrollColumns {
                    request_id,
                    delta: self.diff_review.viewport_columns() as isize,
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
            .context_targets
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

    fn info_accessibility_layout(&self) -> crate::ui::info_overlay::InfoOverlayLayout {
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
        crate::ui::info_overlay::InfoOverlayLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            self.info_overlay.animation(),
            self.info_overlay.title(),
            self.info_overlay.lines().len(),
            self.info_overlay.max_line_chars(),
        )
    }

    fn diff_accessibility_layout(&self) -> crate::ui::diff_review::DiffReviewLayout {
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
        crate::ui::diff_review::DiffReviewLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            self.diff_review.animation(),
            self.diff_review.filename(),
            self.diff_review.rows().len(),
            self.diff_review.max_line_chars(),
        )
    }

    fn context_accessibility_layout(&self) -> crate::ui::context_picker::ContextPickerLayout {
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
        crate::ui::context_picker::ContextPickerLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            self.context_picker.animation(),
            self.context_picker.phase(),
            self.context_picker.sources().len(),
        )
    }

    fn guardrail_accessibility_layout(&self) -> crate::terminal::guardrails::GuardrailLayout {
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
        crate::terminal::guardrails::GuardrailLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            self.guardrail.animation(),
        )
    }

    fn domain_picker_accessibility_layout(
        &self,
        info: &crate::ui::command_palette::PaletteInfo,
    ) -> crate::ui::command_palette::PaletteLayout {
        self.command_palette_accessibility_layout(info)
    }

    fn unicode_picker_accessibility_layout(
        &self,
        info: &crate::ui::command_palette::PaletteInfo,
    ) -> crate::ui::command_palette::PaletteLayout {
        self.command_palette_accessibility_layout(info)
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
        let Some(target) = self
            .accessibility
            .settings_targets
            .get(&request.target_node)
            .cloned()
        else {
            // Settings is modal. Stale requests for controls behind it must
            // never escape into the normal application action path.
            return;
        };

        use crate::ui::settings::SettingsAction;
        match target {
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
                    .settings_nodes
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
            .shortcuts_targets
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
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
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
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
        crate::ui::settings::SettingsModalLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            info.animation,
            info.entries.len(),
        )
    }

    fn handle_commands_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::commands::panel::CommandsPanelAction;

        match request.target_node {
            COMMANDS_NEW => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::StartAdd);
                    self.accessibility.focus_override = Some(COMMANDS_NAME);
                }
                Action::Focus => self.accessibility.focus_override = Some(COMMANDS_NEW),
                _ => {}
            },
            COMMANDS_CLOSE => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::Close);
                    self.accessibility.focus_override = None;
                }
                Action::Focus => self.accessibility.focus_override = Some(COMMANDS_CLOSE),
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
                    .commands_targets
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
                        self.accessibility.focus_override = Some(COMMANDS_NEW);
                    }
                    (
                        CommandsSemanticTarget::Run(_) | CommandsSemanticTarget::Delete(_),
                        Action::Focus,
                    ) => self.accessibility.focus_override = Some(request.target_node),
                    _ => {}
                }
            }
        }
    }

    fn commands_accessibility_layout(
        &self,
        info: &crate::ui::commands::panel::CommandsPanelInfo,
    ) -> crate::ui::commands::panel::CommandsPanelLayout {
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
        crate::ui::commands::panel::CommandsPanelLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            info.animation,
            info.edit_mode != crate::ui::commands::panel::EditMode::Browse,
            info.commands.len(),
        )
    }

    fn handle_session_accessibility_action(&mut self, request: ActionRequest) {
        use crate::session::panel::SessionPanelAction;

        match request.target_node {
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
                        self.accessibility.focus_override = Some(SESSION_RENAME_INPUT);
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
                    .session_targets
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
        info: &crate::session::panel::SessionPanelInfo,
    ) -> crate::session::panel::SessionPanelLayout {
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
        crate::session::panel::SessionPanelLayout::calculate(
            width,
            height,
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
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
        crate::ui::command_palette::PaletteLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            info.animation,
            info.entries.len(),
        )
    }

    fn search_is_visible(&self) -> bool {
        self.search.is_active() || self.search.has_results() || self.search.is_searching()
    }

    fn search_accessibility_layout(
        &self,
        info: &crate::terminal::search::SearchBarInfo,
        window_width: f32,
    ) -> crate::terminal::search::SearchBarLayout {
        self.renderer.as_ref().map_or_else(
            || {
                crate::terminal::search::SearchBarLayout::calculate(
                    info,
                    0.0,
                    window_width,
                    1.0,
                    8.0,
                    16.0,
                )
            },
            |renderer| renderer.search_bar_layout(info, window_width),
        )
    }

    fn with_toast_accessibility(&self, mut snapshot: SemanticSnapshot) -> SemanticSnapshot {
        let toasts = self.toasts.build_accessibility_info();
        if toasts.is_empty() {
            return snapshot;
        }
        let (window_width, window_height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
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
            node.set_bounds(rect(
                placement.rect.x,
                placement.rect.y,
                placement.rect.width,
                placement.rect.height,
            ));
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
        let (
            width,
            height,
            tab_height,
            cell_width,
            cell_height,
            padding,
            content_left,
            content_top,
        ) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                let search_height = if self.search_is_visible() {
                    renderer.cell_height() + 16.0
                } else {
                    0.0
                };
                (
                    width as f32,
                    height as f32,
                    renderer.tab_bar_height(),
                    renderer.cell_width(),
                    renderer.cell_height(),
                    renderer.padding(),
                    renderer.tab_bar_width(),
                    renderer.tab_bar_height() + renderer.chrome_bar_height() + search_height,
                )
            })
            .unwrap_or((1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0));

        if self.guardrail.is_active() {
            return self.guardrail_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_guardrail_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self.info_overlay.is_active() {
            return self.info_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_info_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self.diff_review.is_active() {
            return self.diff_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_diff_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self.context_picker.is_active() {
            return self.context_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_context_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self
            .harvest
            .as_ref()
            .is_some_and(|harvest| harvest.sprout_menu.is_some())
        {
            return self.sprout_accessibility_snapshot(width, height);
        }
        if self
            .harvest
            .as_ref()
            .is_some_and(|harvest| harvest.cellar.is_expanded())
        {
            return self.cellar_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_harvest_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self.domain_picker.is_active() {
            return self.domain_picker_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_domain_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self.unicode_picker.is_active() {
            return self.unicode_picker_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_unicode_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self.command_palette.is_active() {
            return self.command_palette_accessibility_snapshot(width, height);
        }
        if self.accessibility.focus_override == Some(PALETTE_INPUT) {
            self.accessibility.focus_override = None;
        }
        if self.shortcuts_panel.is_active() {
            return self.shortcuts_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_shortcuts_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self.settings_panel.is_active() {
            return self.settings_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_settings_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self.commands_panel.is_open() {
            return self.commands_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_commands_node(node))
        {
            self.accessibility.focus_override = None;
        }
        if self.session_panel.is_open() {
            return self.session_accessibility_snapshot(width, height);
        }
        if self
            .accessibility
            .focus_override
            .is_some_and(|node| self.accessibility.is_session_node(node))
        {
            self.accessibility.focus_override = None;
        }
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

        let mut nodes = Vec::new();
        let mut root_children = vec![TAB_LIST, NEW_TAB, WORKSPACES, COMMANDS, SETTINGS];
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, width, height));

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
            if let Some((x, y, w, h)) = self.renderer.as_ref().and_then(|renderer| {
                renderer.tab_bar_element_rect(
                    crate::renderer::TabBarClick::Tab(index),
                    &tab_info,
                    width,
                    height,
                )
            }) {
                node.set_bounds(rect(x, y, w, h));
            }
            nodes.push((tab_id, node));

            let mut close = Node::new(Role::Button);
            close.set_label(format!("Close {}", tab.title));
            close.add_action(Action::Click);
            close.add_action(Action::Focus);
            if let Some((x, y, w, h)) = self.renderer.as_ref().and_then(|renderer| {
                renderer.tab_bar_element_rect(
                    crate::renderer::TabBarClick::CloseTab(index),
                    &tab_info,
                    width,
                    height,
                )
            }) {
                close.set_bounds(rect(x, y, w, h));
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
            if let Some((x, y, w, h)) = self.renderer.as_ref().and_then(|renderer| {
                renderer.tab_bar_element_rect(target, &tab_info, width, height)
            }) {
                node.set_bounds(rect(x, y, w, h));
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
            let Some((x, y, w, h)) = self.renderer.as_ref().and_then(|renderer| {
                renderer.tab_bar_element_rect(target, &tab_info, width, height)
            }) else {
                continue;
            };
            let mut node = Node::new(Role::Button);
            node.set_label(label);
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.set_bounds(rect(x, y, w, h));
            nodes.push((id, node));
            root_children.push(id);
        }

        if self.search_is_visible() {
            let info = self.search.build_info(self.search_cursor_pos);
            let layout = self.search_accessibility_layout(&info, width);
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
            input.set_value(info.query.clone());
            input.set_character_lengths(
                info.query
                    .chars()
                    .map(|character| character.len_utf8() as u8)
                    .collect::<Vec<_>>(),
            );
            let cursor = info
                .query
                .get(..info.cursor_pos.min(info.query.len()))
                .filter(|_| {
                    info.query
                        .is_char_boundary(info.cursor_pos.min(info.query.len()))
                })
                .map(str::chars)
                .map(Iterator::count)
                .unwrap_or_else(|| info.query.chars().count());
            let position = TextPosition {
                node: SEARCH_INPUT,
                character_index: cursor,
            };
            input.set_text_selection(TextSelection {
                anchor: position,
                focus: position,
            });
            input.set_bounds(search_rect(layout.input));
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
            status.set_bounds(search_rect(layout.status));
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
                button.set_bounds(search_rect(rectangle));
                button.add_action(Action::Click);
                button.add_action(Action::Focus);
                nodes.push((id, button));
            }

            for (id, label, rectangle) in [
                (SEARCH_PREVIOUS, "Previous match", layout.previous),
                (SEARCH_NEXT, "Next match", layout.next),
            ] {
                if let Some(rectangle) = rectangle {
                    let mut button = Node::new(Role::Button);
                    button.set_label(label);
                    button.set_bounds(search_rect(rectangle));
                    button.add_action(Action::Click);
                    button.add_action(Action::Focus);
                    nodes.push((id, button));
                    search_children.push(id);
                }
            }
            let mut close = Node::new(Role::Button);
            close.set_label("Close terminal search");
            close.set_bounds(search_rect(layout.close));
            close.add_action(Action::Click);
            close.add_action(Action::Focus);
            nodes.push((SEARCH_CLOSE, close));
            search_children.push(SEARCH_CLOSE);

            let mut toolbar = Node::new(Role::Toolbar);
            toolbar.set_label("Terminal search");
            toolbar.set_bounds(search_rect(layout.bar));
            toolbar.set_children(search_children);
            nodes.push((SEARCH_TOOLBAR, toolbar));
            root_children.push(SEARCH_TOOLBAR);
        }

        let mut semantic_focus = ROOT;
        if let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) {
            if let Some(panes) = &tab.pane_manager {
                let focused = panes.focused_id();
                let mut ordered: Vec<_> = panes.iter_with_layout().collect();
                ordered.sort_by_key(|(pane, _)| pane.id.0);
                for (pane, pane_rect) in ordered {
                    let id = pane_node(pane.id);
                    root_children.push(id);
                    if pane.id == focused {
                        semantic_focus = id;
                    }
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
                        rect(
                            content_left + padding + pane_rect.x,
                            content_top + pane_rect.y,
                            pane_rect.width,
                            pane_rect.height,
                        ),
                        cell_width,
                        cell_height,
                    );
                }
            } else if let Some(terminal) = tab.focused_terminal() {
                let pane_id = tab.single_pane_id();
                let id = pane_node(pane_id);
                root_children.push(id);
                semantic_focus = id;
                append_terminal_nodes(
                    &mut nodes,
                    id,
                    pane_id,
                    terminal,
                    true,
                    None,
                    tab.focused_pane_read_only(),
                    false,
                    rect(
                        content_left + padding,
                        content_top + padding,
                        width - content_left - padding * 2.0,
                        height - content_top - padding * 2.0,
                    ),
                    cell_width,
                    cell_height,
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

    fn cellar_accessibility_layout(&mut self) -> crate::harvest::CellarLayout {
        let (width, height, cell_width, cell_height) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                let (width, height) = renderer.window_size();
                (
                    width as f32,
                    height as f32,
                    renderer.cell_width(),
                    renderer.cell_height(),
                )
            })
            .unwrap_or((800.0, 600.0, 8.0, 16.0));
        let animation = self
            .harvest
            .as_ref()
            .map_or(1.0, |harvest| harvest.cellar.animation.progress());
        let preview = crate::harvest::CellarLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            animation,
            std::iter::empty(),
        );
        if let Some(harvest) = &mut self.harvest {
            harvest.cellar.set_viewport_rows(preview.max_visible);
        }
        let ids = self
            .harvest
            .as_ref()
            .map(|harvest| {
                harvest
                    .cellar
                    .visible_entries()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        crate::harvest::CellarLayout::calculate(
            width,
            height,
            cell_width,
            cell_height,
            animation,
            ids,
        )
    }

    fn cellar_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        let layout = self.cellar_accessibility_layout();
        let (entries, selected, total, evicted, scroll, healthy) =
            self.harvest.as_ref().map_or_else(
                || (Vec::new(), None, 0, 0, 0, 0),
                |harvest| {
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
                },
            );
        let targets = entries
            .iter()
            .map(|entry| HarvestSemanticTarget::CellarEntry(entry.id))
            .collect::<HashSet<_>>();
        self.accessibility.retain_harvest_targets(&targets);
        let entry_nodes = entries
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    self.accessibility
                        .harvest_node(HarvestSemanticTarget::CellarEntry(entry.id)),
                )
            })
            .collect::<Vec<_>>();

        let mut nodes = Vec::with_capacity(entries.len() + 7);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([CELLAR_DIALOG]);

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
        status.set_bounds(cellar_rect(layout.summary));
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
                "; retained output is truncated at 64 KiB"
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
                node.set_bounds(cellar_rect(*row));
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
        list.set_bounds(cellar_rect(layout.list));
        if scroll > 0 {
            list.add_action(Action::ScrollUp);
        }
        if scroll.saturating_add(layout.max_visible) < total {
            list.add_action(Action::ScrollDown);
        }
        nodes.push((CELLAR_LIST, list));

        let mut close = Node::new(Role::Button);
        close.set_label("Close Root Cellar");
        close.set_bounds(cellar_rect(layout.close));
        close.add_action(Action::Focus);
        close.add_action(Action::Click);
        nodes.push((CELLAR_CLOSE, close));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Root Cellar command history");
        dialog.set_modal();
        dialog.set_bounds(cellar_rect(layout.panel));
        dialog.set_children([CELLAR_TITLE, CELLAR_STATUS, CELLAR_LIST, CELLAR_CLOSE]);
        nodes.push((CELLAR_DIALOG, dialog));
        nodes.push((ROOT, root));

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
        SemanticSnapshot { nodes, focus }
    }

    fn sprout_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        let (request_id, row, col, width_chars, actions, selected, command) = self
            .harvest
            .as_ref()
            .and_then(|harvest| harvest.sprout_menu.as_ref())
            .map(|menu| {
                (
                    menu.request_id,
                    menu.row,
                    menu.col,
                    menu.width(),
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
        let (cell_width, cell_height, padding, content_left, content_top) = self
            .renderer
            .as_ref()
            .map(|renderer| {
                (
                    renderer.cell_width(),
                    renderer.cell_height(),
                    renderer.padding(),
                    renderer.tab_bar_width(),
                    renderer.tab_bar_height() + renderer.chrome_bar_height(),
                )
            })
            .unwrap_or((8.0, 16.0, 8.0, 0.0, 40.0));
        let layout = crate::harvest::SproutMenuLayout::calculate(
            row,
            col,
            actions.len(),
            width_chars,
            window_width,
            window_height,
            content_left,
            content_top,
            padding,
            cell_width,
            cell_height,
        );
        let targets = actions
            .iter()
            .map(|(action, _, _, _)| HarvestSemanticTarget::SproutAction {
                request_id,
                action: *action,
            })
            .collect::<HashSet<_>>();
        self.accessibility.retain_harvest_targets(&targets);
        let action_nodes = actions
            .iter()
            .map(|(action, _, _, _)| {
                self.accessibility
                    .harvest_node(HarvestSemanticTarget::SproutAction {
                        request_id,
                        action: *action,
                    })
            })
            .collect::<Vec<_>>();

        let mut nodes = Vec::with_capacity(actions.len() + 4);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([SPROUT_DIALOG]);

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
            node.set_bounds(sprout_rect(*row));
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
        menu.set_bounds(sprout_rect(layout.panel));
        menu.set_children(action_nodes.clone());
        nodes.push((SPROUT_MENU, menu));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(format!("Harvest actions for {command}"));
        dialog.set_description("Press Escape to close without running an action");
        dialog.set_modal();
        dialog.set_bounds(sprout_rect(layout.panel));
        dialog.set_children([SPROUT_MENU]);
        nodes.push((SPROUT_DIALOG, dialog));
        nodes.push((ROOT, root));

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
        SemanticSnapshot { nodes, focus }
    }

    fn command_palette_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        let mut info = self.command_palette.build_info();
        let mut layout = self.command_palette_accessibility_layout(&info);
        self.command_palette.set_max_visible(layout.rows_that_fit);
        info = self.command_palette.build_info();
        layout = self.command_palette_accessibility_layout(&info);
        let registered_commands = self.command_palette.command_ids().collect::<HashSet<_>>();
        self.accessibility
            .retain_palette_commands(&registered_commands);
        let visible_entries = info
            .entries
            .iter()
            .take(layout.visible_count)
            .cloned()
            .collect::<Vec<_>>();
        let entry_nodes = visible_entries
            .iter()
            .map(|entry| self.accessibility.palette_node(&entry.id))
            .collect::<Vec<_>>();

        let mut nodes = Vec::with_capacity(entry_nodes.len() + 5);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([PALETTE_DIALOG]);

        let input_x = layout.panel_x + layout.padding;
        let input_y = layout.panel_y + layout.padding;
        let input_width = layout.panel_width - layout.padding * 2.0;
        let mut input = Node::new(Role::SearchInput);
        input.set_label("Search commands");
        input.set_placeholder(info.placeholder.clone());
        input.set_value(info.input.clone());
        input.set_character_lengths(
            info.input
                .chars()
                .map(|character| character.len_utf8() as u8)
                .collect::<Vec<_>>(),
        );
        let cursor = info
            .input
            .get(..info.cursor)
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or_else(|| info.input.chars().count());
        let cursor_position = TextPosition {
            node: PALETTE_INPUT,
            character_index: cursor,
        };
        input.set_text_selection(TextSelection {
            anchor: cursor_position,
            focus: cursor_position,
        });
        input.set_bounds(rect(input_x, input_y, input_width, layout.input_height));
        input.set_controls([PALETTE_LIST]);
        input.add_action(Action::Focus);
        input.add_action(Action::SetValue);
        if let Some(selected) = entry_nodes.get(info.selected).copied() {
            input.set_active_descendant(selected);
        }
        nodes.push((PALETTE_INPUT, input));

        for (row, (entry, node_id)) in visible_entries
            .iter()
            .zip(entry_nodes.iter().copied())
            .enumerate()
        {
            let mut node = Node::new(Role::ListBoxOption);
            node.set_label(entry.title.clone());
            let mut description = format!("{} command", entry.category);
            if entry.destructive {
                description.push_str(", destructive");
            }
            node.set_description(description);
            if let Some(shortcut) = &entry.shortcut {
                node.set_keyboard_shortcut(shortcut.clone());
            }
            node.set_selected(row == info.selected);
            node.add_action(Action::Click);
            node.set_bounds(rect(
                layout.panel_x + layout.padding * 0.5,
                layout.list_y + row as f32 * layout.row_height,
                layout.panel_width - layout.padding,
                layout.row_height,
            ));
            nodes.push((node_id, node));
        }

        let mut list = Node::new(Role::ListBox);
        list.set_label(format!("{} results", info.item_noun));
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel_x + layout.padding * 0.5,
            layout.list_y,
            layout.panel_width - layout.padding,
            layout.list_height,
        ));
        list.set_children(entry_nodes);
        nodes.push((PALETTE_LIST, list));

        let mut dialog_children = vec![PALETTE_INPUT, PALETTE_LIST];
        if visible_entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No matching commands");
            empty.set_live(Live::Polite);
            nodes.push((PALETTE_EMPTY, empty));
            dialog_children.push(PALETTE_EMPTY);
        }
        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Command palette");
        dialog.set_modal();
        dialog.set_bounds(rect(
            layout.panel_x,
            layout.panel_y,
            layout.panel_width,
            layout.panel_height,
        ));
        dialog.set_children(dialog_children);
        nodes.push((PALETTE_DIALOG, dialog));
        nodes.push((ROOT, root));

        self.accessibility.focus_override = Some(PALETTE_INPUT);
        SemanticSnapshot {
            nodes,
            focus: PALETTE_INPUT,
        }
    }

    fn domain_picker_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        let mut info = self.domain_picker.build_info();
        let mut layout = self.domain_picker_accessibility_layout(&info);
        self.domain_picker.set_max_visible(layout.rows_that_fit);
        info = self.domain_picker.build_info();
        layout = self.domain_picker_accessibility_layout(&info);

        let registered = self.domain_picker.command_ids().collect::<HashSet<_>>();
        self.accessibility.retain_domain_commands(&registered);
        let visible_entries = info
            .entries
            .iter()
            .take(layout.visible_count)
            .cloned()
            .collect::<Vec<_>>();
        let entry_nodes = visible_entries
            .iter()
            .map(|entry| self.accessibility.domain_node(&entry.id))
            .collect::<Vec<_>>();
        let selected_action = visible_entries
            .get(info.selected)
            .filter(|entry| self.domain_picker.is_actionable(&entry.id))
            .and_then(|_| entry_nodes.get(info.selected))
            .copied();
        let has_actions = visible_entries
            .iter()
            .any(|entry| self.domain_picker.is_actionable(&entry.id));

        let mut nodes = Vec::with_capacity(entry_nodes.len() + 5);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([DOMAIN_DIALOG]);

        let input_x = layout.panel_x + layout.padding;
        let input_y = layout.panel_y + layout.padding;
        let input_width = layout.panel_width - layout.padding * 2.0;
        let mut input = Node::new(Role::SearchInput);
        input.set_label("Search live-domain hosts or domains");
        input.set_placeholder(info.placeholder.clone());
        input.set_value(info.input.clone());
        input.set_character_lengths(
            info.input
                .chars()
                .map(|character| character.len_utf8() as u8)
                .collect::<Vec<_>>(),
        );
        let cursor = info
            .input
            .get(..info.cursor)
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or_else(|| info.input.chars().count());
        let position = TextPosition {
            node: DOMAIN_INPUT,
            character_index: cursor,
        };
        input.set_text_selection(TextSelection {
            anchor: position,
            focus: position,
        });
        input.set_bounds(rect(input_x, input_y, input_width, layout.input_height));
        input.set_controls([DOMAIN_LIST]);
        input.add_action(Action::Focus);
        if self.domain_picker.can_filter() {
            input.add_action(Action::SetValue);
        } else {
            input.set_read_only();
        }
        if let Some(selected) = selected_action {
            input.set_active_descendant(selected);
        }
        nodes.push((DOMAIN_INPUT, input));

        for (row, (entry, node_id)) in visible_entries
            .iter()
            .zip(entry_nodes.iter().copied())
            .enumerate()
        {
            let actionable = self.domain_picker.is_actionable(&entry.id);
            let mut node = Node::new(if actionable {
                Role::ListBoxOption
            } else {
                Role::Status
            });
            node.set_label(entry.title.clone());
            node.set_description(entry.category.clone());
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

        let mut list = Node::new(if has_actions {
            Role::ListBox
        } else {
            Role::List
        });
        list.set_label(format!("Live-domain {} results", info.item_noun));
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
        nodes.push((DOMAIN_LIST, list));

        let mut dialog_children = vec![DOMAIN_INPUT, DOMAIN_LIST];
        if visible_entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No matching live-domain results");
            empty.set_live(Live::Polite);
            nodes.push((DOMAIN_EMPTY, empty));
            dialog_children.push(DOMAIN_EMPTY);
        }
        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Attach live domain");
        dialog.set_description(if self.domain_picker.is_busy() {
            "Waiting for authoritative domain state; Escape returns to hosts"
        } else if !self.domain_picker.can_filter() {
            "Domain discovery status; Escape returns to hosts"
        } else {
            "Choose a local or saved SSH host, then choose a live domain"
        });
        dialog.set_modal();
        dialog.set_bounds(rect(
            layout.panel_x,
            layout.panel_y,
            layout.panel_width,
            layout.panel_height,
        ));
        dialog.set_children(dialog_children);
        nodes.push((DOMAIN_DIALOG, dialog));
        nodes.push((ROOT, root));

        self.accessibility.focus_override = Some(DOMAIN_INPUT);
        SemanticSnapshot {
            nodes,
            focus: DOMAIN_INPUT,
        }
    }

    fn unicode_picker_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        let mut info = self.unicode_picker.build_info();
        let mut layout = self.unicode_picker_accessibility_layout(&info);
        self.unicode_picker.set_max_visible(layout.rows_that_fit);
        info = self.unicode_picker.build_info();
        layout = self.unicode_picker_accessibility_layout(&info);

        let registered = self.unicode_picker.command_ids().collect::<HashSet<_>>();
        self.accessibility.retain_unicode_commands(&registered);
        let visible_entries = info
            .entries
            .iter()
            .take(layout.visible_count)
            .cloned()
            .collect::<Vec<_>>();
        let entry_nodes = visible_entries
            .iter()
            .map(|entry| self.accessibility.unicode_node(&entry.id))
            .collect::<Vec<_>>();

        let mut nodes = Vec::with_capacity(entry_nodes.len() + 5);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([UNICODE_DIALOG]);

        let input_x = layout.panel_x + layout.padding;
        let input_y = layout.panel_y + layout.padding;
        let input_width = layout.panel_width - layout.padding * 2.0;
        let mut input = Node::new(Role::SearchInput);
        input.set_label("Search Unicode characters or enter a code point");
        input.set_placeholder(info.placeholder.clone());
        input.set_value(info.input.clone());
        input.set_character_lengths(
            info.input
                .chars()
                .map(|character| character.len_utf8() as u8)
                .collect::<Vec<_>>(),
        );
        let cursor = info
            .input
            .get(..info.cursor)
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or_else(|| info.input.chars().count());
        let position = TextPosition {
            node: UNICODE_INPUT,
            character_index: cursor,
        };
        input.set_text_selection(TextSelection {
            anchor: position,
            focus: position,
        });
        input.set_bounds(rect(input_x, input_y, input_width, layout.input_height));
        input.set_controls([UNICODE_LIST]);
        input.add_action(Action::Focus);
        input.add_action(Action::SetValue);
        if let Some(selected) = entry_nodes.get(info.selected).copied() {
            input.set_active_descendant(selected);
        }
        nodes.push((UNICODE_INPUT, input));

        for (row, (entry, node_id)) in visible_entries
            .iter()
            .zip(entry_nodes.iter().copied())
            .enumerate()
        {
            let mut node = Node::new(Role::ListBoxOption);
            node.set_label(entry.title.clone());
            node.set_description(entry.category.clone());
            node.set_selected(row == info.selected);
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.set_bounds(rect(
                layout.panel_x + layout.padding * 0.5,
                layout.list_y + row as f32 * layout.row_height,
                layout.panel_width - layout.padding,
                layout.row_height,
            ));
            nodes.push((node_id, node));
        }

        let mut list = Node::new(Role::ListBox);
        list.set_label("Unicode character results");
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel_x + layout.padding * 0.5,
            layout.list_y,
            layout.panel_width - layout.padding,
            layout.list_height,
        ));
        if let Some(selected) = entry_nodes.get(info.selected).copied() {
            list.set_active_descendant(selected);
        }
        list.set_children(entry_nodes);
        nodes.push((UNICODE_LIST, list));

        let mut dialog_children = vec![UNICODE_INPUT, UNICODE_LIST];
        if visible_entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label(info.status.unwrap_or("No matching Unicode character"));
            empty.set_live(Live::Polite);
            nodes.push((UNICODE_EMPTY, empty));
            dialog_children.push(UNICODE_EMPTY);
        }
        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Unicode character picker");
        dialog.set_description(
            "Search common characters or enter U+ followed by a non-control Unicode scalar",
        );
        dialog.set_modal();
        dialog.set_bounds(rect(
            layout.panel_x,
            layout.panel_y,
            layout.panel_width,
            layout.panel_height,
        ));
        dialog.set_children(dialog_children);
        nodes.push((UNICODE_DIALOG, dialog));
        nodes.push((ROOT, root));

        self.accessibility.focus_override = Some(UNICODE_INPUT);
        SemanticSnapshot {
            nodes,
            focus: UNICODE_INPUT,
        }
    }

    fn info_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        let layout = self.info_accessibility_layout();
        self.info_overlay
            .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
        let request_id = self.info_overlay.request_id();
        let awaiting = self.info_overlay.is_awaiting_current();
        let scroll_offset = self.info_overlay.scroll_offset();
        let horizontal_offset = self.info_overlay.horizontal_offset();
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
        self.accessibility.retain_info_targets(&targets);
        let content_id = self.accessibility.info_node(content_target);
        let copy_id = self.accessibility.info_node(copy_target);
        let dismiss_id = self.accessibility.info_node(dismiss_target);
        let input_id = input_visible.then(|| {
            self.accessibility
                .info_node(InfoSemanticTarget::Input(request_id))
        });
        let choice_nodes: Vec<_> = visible_choices
            .iter()
            .map(|choice| {
                let target = InfoSemanticTarget::Choice {
                    request_id,
                    id: choice.id.clone(),
                };
                (choice.clone(), self.accessibility.info_node(target))
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
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([INFO_DIALOG]);

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
        content.set_bounds(info_rect(layout.content));
        let mut content_children = input_id.into_iter().collect::<Vec<_>>();
        content_children.extend(choice_nodes.iter().map(|(_, node)| *node));
        content.set_children(content_children);
        if awaiting {
            content.add_action(Action::Focus);
            if self.info_overlay.scroll_offset() > 0 {
                content.add_action(Action::ScrollUp);
            }
            if self.info_overlay.scroll_offset() < self.info_overlay.max_scroll_offset() {
                content.add_action(Action::ScrollDown);
            }
            if self.info_overlay.horizontal_offset() > 0 {
                content.add_action(Action::ScrollLeft);
            }
            if self.info_overlay.horizontal_offset() < self.info_overlay.max_horizontal_offset() {
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
            node.set_description(format!(
                "Single-line value, at most {} UTF-8 bytes",
                input.max_bytes
            ));
            node.set_value(input.value.clone());
            node.set_character_lengths(
                input
                    .value
                    .chars()
                    .map(|character| character.len_utf8() as u8)
                    .collect::<Vec<_>>(),
            );
            let cursor = input.value[..input.cursor_byte].chars().count();
            let position = TextPosition {
                node: input_id,
                character_index: cursor,
            };
            node.set_text_selection(TextSelection {
                anchor: position,
                focus: position,
            });
            node.set_bounds(info_rect(input_rect));
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
        copy.set_keyboard_shortcut("C");
        copy.set_bounds(info_rect(layout.copy_button));
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
        dismiss.set_bounds(info_rect(layout.continue_button));
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
        dialog.set_bounds(info_rect(layout.card));
        let mut children = vec![INFO_TITLE, content_id];
        children.extend([INFO_STATUS, copy_id, dismiss_id]);
        dialog.set_children(children);
        nodes.push((INFO_DIALOG, dialog));
        nodes.push((ROOT, root));

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
        SemanticSnapshot { nodes, focus }
    }

    fn context_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        use crate::ui::context_picker::{ContextPickerPhase, ContextSourceId};

        let layout = self.context_accessibility_layout();
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
        self.accessibility.retain_context_targets(&targets);
        let cancel_id = self.accessibility.context_node(cancel_target);
        let primary_id = self.accessibility.context_node(primary_target);
        let list_id = (snapshot.phase == ContextPickerPhase::Sources)
            .then(|| self.accessibility.context_node(list_target));
        let input_id = (snapshot.phase == ContextPickerPhase::FilePaths)
            .then(|| self.accessibility.context_node(input_target));
        let source_nodes: Vec<_> = if snapshot.phase == ContextPickerPhase::Sources {
            visible_sources
                .iter()
                .map(|source| {
                    let id = self
                        .accessibility
                        .context_node(ContextSemanticTarget::Source {
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
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([CONTEXT_DIALOG]);

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
            list.set_bounds(context_rect(layout.content));
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
            input.set_bounds(context_rect(layout.file_input));
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
        cancel.set_bounds(context_rect(layout.cancel_button));
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
        primary.set_bounds(context_rect(layout.primary_button));
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
        dialog.set_bounds(context_rect(layout.card));
        let mut children = vec![CONTEXT_TITLE];
        if let Some(main) = list_id.or(input_id) {
            children.push(main);
        }
        children.extend([CONTEXT_STATUS, cancel_id, primary_id]);
        dialog.set_children(children);
        nodes.push((CONTEXT_DIALOG, dialog));
        nodes.push((ROOT, root));

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
        let requested_focus = self.accessibility.focus_override.filter(|node| {
            self.accessibility.is_context_node(*node)
                && nodes.iter().any(|(current, _)| current == node)
                && (snapshot.awaiting || *node == CONTEXT_DIALOG)
        });
        let focus = requested_focus.unwrap_or(if snapshot.awaiting {
            default_focus
        } else {
            CONTEXT_DIALOG
        });
        if self.accessibility.focus_override.is_some() && requested_focus.is_none() {
            self.accessibility.focus_override = None;
        }
        self.accessibility.focus_override = Some(focus);
        SemanticSnapshot { nodes, focus }
    }

    fn diff_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        let layout = self.diff_accessibility_layout();
        self.diff_review
            .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
        let request_id = self.diff_review.request_id();
        let awaiting = self.diff_review.is_awaiting_current();
        let scroll_offset = self.diff_review.scroll_offset();
        let horizontal_offset = self.diff_review.horizontal_offset();
        let total_rows = self.diff_review.rows().len();

        let content_target = DiffSemanticTarget::Content(request_id);
        let copy_target = DiffSemanticTarget::Copy(request_id);
        let dismiss_target = DiffSemanticTarget::Dismiss(request_id);
        let targets = HashSet::from([content_target, copy_target, dismiss_target]);
        self.accessibility.retain_diff_targets(&targets);
        let content_id = self.accessibility.diff_node(content_target);
        let copy_id = self.accessibility.diff_node(copy_target);
        let dismiss_id = self.accessibility.diff_node(dismiss_target);

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
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([DIFF_DIALOG]);

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
        content.set_label("Detected unified diff");
        content.set_value(visible_rows.join("\n"));
        content.set_description(status_text.clone());
        content.set_bounds(diff_rect(layout.content));
        if awaiting {
            content.add_action(Action::Focus);
            if self.diff_review.scroll_offset() > 0 {
                content.add_action(Action::ScrollUp);
            }
            if self.diff_review.scroll_offset() < self.diff_review.max_scroll_offset() {
                content.add_action(Action::ScrollDown);
            }
            if self.diff_review.horizontal_offset() > 0 {
                content.add_action(Action::ScrollLeft);
            }
            if self.diff_review.horizontal_offset() < self.diff_review.max_horizontal_offset() {
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

        let mut copy = Node::new(Role::Button);
        copy.set_label("Copy complete raw diff");
        copy.set_keyboard_shortcut("C");
        copy.set_bounds(diff_rect(layout.copy_button));
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
        dismiss.set_bounds(diff_rect(layout.done_button));
        if awaiting {
            dismiss.add_action(Action::Click);
            dismiss.add_action(Action::Focus);
        } else {
            dismiss.set_disabled();
        }
        nodes.push((dismiss_id, dismiss));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(title_text);
        dialog.set_description("Scrollable review of one detected unified diff");
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        dialog.set_bounds(diff_rect(layout.card));
        dialog.set_children([DIFF_TITLE, content_id, DIFF_STATUS, copy_id, dismiss_id]);
        nodes.push((DIFF_DIALOG, dialog));
        nodes.push((ROOT, root));

        let requested_focus = self.accessibility.focus_override.filter(|node| {
            self.accessibility.is_diff_node(*node)
                && nodes.iter().any(|(current, _)| current == node)
                && (awaiting || *node == DIFF_DIALOG)
        });
        let focus = requested_focus.unwrap_or(if awaiting { dismiss_id } else { DIFF_DIALOG });
        if self.accessibility.focus_override.is_some() && requested_focus.is_none() {
            self.accessibility.focus_override = None;
        }
        self.accessibility.focus_override = Some(focus);
        SemanticSnapshot { nodes, focus }
    }

    fn guardrail_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        let layout = self.guardrail_accessibility_layout();
        let request_id = self.guardrail.request_id();
        let awaiting = self.guardrail.is_awaiting_current();
        let matched = self.guardrail.matched.clone();
        let confirm_target = GuardrailSemanticTarget::Confirm(request_id);
        let cancel_target = GuardrailSemanticTarget::Cancel(request_id);
        self.accessibility
            .retain_guardrail_targets(&HashSet::from([confirm_target, cancel_target]));
        let confirm_id = self.accessibility.guardrail_node(confirm_target);
        let cancel_id = self.accessibility.guardrail_node(cancel_target);

        let mut nodes = Vec::with_capacity(7);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([GUARDRAIL_DIALOG]);

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
                    bounded_semantic_text(&matched.command, 1024),
                    bounded_semantic_text(&matched.reason, 512),
                    format!(
                        "Where: {}; As: {}",
                        bounded_semantic_text(&matched.target, 256),
                        bounded_semantic_text(&matched.user, 128)
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
        cancel.set_bounds(guardrail_rect(layout.cancel_button));
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
        confirm.set_bounds(guardrail_rect(layout.confirm_button));
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
        dialog.set_bounds(guardrail_rect(layout.card));
        dialog.set_children([
            GUARDRAIL_COMMAND,
            GUARDRAIL_REASON,
            GUARDRAIL_CONTEXT,
            cancel_id,
            confirm_id,
        ]);
        nodes.push((GUARDRAIL_DIALOG, dialog));
        nodes.push((ROOT, root));

        let requested_focus = self.accessibility.focus_override.filter(|node| {
            self.accessibility.is_guardrail_node(*node)
                && nodes.iter().any(|(current, _)| current == node)
                && (awaiting || *node == GUARDRAIL_DIALOG)
        });
        let default_focus = if awaiting {
            cancel_id
        } else {
            GUARDRAIL_DIALOG
        };
        let focus = requested_focus.unwrap_or(default_focus);
        if self.accessibility.focus_override.is_some() && requested_focus.is_none() {
            self.accessibility.focus_override = Some(default_focus);
        }
        SemanticSnapshot { nodes, focus }
    }

    fn shortcuts_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        let mut info = self.shortcuts_panel.build_info(&self.shortcut_registry);
        let mut layout = self.shortcuts_accessibility_layout(&info);
        self.shortcuts_panel.set_max_visible(layout.rows_that_fit);
        info = self.shortcuts_panel.build_info(&self.shortcut_registry);
        layout = self.shortcuts_accessibility_layout(&info);

        let mut nodes = Vec::with_capacity(info.entries.len() + 8);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([SHORTCUTS_DIALOG]);

        let mut input = Node::new(Role::SearchInput);
        input.set_label("Filter keyboard shortcuts");
        input.set_placeholder("Filter shortcuts...");
        input.set_value(info.filter.clone());
        input.set_character_lengths(
            info.filter
                .chars()
                .map(|character| character.len_utf8() as u8)
                .collect::<Vec<_>>(),
        );
        let cursor = info
            .filter
            .get(..info.cursor)
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or_else(|| info.filter.chars().count());
        let position = TextPosition {
            node: SHORTCUTS_INPUT,
            character_index: cursor,
        };
        input.set_text_selection(TextSelection {
            anchor: position,
            focus: position,
        });
        input.set_bounds(shortcuts_rect(layout.input));
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
            let node_id = self.accessibility.shortcuts_node(target);
            list_children.push(node_id);
            let bounds = shortcuts_rect(layout.row(row));
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
        list.set_bounds(shortcuts_rect(layout.list));
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
        if info.recording || info.conflict_label.is_some() {
            let mut status = Node::new(Role::Status);
            status.set_label(if info.recording {
                "Recording shortcut. Press a key to bind, Delete to clear, or Escape to cancel."
                    .to_string()
            } else {
                format!(
                    "Shortcut reassigned from {}",
                    info.conflict_label.as_deref().unwrap_or_default()
                )
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
        nodes.push((ROOT, root));

        let default_focus = selected_node.unwrap_or(if info.recording {
            SHORTCUTS_DIALOG
        } else {
            SHORTCUTS_INPUT
        });
        let requested_focus = self.accessibility.focus_override.filter(|node| {
            (!info.recording || *node != SHORTCUTS_INPUT)
                && self.accessibility.is_shortcuts_node(*node)
                && nodes.iter().any(|(current, _)| current == node)
        });
        let focus = requested_focus.unwrap_or(default_focus);
        if self.accessibility.focus_override.is_some() && requested_focus.is_none() {
            self.accessibility.focus_override = Some(default_focus);
        }
        SemanticSnapshot { nodes, focus }
    }

    fn settings_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        use crate::ui::settings::SettingKind;

        let mut info = self
            .settings_panel
            .build_info(&self.config, &self.shortcut_registry);
        let mut layout = self.settings_accessibility_layout(&info);
        self.settings_panel.set_max_visible(layout.rows_that_fit);
        info = self
            .settings_panel
            .build_info(&self.config, &self.shortcut_registry);
        layout = self.settings_accessibility_layout(&info);
        let end = (info.scroll_offset + layout.visible_count).min(info.entries.len());
        let visible_entries = &info.entries[info.scroll_offset..end];

        let mut nodes = Vec::with_capacity(visible_entries.len() + 16);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([SETTINGS_DIALOG]);

        let mut input = Node::new(Role::SearchInput);
        input.set_label("Filter settings");
        input.set_placeholder("Filter settings...");
        input.set_value(info.filter.clone());
        input.set_character_lengths(
            info.filter
                .chars()
                .map(|character| character.len_utf8() as u8)
                .collect::<Vec<_>>(),
        );
        let cursor = info
            .filter
            .get(..info.filter_cursor)
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or_else(|| info.filter.chars().count());
        let cursor_position = TextPosition {
            node: SETTINGS_INPUT,
            character_index: cursor,
        };
        input.set_text_selection(TextSelection {
            anchor: cursor_position,
            focus: cursor_position,
        });
        input.set_bounds(rect(
            layout.input.x,
            layout.input.y,
            layout.input.width,
            layout.input.height,
        ));
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
            let node_id = self.accessibility.settings_node(target);
            list_children.push(node_id);
            let bounds = rect(
                layout.panel_x + layout.padding * 0.5,
                layout.list_y + row as f32 * layout.row_height,
                layout.panel_width - layout.padding,
                layout.row_height,
            );

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
                SettingKind::Numeric { min, max, step } => {
                    let mut node = Node::new(Role::SpinButton);
                    node.set_label(entry.label);
                    node.set_description(format!("{} setting", entry.section.label()));
                    node.set_value(entry.value.clone());
                    if let Some(value) = numeric_setting_value(&entry.value) {
                        node.set_numeric_value(value);
                    }
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
        list.set_label("Settings");
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel_x + layout.padding * 0.5,
            layout.list_y,
            layout.panel_width - layout.padding,
            layout.list_height,
        ));
        list.set_children(list_children);
        nodes.push((SETTINGS_LIST, list));

        let mut dialog_children = vec![SETTINGS_INPUT, SETTINGS_LIST];
        if info.entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No matching settings");
            empty.set_live(Live::Polite);
            nodes.push((SETTINGS_EMPTY, empty));
            dialog_children.push(SETTINGS_EMPTY);
        }

        if let Some((entry, options)) = open_dropdown {
            if let Some((dropdown_rect, _)) =
                self.settings_panel.dropdown_layout(&layout, &info.entries)
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
                            .settings_node(SettingsSemanticTarget::Option {
                                section: entry.section,
                                label: entry.label,
                                value: value.clone(),
                            });
                    let mut option = Node::new(Role::ListBoxOption);
                    option.set_label(value.clone());
                    option.set_selected(index == info.dropdown_highlight);
                    option.add_action(Action::Click);
                    option.set_bounds(rect(
                        dropdown_rect.x + 4.0,
                        dropdown_rect.y + 4.0 + visible_index as f32 * dropdown_rect.option_height,
                        dropdown_rect.width - 8.0,
                        dropdown_rect.option_height,
                    ));
                    nodes.push((option_id, option));
                    option_nodes.push(option_id);
                }
                let mut dropdown = Node::new(Role::ListBox);
                dropdown.set_label(format!("{} options", entry.label));
                dropdown.set_orientation(accesskit::Orientation::Vertical);
                dropdown.set_bounds(rect(
                    dropdown_rect.x,
                    dropdown_rect.y,
                    dropdown_rect.width,
                    dropdown_rect.height,
                ));
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
        nodes.push((ROOT, root));

        let default_focus = if info.dropdown_open {
            selected_setting.unwrap_or(SETTINGS_DIALOG)
        } else {
            SETTINGS_INPUT
        };
        let requested_focus = self.accessibility.focus_override.filter(|node| {
            (!info.dropdown_open || *node != SETTINGS_INPUT)
                && self.accessibility.is_settings_node(*node)
                && nodes.iter().any(|(current, _)| current == node)
        });
        let focus = requested_focus.unwrap_or(default_focus);
        if self.accessibility.focus_override.is_some() && requested_focus.is_none() {
            self.accessibility.focus_override = Some(default_focus);
        }
        SemanticSnapshot { nodes, focus }
    }

    fn commands_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
        use crate::ui::commands::panel::EditMode;

        let info = self.commands_panel.build_info(&self.config.commands);
        let layout = self.commands_accessibility_layout(&info);
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
        self.accessibility.retain_commands(&retained);

        let mut nodes = Vec::with_capacity(info.commands.len() * 3 + 12);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([COMMANDS_DIALOG]);

        let mut close = Node::new(Role::Button);
        close.set_label("Close custom commands");
        close.add_action(Action::Click);
        close.add_action(Action::Focus);
        close.set_bounds(command_rect(layout.close_button));
        nodes.push((COMMANDS_CLOSE, close));

        let mut new_command = Node::new(Role::Button);
        new_command.set_label("New command");
        new_command.add_action(Action::Click);
        new_command.add_action(Action::Focus);
        new_command.set_bounds(command_rect(layout.new_button));
        nodes.push((COMMANDS_NEW, new_command));

        let mut dialog_children = vec![COMMANDS_CLOSE, COMMANDS_NEW];
        if let Some(form) = layout.form {
            let name_active = info.edit_mode == EditMode::EditingName;
            let command_active = info.edit_mode == EditMode::EditingCommand;
            let mut name = command_form_input(
                COMMANDS_NAME,
                "Command name",
                "e.g. SSH Production",
                &info.form_name,
                name_active.then_some(info.form_cursor),
                form.name_input,
            );
            name.add_action(Action::SetValue);
            nodes.push((COMMANDS_NAME, name));

            let mut command = command_form_input(
                COMMANDS_COMMAND,
                "Shell command",
                "e.g. ssh admin@prod.app.com",
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
            save.set_bounds(command_rect(form.save_button));
            nodes.push((COMMANDS_SAVE, save));

            let mut cancel = Node::new(Role::Button);
            cancel.set_label("Cancel new command");
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
            cancel.set_bounds(command_rect(form.cancel_button));
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
                .commands_node(CommandsSemanticTarget::Group(key.clone()));
            let run_id = self
                .accessibility
                .commands_node(CommandsSemanticTarget::Run(key.clone()));
            let delete_id = self
                .accessibility
                .commands_node(CommandsSemanticTarget::Delete(key.clone()));
            let mut run = Node::new(Role::Button);
            run.set_label(format!("Run {}", command.name));
            let mut description = command.command.clone();
            if let Some(alias) = &command.alias {
                description.push_str(&format!(", alias {alias}"));
            }
            run.set_description(description);
            run.add_action(Action::Click);
            run.add_action(Action::Focus);
            run.set_bounds(command_rect(layout.command_run(index)));
            nodes.push((run_id, run));

            let mut delete = Node::new(Role::Button);
            delete.set_label(format!("Delete {}", command.name));
            delete.set_description("Permanently removes this custom command");
            delete.add_action(Action::Click);
            delete.add_action(Action::Focus);
            delete.set_bounds(command_rect(layout.command_delete(index)));
            nodes.push((delete_id, delete));

            let mut group = Node::new(Role::Group);
            group.set_label(command.name.clone());
            group.set_bounds(command_rect(layout.command_row(index)));
            group.set_children([run_id, delete_id]);
            nodes.push((group_id, group));
            command_groups.push(group_id);
        }

        let mut list = Node::new(Role::List);
        list.set_label(format!("{} custom commands", info.commands.len()));
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
            empty.set_label("No custom commands yet");
            empty.set_live(Live::Polite);
            nodes.push((COMMANDS_EMPTY, empty));
            dialog_children.push(COMMANDS_EMPTY);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Custom commands");
        dialog.set_modal();
        dialog.set_bounds(command_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((COMMANDS_DIALOG, dialog));
        nodes.push((ROOT, root));

        let default_focus = match info.edit_mode {
            EditMode::EditingName => COMMANDS_NAME,
            EditMode::EditingCommand => COMMANDS_COMMAND,
            EditMode::Browse => COMMANDS_NEW,
        };
        let requested_focus = self.accessibility.focus_override.filter(|node| {
            self.accessibility.is_commands_node(*node)
                && nodes.iter().any(|(current, _)| current == node)
        });
        let focus = requested_focus.unwrap_or(default_focus);
        if self.accessibility.focus_override.is_some() && requested_focus.is_none() {
            self.accessibility.focus_override = Some(default_focus);
        }
        SemanticSnapshot { nodes, focus }
    }

    fn session_accessibility_snapshot(
        &mut self,
        window_width: f32,
        window_height: f32,
    ) -> SemanticSnapshot {
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
        self.accessibility.retain_sessions(&retained);

        let mut nodes = Vec::with_capacity(layout.visible_count * 4 + 10);
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric terminal emulator");
        root.set_bounds(rect(0.0, 0.0, window_width, window_height));
        root.set_children([SESSION_DIALOG]);

        let mut close = Node::new(Role::Button);
        close.set_label(if info.workspaces {
            "Close workspaces"
        } else {
            "Close legacy snapshots"
        });
        close.add_action(Action::Click);
        close.add_action(Action::Focus);
        close.set_bounds(session_rect(layout.close_button));
        nodes.push((SESSION_CLOSE, close));

        let mut dialog_children = vec![SESSION_CLOSE];
        if let (Some(value), Some(rename)) = (&info.rename_text, layout.rename) {
            let mut input = Node::new(Role::TextInput);
            input.set_label("Workspace name");
            input.set_value(value.clone());
            input.set_character_lengths(
                value
                    .chars()
                    .map(|character| character.len_utf8() as u8)
                    .collect::<Vec<_>>(),
            );
            let cursor = value
                .get(..info.rename_cursor)
                .map(str::chars)
                .map(Iterator::count)
                .unwrap_or_else(|| value.chars().count());
            let position = TextPosition {
                node: SESSION_RENAME_INPUT,
                character_index: cursor,
            };
            input.set_text_selection(TextSelection {
                anchor: position,
                focus: position,
            });
            input.add_action(Action::Focus);
            input.add_action(Action::SetValue);
            input.set_bounds(session_rect(rename.input));
            nodes.push((SESSION_RENAME_INPUT, input));

            let mut save = Node::new(Role::Button);
            save.set_label("Save workspace name");
            save.add_action(Action::Click);
            save.add_action(Action::Focus);
            save.set_bounds(session_rect(rename.save_button));
            nodes.push((SESSION_RENAME_SAVE, save));

            let mut cancel = Node::new(Role::Button);
            cancel.set_label("Cancel workspace rename");
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
            cancel.set_bounds(session_rect(rename.cancel_button));
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
                .session_node(SessionSemanticTarget::Group(key.clone()));
            let load_id = self
                .accessibility
                .session_node(SessionSemanticTarget::Load(key.clone()));
            let delete_id = self
                .accessibility
                .session_node(SessionSemanticTarget::Delete(key.clone()));
            let mut load = Node::new(Role::Button);
            load.set_label(format!("Load {}", entry.display_name));
            load.set_description(if info.workspaces {
                "Restore this complete workspace, including its tabs and panes"
            } else {
                "Restore this legacy focused-terminal snapshot in a new shell"
            });
            if rename_active {
                load.set_disabled();
            } else {
                load.add_action(Action::Click);
                load.add_action(Action::Focus);
            }
            load.set_bounds(session_rect(geometry.load_button));
            nodes.push((load_id, load));

            let mut children = vec![load_id];
            if info.workspaces {
                let rename_id = self
                    .accessibility
                    .session_node(SessionSemanticTarget::Rename(key.clone()));
                let mut rename = Node::new(Role::Button);
                rename.set_label(format!("Rename {}", entry.display_name));
                if rename_active {
                    rename.set_disabled();
                } else {
                    rename.add_action(Action::Click);
                    rename.add_action(Action::Focus);
                }
                if let Some(bounds) = geometry.rename_button {
                    rename.set_bounds(session_rect(bounds));
                }
                nodes.push((rename_id, rename));
                children.push(rename_id);
            }

            let mut delete = Node::new(Role::Button);
            delete.set_label(format!("Delete {}", entry.display_name));
            delete.set_description(if info.workspaces {
                "Permanently deletes this saved workspace"
            } else {
                "Permanently deletes this legacy snapshot"
            });
            if rename_active {
                delete.set_disabled();
            } else {
                delete.add_action(Action::Click);
                delete.add_action(Action::Focus);
            }
            delete.set_bounds(session_rect(geometry.delete_button));
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
            group.set_bounds(session_rect(geometry.row));
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
            format!("{} legacy snapshots", info.sessions.len())
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
                "No legacy snapshots"
            });
            empty.set_live(Live::Polite);
            nodes.push((SESSION_EMPTY, empty));
            dialog_children.push(SESSION_EMPTY);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(if info.workspaces {
            "Workspaces"
        } else {
            "Legacy snapshots"
        });
        dialog.set_description(if info.workspaces {
            "Restoring relaunches saved tabs and panes"
        } else {
            "Restoring opens a new shell"
        });
        dialog.set_modal();
        dialog.set_bounds(session_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((SESSION_DIALOG, dialog));
        nodes.push((ROOT, root));

        let default_focus = if info.rename_text.is_some() {
            SESSION_RENAME_INPUT
        } else {
            selected_load.unwrap_or(SESSION_CLOSE)
        };
        let requested_focus = self.accessibility.focus_override.filter(|node| {
            self.accessibility.is_session_node(*node)
                && nodes.iter().any(|(current, _)| current == node)
        });
        let focus = requested_focus.unwrap_or(default_focus);
        if self.accessibility.focus_override.is_some() && requested_focus.is_none() {
            self.accessibility.focus_override = Some(default_focus);
        }
        SemanticSnapshot { nodes, focus }
    }
}

fn session_rect(rectangle: crate::session::panel::SessionPanelRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn shortcuts_rect(rectangle: crate::ui::shortcuts_panel::ShortcutsPanelRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn guardrail_rect(rectangle: crate::terminal::guardrails::GuardrailRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn info_rect(rectangle: crate::ui::info_overlay::InfoOverlayRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn diff_rect(rectangle: crate::ui::diff_review::DiffReviewRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn context_rect(rectangle: crate::ui::context_picker::ContextPickerRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn bounded_semantic_text(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let mut bounded = characters.by_ref().take(max_chars).collect::<String>();
    if characters.next().is_some() {
        bounded.push('…');
    }
    bounded
}

fn custom_command_keys(commands: &[crate::config::CustomCommand]) -> Vec<CustomCommandSemanticKey> {
    let mut occurrences = HashMap::new();
    commands
        .iter()
        .map(|command| {
            let identity = (
                command.name.clone(),
                command.command.clone(),
                command.alias.clone(),
            );
            let occurrence = occurrences.entry(identity.clone()).or_insert(0);
            let key = CustomCommandSemanticKey {
                name: identity.0,
                command: identity.1,
                alias: identity.2,
                occurrence: *occurrence,
            };
            *occurrence += 1;
            key
        })
        .collect()
}

fn custom_command_index(
    commands: &[crate::config::CustomCommand],
    key: &CustomCommandSemanticKey,
) -> Option<usize> {
    commands
        .iter()
        .enumerate()
        .filter(|(_, command)| {
            command.name == key.name && command.command == key.command && command.alias == key.alias
        })
        .nth(key.occurrence)
        .map(|(index, _)| index)
}

fn command_rect(rectangle: crate::ui::commands::panel::CommandsPanelRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn command_form_input(
    id: NodeId,
    label: &str,
    placeholder: &str,
    value: &str,
    cursor: Option<usize>,
    bounds: crate::ui::commands::panel::CommandsPanelRect,
) -> Node {
    let mut input = Node::new(Role::TextInput);
    input.set_label(label);
    input.set_placeholder(placeholder);
    input.set_value(value);
    input.set_character_lengths(
        value
            .chars()
            .map(|character| character.len_utf8() as u8)
            .collect::<Vec<_>>(),
    );
    if let Some(cursor) = cursor {
        let character_index = value
            .get(..cursor)
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or_else(|| value.chars().count());
        let position = TextPosition {
            node: id,
            character_index,
        };
        input.set_text_selection(TextSelection {
            anchor: position,
            focus: position,
        });
    }
    input.add_action(Action::Focus);
    input.set_bounds(command_rect(bounds));
    input
}

fn numeric_setting_value(value: &str) -> Option<f64> {
    value
        .trim_end_matches(|character: char| !character.is_ascii_digit() && character != '.')
        .parse()
        .ok()
}

#[allow(clippy::too_many_arguments)]
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
) {
    let grid = terminal.grid();
    let mut line_ids = Vec::with_capacity(grid.rows());
    let mut lines = Vec::with_capacity(grid.rows());
    for row in 0..grid.rows() {
        let line_id = line_node(pane_id, row);
        let line = accessible_line(grid, row);
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
    node.set_description("Visible terminal text; scrollback is not exposed");
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
                accessible_selection(selection, pane_id, &lines, grid.scroll_offset())
            })
            .or_else(|| {
                if grid.scroll_offset() != 0 {
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
    let mut value = String::new();
    let mut character_lengths = Vec::new();
    let mut column_to_character = Vec::with_capacity(grid.cols() + 1);
    for col in 0..grid.cols() {
        column_to_character.push(character_lengths.len());
        if let Some(cell) = grid.cell_scrolled(row, col) {
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

fn search_rect(rectangle: crate::terminal::search::SearchBarRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn toast_node(id: u64) -> NodeId {
    NodeId(TOAST_BASE.saturating_add(id))
}

fn cellar_rect(rectangle: crate::harvest::CellarRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

fn sprout_rect(rectangle: crate::harvest::SproutMenuRect) -> Rect {
    rect(rectangle.x, rectangle.y, rectangle.width, rectangle.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use termeric_core::CellContent;

    #[test]
    fn vertical_tab_list_semantics_match_the_visible_left_rail() {
        let (orientation, bounds) = tab_list_semantics(true, 1_000.0, 700.0, 32.0, 220.0);
        assert_eq!(orientation, accesskit::Orientation::Vertical);
        assert_eq!(bounds, rect(0.0, 32.0, 220.0, 668.0));

        let (orientation, bounds) = tab_list_semantics(false, 1_000.0, 700.0, 32.0, 0.0);
        assert_eq!(orientation, accesskit::Orientation::Horizontal);
        assert_eq!(bounds, rect(0.0, 0.0, 1_000.0, 32.0));
    }

    #[test]
    fn hidden_terminal_cells_never_enter_the_semantic_value() {
        let mut cell = Cell {
            content: CellContent::Grapheme("secret👁".into()),
            attrs: CellAttrs::HIDDEN,
            ..Cell::default()
        };
        let mut value = String::new();
        let mut lengths = Vec::new();
        append_accessible_cell(&cell, &mut value, &mut lengths);
        assert_eq!(value, " ");
        assert_eq!(lengths, [1]);

        cell.attrs = CellAttrs::empty();
        value.clear();
        lengths.clear();
        append_accessible_cell(&cell, &mut value, &mut lengths);
        assert_eq!(value, "secret👁");
        assert_eq!(lengths.len(), 7);
    }

    #[test]
    fn incremental_updates_only_send_changed_nodes() {
        let mut state = AccessibilityState::default();
        let mut root = Node::new(Role::Window);
        root.set_label("Termeric");
        let first = state.update(SemanticSnapshot {
            nodes: vec![(ROOT, root.clone())],
            focus: ROOT,
        });
        assert!(first.tree.is_some());
        assert_eq!(first.nodes.len(), 1);
        let steady = state.update(SemanticSnapshot {
            nodes: vec![(ROOT, root)],
            focus: ROOT,
        });
        assert!(steady.tree.is_none());
        assert!(steady.nodes.is_empty());
    }

    #[test]
    fn removed_palette_commands_release_their_semantic_identity() {
        let mut state = AccessibilityState::default();
        let removed = state.palette_node("plugin.removed");
        let retained = state.palette_node("core.retained");
        state.retain_palette_commands(&HashSet::from(["core.retained"]));
        assert!(!state.palette_commands.contains_key(&removed));
        assert!(!state.palette_nodes.contains_key("plugin.removed"));
        assert_eq!(state.palette_nodes.get("core.retained"), Some(&retained));
    }

    #[test]
    fn replaced_unicode_results_release_their_semantic_identity() {
        let mut state = AccessibilityState::default();
        let removed = state.unicode_node("unicode-picker.curated.2192");
        let retained = state.unicode_node("unicode-picker.codepoint.3bb");
        state.retain_unicode_commands(&HashSet::from(["unicode-picker.codepoint.3bb"]));
        assert!(!state.unicode_commands.contains_key(&removed));
        assert!(
            !state
                .unicode_nodes
                .contains_key("unicode-picker.curated.2192")
        );
        assert_eq!(
            state.unicode_nodes.get("unicode-picker.codepoint.3bb"),
            Some(&retained)
        );
    }

    #[test]
    fn terminal_tree_exposes_visible_text_cursor_and_selection_without_hidden_text() {
        let mut terminal = termeric_core::Terminal::new(16, 2);
        terminal.process(b"visible\x1b[8msecret\x1b[28m");
        terminal.set_selection(Some(termeric_core::Selection {
            start: termeric_core::Point::new(0, 0),
            end: termeric_core::Point::new(0, 6),
            ty: termeric_core::SelectionType::Simple,
            active: false,
        }));
        let mut nodes = Vec::new();
        append_terminal_nodes(
            &mut nodes,
            pane_node(PaneId(0)),
            PaneId(0),
            &terminal,
            true,
            None,
            true,
            false,
            rect(0.0, 0.0, 160.0, 40.0),
            10.0,
            20.0,
        );
        let semantic_text = nodes
            .iter()
            .filter_map(|(_, node)| node.value())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(semantic_text.contains("visible"));
        assert!(!semantic_text.contains("secret"));
        let terminal_node = nodes
            .iter()
            .find(|(id, _)| *id == pane_node(PaneId(0)))
            .unwrap();
        assert!(
            terminal_node
                .1
                .label()
                .is_some_and(|label| label.contains("read-only input lock active"))
        );
        let selection = terminal_node.1.text_selection().unwrap();
        assert_eq!(selection.anchor.character_index, 0);
        assert_eq!(selection.focus.character_index, 7);
    }

    #[test]
    fn visible_scrollback_selection_is_mapped_into_semantic_viewport_rows() {
        let mut terminal = termeric_core::Terminal::new(8, 2);
        terminal.process(b"old\r\nnew\r\nlive");
        terminal.grid_mut().set_scroll_offset(1);
        terminal.set_selection(Some(termeric_core::Selection {
            start: termeric_core::Point::new(-1, 0),
            end: termeric_core::Point::new(-1, 2),
            ty: termeric_core::SelectionType::Simple,
            active: false,
        }));
        let mut nodes = Vec::new();
        append_terminal_nodes(
            &mut nodes,
            pane_node(PaneId(0)),
            PaneId(0),
            &terminal,
            true,
            None,
            false,
            false,
            rect(0.0, 0.0, 80.0, 40.0),
            10.0,
            20.0,
        );

        let terminal_node = nodes
            .iter()
            .find(|(id, _)| *id == pane_node(PaneId(0)))
            .unwrap();
        let selection = terminal_node.1.text_selection().unwrap();
        assert_eq!(selection.anchor.node, line_node(PaneId(0), 0));
        assert_eq!(selection.anchor.character_index, 0);
        assert_eq!(selection.focus.node, line_node(PaneId(0), 0));
        assert_eq!(selection.focus.character_index, 3);
    }

    #[test]
    fn keyboard_chrome_sequence_names_every_global_control() {
        let app = App::new_headless(Config::default()).unwrap();
        assert_eq!(
            app.chrome_focus_sequence(),
            [
                ChromeFocus::NewTab,
                ChromeFocus::Commands,
                ChromeFocus::Workspaces,
                ChromeFocus::Settings,
            ]
        );
    }

    #[test]
    fn dispatched_primary_press_returns_chrome_focus_to_terminal() {
        let mut app = App::new_headless(Config::default()).unwrap();
        for focus in [
            ChromeFocus::Tab(19),
            ChromeFocus::CloseTab(19),
            ChromeFocus::PreviousTab,
            ChromeFocus::NextTab,
            ChromeFocus::NewTab,
            ChromeFocus::Commands,
            ChromeFocus::Workspaces,
            ChromeFocus::Settings,
        ] {
            app.set_chrome_focus(focus);
            app.finish_pointer_chrome_focus(MouseButton::Left, ElementState::Pressed);
            assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Terminal);
            assert_eq!(app.accessibility.focus_override, None);
            assert_eq!(app.chrome_focus_render_target(), None);
        }
    }

    #[test]
    fn pointer_release_and_other_buttons_do_not_change_chrome_focus() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.set_chrome_focus(ChromeFocus::Tab(19));
        for (button, state) in [
            (MouseButton::Left, ElementState::Released),
            (MouseButton::Right, ElementState::Pressed),
            (MouseButton::Middle, ElementState::Pressed),
        ] {
            app.finish_pointer_chrome_focus(button, state);
            assert_eq!(app.accessibility.keyboard_focus, ChromeFocus::Tab(19));
            assert_eq!(app.accessibility.focus_override, Some(tab_node(19)));
        }
        // An already-terminal focus must not clear a text surface's AT focus
        // override merely because the normal pointer path finished.
        app.accessibility.keyboard_focus = ChromeFocus::Terminal;
        app.accessibility.focus_override = Some(SEARCH_INPUT);
        app.search.start();
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), Some((0, 5))));
        let preedit = app.ime.preedit().cloned();
        assert!(preedit.is_some());
        app.finish_pointer_chrome_focus(MouseButton::Left, ElementState::Pressed);
        assert_eq!(app.accessibility.focus_override, Some(SEARCH_INPUT));
        assert_eq!(app.ime.preedit(), preedit.as_ref());
    }

    #[test]
    fn command_palette_is_one_modal_search_and_result_tree_with_shared_geometry() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        std::thread::sleep(std::time::Duration::from_millis(250));
        app.command_palette.update_animation();
        let info = app.command_palette.build_info();
        let layout = app.command_palette_accessibility_layout(&info);
        let snapshot = app.accessibility_snapshot();
        let assert_rect_close = |actual: Option<Rect>, expected: Rect| {
            let actual = actual.unwrap();
            for (actual, expected) in [
                (actual.x0, expected.x0),
                (actual.y0, expected.y0),
                (actual.x1, expected.x1),
                (actual.y1, expected.y1),
            ] {
                assert!((actual - expected).abs() < 0.01, "{actual} != {expected}");
            }
        };

        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        assert_eq!(node(ROOT).children(), &[PALETTE_DIALOG]);
        assert_eq!(node(PALETTE_DIALOG).role(), Role::Dialog);
        assert!(node(PALETTE_DIALOG).is_modal());
        assert_eq!(node(PALETTE_INPUT).role(), Role::SearchInput);
        assert_eq!(node(PALETTE_INPUT).controls(), &[PALETTE_LIST]);
        assert!(node(PALETTE_INPUT).supports_action(Action::SetValue));
        assert_eq!(snapshot.focus, PALETTE_INPUT);
        assert_rect_close(
            node(PALETTE_INPUT).bounds(),
            rect(
                layout.panel_x + layout.padding,
                layout.panel_y + layout.padding,
                layout.panel_width - layout.padding * 2.0,
                layout.input_height,
            ),
        );

        let selected = node(PALETTE_INPUT).active_descendant().unwrap();
        assert_eq!(node(selected).role(), Role::ListBoxOption);
        assert_eq!(node(selected).is_selected(), Some(true));
        assert!(node(selected).supports_action(Action::Click));
        assert_rect_close(
            node(selected).bounds(),
            rect(
                layout.panel_x + layout.padding * 0.5,
                layout.list_y,
                layout.panel_width - layout.padding,
                layout.row_height,
            ),
        );
    }

    #[test]
    fn terminal_search_exposes_editable_status_controls_and_shared_geometry() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let terminal = termeric_core::Terminal::new(40, 2);
        app.search.start();
        assert!(app.search.set_query("needle", terminal.grid()));
        app.search_cursor_pos = "needle".len();
        let info = app.search.build_info(app.search_cursor_pos);
        let layout = app.search_accessibility_layout(&info, 1.0);
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };

        assert!(node(ROOT).children().contains(&SEARCH_TOOLBAR));
        assert_eq!(snapshot.focus, SEARCH_INPUT);
        assert_eq!(node(SEARCH_INPUT).role(), Role::SearchInput);
        assert_eq!(node(SEARCH_INPUT).value(), Some("needle"));
        assert!(node(SEARCH_INPUT).supports_action(Action::SetValue));
        assert_eq!(node(SEARCH_STATUS).role(), Role::Status);
        assert_eq!(node(SEARCH_STATUS).live(), Some(Live::Polite));
        assert_eq!(node(SEARCH_FILTER).toggled(), Some(Toggled::False));
        assert!(node(SEARCH_CLOSE).supports_action(Action::Click));
        assert_eq!(node(SEARCH_INPUT).bounds(), Some(search_rect(layout.input)));
        assert_eq!(node(SEARCH_REGEX).bounds(), Some(search_rect(layout.regex)));

        app.handle_accessibility_action(ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: SEARCH_REGEX,
            data: None,
        });
        assert_eq!(app.accessibility_snapshot().focus, SEARCH_REGEX);
        app.search.confirm();
        assert_ne!(app.accessibility_snapshot().focus, SEARCH_INPUT);

        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SEARCH_CLOSE,
            data: None,
        });
        assert!(!app.search_is_visible());
        assert!(
            !app.accessibility_snapshot()
                .nodes
                .iter()
                .any(|(id, _)| *id == SEARCH_TOOLBAR)
        );
    }

    #[test]
    fn toasts_are_stable_bounded_live_regions_even_above_modals() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.toasts.error("failure");
        app.command_palette.open();
        let base = app.accessibility_snapshot();
        let snapshot = app.with_toast_accessibility(base);
        let toast = toast_node(1);
        let root = &snapshot.nodes.iter().find(|(id, _)| *id == ROOT).unwrap().1;
        assert!(root.children().contains(&PALETTE_DIALOG));
        assert!(root.children().contains(&toast));
        let node = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == toast)
            .unwrap()
            .1;
        assert_eq!(node.role(), Role::Status);
        assert_eq!(node.live(), Some(Live::Assertive));
        assert_eq!(node.label(), Some("Error: failure"));

        let second_base = app.accessibility_snapshot();
        let second = app.with_toast_accessibility(second_base);
        assert!(second.nodes.iter().any(|(id, _)| *id == toast));
    }

    fn open_harvest_cellar_fixture(app: &mut App, count: usize) {
        let mut harvest = crate::harvest::HarvestMode::with_capacity(count.max(1));
        harvest.cellar.animation = crate::ui::animation::PanelAnimation::with_duration(0.0);
        for index in 0..count {
            let mut entry = crate::harvest::CommandEntry::new_for_source(
                crate::harvest::HarvestSource {
                    tab_id: 7,
                    pane_id: 2,
                },
                index as i32,
                Some(if index % 3 == 0 { 1 } else { 0 }),
            );
            entry.command = format!("cargo test fixture-{index}");
            harvest.cellar.add(entry);
        }
        harvest.cellar.toggle_expanded();
        app.harvest = Some(harvest);
    }

    #[test]
    fn root_cellar_is_bounded_modal_command_history_with_shared_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        open_harvest_cellar_fixture(&mut app, 30);
        let layout = app.cellar_accessibility_layout();
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        assert_eq!(node(ROOT).children(), &[CELLAR_DIALOG]);
        assert!(node(CELLAR_DIALOG).is_modal());
        assert_eq!(node(CELLAR_LIST).role(), Role::ListBox);
        assert!(node(CELLAR_LIST).supports_action(Action::ScrollUp));
        assert_eq!(
            node(CELLAR_DIALOG).bounds(),
            Some(cellar_rect(layout.panel))
        );
        assert_eq!(node(CELLAR_CLOSE).bounds(), Some(cellar_rect(layout.close)));
        let focused = snapshot.focus;
        assert_eq!(node(focused).role(), Role::ListBoxOption);
        assert!(node(focused).label().unwrap().contains("fixture-29"));
        assert!(
            node(focused)
                .description()
                .unwrap()
                .contains("tab 7, pane 3")
        );

        app.handle_accessibility_action(ActionRequest {
            action: Action::ScrollUp,
            target_tree: TreeId::ROOT,
            target_node: CELLAR_LIST,
            data: None,
        });
        let scrolled = app.accessibility_snapshot();
        assert_ne!(scrolled.focus, focused);

        let selected_id = app.harvest.as_ref().unwrap().cellar.selected_id().unwrap();
        let selected_node = *app
            .accessibility
            .harvest_nodes
            .get(&HarvestSemanticTarget::CellarEntry(selected_id))
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: selected_node,
            data: None,
        });
        assert!(!app.harvest.as_ref().unwrap().cellar.is_expanded());
        assert!(app.harvest.as_ref().unwrap().sprout_menu.is_some());
        assert_eq!(
            app.accessibility_snapshot()
                .nodes
                .iter()
                .find(|(id, _)| *id == ROOT)
                .unwrap()
                .1
                .children(),
            &[SPROUT_DIALOG]
        );
    }

    #[test]
    fn root_cellar_announces_retention_eviction_and_clipped_output() {
        let mut app = App::new_headless(Config::default()).unwrap();
        open_harvest_cellar_fixture(&mut app, 2);
        let mut clipped = crate::harvest::CommandEntry::new_for_source(
            crate::harvest::HarvestSource {
                tab_id: 7,
                pane_id: 2,
            },
            3,
            Some(0),
        );
        clipped.command = "cargo test clipped".into();
        clipped.output_truncated = true;
        app.harvest.as_mut().unwrap().cellar.add(clipped);

        let snapshot = app.accessibility_snapshot();
        let status = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == CELLAR_STATUS)
            .unwrap()
            .1;
        assert!(status.label().unwrap().contains("1 older commands evicted"));
        let selected = snapshot.focus;
        let selected = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == selected)
            .unwrap()
            .1;
        assert!(
            selected
                .description()
                .unwrap()
                .contains("truncated at 64 KiB")
        );
    }

    #[test]
    fn sprout_semantics_are_request_scoped_and_never_invoke_explain_in_tests() {
        let mut app = App::new_headless(Config::default()).unwrap();
        open_harvest_cellar_fixture(&mut app, 1);
        let entry = app
            .harvest
            .as_ref()
            .unwrap()
            .cellar
            .latest()
            .unwrap()
            .clone();
        let availability = app.sprout_availability_for_entry(&entry);
        app.harvest.as_mut().unwrap().show_sprout_menu(
            entry.row.max(0) as usize,
            0,
            Some(entry.clone()),
            availability,
        );
        let first = app.accessibility_snapshot();
        let first_request = app
            .harvest
            .as_ref()
            .unwrap()
            .sprout_menu
            .as_ref()
            .unwrap()
            .request_id;
        let old_copy = *app
            .accessibility
            .harvest_nodes
            .get(&HarvestSemanticTarget::SproutAction {
                request_id: first_request,
                action: crate::harvest::sprouts::SproutAction::CopyCommand,
            })
            .unwrap();
        let explain = *app
            .accessibility
            .harvest_nodes
            .get(&HarvestSemanticTarget::SproutAction {
                request_id: first_request,
                action: crate::harvest::sprouts::SproutAction::Explain,
            })
            .unwrap();
        let search = *app
            .accessibility
            .harvest_nodes
            .get(&HarvestSemanticTarget::SproutAction {
                request_id: first_request,
                action: crate::harvest::sprouts::SproutAction::SearchOutput,
            })
            .unwrap();
        let node = |id| {
            &first
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        assert_eq!(node(ROOT).children(), &[SPROUT_DIALOG]);
        assert!(node(SPROUT_DIALOG).is_modal());
        assert_eq!(node(SPROUT_MENU).role(), Role::Menu);
        assert!(node(explain).description().unwrap().contains("AI plugin"));
        assert!(node(old_copy).supports_action(Action::Click));
        assert!(node(search).is_disabled());
        assert!(!node(search).supports_action(Action::Click));
        assert!(node(search).label().unwrap().contains("not retained"));
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: search,
            data: None,
        });
        assert_eq!(
            app.harvest
                .as_ref()
                .unwrap()
                .sprout_menu
                .as_ref()
                .unwrap()
                .request_id,
            first_request
        );

        let availability = app.sprout_availability_for_entry(&entry);
        app.harvest.as_mut().unwrap().show_sprout_menu(
            entry.row.max(0) as usize,
            0,
            Some(entry),
            availability,
        );
        app.accessibility_snapshot();
        let replacement_request = app
            .harvest
            .as_ref()
            .unwrap()
            .sprout_menu
            .as_ref()
            .unwrap()
            .request_id;
        assert_ne!(replacement_request, first_request);
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: old_copy,
            data: None,
        });
        assert_eq!(
            app.harvest
                .as_ref()
                .unwrap()
                .sprout_menu
                .as_ref()
                .unwrap()
                .request_id,
            replacement_request
        );
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });
        assert!(!app.settings_panel.is_active());

        let copy = *app
            .accessibility
            .harvest_nodes
            .get(&HarvestSemanticTarget::SproutAction {
                request_id: replacement_request,
                action: crate::harvest::sprouts::SproutAction::CopyCommand,
            })
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: copy,
            data: None,
        });
        assert!(app.harvest.as_ref().unwrap().sprout_menu.is_none());
    }

    #[test]
    fn palette_accessibility_action_executes_the_announced_stable_command_only() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        for character in "toggle fullscreen".chars() {
            app.command_palette.handle_char(character);
        }
        let first = app.accessibility_snapshot();
        let command_node = *app
            .accessibility
            .palette_nodes
            .get("core.toggle-fullscreen")
            .unwrap();
        assert!(first.nodes.iter().any(|(id, _)| *id == command_node));

        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: command_node,
            data: None,
        });
        assert!(app.fullscreen_requested);
        assert!(!app.command_palette.is_active());

        app.command_palette.open();
        for character in "toggle fullscreen".chars() {
            app.command_palette.handle_char(character);
        }
        let second = app.accessibility_snapshot();
        assert!(second.nodes.iter().any(|(id, _)| *id == command_node));
        assert_eq!(
            app.accessibility
                .palette_nodes
                .get("core.toggle-fullscreen"),
            Some(&command_node),
            "semantic command identity must survive close, filter, and reopen"
        );
    }

    #[test]
    fn modal_palette_consumes_stale_background_accessibility_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });
        assert!(app.command_palette.is_active());
        assert!(!app.settings_panel.is_active());
    }

    #[test]
    fn palette_semantic_filter_is_bounded_utf8_and_never_executes_a_command() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.command_palette.open();
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: PALETTE_INPUT,
            data: Some(ActionData::Value("toggle fullscreen 🫚".into())),
        });
        assert_eq!(
            app.command_palette.build_info().input,
            "toggle fullscreen 🫚"
        );
        assert!(app.command_palette.is_active());
        assert!(!app.fullscreen_requested);

        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: PALETTE_INPUT,
            data: Some(ActionData::Value("bad\nfilter".into())),
        });
        assert_eq!(
            app.command_palette.build_info().input,
            "toggle fullscreen 🫚"
        );
        assert!(!app.fullscreen_requested);
    }

    #[test]
    fn settings_is_one_modal_tree_with_typed_controls_and_shared_geometry() {
        use crate::ui::settings::SettingsSection;

        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        let info = app
            .settings_panel
            .build_info(&app.config, &app.shortcut_registry);
        let layout = app.settings_accessibility_layout(&info);
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };

        assert_eq!(node(ROOT).children(), &[SETTINGS_DIALOG]);
        assert_eq!(node(SETTINGS_DIALOG).role(), Role::Dialog);
        assert!(node(SETTINGS_DIALOG).is_modal());
        assert_eq!(node(SETTINGS_INPUT).role(), Role::SearchInput);
        assert_eq!(node(SETTINGS_INPUT).controls(), &[SETTINGS_LIST]);
        assert_eq!(node(SETTINGS_LIST).role(), Role::List);
        assert_eq!(snapshot.focus, SETTINGS_INPUT);

        let size = app
            .accessibility
            .settings_nodes
            .get(&SettingsSemanticTarget::Setting {
                section: SettingsSection::Font,
                label: "Size",
            })
            .copied()
            .unwrap();
        assert_eq!(node(size).role(), Role::SpinButton);
        assert!(node(size).supports_action(Action::Increment));
        assert_eq!(node(SETTINGS_INPUT).active_descendant(), Some(size));
        let normal_weight = app
            .accessibility
            .settings_nodes
            .get(&SettingsSemanticTarget::Setting {
                section: SettingsSection::Font,
                label: "Normal Weight",
            })
            .copied()
            .unwrap();
        assert_eq!(node(normal_weight).role(), Role::SpinButton);
        assert_eq!(node(normal_weight).numeric_value(), Some(400.0));
        assert!(node(normal_weight).supports_action(Action::Increment));
        let actual = node(size).bounds().unwrap();
        let dialog = node(SETTINGS_DIALOG).bounds().unwrap();
        let expected = rect(
            dialog.x0 as f32 + layout.padding * 0.5,
            dialog.y0 as f32 + (layout.list_y - layout.panel_y) + layout.row_height,
            layout.panel_width - layout.padding,
            layout.row_height,
        );
        for (actual, expected) in [
            (actual.x0, expected.x0),
            (actual.y0, expected.y0),
            (actual.x1, expected.x1),
            (actual.y1, expected.y1),
        ] {
            assert!((actual - expected).abs() < 0.05, "{actual} != {expected}");
        }
    }

    #[test]
    fn settings_accessibility_actions_use_the_existing_validated_action_path() {
        use crate::ui::settings::SettingsSection;

        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        assert!(app.settings_panel.accessibility_focus_setting(
            SettingsSection::Window,
            "Opacity",
            &app.config,
            &app.shortcut_registry,
        ));
        app.accessibility_snapshot();
        let opacity = *app
            .accessibility
            .settings_nodes
            .get(&SettingsSemanticTarget::Setting {
                section: SettingsSection::Window,
                label: "Opacity",
            })
            .unwrap();
        let before = app.config.window.opacity;
        app.handle_accessibility_action(ActionRequest {
            action: Action::Decrement,
            target_tree: TreeId::ROOT,
            target_node: opacity,
            data: None,
        });
        assert!((app.config.window.opacity - (before - 0.05)).abs() < f32::EPSILON);

        app.settings_panel.open_to_section(SettingsSection::Font);
        assert!(app.settings_panel.accessibility_focus_setting(
            SettingsSection::Font,
            "Normal Weight",
            &app.config,
            &app.shortcut_registry,
        ));
        app.accessibility_snapshot();
        let weight = *app
            .accessibility
            .settings_nodes
            .get(&SettingsSemanticTarget::Setting {
                section: SettingsSection::Font,
                label: "Normal Weight",
            })
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Increment,
            target_tree: TreeId::ROOT,
            target_node: weight,
            data: None,
        });
        assert_eq!(app.config.font.weight, 425);

        app.settings_panel
            .open_to_section(SettingsSection::Sessions);
        app.accessibility_snapshot();
        let auto_save = *app
            .accessibility
            .settings_nodes
            .get(&SettingsSemanticTarget::Setting {
                section: SettingsSection::Sessions,
                label: "Auto Save Workspace",
            })
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Expand,
            target_tree: TreeId::ROOT,
            target_node: auto_save,
            data: None,
        });
        app.accessibility_snapshot();
        let expected = !app.config.session.auto_save;
        let value = if expected { "On" } else { "Off" };
        let option = *app
            .accessibility
            .settings_nodes
            .get(&SettingsSemanticTarget::Option {
                section: SettingsSection::Sessions,
                label: "Auto Save Workspace",
                value: value.into(),
            })
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: option,
            data: None,
        });
        assert_eq!(app.config.session.auto_save, expected);
        assert!(!app.settings_panel.is_dropdown_open());
    }

    #[test]
    fn runtime_plugin_manager_setting_is_an_accessible_button() {
        use crate::ui::settings::SettingsSection;

        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        assert!(app.settings_panel.accessibility_focus_setting(
            SettingsSection::Plugins,
            "Manage Third-party Plugins",
            &app.config,
            &app.shortcut_registry,
        ));
        let snapshot = app.accessibility_snapshot();
        let manager = *app
            .accessibility
            .settings_nodes
            .get(&SettingsSemanticTarget::Setting {
                section: SettingsSection::Plugins,
                label: "Manage Third-party Plugins",
            })
            .unwrap();
        let node = &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == manager)
            .unwrap()
            .1;
        assert_eq!(node.role(), Role::Button);
        assert!(node.supports_action(Action::Click));

        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: manager,
            data: None,
        });
        assert!(!app.settings_panel.is_active());
        assert!(app.info_overlay.is_awaiting_current());
        assert_eq!(app.info_overlay.title(), "Third-party Plugin Manager");
    }

    #[test]
    fn font_range_manager_setting_is_an_accessible_button() {
        use crate::ui::settings::SettingsSection;

        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        assert!(app.settings_panel.accessibility_focus_setting(
            SettingsSection::Font,
            "Range Overrides",
            &app.config,
            &app.shortcut_registry,
        ));
        let snapshot = app.accessibility_snapshot();
        let manager = *app
            .accessibility
            .settings_nodes
            .get(&SettingsSemanticTarget::Setting {
                section: SettingsSection::Font,
                label: "Range Overrides",
            })
            .unwrap();
        let node = &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == manager)
            .unwrap()
            .1;
        assert_eq!(node.role(), Role::Button);
        assert!(node.supports_action(Action::Click));

        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: manager,
            data: None,
        });
        assert!(!app.settings_panel.is_active());
        assert!(app.info_overlay.is_awaiting_current());
        assert_eq!(app.info_overlay.title(), "Font Range Overrides");

        let manager_request = app.info_overlay.request_id();
        app.accessibility_snapshot();
        let inspect = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Choice {
                request_id: manager_request,
                id: "inspect".into(),
            })
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: inspect,
            data: None,
        });
        assert_eq!(app.info_overlay.title(), "Inspect Glyph Coverage");

        let input_request = app.info_overlay.request_id();
        let snapshot = app.accessibility_snapshot();
        let input = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Input(input_request))
            .unwrap();
        let submit = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Dismiss(input_request))
            .unwrap();
        let input_node = &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == input)
            .unwrap()
            .1;
        assert_eq!(input_node.role(), Role::TextInput);
        assert_eq!(input_node.label(), Some("Text"));
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: input,
            data: Some(ActionData::Value("U+0634".into())),
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: submit,
            data: None,
        });
        assert_eq!(app.info_overlay.title(), "Glyph Coverage Report");
    }

    #[test]
    fn font_feature_manager_is_accessible_and_uses_the_shared_mutation_path() {
        use crate::ui::settings::SettingsSection;

        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        assert!(app.settings_panel.accessibility_focus_setting(
            SettingsSection::Font,
            "OpenType Features",
            &app.config,
            &app.shortcut_registry,
        ));
        let snapshot = app.accessibility_snapshot();
        let manager = *app
            .accessibility
            .settings_nodes
            .get(&SettingsSemanticTarget::Setting {
                section: SettingsSection::Font,
                label: "OpenType Features",
            })
            .unwrap();
        let node = &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == manager)
            .unwrap()
            .1;
        assert_eq!(node.role(), Role::Button);
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: manager,
            data: None,
        });
        assert_eq!(app.info_overlay.title(), "OpenType Feature Overrides");

        let manager_request = app.info_overlay.request_id();
        app.accessibility_snapshot();
        let add = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Choice {
                request_id: manager_request,
                id: "add".into(),
            })
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: add,
            data: None,
        });

        let input_request = app.info_overlay.request_id();
        let snapshot = app.accessibility_snapshot();
        let input = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Input(input_request))
            .unwrap();
        let submit = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Dismiss(input_request))
            .unwrap();
        let input_node = &snapshot
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == input)
            .unwrap()
            .1;
        assert_eq!(input_node.role(), Role::TextInput);
        assert_eq!(input_node.label(), Some("Feature"));
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: input,
            data: Some(ActionData::Value("ss01 = 1".into())),
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: submit,
            data: None,
        });
        assert_eq!(app.config.font.features.get("ss01"), Some(&1));
        assert_eq!(app.info_overlay.title(), "OpenType Feature Overrides");
    }

    #[test]
    fn settings_filter_semantic_replacement_updates_text_caret_and_results_only() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        let original = toml::to_string(&app.config).unwrap();
        let snapshot = app.accessibility_snapshot();
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == SETTINGS_INPUT)
            .unwrap()
            .1;
        assert!(input.supports_action(Action::SetValue));
        assert!(!input.is_read_only());
        for text in ["size", "e\u{301}👩\u{200d}💻", "", &"🫚".repeat(256)] {
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            app.handle_accessibility_action(ActionRequest {
                action: Action::SetValue,
                target_tree: TreeId::ROOT,
                target_node: SETTINGS_INPUT,
                data: Some(ActionData::Value(text.into())),
            });
            let snapshot = app.accessibility_snapshot();
            let input = &snapshot
                .nodes
                .iter()
                .find(|(id, _)| *id == SETTINGS_INPUT)
                .unwrap()
                .1;
            assert_eq!(input.value(), Some(text));
            assert_eq!(
                input.text_selection().unwrap().focus.character_index,
                text.chars().count()
            );
            assert_eq!(
                input
                    .character_lengths()
                    .iter()
                    .map(|size| usize::from(*size))
                    .sum::<usize>(),
                text.len()
            );
            assert_eq!(snapshot.focus, SETTINGS_INPUT);
            assert!(app.ime.preedit().is_none());
            assert!(app.redraw_scheduler.has_pending());
            assert_eq!(toml::to_string(&app.config).unwrap(), original);
            if text == "size" {
                assert!(snapshot.nodes.iter().any(
                    |(_, node)| node.label() == Some("Size") && node.role() == Role::SpinButton
                ));
            }
        }
    }

    #[test]
    fn settings_filter_semantic_rejection_preserves_state_and_dropdown_focus() {
        use crate::ui::settings::SettingsKey;
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        assert!(app.settings_panel.accessibility_set_filter("family"));
        let original = toml::to_string(&app.config).unwrap();
        app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
        for data in [
            None,
            Some(ActionData::Value("bad\nquery".into())),
            Some(ActionData::Value("x".repeat(257).into())),
        ] {
            app.handle_accessibility_action(ActionRequest {
                action: Action::SetValue,
                target_tree: TreeId::ROOT,
                target_node: SETTINGS_INPUT,
                data,
            });
            assert!(app.ime.preedit().is_some());
            assert_eq!(
                app.settings_panel
                    .build_info(&app.config, &app.shortcut_registry)
                    .filter,
                "family"
            );
        }
        app.accessibility.focus_override = Some(SETTINGS_INPUT);
        app.settings_panel
            .handle_key(SettingsKey::Right, &app.config, &app.shortcut_registry);
        assert!(app.settings_panel.is_dropdown_open());
        let snapshot = app.accessibility_snapshot();
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == SETTINGS_INPUT)
            .unwrap()
            .1;
        assert!(input.is_read_only());
        assert!(!input.supports_action(Action::SetValue));
        assert!(!input.supports_action(Action::Focus));
        let focus = snapshot.focus;
        assert_ne!(focus, SETTINGS_INPUT);
        assert_eq!(
            snapshot
                .nodes
                .iter()
                .find(|(id, _)| *id == focus)
                .unwrap()
                .1
                .is_expanded(),
            Some(true)
        );
        for action in [Action::Focus, Action::SetValue] {
            app.handle_accessibility_action(ActionRequest {
                action,
                target_tree: TreeId::ROOT,
                target_node: SETTINGS_INPUT,
                data: Some(ActionData::Value("size".into())),
            });
            assert_eq!(app.accessibility_snapshot().focus, focus);
            assert_eq!(
                app.settings_panel
                    .build_info(&app.config, &app.shortcut_registry)
                    .filter,
                "family"
            );
        }
        app.settings_panel
            .handle_key(SettingsKey::Escape, &app.config, &app.shortcut_registry);
        let snapshot = app.accessibility_snapshot();
        assert!(
            snapshot
                .nodes
                .iter()
                .find(|(id, _)| *id == SETTINGS_INPUT)
                .unwrap()
                .1
                .supports_action(Action::SetValue)
        );
        assert_eq!(toml::to_string(&app.config).unwrap(), original);
    }

    #[test]
    fn settings_filter_semantic_requests_cannot_edit_hidden_or_covered_fields() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        assert!(app.settings_panel.accessibility_set_filter("size"));
        app.command_palette.open();
        for covered in [true, false] {
            if !covered {
                app.command_palette.close();
                app.settings_panel.close();
            }
            app.handle_accessibility_action(ActionRequest {
                action: Action::SetValue,
                target_tree: TreeId::ROOT,
                target_node: SETTINGS_INPUT,
                data: Some(ActionData::Value("hidden".into())),
            });
            assert_eq!(
                app.settings_panel
                    .build_info(&app.config, &app.shortcut_registry)
                    .filter,
                "size"
            );
            assert!(app.command_palette.build_info().input.is_empty());
        }
    }

    #[test]
    fn settings_filter_reports_utf8_cursor_and_empty_results() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        assert!(app.settings_panel.insert_filter_text("🫚absent"));
        let snapshot = app.accessibility_snapshot();
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == SETTINGS_INPUT)
            .unwrap()
            .1;
        assert_eq!(input.character_lengths()[0], 4);
        assert_eq!(
            input.text_selection().unwrap().focus.character_index,
            "🫚absent".chars().count()
        );
        assert!(snapshot.nodes.iter().any(
            |(id, node)| *id == SETTINGS_EMPTY && node.label() == Some("No matching settings")
        ));
    }

    #[test]
    fn modal_settings_consumes_stale_background_accessibility_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.settings_panel.toggle();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: COMMANDS,
            data: None,
        });
        assert!(app.settings_panel.is_active());
        assert!(!app.command_palette.is_active());
    }

    #[test]
    fn custom_commands_is_one_modal_tree_with_shared_action_geometry() {
        use crate::config::CustomCommand;

        let mut app = App::new_headless(Config::default()).unwrap();
        app.config.commands = vec![CustomCommand {
            name: "Inspect logs".into(),
            command: "printf local-fixture".into(),
            alias: Some("logs".into()),
        }];
        // Geometry equality is the subject of this test. Freeze the optional
        // slide transition so the independently built expected layout and the
        // accessibility snapshot sample the same panel position.
        app.commands_panel.set_reduced_motion(true);
        app.commands_panel.toggle();
        let info = app.commands_panel.build_info(&app.config.commands);
        let layout = app.commands_accessibility_layout(&info);
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        assert_eq!(node(ROOT).children(), &[COMMANDS_DIALOG]);
        assert!(node(COMMANDS_DIALOG).is_modal());
        assert_eq!(node(COMMANDS_NEW).role(), Role::Button);
        assert_eq!(node(COMMANDS_CLOSE).role(), Role::Button);
        assert_eq!(node(COMMANDS_LIST).role(), Role::List);
        assert_eq!(snapshot.focus, COMMANDS_NEW);

        let key = custom_command_keys(&app.config.commands).remove(0);
        let group = *app
            .accessibility
            .commands_nodes
            .get(&CommandsSemanticTarget::Group(key.clone()))
            .unwrap();
        let run = *app
            .accessibility
            .commands_nodes
            .get(&CommandsSemanticTarget::Run(key.clone()))
            .unwrap();
        let delete = *app
            .accessibility
            .commands_nodes
            .get(&CommandsSemanticTarget::Delete(key))
            .unwrap();
        assert_eq!(node(group).role(), Role::Group);
        assert_eq!(node(group).children(), &[run, delete]);
        assert_eq!(node(run).label(), Some("Run Inspect logs"));
        assert_eq!(node(delete).label(), Some("Delete Inspect logs"));
        assert!(node(run).supports_action(Action::Click));

        let assert_rect_close = |actual: Option<Rect>, expected: Rect| {
            let actual = actual.unwrap();
            for (actual, expected) in [
                (actual.x0, expected.x0),
                (actual.y0, expected.y0),
                (actual.x1, expected.x1),
                (actual.y1, expected.y1),
            ] {
                assert!((actual - expected).abs() < 0.05, "{actual} != {expected}");
            }
        };
        assert_rect_close(node(run).bounds(), command_rect(layout.command_run(0)));
        assert_rect_close(
            node(delete).bounds(),
            command_rect(layout.command_delete(0)),
        );
    }

    #[test]
    fn custom_command_form_accepts_bounded_utf8_values_without_executing_them() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.toggle();
        app.accessibility_snapshot();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: COMMANDS_NEW,
            data: None,
        });
        let form = app.accessibility_snapshot();
        assert_eq!(form.focus, COMMANDS_NAME);
        assert!(form.nodes.iter().any(|(id, node)| {
            *id == COMMANDS_COMMAND
                && node.role() == Role::TextInput
                && node.supports_action(Action::SetValue)
        }));

        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: COMMANDS_NAME,
            data: Some(ActionData::Value("Deploy 🫚".into())),
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: COMMANDS_COMMAND,
            data: Some(ActionData::Value("printf inert-fixture".into())),
        });
        let info = app.commands_panel.build_info(&app.config.commands);
        assert_eq!(info.form_name, "Deploy 🫚");
        assert_eq!(info.form_command, "printf inert-fixture");

        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: COMMANDS_COMMAND,
            data: Some(ActionData::Value("rejected\nsecond line".into())),
        });
        assert_eq!(
            app.commands_panel
                .build_info(&app.config.commands)
                .form_command,
            "printf inert-fixture"
        );
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: COMMANDS_CANCEL,
            data: None,
        });
        assert!(!app.commands_panel.is_editing());
        assert!(app.config.commands.is_empty());
    }

    #[test]
    fn stale_custom_command_actions_cannot_retarget_another_command() {
        use crate::config::CustomCommand;

        let mut app = App::new_headless(Config::default()).unwrap();
        app.config.commands = vec![
            CustomCommand {
                name: "First".into(),
                command: "printf first".into(),
                alias: None,
            },
            CustomCommand {
                name: "Second".into(),
                command: "printf second".into(),
                alias: None,
            },
        ];
        app.commands_panel.toggle();
        app.accessibility_snapshot();
        let first_key = custom_command_keys(&app.config.commands).remove(0);
        let stale_delete = *app
            .accessibility
            .commands_nodes
            .get(&CommandsSemanticTarget::Delete(first_key))
            .unwrap();
        app.config.commands.remove(0);
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: stale_delete,
            data: None,
        });
        assert_eq!(app.config.commands.len(), 1);
        assert_eq!(app.config.commands[0].name, "Second");

        app.accessibility_snapshot();
        assert!(
            !app.accessibility
                .commands_targets
                .contains_key(&stale_delete)
        );
    }

    #[test]
    fn modal_custom_commands_consumes_stale_background_accessibility_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.commands_panel.toggle();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });
        assert!(app.commands_panel.is_open());
        assert!(!app.settings_panel.is_active());
    }

    fn test_session_entries(count: usize) -> Vec<crate::session::panel::SessionEntry> {
        (0..count)
            .map(|index| crate::session::panel::SessionEntry {
                id: format!("workspace-{index}"),
                display_name: format!("Workspace {index}"),
                subtitle: format!("{} tabs", index + 1),
                last_modified: "local fixture".into(),
            })
            .collect()
    }

    #[test]
    fn workspace_manager_is_modal_bounded_and_uses_shared_action_geometry() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.session_panel
            .set_test_entries(test_session_entries(20), true);
        let preview = app.session_panel.build_info();
        let layout = app.session_accessibility_layout(&preview);
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        assert_eq!(node(ROOT).children(), &[SESSION_DIALOG]);
        assert_eq!(node(SESSION_DIALOG).role(), Role::Dialog);
        assert!(node(SESSION_DIALOG).is_modal());
        assert_eq!(node(SESSION_LIST).role(), Role::List);

        let key = SessionSemanticKey {
            id: "workspace-0".into(),
            workspaces: true,
        };
        let group = *app
            .accessibility
            .session_nodes
            .get(&SessionSemanticTarget::Group(key.clone()))
            .unwrap();
        let load = *app
            .accessibility
            .session_nodes
            .get(&SessionSemanticTarget::Load(key.clone()))
            .unwrap();
        let rename = *app
            .accessibility
            .session_nodes
            .get(&SessionSemanticTarget::Rename(key.clone()))
            .unwrap();
        let delete = *app
            .accessibility
            .session_nodes
            .get(&SessionSemanticTarget::Delete(key))
            .unwrap();
        assert_eq!(node(group).children(), &[load, rename, delete]);
        assert_eq!(node(SESSION_LIST).active_descendant(), Some(group));
        assert_eq!(snapshot.focus, load);
        assert!(node(load).supports_action(Action::Click));

        let actual = node(load).bounds().unwrap();
        let button = layout.entry(0).load_button;
        let dialog = node(SESSION_DIALOG).bounds().unwrap();
        let expected = rect(
            dialog.x0 as f32 + (button.x - layout.panel.x),
            dialog.y0 as f32 + (button.y - layout.panel.y),
            button.width,
            button.height,
        );
        for (actual, expected) in [
            (actual.x0, expected.x0),
            (actual.y0, expected.y0),
            (actual.x1, expected.x1),
            (actual.y1, expected.y1),
        ] {
            assert!((actual - expected).abs() < 0.1, "{actual} != {expected}");
        }

        let exposed_groups = node(SESSION_LIST).children().len();
        assert_eq!(exposed_groups, layout.visible_count);
        assert!(exposed_groups < 20);
    }

    #[test]
    fn workspace_rename_semantics_accept_utf8_and_cancel_without_persistence() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.session_panel
            .set_test_entries(test_session_entries(1), true);
        app.accessibility_snapshot();
        let key = SessionSemanticKey {
            id: "workspace-0".into(),
            workspaces: true,
        };
        let rename = *app
            .accessibility
            .session_nodes
            .get(&SessionSemanticTarget::Rename(key))
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: rename,
            data: None,
        });
        let form = app.accessibility_snapshot();
        assert_eq!(form.focus, SESSION_RENAME_INPUT);
        assert!(form.nodes.iter().any(|(id, node)| {
            *id == SESSION_RENAME_SAVE && node.bounds().is_some() && node.role() == Role::Button
        }));
        let load = *app
            .accessibility
            .session_nodes
            .get(&SessionSemanticTarget::Load(SessionSemanticKey {
                id: "workspace-0".into(),
                workspaces: true,
            }))
            .unwrap();
        assert!(
            form.nodes
                .iter()
                .find(|(id, _)| *id == load)
                .unwrap()
                .1
                .is_disabled()
        );

        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SESSION_RENAME_INPUT,
            data: Some(ActionData::Value("New roots 🫚".into())),
        });
        assert_eq!(
            app.session_panel.build_info().rename_text.as_deref(),
            Some("New roots 🫚")
        );
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SESSION_RENAME_INPUT,
            data: Some(ActionData::Value("bad\nname".into())),
        });
        assert_eq!(
            app.session_panel.build_info().rename_text.as_deref(),
            Some("New roots 🫚")
        );
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SESSION_RENAME_CANCEL,
            data: None,
        });
        assert!(!app.session_panel.is_renaming());
        assert_eq!(app.session_panel.sessions()[0].display_name, "Workspace 0");
    }

    #[test]
    fn stale_workspace_actions_cannot_retarget_another_entry() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.session_panel
            .set_test_entries(test_session_entries(2), true);
        app.accessibility_snapshot();
        let stale_delete = *app
            .accessibility
            .session_nodes
            .get(&SessionSemanticTarget::Delete(SessionSemanticKey {
                id: "workspace-0".into(),
                workspaces: true,
            }))
            .unwrap();
        app.session_panel
            .set_test_entries(test_session_entries(2).into_iter().skip(1).collect(), true);
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: stale_delete,
            data: None,
        });
        assert_eq!(app.session_panel.sessions().len(), 1);
        assert_eq!(app.session_panel.sessions()[0].id, "workspace-1");
        app.accessibility_snapshot();
        assert!(
            !app.accessibility
                .session_targets
                .contains_key(&stale_delete)
        );
    }

    #[test]
    fn legacy_snapshot_semantics_never_offer_rename() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.session_panel
            .set_test_entries(test_session_entries(1), false);
        let snapshot = app.accessibility_snapshot();
        assert_eq!(
            snapshot
                .nodes
                .iter()
                .find(|(id, _)| *id == SESSION_DIALOG)
                .unwrap()
                .1
                .label(),
            Some("Legacy snapshots")
        );
        assert!(
            !app.accessibility
                .session_targets
                .keys()
                .any(|target| matches!(
                    app.accessibility.session_targets.get(target),
                    Some(SessionSemanticTarget::Rename(_))
                ))
        );
    }

    #[test]
    fn modal_workspace_manager_consumes_stale_background_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.session_panel
            .set_test_entries(test_session_entries(1), true);
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });
        assert!(app.session_panel.is_open());
        assert!(!app.settings_panel.is_active());
    }

    #[test]
    fn shortcuts_editor_is_one_bounded_modal_tree_with_shared_geometry() {
        use crate::input::keybindings::BindableAction;

        let mut app = App::new_headless(Config::default()).unwrap();
        app.shortcuts_panel.open();
        let preview = app.shortcuts_panel.build_info(&app.shortcut_registry);
        let layout = app.shortcuts_accessibility_layout(&preview);
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        assert_eq!(node(ROOT).children(), &[SHORTCUTS_DIALOG]);
        assert_eq!(node(SHORTCUTS_DIALOG).role(), Role::Dialog);
        assert!(node(SHORTCUTS_DIALOG).is_modal());
        assert_eq!(node(SHORTCUTS_INPUT).role(), Role::SearchInput);
        assert_eq!(node(SHORTCUTS_INPUT).controls(), &[SHORTCUTS_LIST]);
        assert!(node(SHORTCUTS_INPUT).supports_action(Action::SetValue));
        assert_eq!(node(SHORTCUTS_LIST).role(), Role::List);
        assert!(node(SHORTCUTS_LIST).children().len() <= layout.rows_that_fit);

        let copy = *app
            .accessibility
            .shortcuts_nodes
            .get(&ShortcutsSemanticTarget::Action(BindableAction::Copy))
            .unwrap();
        assert_eq!(node(copy).role(), Role::Button);
        assert!(node(copy).supports_action(Action::Click));
        assert!(node(copy).supports_action(Action::CustomAction));
        let row = node(copy).bounds().unwrap();
        let dialog = node(SHORTCUTS_DIALOG).bounds().unwrap();
        let first_action = layout.row(1);
        let expected_row = rect(
            dialog.x0 as f32 + (first_action.x - layout.panel_x),
            dialog.y0 as f32 + (first_action.y - layout.panel_y),
            first_action.width,
            first_action.height,
        );
        for (actual, expected) in [
            (row.x0, expected_row.x0),
            (row.y0, expected_row.y0),
            (row.x1, expected_row.x1),
            (row.y1, expected_row.y1),
        ] {
            assert!((actual - expected).abs() < 0.1, "{actual} != {expected}");
        }
    }

    #[test]
    fn shortcut_filter_semantic_edits_retire_preedit_without_changing_bindings() {
        use crate::input::keybindings::BindableAction;
        let mut app = App::new_headless(Config::default()).unwrap();
        app.shortcuts_panel.open();
        let bindings = |app: &App| {
            BindableAction::all()
                .iter()
                .map(|action| app.shortcut_registry.binding_for(*action).cloned())
                .collect::<Vec<_>>()
        };
        let original_bindings = bindings(&app);
        let original_config = toml::to_string(&app.config).unwrap();
        for value in ["copy", "e\u{301}👩\u{200d}💻", &"🫚".repeat(128), ""] {
            app.handle_ime_event(winit::event::Ime::Preedit("draft".into(), None));
            app.handle_accessibility_action(ActionRequest {
                action: Action::SetValue,
                target_tree: TreeId::ROOT,
                target_node: SHORTCUTS_INPUT,
                data: Some(ActionData::Value(value.into())),
            });
            let snapshot = app.accessibility_snapshot();
            let input = &snapshot
                .nodes
                .iter()
                .find(|(id, _)| *id == SHORTCUTS_INPUT)
                .unwrap()
                .1;
            assert_eq!(input.value(), Some(value));
            assert_eq!(
                input.text_selection().unwrap().focus.character_index,
                value.chars().count()
            );
            assert!(input.supports_action(Action::SetValue));
            assert!(!input.is_read_only());
            assert_eq!(snapshot.focus, SHORTCUTS_INPUT);
            assert!(app.ime.preedit().is_none());
            assert!(app.redraw_scheduler.has_pending());
        }
        app.handle_ime_event(winit::event::Ime::Preedit("keep".into(), None));
        for data in [
            None,
            Some(ActionData::Value("bad\nquery".into())),
            Some(ActionData::Value("x".repeat(129).into())),
        ] {
            app.handle_accessibility_action(ActionRequest {
                action: Action::SetValue,
                target_tree: TreeId::ROOT,
                target_node: SHORTCUTS_INPUT,
                data,
            });
            assert!(
                app.shortcuts_panel
                    .build_info(&app.shortcut_registry)
                    .filter
                    .is_empty()
            );
            assert!(app.ime.preedit().is_some());
        }
        assert_eq!(bindings(&app), original_bindings);
        assert_eq!(toml::to_string(&app.config).unwrap(), original_config);
    }

    #[test]
    fn shortcut_recording_rejects_filter_focus_and_restores_editability_on_every_exit() {
        use crate::input::keybindings::{BindableAction, KeyChord};
        use crate::ui::shortcuts_panel::{ShortcutsAction, ShortcutsKey};
        for exit in ["cancel", "timeout", "capture"] {
            let mut app = App::new_headless(Config::default()).unwrap();
            app.shortcuts_panel.open();
            app.accessibility.focus_override = Some(SHORTCUTS_INPUT);
            app.apply_shortcuts_action(ShortcutsAction::StartRecording(BindableAction::Copy));
            let deadline = app
                .shortcuts_panel
                .recording_deadline(Duration::from_secs(30));
            let snapshot = app.accessibility_snapshot();
            let copy = *app
                .accessibility
                .shortcuts_nodes
                .get(&ShortcutsSemanticTarget::Action(BindableAction::Copy))
                .unwrap();
            assert_eq!(snapshot.focus, copy);
            let input = &snapshot
                .nodes
                .iter()
                .find(|(id, _)| *id == SHORTCUTS_INPUT)
                .unwrap()
                .1;
            assert!(input.is_read_only());
            assert!(!input.supports_action(Action::SetValue));
            assert!(!input.supports_action(Action::Focus));
            for action in [Action::Focus, Action::SetValue] {
                app.handle_accessibility_action(ActionRequest {
                    action,
                    target_tree: TreeId::ROOT,
                    target_node: SHORTCUTS_INPUT,
                    data: Some(ActionData::Value("paste".into())),
                });
                assert_eq!(app.accessibility_snapshot().focus, copy);
                assert!(
                    app.shortcuts_panel
                        .build_info(&app.shortcut_registry)
                        .filter
                        .is_empty()
                );
                assert_eq!(
                    app.shortcuts_panel
                        .recording_deadline(Duration::from_secs(30)),
                    deadline
                );
                assert!(app.shortcuts_panel.is_recording());
            }
            match exit {
                "cancel" => {
                    app.shortcuts_panel
                        .handle_key(ShortcutsKey::Escape, &app.shortcut_registry);
                }
                "timeout" => {
                    assert!(app.shortcuts_panel.tick_recording_timeout(Duration::ZERO));
                }
                "capture" => {
                    let chord = KeyChord::new("F24", true, true, true, false);
                    assert_eq!(
                        app.shortcuts_panel.handle_key(
                            ShortcutsKey::RecordedChord(chord.clone()),
                            &app.shortcut_registry
                        ),
                        ShortcutsAction::SetShortcut {
                            action: BindableAction::Copy,
                            chord: Some(chord)
                        }
                    );
                }
                _ => unreachable!(),
            }
            assert!(!app.shortcuts_panel.is_recording());
            let snapshot = app.accessibility_snapshot();
            let input = &snapshot
                .nodes
                .iter()
                .find(|(id, _)| *id == SHORTCUTS_INPUT)
                .unwrap()
                .1;
            assert!(!input.is_read_only());
            assert!(input.supports_action(Action::SetValue));
            assert!(input.supports_action(Action::Focus));
            app.handle_accessibility_action(ActionRequest {
                action: Action::SetValue,
                target_tree: TreeId::ROOT,
                target_node: SHORTCUTS_INPUT,
                data: Some(ActionData::Value("paste".into())),
            });
            assert_eq!(app.accessibility_snapshot().focus, SHORTCUTS_INPUT);
            assert_eq!(
                app.shortcuts_panel
                    .build_info(&app.shortcut_registry)
                    .filter,
                "paste"
            );
        }
    }

    #[test]
    fn shortcut_recording_never_falls_back_to_read_only_filter_when_target_scrolls_away() {
        use crate::input::keybindings::BindableAction;
        use crate::ui::shortcuts_panel::ShortcutsAction;
        let mut app = App::new_headless(Config::default()).unwrap();
        app.shortcuts_panel.open();
        app.apply_shortcuts_action(ShortcutsAction::StartRecording(BindableAction::Copy));
        app.accessibility.focus_override = Some(SHORTCUTS_INPUT);
        app.shortcuts_panel
            .scroll_down(usize::MAX, &app.shortcut_registry);
        let snapshot = app.accessibility_snapshot();
        assert_eq!(snapshot.focus, SHORTCUTS_DIALOG);
        assert!(app.shortcuts_panel.is_recording());
        assert!(
            snapshot
                .nodes
                .iter()
                .any(|(id, node)| *id == SHORTCUTS_STATUS && node.live() == Some(Live::Polite))
        );
    }

    #[test]
    fn shortcut_semantic_actions_record_clear_and_reject_stale_filtered_targets() {
        use crate::input::keybindings::BindableAction;

        let mut app = App::new_headless(Config::default()).unwrap();
        app.shortcuts_panel.open();
        app.accessibility_snapshot();
        let copy = *app
            .accessibility
            .shortcuts_nodes
            .get(&ShortcutsSemanticTarget::Action(BindableAction::Copy))
            .unwrap();

        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: copy,
            data: None,
        });
        assert!(app.shortcuts_panel.is_recording());
        let recording = app.accessibility_snapshot();
        assert!(
            recording
                .nodes
                .iter()
                .any(|(id, node)| { *id == SHORTCUTS_STATUS && node.live() == Some(Live::Polite) })
        );
        app.shortcuts_panel.handle_key(
            crate::ui::shortcuts_panel::ShortcutsKey::Escape,
            &app.shortcut_registry,
        );

        app.handle_accessibility_action(ActionRequest {
            action: Action::CustomAction,
            target_tree: TreeId::ROOT,
            target_node: copy,
            data: Some(ActionData::CustomAction(SHORTCUT_CLEAR_ACTION)),
        });
        assert!(
            app.shortcut_registry
                .binding_for(BindableAction::Copy)
                .is_none()
        );

        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: SHORTCUTS_INPUT,
            data: Some(ActionData::Value("paste".into())),
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: copy,
            data: None,
        });
        assert!(!app.shortcuts_panel.is_recording());
    }

    #[test]
    fn modal_shortcuts_editor_consumes_stale_background_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.shortcuts_panel.open();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });
        assert!(app.shortcuts_panel.is_active());
        assert!(!app.settings_panel.is_active());
    }

    fn domain_picker_config(hosts: usize) -> Config {
        let mut config = Config::default();
        config.domains.ssh_hosts = (0..hosts)
            .map(|index| {
                (
                    format!("host-{index:02}"),
                    crate::config::SshHostProfile {
                        destination: format!("developer@host-{index:02}.example"),
                        description: Some(format!("Build host {index}")),
                    },
                )
            })
            .collect();
        config
    }

    #[test]
    fn domain_picker_is_one_bounded_modal_tree_with_stable_host_actions() {
        let mut app = App::new_headless(domain_picker_config(20)).unwrap();
        app.domain_picker.open(&app.config.domains);
        let preview = app.domain_picker.build_info();
        let layout = app.domain_picker_accessibility_layout(&preview);
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        assert_eq!(node(ROOT).children(), &[DOMAIN_DIALOG]);
        assert_eq!(node(DOMAIN_DIALOG).role(), Role::Dialog);
        assert!(node(DOMAIN_DIALOG).is_modal());
        assert_eq!(node(DOMAIN_INPUT).role(), Role::SearchInput);
        assert_eq!(node(DOMAIN_INPUT).controls(), &[DOMAIN_LIST]);
        assert!(node(DOMAIN_INPUT).supports_action(Action::SetValue));
        assert_eq!(node(DOMAIN_LIST).role(), Role::ListBox);
        assert!(node(DOMAIN_LIST).children().len() <= layout.rows_that_fit);
        assert!(node(DOMAIN_LIST).children().len() < 21);

        let local = *app
            .accessibility
            .domain_nodes
            .get("domain-picker.authority.local")
            .unwrap();
        assert_eq!(node(local).role(), Role::ListBoxOption);
        assert!(node(local).supports_action(Action::Click));
        let first = node(node(DOMAIN_LIST).children()[0]).bounds().unwrap();
        let dialog = node(DOMAIN_DIALOG).bounds().unwrap();
        assert!((first.x0 - (dialog.x0 + f64::from(layout.padding * 0.5))).abs() < 0.1);
        assert!((first.y0 - (dialog.y0 + f64::from(layout.list_y - layout.panel_y))).abs() < 0.1);
    }

    #[test]
    fn domain_semantic_activation_is_exact_and_headless_discovery_stays_local() {
        let mut app = App::new_headless(domain_picker_config(1)).unwrap();
        app.domain_picker.open(&app.config.domains);
        app.accessibility_snapshot();
        let remote_id = "domain-picker.authority.ssh.host-00";
        let remote = *app.accessibility.domain_nodes.get(remote_id).unwrap();

        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: DOMAIN_INPUT,
            data: Some(ActionData::Value("Build host 0".into())),
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: remote,
            data: None,
        });

        // Headless orchestration rejects before constructing a client, so this
        // proves routing without touching a socket or invoking SSH.
        assert!(app.domain_picker.is_active());
        assert_eq!(app.domain_picker.build_info().item_noun, "result");
        let failed = app.accessibility_snapshot();
        let status = failed
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::Status)
            .unwrap();
        assert_eq!(status.1.live(), Some(Live::Polite));
        assert!(
            status
                .1
                .label()
                .is_some_and(|label| label.contains("failed"))
        );
    }

    #[test]
    fn domain_picker_rejects_hidden_stale_and_background_actions() {
        let mut app = App::new_headless(domain_picker_config(1)).unwrap();
        app.domain_picker.open(&app.config.domains);
        app.accessibility_snapshot();
        let local = *app
            .accessibility
            .domain_nodes
            .get("domain-picker.authority.local")
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: DOMAIN_INPUT,
            data: Some(ActionData::Value("no such host".into())),
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: local,
            data: None,
        });
        assert!(app.domain_picker.can_filter());
        assert_eq!(app.domain_picker.build_info().item_noun, "host");

        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });
        assert!(app.domain_picker.is_active());
        assert!(!app.settings_panel.is_active());
    }

    #[test]
    fn unicode_picker_is_one_bounded_modal_tree_with_stable_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.unicode_picker.open();
        let preview = app.unicode_picker.build_info();
        let layout = app.unicode_picker_accessibility_layout(&preview);
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        assert_eq!(node(ROOT).children(), &[UNICODE_DIALOG]);
        assert_eq!(node(UNICODE_DIALOG).role(), Role::Dialog);
        assert!(node(UNICODE_DIALOG).is_modal());
        assert_eq!(node(UNICODE_INPUT).role(), Role::SearchInput);
        assert_eq!(node(UNICODE_INPUT).controls(), &[UNICODE_LIST]);
        assert!(node(UNICODE_INPUT).supports_action(Action::SetValue));
        assert_eq!(node(UNICODE_LIST).role(), Role::ListBox);
        assert!(node(UNICODE_LIST).children().len() <= layout.rows_that_fit);

        let arrow = *app
            .accessibility
            .unicode_nodes
            .get("unicode-picker.curated.2192")
            .unwrap();
        assert_eq!(node(arrow).role(), Role::ListBoxOption);
        assert!(node(arrow).supports_action(Action::Click));
        assert!(node(arrow).supports_action(Action::Focus));
        let first = node(node(UNICODE_LIST).children()[0]).bounds().unwrap();
        let dialog = node(UNICODE_DIALOG).bounds().unwrap();
        assert!((first.x0 - (dialog.x0 + f64::from(layout.padding * 0.5))).abs() < 0.1);
        assert!((first.y0 - (dialog.y0 + f64::from(layout.list_y - layout.panel_y))).abs() < 0.1);
    }

    #[test]
    fn unicode_accessibility_filter_retires_stale_results_and_routes_exact_focus() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.unicode_picker.open();
        app.accessibility_snapshot();
        let stale = *app
            .accessibility
            .unicode_nodes
            .get("unicode-picker.curated.2192")
            .unwrap();

        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: UNICODE_INPUT,
            data: Some(ActionData::Value("U+03BB".into())),
        });
        let filtered = app.accessibility_snapshot();
        let exact_id = "unicode-picker.codepoint.3bb";
        let exact = *app.accessibility.unicode_nodes.get(exact_id).unwrap();
        assert!(!app.accessibility.unicode_commands.contains_key(&stale));
        let input = &filtered
            .nodes
            .iter()
            .find(|(id, _)| *id == UNICODE_INPUT)
            .unwrap()
            .1;
        assert_eq!(input.active_descendant(), Some(exact));

        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: stale,
            data: None,
        });
        assert!(app.unicode_picker.is_active());
        app.handle_accessibility_action(ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: exact,
            data: None,
        });
        assert_eq!(app.accessibility.focus_override, Some(UNICODE_INPUT));
        assert_eq!(app.unicode_picker.build_info().entries[0].id, exact_id);

        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });
        assert!(app.unicode_picker.is_active());
        assert!(!app.settings_panel.is_active());
    }

    #[test]
    fn palette_editing_publishes_the_moved_utf8_caret_to_accessibility() {
        use crate::ui::text_input::TextInputEdit;
        for unicode in [false, true] {
            let mut app = App::new_headless(Config::default()).unwrap();
            if unicode {
                app.unicode_picker.open();
                app.unicode_picker.accessibility_set_filter("a界e\u{301}z");
                app.unicode_picker.edit_input(TextInputEdit::Home);
                app.unicode_picker.edit_input(TextInputEdit::Right);
                app.unicode_picker.edit_input(TextInputEdit::Right);
                app.unicode_picker.edit_input(TextInputEdit::Right);
            } else {
                app.command_palette.open();
                app.command_palette.accessibility_set_input("a界e\u{301}z");
                app.command_palette.edit_input(TextInputEdit::Home);
                app.command_palette.edit_input(TextInputEdit::Right);
                app.command_palette.edit_input(TextInputEdit::Right);
                app.command_palette.edit_input(TextInputEdit::Right);
            }
            let snapshot = app.accessibility_snapshot();
            let id = if unicode {
                UNICODE_INPUT
            } else {
                PALETTE_INPUT
            };
            let (_, input) = snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap();
            let selection = input.text_selection().unwrap();
            assert_eq!(selection.anchor.character_index, 4);
            assert_eq!(selection.focus.character_index, 4);
            assert_eq!(input.value(), Some("a界e\u{301}z"));
            app.unicode_picker.close();
            app.unicode_picker.finish_search_for_test();
        }
    }

    #[test]
    fn unicode_name_search_keeps_the_semantic_registry_bounded() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.unicode_picker.open();
        app.accessibility_snapshot();
        let stale = *app
            .accessibility
            .unicode_nodes
            .get("unicode-picker.curated.2192")
            .unwrap();

        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: UNICODE_INPUT,
            data: Some(ActionData::Value("snowman".into())),
        });
        let pending = app.accessibility_snapshot();
        assert!(pending.nodes.iter().any(|(id, node)| {
            *id == UNICODE_EMPTY && node.label() == Some("Searching characters…")
        }));
        assert!(!app.accessibility.unicode_commands.contains_key(&stale));
        app.unicode_picker.finish_search_for_test();
        let snapshot = app.accessibility_snapshot();
        let info = app.unicode_picker.build_info();
        assert!(info.total_count <= 256);
        assert!(
            info.entries
                .iter()
                .any(|entry| entry.id == "unicode-picker.named.2603")
        );
        assert!(!app.accessibility.unicode_commands.contains_key(&stale));
        assert!(
            snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == UNICODE_LIST)
                .is_some_and(|(_, list)| list.children().len() <= info.entries.len())
        );
    }

    #[test]
    fn cldr_emoji_results_expose_exact_sequence_names_and_retire_stale_nodes() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.unicode_picker.open();
        assert!(
            app.unicode_picker
                .accessibility_set_filter("flag united states")
        );
        app.unicode_picker.finish_search_for_test();
        let snapshot = app.accessibility_snapshot();
        let command = "unicode-picker.cldr.1f1fa-1f1f8";
        let node_id = *app.accessibility.unicode_nodes.get(command).unwrap();
        let node = snapshot
            .nodes
            .iter()
            .find(|(candidate, _)| *candidate == node_id)
            .map(|(_, node)| node)
            .unwrap();
        assert!(node.label().is_some_and(|label| {
            label.contains("🇺🇸") && label.contains("flag: United States")
        }));
        assert!(node.description().is_some_and(|description| {
            description.contains("CLDR 48.0.0 en") && description.contains("U+1F1FA U+1F1F8")
        }));

        assert!(app.unicode_picker.accessibility_set_filter("party pinata"));
        app.unicode_picker.finish_search_for_test();
        app.accessibility_snapshot();
        assert!(!app.accessibility.unicode_nodes.contains_key(command));
    }

    #[test]
    fn pending_domain_picker_exposes_read_only_live_status() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.domain_picker.open(&app.config.domains);
        assert!(app.domain_picker.activate().is_some());
        let before = app.domain_picker.build_info().input;
        let snapshot = app.accessibility_snapshot();
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == DOMAIN_INPUT)
            .unwrap()
            .1;
        assert!(input.is_read_only());
        assert!(!input.supports_action(Action::SetValue));
        assert!(
            snapshot.nodes.iter().any(|(_, node)| {
                node.role() == Role::Status && node.live() == Some(Live::Polite)
            })
        );
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: DOMAIN_INPUT,
            data: Some(ActionData::Value("must not replace".into())),
        });
        assert_eq!(app.domain_picker.build_info().input, before);
    }

    fn show_info_fixture(app: &mut App) -> u64 {
        let mut lines: Vec<_> = (0..40)
            .map(|index| format!("fixture line {index} {}", "x".repeat(300)))
            .collect();
        lines[1] = "  1  Safe fixture choice".into();
        app.info_overlay.show_choices(
            "Hermetic notice",
            lines,
            vec![crate::ui::info_overlay::InfoOverlayChoice::new(
                "safe-choice",
                "Safe fixture choice",
                1,
                Some('1'),
            )],
        )
    }

    #[test]
    fn info_overlay_keyboard_and_assistive_focus_share_the_same_owner_and_bounds() {
        use crate::ui::info_overlay::{InfoOverlayAction, InfoOverlayFocus};
        let mut app = App::new_headless(Config::default()).unwrap();
        // Compare one settled phase, not two advancing Instant samples.
        app.info_overlay.set_reduced_motion(true);
        let request = show_info_fixture(&mut app);
        app.accessibility_snapshot();
        app.info_overlay.cycle_focus(request, false);
        let snapshot = app.accessibility_snapshot();
        let choice = app.accessibility.info_nodes[&InfoSemanticTarget::Choice {
            request_id: request,
            id: "safe-choice".into(),
        }];
        assert_eq!(snapshot.focus, choice);
        let layout = app.info_accessibility_layout();
        let bounds = snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == choice)
            .unwrap()
            .1
            .bounds()
            .unwrap();
        let expected = info_rect(
            layout
                .source_line_rect(1, app.info_overlay.scroll_offset(), 40)
                .unwrap(),
        );
        assert_eq!(bounds, expected);
        assert_eq!(
            app.info_overlay.activate_focused(request),
            InfoOverlayAction::Choose {
                request_id: request,
                choice_id: "safe-choice".into()
            }
        );
        let copy = app.accessibility.info_nodes[&InfoSemanticTarget::Copy(request)];
        app.handle_accessibility_action(ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: copy,
            data: None,
        });
        assert_eq!(app.info_overlay.focus(), Some(&InfoOverlayFocus::Copy));
        assert_eq!(
            app.info_overlay.activate_focused(request),
            InfoOverlayAction::Copy {
                request_id: request
            }
        );
        assert_eq!(app.accessibility_snapshot().focus, copy);
        let replacement = app
            .info_overlay
            .show("Replacement", vec!["No choice".into()]);
        app.handle_accessibility_action(ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: choice,
            data: None,
        });
        assert_eq!(app.info_overlay.focus(), Some(&InfoOverlayFocus::Continue));
        assert_eq!(
            app.info_overlay.activate_focused(replacement),
            InfoOverlayAction::Dismiss {
                request_id: replacement
            }
        );
    }

    #[test]
    fn info_overlay_is_one_scrollable_modal_tree_with_shared_geometry() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.info_overlay.set_reduced_motion(true);
        let request = show_info_fixture(&mut app);
        let layout = app.info_accessibility_layout();
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        let content = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Content(request))
            .unwrap();
        let copy = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Copy(request))
            .unwrap();
        let dismiss = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Dismiss(request))
            .unwrap();
        let choice = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Choice {
                request_id: request,
                id: "safe-choice".into(),
            })
            .unwrap();

        assert_eq!(node(ROOT).children(), &[INFO_DIALOG]);
        assert_eq!(node(INFO_DIALOG).role(), Role::Dialog);
        assert!(node(INFO_DIALOG).is_modal());
        assert_eq!(node(content).role(), Role::Document);
        assert!(node(content).supports_action(Action::ScrollDown));
        assert!(node(content).supports_action(Action::ScrollRight));
        assert_eq!(node(choice).role(), Role::Button);
        assert!(node(choice).supports_action(Action::Click));
        assert_eq!(snapshot.focus, dismiss);
        assert_eq!(node(copy).bounds(), Some(info_rect(layout.copy_button)));
        assert_eq!(
            node(dismiss).bounds(),
            Some(info_rect(layout.continue_button))
        );
        assert_eq!(node(content).bounds(), Some(info_rect(layout.content)));

        app.handle_accessibility_action(ActionRequest {
            action: Action::ScrollDown,
            target_tree: TreeId::ROOT,
            target_node: content,
            data: None,
        });
        assert!(app.info_overlay.scroll_offset() > 0);
        let scrolled = app.accessibility_snapshot();
        let scrolled_content = &scrolled
            .nodes
            .iter()
            .find(|(id, _)| *id == content)
            .unwrap()
            .1;
        assert!(
            scrolled_content
                .value()
                .is_some_and(|value| !value.contains("fixture line 0"))
        );
    }

    #[test]
    fn info_overlay_rejects_retired_choices_and_background_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let stale_request = show_info_fixture(&mut app);
        app.pending_profile_selection = Some(PendingProfileSelection {
            request_id: stale_request,
            terminal: crate::tabs::TerminalIdentity {
                tab_id: 0,
                pane_id: PaneId(0),
            },
            plugin_id: "fake-ai".into(),
            profiles: std::collections::BTreeMap::from([(
                "safe-choice".into(),
                Some("safe-choice".into()),
            )]),
        });
        app.accessibility_snapshot();
        let stale_choice = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Choice {
                request_id: stale_request,
                id: "safe-choice".into(),
            })
            .unwrap();

        let current = app
            .info_overlay
            .show("Replacement notice", vec!["No choices are active".into()]);
        app.accessibility_snapshot();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: stale_choice,
            data: None,
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });

        assert!(app.info_overlay.is_awaiting(current));
        assert!(app.pending_profile_selection.is_some());
        assert!(!app.settings_panel.is_active());
    }

    #[test]
    fn info_overlay_dismiss_action_revokes_the_request_immediately() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = show_info_fixture(&mut app);
        app.accessibility_snapshot();
        let dismiss = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Dismiss(request))
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: dismiss,
            data: None,
        });
        assert!(app.info_overlay.is_active());
        assert!(!app.info_overlay.is_awaiting(request));
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: dismiss,
            data: None,
        });
        assert!(!app.info_overlay.is_awaiting(request));
    }

    #[test]
    fn info_overlay_input_is_editable_focused_and_uses_shared_row_geometry() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.info_overlay.set_reduced_motion(true);
        let request = app.info_overlay.show_input(
            "Edit setting",
            vec!["Typed value".into(), String::new(), "Enter saves".into()],
            1,
            "Value: ",
            "root",
            32,
            Vec::new(),
        );
        let layout = app.info_accessibility_layout();
        let snapshot = app.accessibility_snapshot();
        let input_id = *app
            .accessibility
            .info_nodes
            .get(&InfoSemanticTarget::Input(request))
            .unwrap();
        let input = &snapshot
            .nodes
            .iter()
            .find(|(id, _)| *id == input_id)
            .unwrap()
            .1;
        assert_eq!(input.role(), Role::TextInput);
        assert_eq!(input.value(), Some("root"));
        assert!(input.supports_action(Action::SetValue));
        assert_eq!(snapshot.focus, input_id);
        let actual = input.bounds().unwrap();
        let expected = layout
            .source_line_rect(1, app.info_overlay.scroll_offset(), 3)
            .map(info_rect)
            .unwrap();
        assert_eq!(actual, expected);

        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: input_id,
            data: Some(ActionData::Value("warm 🫚".into())),
        });
        assert_eq!(app.info_overlay.input_value(request), Some("warm 🫚"));
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: input_id,
            data: Some(ActionData::Value("bad\nvalue".into())),
        });
        assert_eq!(app.info_overlay.input_value(request), Some("warm 🫚"));
    }

    fn show_diff_fixture(app: &mut App, id: u64) -> u64 {
        use crate::terminal::diff_detector::{DetectedDiff, DiffHunk, DiffLine};

        app.diff_review.show(
            &DetectedDiff {
                id,
                filename: "src/hermetic_fixture.rs".into(),
                hunks: vec![DiffHunk {
                    header: "@@ -1,40 +1,40 @@".into(),
                    lines: (0..40)
                        .map(|index| {
                            DiffLine::Added(format!("fixture row {index} {}", "x".repeat(300)))
                        })
                        .collect(),
                }],
                raw: "--- a/src/hermetic_fixture.rs\n+++ b/src/hermetic_fixture.rs".into(),
                reviewed: false,
            },
            usize::MAX,
            PaneId(0),
        )
    }

    #[test]
    fn diff_review_is_one_scrollable_modal_tree_with_shared_geometry() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.diff_review.set_reduced_motion(true);
        let request = show_diff_fixture(&mut app, 41);
        let layout = app.diff_accessibility_layout();
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        let content = *app
            .accessibility
            .diff_nodes
            .get(&DiffSemanticTarget::Content(request))
            .unwrap();
        let copy = *app
            .accessibility
            .diff_nodes
            .get(&DiffSemanticTarget::Copy(request))
            .unwrap();
        let dismiss = *app
            .accessibility
            .diff_nodes
            .get(&DiffSemanticTarget::Dismiss(request))
            .unwrap();

        assert_eq!(node(ROOT).children(), &[DIFF_DIALOG]);
        assert_eq!(node(DIFF_DIALOG).role(), Role::Dialog);
        assert!(node(DIFF_DIALOG).is_modal());
        assert_eq!(node(content).role(), Role::Document);
        assert!(node(content).supports_action(Action::ScrollDown));
        assert!(node(content).supports_action(Action::ScrollRight));
        assert_eq!(snapshot.focus, dismiss);
        assert_eq!(node(copy).bounds(), Some(diff_rect(layout.copy_button)));
        assert_eq!(node(dismiss).bounds(), Some(diff_rect(layout.done_button)));
        assert_eq!(node(content).bounds(), Some(diff_rect(layout.content)));

        app.handle_accessibility_action(ActionRequest {
            action: Action::ScrollDown,
            target_tree: TreeId::ROOT,
            target_node: content,
            data: None,
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::ScrollRight,
            target_tree: TreeId::ROOT,
            target_node: content,
            data: None,
        });
        assert!(app.diff_review.scroll_offset() > 0);
        assert!(app.diff_review.horizontal_offset() > 0);
    }

    #[test]
    fn diff_review_rejects_retired_and_background_semantic_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let stale_request = show_diff_fixture(&mut app, 51);
        app.accessibility_snapshot();
        let stale_dismiss = *app
            .accessibility
            .diff_nodes
            .get(&DiffSemanticTarget::Dismiss(stale_request))
            .unwrap();

        let current = show_diff_fixture(&mut app, 52);
        app.accessibility_snapshot();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: stale_dismiss,
            data: None,
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });

        assert!(app.diff_review.is_awaiting(current));
        assert!(!app.settings_panel.is_active());
    }

    #[test]
    fn diff_review_dismiss_revokes_actions_during_its_exit_animation() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = show_diff_fixture(&mut app, 61);
        app.accessibility_snapshot();
        let dismiss = *app
            .accessibility
            .diff_nodes
            .get(&DiffSemanticTarget::Dismiss(request))
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: dismiss,
            data: None,
        });
        assert!(app.diff_review.is_active());
        assert!(!app.diff_review.is_awaiting(request));
        assert_eq!(app.diff_review.target(request), None);
    }

    fn show_context_fixture(app: &mut App, tab_id: usize, cwd: Option<String>) -> u64 {
        app.context_picker.open(
            cwd,
            crate::ui::context_picker::ContextTarget {
                plugin_id: "fixture-plugin".into(),
                terminal: crate::tabs::TerminalIdentity {
                    tab_id,
                    pane_id: crate::panes::PaneId(0),
                },
            },
        )
    }

    #[test]
    fn context_picker_is_one_modal_checkbox_tree_with_shared_geometry() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.context_picker.set_reduced_motion(true);
        let request = show_context_fixture(&mut app, 70, None);
        let layout = app.context_accessibility_layout();
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        let list = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::List(request))
            .unwrap();
        let terminal = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::Source {
                request_id: request,
                source: crate::ui::context_picker::ContextSourceId::TerminalOutput,
            })
            .unwrap();
        let cancel = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::Cancel(request))
            .unwrap();
        let primary = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::Primary(request))
            .unwrap();

        assert_eq!(node(ROOT).children(), &[CONTEXT_DIALOG]);
        assert_eq!(node(CONTEXT_DIALOG).role(), Role::Dialog);
        assert!(node(CONTEXT_DIALOG).is_modal());
        assert_eq!(node(list).role(), Role::List);
        assert_eq!(node(terminal).role(), Role::CheckBox);
        assert!(node(terminal).supports_action(Action::Click));
        assert!(node(cancel).supports_action(Action::Click));
        assert!(node(primary).supports_action(Action::Click));
        let actual = node(list).bounds().unwrap();
        let expected = context_rect(layout.content);
        assert!((actual.x0 - expected.x0).abs() < 0.05);
        assert!((actual.y0 - expected.y0).abs() < 0.05);

        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: terminal,
            data: None,
        });
        assert!(
            app.context_picker
                .snapshot()
                .sources
                .iter()
                .find(|source| {
                    source.id == crate::ui::context_picker::ContextSourceId::TerminalOutput
                })
                .unwrap()
                .selected
        );
    }

    #[test]
    fn context_file_editor_is_settable_and_workspace_bounded_semantically() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("fixture.rs"), "fn fixture() {}\n").unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        let request =
            show_context_fixture(&mut app, 71, Some(directory.path().display().to_string()));
        app.accessibility_snapshot();
        let files = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::Source {
                request_id: request,
                source: crate::ui::context_picker::ContextSourceId::SpecificFiles,
            })
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: files,
            data: None,
        });

        let form = app.accessibility_snapshot();
        let input = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::FileInput(request))
            .unwrap();
        let primary = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::Primary(request))
            .unwrap();
        let input_node = &form
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == input)
            .unwrap()
            .1;
        assert_eq!(input_node.role(), Role::TextInput);
        assert!(input_node.supports_action(Action::SetValue));
        app.handle_accessibility_action(ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: input,
            data: Some(ActionData::Value("fixture.rs".into())),
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: primary,
            data: None,
        });
        let source = app
            .context_picker
            .snapshot()
            .sources
            .into_iter()
            .find(|source| source.id == crate::ui::context_picker::ContextSourceId::SpecificFiles)
            .unwrap();
        assert!(source.selected);
        assert!(source.label.contains("(1)"));
    }

    #[test]
    fn context_picker_rejects_stale_background_and_cancelled_gather_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let stale = show_context_fixture(&mut app, 80, None);
        app.accessibility_snapshot();
        let stale_cancel = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::Cancel(stale))
            .unwrap();
        let current = show_context_fixture(&mut app, 81, None);
        app.accessibility_snapshot();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: stale_cancel,
            data: None,
        });
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });
        assert!(app.context_picker.is_awaiting(current));
        assert!(!app.settings_panel.is_active());

        app.context_picker
            .apply(crate::ui::context_picker::ContextPickerAction::Toggle {
                request_id: current,
                source: crate::ui::context_picker::ContextSourceId::TerminalOutput,
            });
        assert!(
            app.context_picker
                .begin_gather(current, Some("fixture output".into()))
                .is_some()
        );
        let gathering = app.accessibility_snapshot();
        let cancel = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::Cancel(current))
            .unwrap();
        let primary = *app
            .accessibility
            .context_nodes
            .get(&ContextSemanticTarget::Primary(current))
            .unwrap();
        assert!(
            gathering
                .nodes
                .iter()
                .find(|(id, _)| *id == primary)
                .unwrap()
                .1
                .is_disabled()
        );
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: cancel,
            data: None,
        });
        assert!(!app.context_picker.is_awaiting(current));
        assert_eq!(
            app.context_picker.complete(current, Ok("stale".into())),
            crate::ui::context_picker::ContextPickerCompletion::Ignored
        );
    }

    fn show_guardrail_fixture(app: &mut App) -> u64 {
        app.guardrail.show(
            crate::terminal::guardrails::GuardrailMatch {
                command: "fixture command; never execute".into(),
                reason: "Hermetic destructive-command warning".into(),
                severity: crate::terminal::guardrails::Severity::Critical,
                target: "Missing fixture terminal".into(),
                user: "tester".into(),
            },
            vec![crate::terminal::guardrails::GuardedWrite::for_pane(
                usize::MAX,
                crate::panes::PaneId(0),
                b"fixture-enter".to_vec(),
            )],
        )
    }

    #[test]
    fn guardrail_is_assertive_modal_with_safe_default_and_shared_geometry() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = show_guardrail_fixture(&mut app);
        let layout = app.guardrail_accessibility_layout();
        let snapshot = app.accessibility_snapshot();
        let node = |id| {
            &snapshot
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .unwrap()
                .1
        };
        let confirm = *app
            .accessibility
            .guardrail_nodes
            .get(&GuardrailSemanticTarget::Confirm(request))
            .unwrap();
        let cancel = *app
            .accessibility
            .guardrail_nodes
            .get(&GuardrailSemanticTarget::Cancel(request))
            .unwrap();

        assert_eq!(node(ROOT).children(), &[GUARDRAIL_DIALOG]);
        assert_eq!(node(GUARDRAIL_DIALOG).role(), Role::AlertDialog);
        assert!(node(GUARDRAIL_DIALOG).is_modal());
        assert_eq!(node(GUARDRAIL_REASON).role(), Role::Alert);
        assert_eq!(node(GUARDRAIL_REASON).live(), Some(Live::Assertive));
        assert_eq!(snapshot.focus, cancel, "Cancel is the safe default");
        assert!(node(cancel).supports_action(Action::Click));
        assert!(node(confirm).supports_action(Action::Click));
        assert!(
            node(confirm)
                .label()
                .is_some_and(|label| label.contains("dangerous"))
        );

        let dialog = node(GUARDRAIL_DIALOG).bounds().unwrap();
        let actual_cancel = node(cancel).bounds().unwrap();
        let expected_cancel = layout.cancel_button;
        assert!(
            (actual_cancel.x0 - (dialog.x0 + f64::from(expected_cancel.x - layout.card.x))).abs()
                < 0.1
        );
        assert!(
            (actual_cancel.y0 - (dialog.y0 + f64::from(expected_cancel.y - layout.card.y))).abs()
                < 0.1
        );
    }

    #[test]
    fn guardrail_cancel_revokes_immediately_and_stale_confirm_cannot_retarget() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let first = show_guardrail_fixture(&mut app);
        app.accessibility_snapshot();
        let stale_confirm = *app
            .accessibility
            .guardrail_nodes
            .get(&GuardrailSemanticTarget::Confirm(first))
            .unwrap();
        let cancel = *app
            .accessibility
            .guardrail_nodes
            .get(&GuardrailSemanticTarget::Cancel(first))
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: cancel,
            data: None,
        });
        assert!(app.guardrail.is_active());
        assert!(!app.guardrail.is_awaiting(first));
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: stale_confirm,
            data: None,
        });
        assert!(!app.guardrail.is_awaiting(first));

        let second = show_guardrail_fixture(&mut app);
        assert_ne!(first, second);
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: stale_confirm,
            data: None,
        });
        assert!(app.guardrail.is_awaiting(second));
    }

    #[test]
    fn current_guardrail_confirm_with_missing_origin_never_falls_back() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let request = show_guardrail_fixture(&mut app);
        app.accessibility_snapshot();
        let confirm = *app
            .accessibility
            .guardrail_nodes
            .get(&GuardrailSemanticTarget::Confirm(request))
            .unwrap();
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: confirm,
            data: None,
        });
        assert!(!app.guardrail.is_awaiting(request));
        assert!(app.tabs.is_none(), "no fallback terminal was created");
    }

    #[test]
    fn modal_guardrail_consumes_background_actions() {
        let mut app = App::new_headless(Config::default()).unwrap();
        show_guardrail_fixture(&mut app);
        app.handle_accessibility_action(ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: SETTINGS,
            data: None,
        });
        assert!(app.guardrail.is_awaiting_current());
        assert!(!app.settings_panel.is_active());
    }
}
