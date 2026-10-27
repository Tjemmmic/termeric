//! Tab and pane actions the control plane and plugins request by stable ID,
//! with the policy each requester gets.

use super::redraw_scheduler::RedrawReason;
use super::*;
use crate::panes::Pane;
use crate::tabs::Tab;

/// Who asked for a tab or pane change from outside the window's own input:
/// the running-GUI control plane or a declarative plugin. Internal automation
/// picks the policy it matches — notification activation acts like a control
/// client, the supervisor like a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Requester {
    Control,
    Plugin,
}

impl Requester {
    /// Whether the change dispatches its derived lifecycle hooks. A plugin's
    /// correlated action result is its only follow-up event, so it never hears
    /// its own change again as a hook, which would allow hook-driven loops.
    pub(super) fn dispatches_hooks(self) -> bool {
        self == Self::Control
    }

    /// Plugins may not close a window's last tab; window closure needs
    /// separate authority.
    fn may_close_last_tab(self) -> bool {
        self == Self::Control
    }

    /// A plugin may target a window other than the focused one, so its
    /// guardrail confirmation names the window.
    fn names_target_window(self) -> bool {
        self == Self::Plugin
    }

    fn label(self) -> &'static str {
        match self {
            Self::Control => "Control-requested",
            Self::Plugin => "Plugin-requested",
        }
    }
}

pub(super) fn validate_ordinary_tab_creation_capacity(
    tab_count: usize,
) -> std::result::Result<(), String> {
    if tab_count >= crate::control::protocol::MAX_TABS_PER_WINDOW {
        return Err(format!(
            "GUI window tab limit of {} reached",
            crate::control::protocol::MAX_TABS_PER_WINDOW
        ));
    }
    Ok(())
}

/// Why another program's input was refused: the person has not allowed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ControlInputDisabled;

impl std::fmt::Display for ControlInputDisabled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "External control input is disabled; set security.allow_gui_control_input = true",
        )
    }
}

impl From<ControlInputDisabled> for String {
    fn from(refusal: ControlInputDisabled) -> Self {
        refusal.to_string()
    }
}

/// A requester's stable tab ID as this platform's tab ID.
pub(super) fn control_tab_id(tab_id: u64) -> std::result::Result<usize, String> {
    usize::try_from(tab_id).map_err(|_| "Target tab ID does not fit this platform".to_string())
}

/// Pane `pane_id` of a requested tab, including one whose shell has exited.
pub(super) fn control_pane(tab: &Tab, pane_id: u32) -> std::result::Result<&Pane, String> {
    tab.pane(PaneId(pane_id))
        .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {}", tab.id))
}

/// Pane `pane_id` of a requested tab, refusing one whose shell has exited.
pub(super) fn control_running_pane(tab: &Tab, pane_id: u32) -> std::result::Result<&Pane, String> {
    let pane = control_pane(tab, pane_id)?;
    if pane.closed {
        return Err(format!("GUI pane {pane_id} in tab {} is closed", tab.id));
    }
    Ok(pane)
}

fn plugin_pane_layout_state(manager: &PaneManager) -> (Vec<u32>, u32, Option<u32>) {
    (
        manager.pane_ids().into_iter().map(|id| id.0).collect(),
        manager.focused_id().0,
        manager.zoomed_id().map(|id| id.0),
    )
}

impl App {
    /// Refuses text and answers from other programs (control clients and
    /// plugins) unless `security.allow_gui_control_input` allows them.
    pub(super) fn require_control_input(&self) -> Result<(), ControlInputDisabled> {
        if self.config.security.allow_gui_control_input {
            Ok(())
        } else {
            Err(ControlInputDisabled)
        }
    }

    pub(super) fn control_tabs(&self) -> std::result::Result<&TabManager, String> {
        self.tabs
            .as_ref()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())
    }

    pub(super) fn control_tabs_mut(&mut self) -> std::result::Result<&mut TabManager, String> {
        self.tabs
            .as_mut()
            .ok_or_else(|| "Target window has no initialized tab manager".to_string())
    }

    /// The tab a requester names by stable ID, with its index in the tab
    /// bar, refusing one that is closing.
    pub(super) fn control_tab_at(&self, tab_id: u64) -> std::result::Result<(usize, &Tab), String> {
        let tab_id = control_tab_id(tab_id)?;
        let (index, tab) = self
            .control_tabs()?
            .iter()
            .find(|(_, tab)| tab.id == tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the target window"))?;
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is already closing"));
        }
        Ok((index, tab))
    }

    /// [`Self::control_tab_at`] without the index.
    pub(super) fn control_tab(&self, tab_id: u64) -> std::result::Result<&Tab, String> {
        self.control_tab_at(tab_id).map(|(_, tab)| tab)
    }

    pub(super) fn control_tab_mut(&mut self, tab_id: u64) -> std::result::Result<&mut Tab, String> {
        let (index, _) = self.control_tab_at(tab_id)?;
        Ok(&mut self
            .tabs
            .as_mut()
            .expect("validated tab manager disappeared")
            .all_tabs_mut()[index])
    }

    /// Launch exact argv in an ordinary local tab for the private running-GUI
    /// control plane. The caller has already passed bounded wire validation.
    pub(super) fn launch_control_tab(
        &mut self,
        background: bool,
        program: &str,
        args: &[String],
        working_directory: Option<&str>,
        title: Option<&str>,
        environment: &[crate::control::protocol::EnvironmentVariable],
        environment_policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> std::result::Result<u64, String> {
        let tab_count = self.tabs.as_ref().map_or(0, TabManager::count);
        validate_ordinary_tab_creation_capacity(tab_count)?;
        if self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .is_some_and(|tab| tab.live_domain_tab_route().is_some())
        {
            return Err(
                "Exact local command launch is unavailable while the target window's active tab belongs to a live domain; use `termeric domain spawn`"
                    .into(),
            );
        }
        if let Some(directory) = working_directory
            && !std::path::Path::new(directory).is_dir()
        {
            return Err(format!(
                "GUI control launch working directory {directory:?} does not exist or is not a directory"
            ));
        }
        let id = self
            .control_tabs_mut()?
            .launch_command_tab(
                crate::tabs::CommandLaunch {
                    program: program.to_string(),
                    args: args.to_vec(),
                    working_dir: working_directory.map(str::to_string),
                    title: title.map(str::to_string),
                    tab_type: None,
                    env: environment
                        .iter()
                        .map(|entry| (entry.name.clone(), entry.value.clone()))
                        .collect(),
                    environment_policy: environment_policy.clone(),
                },
                background,
            )
            .map_err(|error| format!("Could not launch command tab: {error:#}"))?;
        self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated { tab_id: id });
        if !background || tab_count == 0 {
            self.refresh_focused_terminal_surfaces();
        } else {
            // A quiet child may never emit a PTY wake. Update tab chrome without
            // invalidating the current pane's IME/search/selection surfaces.
            self.request_redraw(RedrawReason::WindowEvent);
        }
        u64::try_from(id).map_err(|_| "Created tab ID does not fit the control protocol".into())
    }

    /// Make one requester's change. An active-tab change made before it is
    /// reported first, so it keeps its own event; the change itself raises
    /// tab-switched only when the requester dispatches hooks.
    fn as_requester<T>(&mut self, requester: Requester, change: impl FnOnce(&mut Self) -> T) -> T {
        self.report_active_tab_switch();
        let result = change(self);
        if requester.dispatches_hooks() {
            self.report_active_tab_switch();
        } else {
            self.absorb_active_tab_switch();
        }
        result
    }

    /// Create one ordinary local tab synchronously. Live-domain tabs use an
    /// asynchronous topology round trip, so they are rejected until the
    /// requester can be acknowledged with the resulting stable local ID
    /// without blocking the event loop.
    pub(super) fn create_tab_for(
        &mut self,
        requester: Requester,
    ) -> std::result::Result<u64, String> {
        self.as_requester(requester, |app| app.create_tab_as(requester))
    }

    fn create_tab_as(&mut self, requester: Requester) -> std::result::Result<u64, String> {
        if self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .is_some_and(|tab| tab.live_domain_tab_route().is_some())
        {
            return Err(
                "Ordinary local tab creation is unavailable while the target window's active tab belongs to a live domain"
                    .into(),
            );
        }
        let id = self.create_local_tab_in(requester.dispatches_hooks(), None)?;
        u64::try_from(id).map_err(|_| "Created tab ID does not fit the control protocol".into())
    }

    /// Opens a shell tab, in `cwd` when given.
    pub(super) fn create_local_tab_in(
        &mut self,
        dispatch_plugin_hook: bool,
        cwd: Option<&std::path::Path>,
    ) -> std::result::Result<usize, String> {
        let tab_count = self.tabs.as_ref().map_or(0, TabManager::count);
        validate_ordinary_tab_creation_capacity(tab_count)?;
        let tabs = self.control_tabs_mut()?;
        let id = match cwd {
            Some(cwd) => tabs.new_tab_with_cwd(Some(&cwd.to_string_lossy()), None),
            None => tabs.new_tab(),
        }
        .map_err(|error| format!("Could not create a new tab: {error:#}"))?;
        log::info!("Created new tab {id}");
        self.maybe_show_splash(true);
        if dispatch_plugin_hook {
            self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated { tab_id: id });
        }
        self.refresh_focused_terminal_surfaces();
        Ok(id)
    }

    pub(super) fn select_tab_for(
        &mut self,
        requester: Requester,
        tab_id: u64,
    ) -> std::result::Result<bool, String> {
        self.as_requester(requester, |app| app.select_tab(tab_id))
    }

    fn select_tab(&mut self, tab_id: u64) -> std::result::Result<bool, String> {
        let (index, _) = self.control_tab_at(tab_id)?;
        let changed = self.control_tabs()?.active_index() != index;
        if changed {
            self.tabs
                .as_mut()
                .expect("validated tab manager disappeared")
                .switch_to(index);
            self.refresh_focused_terminal_surfaces();
            self.request_redraw(RedrawReason::Action);
        }
        Ok(changed)
    }

    /// Reorder one exact local tab without making it active. The tab manager
    /// validates the complete move before changing visible order.
    pub(super) fn move_control_tab_within_window(
        &mut self,
        tab_id: u64,
        destination_index: u16,
    ) -> std::result::Result<bool, String> {
        let tab_id = control_tab_id(tab_id)?;
        let changed = self
            .control_tabs_mut()?
            .move_tab_by_id_to(tab_id, usize::from(destination_index))
            .map_err(|error| format!("Could not move tab {tab_id}: {error:#}"))?;
        if changed {
            self.tab_dragging = None;
            self.request_redraw(RedrawReason::Action);
        }
        Ok(changed)
    }

    pub(super) fn move_control_pane_within_window(
        &mut self,
        source_tab_id: u64,
        pane_id: u32,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: SplitDirection,
    ) -> std::result::Result<u32, String> {
        let source_tab_id = usize::try_from(source_tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let destination_tab_id = usize::try_from(destination_tab_id)
            .map_err(|_| "Destination tab ID does not fit this platform".to_string())?;
        let id = self
            .control_tabs_mut()?
            .transfer_pane_by_id(
                source_tab_id,
                PaneId(pane_id),
                destination_tab_id,
                PaneId(target_pane_id),
                direction,
            )
            .map_err(|error| format!("Could not move pane {pane_id}: {error:#}"))?;
        self.broadcast = BroadcastState::Inactive;
        self.leave_copy_mode(false);
        self.terminal_mouse_capture = None;
        self.refresh_focused_terminal_surfaces();
        self.request_redraw(RedrawReason::Action);
        Ok(id.0)
    }

    pub(super) fn swap_plugin_panes(
        &mut self,
        tab_id: u64,
        first_pane_id: u32,
        second_pane_id: u32,
    ) -> std::result::Result<(Vec<u32>, u32, Option<u32>), String> {
        let active_before = self.tabs.as_ref().and_then(TabManager::active_tab_id);
        let state = {
            let manager = self.plugin_pane_layout_manager(tab_id)?;
            manager
                .swap_exact(PaneId(first_pane_id), PaneId(second_pane_id))
                .map_err(|error| format!("Could not swap panes: {error:#}"))?;
            plugin_pane_layout_state(manager)
        };
        self.finish_plugin_pane_layout(active_before);
        Ok(state)
    }

    pub(super) fn balance_plugin_panes(
        &mut self,
        tab_id: u64,
    ) -> std::result::Result<(bool, Vec<u32>, u32, Option<u32>), String> {
        let active_before = self.tabs.as_ref().and_then(TabManager::active_tab_id);
        let (changed, state) = {
            let manager = self.plugin_pane_layout_manager(tab_id)?;
            let changed = manager
                .balance_exact()
                .map_err(|error| format!("Could not balance panes: {error:#}"))?;
            (changed, plugin_pane_layout_state(manager))
        };
        self.finish_plugin_pane_layout(active_before);
        Ok((changed, state.0, state.1, state.2))
    }

    pub(super) fn apply_plugin_pane_layout(
        &mut self,
        tab_id: u64,
        preset: crate::panes::PaneLayoutPreset,
    ) -> std::result::Result<(Vec<u32>, u32, Option<u32>), String> {
        let active_before = self.tabs.as_ref().and_then(TabManager::active_tab_id);
        let state = {
            let manager = self.plugin_pane_layout_manager(tab_id)?;
            manager
                .apply_layout_preset_exact(preset)
                .map_err(|error| format!("Could not apply pane layout: {error:#}"))?;
            plugin_pane_layout_state(manager)
        };
        self.finish_plugin_pane_layout(active_before);
        Ok(state)
    }

    fn plugin_pane_layout_manager(
        &mut self,
        tab_id: u64,
    ) -> std::result::Result<&mut PaneManager, String> {
        let tab = self.control_tab_mut(tab_id)?;
        if tab.live_domain_tab_route().is_some() {
            return Err(format!(
                "GUI tab {tab_id} belongs to daemon-authoritative live-domain topology"
            ));
        }
        if matches!(tab.tab_type, crate::tabs::TabType::Serial { .. }) {
            return Err(format!("GUI tab {tab_id} is a serial terminal"));
        }
        tab.split_panes_mut()
            .ok_or_else(|| format!("GUI tab {tab_id} is not split"))
    }

    fn finish_plugin_pane_layout(&mut self, active_before: Option<usize>) {
        debug_assert_eq!(
            self.tabs.as_ref().and_then(TabManager::active_tab_id),
            active_before,
            "plugin pane layout must not change the active tab"
        );
        self.divider_dragging = None;
        self.refresh_focused_terminal_surfaces();
        self.request_redraw(RedrawReason::Action);
    }

    pub(super) fn resize_control_divider(
        &mut self,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    ) -> std::result::Result<(u16, bool), String> {
        let tab = self.control_tab_mut(tab_id)?;
        if tab.live_domain_tab_route().is_some() {
            return Err(
                "Control-requested divider resize is not yet supported for live-domain tabs".into(),
            );
        }
        let manager = tab
            .split_panes_mut()
            .ok_or_else(|| format!("GUI tab {tab_id} has no split dividers"))?;
        let result = manager
            .resize_local_divider(divider_id, ratio_millis)
            .map_err(|error| format!("Could not resize divider {divider_id}: {error:#}"))?;
        if result.1 {
            self.divider_dragging = None;
            self.request_redraw(RedrawReason::Action);
        }
        Ok(result)
    }

    pub(super) fn read_control_text(
        &self,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    ) -> std::result::Result<(crate::control::protocol::TextScreen, u16, String, bool), String>
    {
        // A pane whose shell has exited keeps its text, so it can be read.
        let terminal = &control_pane(self.control_tab(tab_id)?, pane_id)?.terminal;
        let alternate = terminal.presentation_alternate_screen_active();
        let (text, included_history, truncated) = terminal.bounded_presentation_text(
            usize::from(history_lines),
            usize::try_from(max_bytes).expect("u32 byte limit fits usize"),
        );
        let included_history =
            u16::try_from(included_history).expect("bounded terminal-text history fits u16");
        Ok((
            if alternate {
                crate::control::protocol::TextScreen::Alternate
            } else {
                crate::control::protocol::TextScreen::Primary
            },
            included_history,
            text,
            truncated,
        ))
    }

    /// Send bounded external text to an exact terminal in this window.
    pub(super) fn send_text_for(
        &mut self,
        requester: Requester,
        tab_id: u64,
        pane_id: u32,
        text: &str,
        paste: bool,
        enter: bool,
    ) -> std::result::Result<
        (
            u32,
            u32,
            crate::control::protocol::InputDisposition,
            Option<u64>,
        ),
        String,
    > {
        self.require_control_input()?;
        if self.guardrail.is_awaiting_current() {
            return Err("A destructive-command confirmation is already pending".into());
        }
        let (tab_id, origin, mode, read_only) = {
            let tab = self.control_tab(tab_id)?;
            let tab_id = tab.id;
            let pane = control_running_pane(tab, pane_id)?;
            (
                tab_id,
                crate::tabs::TerminalEventOrigin {
                    tab_id,
                    pane_id: PaneId(pane_id),
                },
                pane.terminal.mode(),
                pane.read_only,
            )
        };

        if read_only {
            return Err(format!("GUI tab {tab_id}, pane {pane_id} is read-only"));
        }

        let input_bytes = text.len().saturating_add(usize::from(enter));
        let mut wire = if paste {
            crate::input::encode_paste(mode, text)
        } else {
            text.as_bytes().to_vec()
        };
        if enter {
            wire.push(b'\r');
        }
        let input_bytes = u32::try_from(input_bytes).expect("bounded control input fits u32");
        let wire_bytes = u32::try_from(wire.len()).expect("bounded control wire input fits u32");

        // Every automated write, from the control socket (and so `termeric
        // mcp`) or a plugin, has a destructive command confirmed, in any tab.
        if let Some(matched) = crate::terminal::guardrails::check_command(text) {
            let target = if requester.names_target_window() {
                format!(
                    "GUI window {}, tab {tab_id}, pane {pane_id}",
                    self.control_window_id
                )
            } else {
                format!("GUI tab {tab_id}, pane {pane_id}")
            };
            let confirmation_id = self.guardrail.show(
                matched,
                target,
                vec![crate::terminal::guardrails::GuardedWrite::for_pane(
                    tab_id,
                    PaneId(pane_id),
                    wire,
                )],
            );
            self.request_redraw(RedrawReason::Action);
            return Ok((
                input_bytes,
                wire_bytes,
                crate::control::protocol::InputDisposition::PendingConfirmation,
                Some(confirmation_id),
            ));
        }

        self.control_tabs_mut()?
            .write_user_to_origin(origin, &wire)
            .map_err(|error| format!("Could not write GUI control input: {error}"))?;
        Ok((
            input_bytes,
            wire_bytes,
            crate::control::protocol::InputDisposition::Sent,
            None,
        ))
    }

    /// Select an exact tab and focus one exact stable pane within it. The
    /// complete path is validated before either focus component changes.
    pub(super) fn focus_pane_for(
        &mut self,
        requester: Requester,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<(bool, bool), String> {
        self.as_requester(requester, |app| app.focus_pane(tab_id, pane_id))
    }

    fn focus_pane(
        &mut self,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<(bool, bool), String> {
        let (index, tab) = self.control_tab_at(tab_id)?;
        let tab_id = tab.id;
        let target = control_running_pane(tab, pane_id)?.id;
        let pane_changed = tab.panes.focused_id() != target;
        let tab_changed = self.control_tabs()?.active_index() != index;

        let live_route = {
            let tabs = self
                .tabs
                .as_mut()
                .expect("validated tab manager disappeared");
            if tab_changed {
                tabs.switch_to(index);
            }
            let tab = tabs
                .tab_mut_by_id(tab_id)
                .expect("validated control tab disappeared");
            if pane_changed {
                tab.panes.focus(target);
            }
            tab.live_domain_focus_route()
                .map(|(domain, pane)| (domain.to_string(), pane))
        };
        if let (Some(authority), Some((domain, pane))) =
            (self.live_domain_authority.clone(), live_route)
        {
            crate::domain::request_focus_background(authority, domain, pane);
        }
        if tab_changed || pane_changed {
            self.refresh_focused_terminal_surfaces();
            self.request_redraw(RedrawReason::Action);
        }
        Ok((tab_changed, pane_changed))
    }

    /// Begin closing an ordinary tab. Returns the remaining tab count, the
    /// newly active tab (always present when the requester may not close the
    /// last tab), and whether the active tab changed.
    pub(super) fn close_tab_for(
        &mut self,
        requester: Requester,
        tab_id: u64,
    ) -> std::result::Result<(u16, Option<u64>, bool), String> {
        self.as_requester(requester, |app| app.close_tab_as(requester, tab_id))
    }

    fn close_tab_as(
        &mut self,
        requester: Requester,
        tab_id: u64,
    ) -> std::result::Result<(u16, Option<u64>, bool), String> {
        let (index, tab) = self.control_tab_at(tab_id)?;
        let tab_id = tab.id;
        if tab.live_domain_tab_route().is_some() {
            return Err(format!(
                "{} tab closure is not yet supported for live-domain tabs",
                requester.label()
            ));
        }
        let tabs = self.control_tabs()?;
        let non_closing_tabs = tabs
            .iter()
            .filter(|(_, tab)| !tab.closed && tab.closing_at.is_none())
            .count();
        if !requester.may_close_last_tab() && non_closing_tabs <= 1 {
            return Err(format!(
                "Cannot close the final non-closing tab {tab_id}; window closure requires separate authority"
            ));
        }
        let previous_active = tabs.active_tab_id();
        let close_copy_mode = self
            .copy_mode_target
            .is_some_and(|target| target.tab_id == tab_id);
        if !self
            .tabs
            .as_mut()
            .expect("validated tab manager disappeared")
            .close_tab(index)
        {
            return Err(format!("GUI tab {tab_id} could not begin closing"));
        }
        if close_copy_mode {
            self.leave_copy_mode(false);
        }
        if requester.dispatches_hooks() {
            self.dispatch_plugin_hook(crate::plugin::Hook::TabClosed { tab_id });
        }
        self.refresh_focused_terminal_surfaces();
        self.request_redraw(RedrawReason::Action);
        let active_local = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| !tab.closed && tab.closing_at.is_none())
            .map(|tab| tab.id);
        let active = active_local.map(|id| u64::try_from(id).expect("tab ID fits u64"));
        Ok((
            u16::try_from(non_closing_tabs - 1).expect("tab count is bounded below u16::MAX"),
            active,
            previous_active != active_local,
        ))
    }

    pub(super) fn close_pane_for(
        &mut self,
        requester: Requester,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<(u16, u32), String> {
        let tab = self.control_tab(tab_id)?;
        let tab_id = tab.id;
        if tab.live_domain_tab_route().is_some() {
            return Err(format!(
                "{} pane closure is not yet supported for live-domain tabs",
                requester.label()
            ));
        }
        if !tab.allows_pane_splits() {
            return Err("Serial terminal tabs do not contain closable panes".into());
        }
        if tab.split_panes().is_none() {
            return Err(format!(
                "Cannot close the sole pane {pane_id} in unsplit tab {tab_id}; close the tab instead"
            ));
        }
        let target = control_running_pane(tab, pane_id)?.id;
        let close_copy_mode = self.copy_mode_target
            == Some(crate::tabs::TerminalIdentity {
                tab_id,
                pane_id: target,
            });

        let (remaining, focused) = {
            let tab = self
                .tabs
                .as_mut()
                .expect("validated tab manager disappeared")
                .tab_mut_by_id(tab_id)
                .expect("validated control tab disappeared");
            if !tab.panes.close(target) {
                return Err(format!("GUI pane {pane_id} could not be closed"));
            }
            tab.reconcile_primary_pane_launch();
            (tab.panes.pane_count(), tab.panes.focused_id().0)
        };
        if close_copy_mode {
            self.leave_copy_mode(false);
        }
        if !matches!(self.broadcast, BroadcastState::Inactive) {
            self.broadcast = BroadcastState::Inactive;
            self.toasts.info("Broadcast cancelled after pane closure");
        }
        self.refresh_focused_terminal_surfaces();
        self.request_redraw(RedrawReason::Action);
        Ok((
            u16::try_from(remaining).expect("pane count is bounded below u16::MAX"),
            focused,
        ))
    }
}
