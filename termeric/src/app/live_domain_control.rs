//! Live-domain topology from the GUI side: attaching a window and following
//! its domain, queued control round trips to the daemon, their completions,
//! and reconciling the window to topology updates.

use super::control_actions::{control_pane, control_running_pane, control_tab_id};
use super::redraw_scheduler::RedrawReason;
use super::*;
use anyhow::Context;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum LiveDomainControl {
    SpawnTab {
        domain_id: String,
        spawn: crate::domain::model::SpawnSpec,
    },
    SplitPane {
        domain_id: String,
        remote_pane_id: u32,
        direction: crate::panes::SplitDirection,
        spawn: crate::domain::model::SpawnSpec,
    },
    ResizeSplit {
        domain_id: String,
        remote_split_id: u32,
        ratio_millis: u16,
    },
    RenameTab {
        domain_id: String,
        remote_tab_id: u32,
        title: Option<String>,
    },
    SetTabPinned {
        domain_id: String,
        remote_tab_id: u32,
        pinned: bool,
    },
    MoveTab {
        domain_id: String,
        remote_tab_id: u32,
        index: u16,
    },
    SwapPaneNext {
        domain_id: String,
        remote_pane_id: u32,
    },
    BalanceTab {
        domain_id: String,
        remote_tab_id: u32,
    },
    ApplyPaneLayout {
        domain_id: String,
        remote_tab_id: u32,
        preset: crate::panes::PaneLayoutPreset,
    },
    MovePane {
        domain_id: String,
        remote_pane_id: u32,
        source_tab_id: u32,
        destination_tab_id: u32,
        target_pane_id: u32,
        direction: crate::panes::SplitDirection,
    },
    ClosePane {
        domain_id: String,
        remote_pane_id: u32,
        local_tab_id: usize,
    },
    CloseTab {
        domain_id: String,
        remote_tab_id: u32,
        local_tab_id: usize,
    },
}

impl LiveDomainControl {
    pub(super) fn domain_id(&self) -> &str {
        match self {
            Self::SpawnTab { domain_id, .. }
            | Self::SplitPane { domain_id, .. }
            | Self::ResizeSplit { domain_id, .. }
            | Self::RenameTab { domain_id, .. }
            | Self::SetTabPinned { domain_id, .. }
            | Self::MoveTab { domain_id, .. }
            | Self::SwapPaneNext { domain_id, .. }
            | Self::BalanceTab { domain_id, .. }
            | Self::ApplyPaneLayout { domain_id, .. }
            | Self::MovePane { domain_id, .. }
            | Self::ClosePane { domain_id, .. }
            | Self::CloseTab { domain_id, .. } => domain_id,
        }
    }

    pub(super) fn request(&self) -> crate::domain::protocol::Request {
        match self {
            Self::SpawnTab { domain_id, spawn } => crate::domain::protocol::Request::SpawnTab {
                domain: domain_id.clone(),
                spawn: spawn.clone(),
            },
            Self::SplitPane {
                domain_id,
                remote_pane_id,
                direction,
                spawn,
            } => crate::domain::protocol::Request::SplitPane {
                domain: domain_id.clone(),
                pane: *remote_pane_id,
                direction: *direction,
                spawn: spawn.clone(),
            },
            Self::ResizeSplit {
                domain_id,
                remote_split_id,
                ratio_millis,
            } => crate::domain::protocol::Request::ResizeSplit {
                domain: domain_id.clone(),
                split: *remote_split_id,
                ratio_millis: *ratio_millis,
            },
            Self::RenameTab {
                domain_id,
                remote_tab_id,
                title,
            } => crate::domain::protocol::Request::RenameTab {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
                title: title.clone(),
            },
            Self::SetTabPinned {
                domain_id,
                remote_tab_id,
                pinned,
            } => crate::domain::protocol::Request::SetTabPinned {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
                pinned: *pinned,
            },
            Self::MoveTab {
                domain_id,
                remote_tab_id,
                index,
            } => crate::domain::protocol::Request::MoveTab {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
                index: *index,
            },
            Self::SwapPaneNext {
                domain_id,
                remote_pane_id,
            } => crate::domain::protocol::Request::SwapPaneNext {
                domain: domain_id.clone(),
                pane: *remote_pane_id,
            },
            Self::BalanceTab {
                domain_id,
                remote_tab_id,
            } => crate::domain::protocol::Request::BalanceTab {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
            },
            Self::ApplyPaneLayout {
                domain_id,
                remote_tab_id,
                preset,
            } => crate::domain::protocol::Request::ApplyPaneLayout {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
                preset: *preset,
            },
            Self::MovePane {
                domain_id,
                remote_pane_id,
                source_tab_id: _,
                destination_tab_id,
                target_pane_id,
                direction,
            } => crate::domain::protocol::Request::MovePane {
                domain: domain_id.clone(),
                pane: *remote_pane_id,
                tab: *destination_tab_id,
                target: Some(*target_pane_id),
                direction: *direction,
            },
            Self::ClosePane {
                domain_id,
                remote_pane_id,
                ..
            } => crate::domain::protocol::Request::ClosePane {
                domain: domain_id.clone(),
                pane: *remote_pane_id,
            },
            Self::CloseTab {
                domain_id,
                remote_tab_id,
                ..
            } => crate::domain::protocol::Request::CloseTab {
                domain: domain_id.clone(),
                tab: *remote_tab_id,
            },
        }
    }
}

/// What a completed live-domain change reports to the person.
fn live_domain_control_message(control: &LiveDomainControl) -> String {
    match control {
        LiveDomainControl::SpawnTab { .. } => "Created live-domain tab".to_string(),
        LiveDomainControl::SplitPane { remote_pane_id, .. } => {
            format!("Split live-domain pane {remote_pane_id}")
        }
        LiveDomainControl::ResizeSplit {
            remote_split_id,
            ratio_millis,
            ..
        } => format!("Resized live-domain split {remote_split_id} to {ratio_millis}‰"),
        LiveDomainControl::RenameTab {
            remote_tab_id,
            title,
            ..
        } => title.as_deref().map_or_else(
            || format!("Cleared live-domain tab {remote_tab_id} title"),
            |title| format!("Renamed live-domain tab {remote_tab_id} to {title}"),
        ),
        LiveDomainControl::SetTabPinned {
            remote_tab_id,
            pinned,
            ..
        } => format!(
            "{} live-domain tab {remote_tab_id}",
            if *pinned { "Pinned" } else { "Unpinned" }
        ),
        LiveDomainControl::MoveTab {
            remote_tab_id,
            index,
            ..
        } => format!("Moved live-domain tab {remote_tab_id} to position {index}"),
        LiveDomainControl::SwapPaneNext { remote_pane_id, .. } => {
            format!("Swapped live-domain pane {remote_pane_id} with its next pane")
        }
        LiveDomainControl::BalanceTab { remote_tab_id, .. } => {
            format!("Balanced live-domain tab {remote_tab_id}")
        }
        LiveDomainControl::ApplyPaneLayout {
            remote_tab_id,
            preset,
            ..
        } => format!("Applied {preset:?} layout to live-domain tab {remote_tab_id}"),
        LiveDomainControl::MovePane {
            remote_pane_id,
            destination_tab_id,
            ..
        } => format!("Moved live-domain pane {remote_pane_id} to tab {destination_tab_id}"),
        LiveDomainControl::ClosePane { remote_pane_id, .. } => {
            format!("Closed live-domain pane {remote_pane_id}")
        }
        LiveDomainControl::CloseTab { remote_tab_id, .. } => {
            format!("Closed live-domain tab {remote_tab_id}")
        }
    }
}

impl App {
    /// Attach this window to the live domain `target` at startup: its tabs
    /// become relay tabs, and the window follows the domain's topology.
    pub(super) fn attach_live_domain_window(
        &mut self,
        target: crate::domain::DomainTarget,
    ) -> Result<()> {
        let domain_id = target.domain_id;
        let domain = if let Some(domain) = self.launch.domain_snapshot.take() {
            if domain.id != domain_id {
                anyhow::bail!("prepared domain snapshot does not match its launch target");
            }
            domain
        } else {
            target.authority.client()?.describe(domain_id.clone())?
        };
        let executable = crate::paths::executable_text()?;
        let relay_command = target.authority.relay_command(executable);
        self.tabs
            .as_mut()
            .context("tab manager was lost during domain attachment")?
            .attach_domain(&domain, &relay_command)?;
        self.live_domain_topologies.insert(
            domain_id.clone(),
            crate::domain::model::DomainTopology::from(&domain),
        );
        self.live_domain_authority = Some(target.authority.clone());
        self.live_domain_relay_command = Some(relay_command);
        self.start_live_domain_topology_subscription(target.authority.clone(), domain_id.clone())?;
        // A live domain is not a snapshot workspace. Never auto-save relay
        // processes as if they were relaunchable shell sessions.
        self.automatic_persistence = false;
        log::info!(
            "Attached window to persistent domain {domain_id} via {:?}",
            target.authority
        );
        Ok(())
    }

    fn start_live_domain_topology_subscription(
        &mut self,
        authority: crate::domain::DomainAuthority,
        domain_id: String,
    ) -> Result<()> {
        if self.window.is_none() {
            anyhow::bail!("live-domain topology requires a native window");
        }
        let window_key = self.window_key();
        let proxy = EVENT_LOOP_PROXY
            .get()
            .cloned()
            .context("live-domain topology event channel is unavailable")?;
        let alive = Arc::clone(&self.live_domain_topology_alive);
        std::thread::Builder::new()
            .name("termeric-domain-topology".into())
            .spawn(move || {
                let mut failure_reported = false;
                let mut retry_delay = std::time::Duration::from_millis(250);
                while alive.load(Ordering::Acquire) {
                    let mut window_open = true;
                    let mut domain_closed = false;
                    let result = authority.client().and_then(|client| {
                        client.subscribe_topology(domain_id.clone(), &alive, |event| {
                            domain_closed =
                                matches!(event, crate::domain::DomainTopologyEvent::Closed { .. });
                            failure_reported = false;
                            retry_delay = std::time::Duration::from_millis(250);
                            window_open = proxy
                                .send_event(AppEvent::WindowResult {
                                    window: window_key,
                                    result: WindowResult::LiveDomainTopology {
                                        domain_id: domain_id.clone(),
                                        result: Ok(event),
                                    },
                                })
                                .is_ok();
                            window_open && !domain_closed
                        })
                    });
                    if !alive.load(Ordering::Acquire) || !window_open || domain_closed {
                        return;
                    }
                    if let Err(error) = result
                        && !failure_reported
                    {
                        failure_reported = proxy
                            .send_event(AppEvent::WindowResult {
                                window: window_key,
                                result: WindowResult::LiveDomainTopology {
                                    domain_id: domain_id.clone(),
                                    result: Err(format!("{error:#}")),
                                },
                            })
                            .is_ok();
                        if !failure_reported {
                            return;
                        }
                    }
                    std::thread::sleep(retry_delay);
                    retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(5));
                }
            })
            .context("start live-domain topology subscriber")?;
        Ok(())
    }

    pub(super) fn persist_live_domain_focus(&self) {
        let route = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(crate::tabs::Tab::live_domain_focus_route)
            .map(|(domain, pane)| (domain.to_string(), pane));
        if let (Some(authority), Some((domain, pane))) = (self.live_domain_authority.clone(), route)
        {
            crate::domain::request_focus_background(authority, domain, pane);
        }
    }

    /// Whether a change to live domain `domain` awaits the daemon. Each
    /// domain has at most one in flight, so its changes reach the daemon in
    /// the order the person made them.
    pub(super) fn live_domain_busy(&self, domain: &str) -> bool {
        self.pending_live_domain_controls
            .iter()
            .any(|pending| pending.domain_id() == domain)
    }

    pub(super) fn queue_live_domain_control(&mut self, control: LiveDomainControl) {
        if let Err(error) = self.start_live_domain_control(control) {
            self.toasts.error(error);
        }
    }

    pub(super) fn start_live_domain_control(
        &mut self,
        control: LiveDomainControl,
    ) -> std::result::Result<(), String> {
        if self.live_domain_busy(control.domain_id()) {
            self.restore_live_domain_preview(&control);
            return Err("Another change for this live domain is already pending".into());
        }
        let inserted = self.pending_live_domain_controls.insert(control.clone());
        debug_assert!(inserted);
        if self.window.is_none() {
            self.pending_live_domain_controls.remove(&control);
            self.restore_live_domain_preview(&control);
            return Err("Live-domain controls require an attached native window".into());
        }
        let window_key = self.window_key();
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            self.pending_live_domain_controls.remove(&control);
            self.restore_live_domain_preview(&control);
            return Err("Live-domain control channel is unavailable".into());
        };
        let Some(authority) = self.live_domain_authority.clone() else {
            self.pending_live_domain_controls.remove(&control);
            self.restore_live_domain_preview(&control);
            return Err("Live-domain control authority is unavailable".into());
        };
        let worker_control = control.clone();
        let spawn = std::thread::Builder::new()
            .name("termeric-domain-control".into())
            .spawn(move || {
                let result = authority
                    .client()
                    .and_then(|client| client.request(worker_control.request()))
                    .and_then(|response| match response {
                        crate::domain::protocol::Response::Domain { domain } => Ok(domain),
                        _ => anyhow::bail!(
                            "domain server returned an invalid topology-control response"
                        ),
                    })
                    .map_err(|error| format!("{error:#}"));
                let _ = proxy.send_event(AppEvent::LiveDomainControlCompleted {
                    window: window_key,
                    control: worker_control,
                    result,
                });
            });
        if let Err(error) = spawn {
            self.pending_live_domain_controls.remove(&control);
            self.restore_live_domain_preview(&control);
            return Err(format!("Could not start live-domain control: {error}"));
        }
        Ok(())
    }

    fn restore_live_domain_preview(&mut self, control: &LiveDomainControl) {
        let domain_id = match control {
            LiveDomainControl::ResizeSplit { domain_id, .. }
            | LiveDomainControl::RenameTab { domain_id, .. }
            | LiveDomainControl::MoveTab { domain_id, .. } => domain_id,
            LiveDomainControl::SpawnTab { .. }
            | LiveDomainControl::SplitPane { .. }
            | LiveDomainControl::SetTabPinned { .. }
            | LiveDomainControl::SwapPaneNext { .. }
            | LiveDomainControl::BalanceTab { .. }
            | LiveDomainControl::ApplyPaneLayout { .. }
            | LiveDomainControl::MovePane { .. }
            | LiveDomainControl::ClosePane { .. }
            | LiveDomainControl::CloseTab { .. } => return,
        };
        let Some(topology) = self.live_domain_topologies.get(domain_id).cloned() else {
            return;
        };
        let Some(relay_command) = self.live_domain_relay_command.as_ref() else {
            return;
        };
        let Some(tabs) = &mut self.tabs else {
            return;
        };
        match tabs.reconcile_live_domain(&topology, relay_command) {
            Ok(()) => self.sync_after_live_domain_topology(),
            Err(error) => log::error!("Could not restore rejected live-domain preview: {error:#}"),
        }
    }

    /// A new live-domain pane beside the active terminal, in its visible
    /// working directory when `inherit_visible_cwd`.
    pub(super) fn live_domain_spawn_spec(
        &self,
        inherit_visible_cwd: bool,
    ) -> crate::domain::model::SpawnSpec {
        let terminal = self.active_terminal();
        let working_directory = terminal
            .filter(|_| inherit_visible_cwd)
            .and_then(crate::terminal::process::terminal_working_directory);
        self.live_domain_spawn_spec_sized_like(terminal, working_directory)
    }

    /// The configured shell as a live-domain pane the size of `terminal`
    /// (the daemon's default without one), clamped to what the daemon
    /// accepts.
    fn live_domain_spawn_spec_sized_like(
        &self,
        terminal: Option<&termeric_core::Terminal>,
        working_directory: Option<String>,
    ) -> crate::domain::model::SpawnSpec {
        use crate::domain::model::{MAX_TERMINAL_CELLS_PER_SIDE, SpawnSpec};

        let cells = |count: usize| count.min(usize::from(MAX_TERMINAL_CELLS_PER_SIDE)) as u16;
        let mut spec = SpawnSpec {
            program: self.config.shell.program.clone(),
            args: self.config.shell.args.clone(),
            working_directory,
            ..SpawnSpec::default()
        };
        if let Some(terminal) = terminal {
            spec.cols = cells(terminal.cols());
            spec.rows = cells(terminal.rows());
        }
        spec
    }

    pub(super) fn control_live_domain_create_tab(
        &self,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            return Err("Target window has no active tab".into());
        };
        if tab.closed || tab.closing_at.is_some() {
            return Err(format!("GUI tab {} is already closing", tab.id));
        }
        let Some((domain_id, _)) = tab.live_domain_tab_route() else {
            return Ok(None);
        };
        Ok(Some(LiveDomainControl::SpawnTab {
            domain_id: domain_id.to_string(),
            spawn: self.live_domain_spawn_spec(true),
        }))
    }

    pub(super) fn control_live_domain_split(
        &self,
        tab_id: u64,
        pane_id: u32,
        direction: crate::panes::SplitDirection,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let tab = self.control_tab(tab_id)?;
        let Some(live) = &tab.live_domain else {
            return Ok(None);
        };
        let remote_pane_id = *live
            .pane_ids
            .get(&PaneId(pane_id))
            .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {}", tab.id))?;
        let terminal = &control_pane(tab, pane_id)?.terminal;
        let spawn = self.live_domain_spawn_spec_sized_like(Some(terminal), None);
        Ok(Some(LiveDomainControl::SplitPane {
            domain_id: live.domain_id.clone(),
            remote_pane_id,
            direction,
            spawn,
        }))
    }

    pub(super) fn control_live_domain_close_tab(
        &self,
        tab_id: u64,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let tab = self.control_tab(tab_id)?;
        let tab_id = tab.id;
        let Some((domain_id, remote_tab_id)) = tab.live_domain_tab_route() else {
            return Ok(None);
        };
        let tabs = self.control_tabs()?;
        let domain_tabs = self.live_domain_topologies.get(domain_id).map_or_else(
            || tabs.live_domain_tab_count(domain_id),
            |topology| topology.tabs.len(),
        );
        if domain_tabs <= 1 {
            return Err(
                "Cannot close the domain's last tab; use `termeric domain kill` to terminate it"
                    .into(),
            );
        }
        Ok(Some(LiveDomainControl::CloseTab {
            domain_id: domain_id.to_string(),
            remote_tab_id,
            local_tab_id: tab_id,
        }))
    }

    pub(super) fn control_live_domain_close_pane(
        &self,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let tab = self.control_tab(tab_id)?;
        let tab_id = tab.id;
        let Some(live) = &tab.live_domain else {
            return Ok(None);
        };
        let remote_pane_id = *live
            .pane_ids
            .get(&PaneId(pane_id))
            .ok_or_else(|| format!("GUI pane {pane_id} does not exist in tab {tab_id}"))?;
        if tab.panes.pane_count() <= 1 {
            return Err(format!(
                "Cannot close the sole pane {pane_id} in unsplit tab {tab_id}; close the tab instead"
            ));
        }
        control_running_pane(tab, pane_id)?;
        Ok(Some(LiveDomainControl::ClosePane {
            domain_id: live.domain_id.clone(),
            remote_pane_id,
            local_tab_id: tab_id,
        }))
    }

    pub(super) fn control_live_pane_status(
        &self,
        tab_id: u64,
    ) -> std::result::Result<(u16, u32), String> {
        let tab_id = control_tab_id(tab_id)?;
        let tab = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(tab_id))
            .ok_or_else(|| format!("GUI tab {tab_id} disappeared after live-domain closure"))?;
        let (count, focused) = (tab.panes.pane_count(), tab.panes.focused_id().0);
        Ok((
            u16::try_from(count).expect("pane count is bounded below u16::MAX"),
            focused,
        ))
    }

    pub(super) fn control_live_domain_resize_divider(
        &self,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    ) -> std::result::Result<Option<(LiveDomainControl, u16)>, String> {
        if !termeric_core::split::RATIO_MILLIS.contains(&ratio_millis) {
            return Err("Split ratio must be between 100 and 900 thousandths".into());
        }
        let tab = self.control_tab(tab_id)?;
        let tab_id = tab.id;
        let Some((domain_id, _)) = tab.live_domain_tab_route() else {
            return Ok(None);
        };
        let remote_split_id = u32::try_from(divider_id)
            .map_err(|_| format!("GUI divider {divider_id} is not a live-domain divider"))?;
        let previous_ratio_millis = tab
            .split_panes()
            .ok_or_else(|| format!("GUI tab {tab_id} has no split dividers"))?
            .get_dividers()
            .into_iter()
            .find_map(|divider| {
                (divider.target == crate::panes::DividerTarget::Domain(remote_split_id))
                    .then_some(divider.ratio_millis)
            })
            .ok_or_else(|| format!("GUI divider {divider_id} does not exist in tab {tab_id}"))?;
        Ok(Some((
            LiveDomainControl::ResizeSplit {
                domain_id: domain_id.to_string(),
                remote_split_id,
                ratio_millis,
            },
            previous_ratio_millis,
        )))
    }

    pub(super) fn control_live_divider_ratio(
        &self,
        tab_id: u64,
        divider_id: u64,
    ) -> std::result::Result<u16, String> {
        let tab_id = control_tab_id(tab_id)?;
        let remote_split_id = u32::try_from(divider_id)
            .map_err(|_| format!("GUI divider {divider_id} is not a live-domain divider"))?;
        self.tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(tab_id))
            .and_then(|tab| {
                tab.panes.get_dividers().into_iter().find_map(|divider| {
                    (divider.target == crate::panes::DividerTarget::Domain(remote_split_id))
                        .then_some(divider.ratio_millis)
                })
            })
            .ok_or_else(|| {
                format!("GUI divider {divider_id} is unavailable after live-domain resize")
            })
    }

    pub(super) fn control_live_domain_move_tab(
        &self,
        tab_id: u64,
        destination_index: u16,
    ) -> std::result::Result<Option<(LiveDomainControl, bool)>, String> {
        let tab_id = control_tab_id(tab_id)?;
        let destination_index = usize::from(destination_index);
        let tabs = self.control_tabs()?;
        if destination_index >= tabs.count() {
            return Err(format!(
                "Destination tab index {destination_index} is outside this window's {} tabs",
                tabs.count()
            ));
        }
        let (source_index, source) = tabs
            .iter()
            .find(|(_, tab)| tab.id == tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the source window"))?;
        if source.closed || source.closing_at.is_some() {
            return Err(format!("GUI tab {tab_id} is already closing"));
        }
        let Some((domain_id, remote_tab_id)) = source.live_domain_tab_route() else {
            return Ok(None);
        };
        let destination = tabs
            .iter()
            .find_map(|(index, tab)| (index == destination_index).then_some(tab))
            .expect("bounded destination tab index remains present");
        let Some((destination_domain, destination_remote_tab)) =
            destination.live_domain_tab_route()
        else {
            return Err(
                "A live-domain tab can move only to a visual slot owned by the same domain".into(),
            );
        };
        if destination_domain != domain_id {
            return Err(
                "A live-domain tab can move only to a visual slot owned by the same domain".into(),
            );
        }
        if destination.pinned != source.pinned {
            return Err("Destination tab index crosses the pinned-tab boundary".into());
        }
        let domain_index = tabs
            .live_domain_tab_index(domain_id, destination_remote_tab)
            .expect("the destination is one of the domain's tabs");
        let topology = self
            .live_domain_topologies
            .get(domain_id)
            .ok_or_else(|| "Authoritative live-domain topology is unavailable".to_string())?;
        if topology.tabs.get(domain_index).map(|tab| tab.id) != Some(destination_remote_tab) {
            return Err(
                "GUI live-domain tab order is stale; wait for topology reconciliation".into(),
            );
        }
        let domain_index = u16::try_from(domain_index)
            .map_err(|_| "Live-domain tab index does not fit the control protocol".to_string())?;
        Ok(Some((
            LiveDomainControl::MoveTab {
                domain_id: domain_id.to_string(),
                remote_tab_id,
                index: domain_index,
            },
            source_index != destination_index,
        )))
    }

    pub(super) fn control_tab_visual_index(&self, tab_id: u64) -> std::result::Result<u16, String> {
        let tab_id = control_tab_id(tab_id)?;
        let index = self
            .tabs
            .as_ref()
            .and_then(|tabs| {
                tabs.iter()
                    .find_map(|(index, tab)| (tab.id == tab_id).then_some(index))
            })
            .ok_or_else(|| format!("GUI tab {tab_id} disappeared after live-domain movement"))?;
        u16::try_from(index).map_err(|_| "GUI tab index does not fit the control protocol".into())
    }

    pub(super) fn control_live_domain_move_pane(
        &self,
        source_tab_id: u64,
        pane_id: u32,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: crate::panes::SplitDirection,
    ) -> std::result::Result<Option<LiveDomainControl>, String> {
        let source_tab_id = usize::try_from(source_tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let destination_tab_id = usize::try_from(destination_tab_id)
            .map_err(|_| "Destination tab ID does not fit this platform".to_string())?;
        if source_tab_id == destination_tab_id {
            return Err("Moving a pane within the same tab is not supported".into());
        }
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| "Source window has no initialized tab manager".to_string())?;
        let source = tabs.tab_by_id(source_tab_id).ok_or_else(|| {
            format!("GUI tab {source_tab_id} does not exist in the source window")
        })?;
        let destination = tabs.tab_by_id(destination_tab_id).ok_or_else(|| {
            format!("GUI tab {destination_tab_id} does not exist in the destination window")
        })?;
        if source.closed || source.closing_at.is_some() {
            return Err(format!("GUI tab {source_tab_id} is already closing"));
        }
        if destination.closed || destination.closing_at.is_some() {
            return Err(format!("GUI tab {destination_tab_id} is already closing"));
        }
        let (source_live, destination_live) = (&source.live_domain, &destination.live_domain);
        let (Some(source_live), Some(destination_live)) = (source_live, destination_live) else {
            if source_live.is_none() && destination_live.is_none() {
                return Ok(None);
            }
            return Err("Live-domain panes can move only between tabs in the same domain".into());
        };
        if source_live.domain_id != destination_live.domain_id {
            return Err("Live-domain panes can move only between tabs in the same domain".into());
        }
        let source_pane = PaneId(pane_id);
        let remote_pane_id = *source_live.pane_ids.get(&source_pane).ok_or_else(|| {
            format!("GUI pane {pane_id} does not exist in source tab {source_tab_id}")
        })?;
        let source_manager = source.split_panes().ok_or_else(|| {
            format!("Cannot move the sole pane {pane_id} from unsplit tab {source_tab_id}")
        })?;
        if source_manager.pane_count() <= 1 {
            return Err(format!(
                "Cannot move the last pane {pane_id} from tab {source_tab_id}"
            ));
        }
        if source_manager
            .get_pane(source_pane)
            .is_none_or(|pane| pane.closed)
        {
            return Err(format!(
                "GUI pane {pane_id} in source tab {source_tab_id} is closed or missing"
            ));
        }
        let target = PaneId(target_pane_id);
        let remote_target_pane_id = *destination_live
            .pane_ids
            .get(&target)
            .ok_or_else(|| {
                format!(
                    "GUI target pane {target_pane_id} does not exist in destination tab {destination_tab_id}"
                )
            })?;
        if destination.pane(target).is_none_or(|pane| pane.closed) {
            return Err(format!(
                "GUI target pane {target_pane_id} in destination tab {destination_tab_id} is closed or missing"
            ));
        }
        Ok(Some(LiveDomainControl::MovePane {
            domain_id: source_live.domain_id.clone(),
            remote_pane_id,
            source_tab_id: source_live.tab_id,
            destination_tab_id: destination_live.tab_id,
            target_pane_id: remote_target_pane_id,
            direction,
        }))
    }

    pub(super) fn control_local_live_tab_id(
        &self,
        domain_id: &str,
        remote_tab_id: u32,
    ) -> Option<u64> {
        self.tabs.as_ref()?.all_tabs().iter().find_map(|tab| {
            (tab.live_domain_tab_route() == Some((domain_id, remote_tab_id)))
                .then_some(tab.id as u64)
        })
    }

    pub(super) fn control_local_live_pane_id(
        &self,
        domain_id: &str,
        remote_tab_id: u32,
        remote_pane_id: u32,
    ) -> Option<u32> {
        self.tabs.as_ref()?.all_tabs().iter().find_map(|tab| {
            let live = tab.live_domain.as_ref()?;
            if live.domain_id != domain_id || live.tab_id != remote_tab_id {
                return None;
            }
            live.pane_ids
                .iter()
                .find_map(|(local, remote)| (*remote == remote_pane_id).then_some(local.0))
        })
    }

    /// This window's tabs of `domain_id`, by remote tab ID.
    fn live_tab_ids(&self, domain_id: &str) -> std::collections::HashMap<u32, usize> {
        self.tabs
            .as_ref()
            .map(|tabs| {
                tabs.live_domain_tabs(domain_id)
                    .filter_map(|tab| {
                        tab.live_domain_tab_route()
                            .map(|(_, remote)| (remote, tab.id))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Make this window's tabs match `topology`, remember it, keep a rename
    /// draft's title over the domain's, tell plugins which tabs appeared and
    /// went away, and resize.
    fn reconcile_live_domain_and_notify(
        &mut self,
        topology: &crate::domain::model::DomainTopology,
    ) -> anyhow::Result<()> {
        let before = self.live_tab_ids(&topology.domain_id);
        let relay = self
            .live_domain_relay_command
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("live-domain relay state is unavailable"))?;
        self.tabs
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("tab manager is unavailable"))?
            .reconcile_live_domain(topology, relay)?;
        self.live_domain_topologies
            .insert(topology.domain_id.clone(), topology.clone());
        if let Some(rename) = &self.tab_rename
            && let Some(tab) = self
                .tabs
                .as_mut()
                .and_then(|tabs| tabs.tab_mut_by_id(rename.tab_id))
        {
            tab.set_user_title(Some(rename.input.text().to_owned()));
        }
        let after = self.live_tab_ids(&topology.domain_id);
        for (remote, tab_id) in &after {
            if !before.contains_key(remote) {
                self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated { tab_id: *tab_id });
            }
        }
        for (remote, tab_id) in &before {
            if !topology.tabs.iter().any(|tab| tab.id == *remote) {
                self.dispatch_plugin_hook(crate::plugin::Hook::TabClosed { tab_id: *tab_id });
            }
        }
        self.sync_after_live_domain_topology();
        Ok(())
    }

    /// Bring broadcast targets, terminal sizes, and the focused terminal's
    /// surfaces in line with a live domain's tabs after a topology update.
    /// Every update is synchronized: a revision can move, split, or close
    /// panes without changing which tabs exist.
    fn sync_after_live_domain_topology(&mut self) {
        self.reconcile_broadcast_state();
        if let Some(size) = self.window.as_ref().map(|window| window.inner_size()) {
            self.sync_terminal_geometry(size.width, size.height);
        }
        self.refresh_focused_terminal_surfaces();
    }

    pub(super) fn handle_live_domain_control_completed(
        &mut self,
        control: LiveDomainControl,
        result: std::result::Result<crate::domain::model::DomainSnapshot, String>,
    ) {
        self.pending_live_domain_controls.remove(&control);
        let Ok(snapshot) = result else {
            let error = result.unwrap_err();
            self.restore_live_domain_preview(&control);
            self.toasts
                .error(format!("Live-domain change failed: {error}"));
            self.request_redraw(RedrawReason::BackgroundTask);
            return;
        };
        let topology = crate::domain::model::DomainTopology::from(&snapshot);
        if let Err(error) = self.reconcile_live_domain_and_notify(&topology) {
            self.toasts.error(format!(
                "Live-domain mutation succeeded but GUI reconciliation failed: {error:#}"
            ));
        }
        let message = live_domain_control_message(&control);
        let closed = match control {
            LiveDomainControl::ClosePane {
                remote_pane_id,
                local_tab_id,
                ..
            } => self.tabs.as_mut().is_some_and(|tabs| {
                tabs.reconcile_closed_live_domain_pane(local_tab_id, remote_pane_id)
            }),
            LiveDomainControl::CloseTab { local_tab_id, .. } => {
                let closed = self
                    .tabs
                    .as_mut()
                    .is_some_and(|tabs| tabs.close_tab_by_id(local_tab_id));
                if closed {
                    self.dispatch_plugin_hook(crate::plugin::Hook::TabClosed {
                        tab_id: local_tab_id,
                    });
                }
                closed
            }
            _ => false,
        };
        if closed {
            self.sync_after_live_domain_topology();
        }
        self.toasts.success(message);
        self.request_redraw(RedrawReason::BackgroundTask);
    }

    pub(super) fn handle_live_domain_topology(
        &mut self,
        domain_id: String,
        result: std::result::Result<crate::domain::DomainTopologyEvent, String>,
    ) {
        let event = match result {
            Ok(event) => event,
            Err(error) => {
                if self.disconnected_live_domains.insert(domain_id.clone()) {
                    let route = self.live_domain_route_label(&domain_id);
                    self.toasts
                        .warning(format!("Live {route} disconnected; retrying: {error}"));
                }
                self.request_redraw(RedrawReason::BackgroundTask);
                return;
            }
        };
        match event {
            crate::domain::DomainTopologyEvent::Snapshot { revision, domain } => {
                if domain.domain_id != domain_id {
                    self.toasts.error(format!(
                        "Rejected live-domain topology for {}; expected {domain_id}",
                        domain.domain_id
                    ));
                    self.request_redraw(RedrawReason::BackgroundTask);
                    return;
                }
                let applied_revision = self.live_domain_revisions.get(&domain.domain_id).copied();
                let recovering = self.disconnected_live_domains.contains(&domain_id);
                if applied_revision.is_some_and(|applied| {
                    applied > revision || (applied == revision && !recovering)
                }) {
                    return;
                }
                if self.live_domain_relay_command.is_none() {
                    self.toasts.error(
                        "Live-domain topology arrived without a configured relay executable",
                    );
                    return;
                }
                match self.reconcile_live_domain_and_notify(&domain) {
                    Ok(()) => {
                        self.live_domain_revisions
                            .insert(domain.domain_id.clone(), revision);
                        if self.disconnected_live_domains.remove(&domain_id) {
                            let route = self.live_domain_route_label(&domain_id);
                            self.toasts.success(format!("Reconnected live {route}"));
                        }
                    }
                    Err(error) => self.toasts.error(format!(
                        "Could not apply live-domain topology revision {revision}: {error:#}"
                    )),
                }
            }
            crate::domain::DomainTopologyEvent::Closed { domain, message } => {
                if domain != domain_id {
                    self.toasts.error(format!(
                        "Rejected live-domain close for {domain}; expected {domain_id}"
                    ));
                    self.request_redraw(RedrawReason::BackgroundTask);
                    return;
                }
                self.disconnected_live_domains.remove(&domain);
                self.live_domain_revisions.remove(&domain);
                self.live_domain_topologies.remove(&domain);
                let closed = self
                    .tabs
                    .as_mut()
                    .map_or(0, |tabs| tabs.close_live_domain_tabs(&domain));
                if closed > 0 {
                    self.reconcile_broadcast_state();
                    self.refresh_focused_terminal_surfaces();
                }
                self.toasts
                    .info(format!("Live domain {domain} closed: {message}"));
            }
        }
        self.request_redraw(RedrawReason::BackgroundTask);
    }

    fn live_domain_route_label(&self, domain_id: &str) -> String {
        match &self.live_domain_authority {
            Some(crate::domain::DomainAuthority::Local) => {
                format!("domain {domain_id} on this host")
            }
            Some(crate::domain::DomainAuthority::Ssh { destination }) => {
                format!("domain {domain_id} via SSH to {destination}")
            }
            None => format!("domain {domain_id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_domain_disconnect_feedback_names_the_ssh_route() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.live_domain_authority = Some(crate::domain::DomainAuthority::Ssh {
            destination: "builder@example.test".into(),
        });

        app.handle_live_domain_topology("domain-ssh".into(), Err("connection reset".into()));

        let toast = app.toasts.iter().last().unwrap();
        assert_eq!(toast.severity, crate::ui::toast::ToastSeverity::Warning);
        assert_eq!(
            toast.message,
            "Live domain domain-ssh via SSH to builder@example.test disconnected; retrying: connection reset"
        );
    }

    #[test]
    fn failed_topology_reconciliation_does_not_declare_recovery() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.live_domain_authority = Some(crate::domain::DomainAuthority::Local);
        app.live_domain_relay_command = Some(crate::domain::DomainRelayCommand::local("ignored"));
        app.handle_live_domain_topology("domain-reconcile".into(), Err("socket closed".into()));

        app.handle_live_domain_topology(
            "domain-reconcile".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 1,
                domain: crate::domain::model::DomainTopology {
                    domain_id: "domain-reconcile".into(),
                    active_tab: 0,
                    tabs: Vec::new(),
                },
            }),
        );

        assert!(app.disconnected_live_domains.contains("domain-reconcile"));
        assert!(app.toasts.iter().any(|toast| {
            toast.severity == crate::ui::toast::ToastSeverity::Error
                && toast.message.contains("tab manager is unavailable")
        }));
        assert!(
            app.toasts
                .iter()
                .all(|toast| toast.severity != crate::ui::toast::ToastSeverity::Success)
        );
    }

    #[test]
    fn domain_control_completion_is_stable_id_routed_idempotent_and_error_safe() {
        use crate::domain::model::{DomainSnapshot, LivePaneLayout, LiveTab};
        let remote_tab = |tab_id, pane_id| LiveTab::fixture(tab_id, LivePaneLayout::pane(pane_id));
        let snapshot = DomainSnapshot::fixture(
            "domain-control",
            20,
            vec![remote_tab(10, 100), remote_tab(20, 200)],
        );
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            1_000,
            1024 * 1024,
        );
        tabs.attach_domain(
            &snapshot,
            &crate::domain::DomainRelayCommand::local("/bin/true"),
        )
        .unwrap();
        let local_tab_id = tabs
            .iter()
            .find_map(|(_, tab)| {
                (tab.live_domain_tab_route() == Some(("domain-control", 10))).then_some(tab.id)
            })
            .unwrap();
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        app.tabs = Some(tabs);

        let close = LiveDomainControl::CloseTab {
            domain_id: "domain-control".into(),
            remote_tab_id: 10,
            local_tab_id,
        };
        // A control can become pending after a drag starts. Interruption must
        // restore its rejected preview without canceling the earlier request.
        app.live_domain_relay_command = Some(crate::domain::DomainRelayCommand::local("/bin/true"));
        app.live_domain_topologies
            .insert(snapshot.id.clone(), (&snapshot).into());
        let dragged_id = app.tabs.as_ref().unwrap().active_tab().unwrap().id;
        app.pending_live_domain_controls.insert(close.clone());
        let interruptions: [fn(&mut App); 4] = [
            |app| app.handle_window_focus_change(false),
            |app| app.handle_resize(960, 720),
            |app| app.handle_scale_factor_change(1.5, winit::dpi::PhysicalSize::new(1440, 1080)),
            |app| app.sync_terminal_geometry(960, 720),
        ];
        for interrupt in interruptions {
            assert!(app.tabs.as_mut().unwrap().move_active_to(0));
            app.tab_dragging = Some(TabDrag {
                tab_id: dragged_id,
                grab: None,
                live: Some(LiveDomainTabDrag {
                    domain_id: snapshot.id.clone(),
                    remote_tab_id: 20,
                    initial_index: 1,
                }),
            });
            interrupt(&mut app);
            assert!(app.tab_dragging.is_none());
            assert_eq!(app.pending_live_domain_controls.len(), 1);
            assert!(app.pending_live_domain_controls.contains(&close));
            assert_eq!(
                app.tabs
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|(_, tab)| tab.live_domain_tab_route().unwrap().1)
                    .collect::<Vec<_>>(),
                vec![10, 20]
            );
        }
        assert!(
            app.toasts
                .iter()
                .any(|toast| toast.message.contains("already pending"))
        );
        app.pending_live_domain_controls.remove(&close);
        assert_eq!(
            close.request(),
            crate::domain::protocol::Request::CloseTab {
                domain: "domain-control".into(),
                tab: 10,
            }
        );
        let spawn = crate::domain::model::SpawnSpec::default();
        assert_eq!(
            LiveDomainControl::SpawnTab {
                domain_id: "domain-control".into(),
                spawn: spawn.clone(),
            }
            .request(),
            crate::domain::protocol::Request::SpawnTab {
                domain: "domain-control".into(),
                spawn: spawn.clone(),
            }
        );
        assert_eq!(
            LiveDomainControl::SplitPane {
                domain_id: "domain-control".into(),
                remote_pane_id: 200,
                direction: crate::panes::SplitDirection::Vertical,
                spawn: spawn.clone(),
            }
            .request(),
            crate::domain::protocol::Request::SplitPane {
                domain: "domain-control".into(),
                pane: 200,
                direction: crate::panes::SplitDirection::Vertical,
                spawn,
            }
        );
        assert_eq!(
            LiveDomainControl::ResizeSplit {
                domain_id: "domain-control".into(),
                remote_split_id: 44,
                ratio_millis: 375,
            }
            .request(),
            crate::domain::protocol::Request::ResizeSplit {
                domain: "domain-control".into(),
                split: 44,
                ratio_millis: 375,
            }
        );
        assert_eq!(
            LiveDomainControl::RenameTab {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
                title: Some("Build λ".into()),
            }
            .request(),
            crate::domain::protocol::Request::RenameTab {
                domain: "domain-control".into(),
                tab: 10,
                title: Some("Build λ".into()),
            }
        );
        assert_eq!(
            LiveDomainControl::SetTabPinned {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
                pinned: true,
            }
            .request(),
            crate::domain::protocol::Request::SetTabPinned {
                domain: "domain-control".into(),
                tab: 10,
                pinned: true,
            }
        );
        assert_eq!(
            LiveDomainControl::MoveTab {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
                index: 1,
            }
            .request(),
            crate::domain::protocol::Request::MoveTab {
                domain: "domain-control".into(),
                tab: 10,
                index: 1,
            }
        );
        assert_eq!(
            LiveDomainControl::SwapPaneNext {
                domain_id: "domain-control".into(),
                remote_pane_id: 100,
            }
            .request(),
            crate::domain::protocol::Request::SwapPaneNext {
                domain: "domain-control".into(),
                pane: 100,
            }
        );
        assert_eq!(
            LiveDomainControl::BalanceTab {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
            }
            .request(),
            crate::domain::protocol::Request::BalanceTab {
                domain: "domain-control".into(),
                tab: 10,
            }
        );
        assert_eq!(
            LiveDomainControl::ApplyPaneLayout {
                domain_id: "domain-control".into(),
                remote_tab_id: 10,
                preset: crate::panes::PaneLayoutPreset::Tall,
            }
            .request(),
            crate::domain::protocol::Request::ApplyPaneLayout {
                domain: "domain-control".into(),
                tab: 10,
                preset: crate::panes::PaneLayoutPreset::Tall,
            }
        );
        assert_eq!(
            LiveDomainControl::MovePane {
                domain_id: "domain-control".into(),
                remote_pane_id: 100,
                source_tab_id: 10,
                destination_tab_id: 11,
                target_pane_id: 110,
                direction: crate::panes::SplitDirection::Horizontal,
            }
            .request(),
            crate::domain::protocol::Request::MovePane {
                domain: "domain-control".into(),
                pane: 100,
                tab: 11,
                target: Some(110),
                direction: crate::panes::SplitDirection::Horizontal,
            }
        );
        app.pending_live_domain_controls.insert(close.clone());
        app.handle_live_domain_control_completed(close.clone(), Ok(snapshot.clone()));
        assert!(!app.pending_live_domain_controls.contains(&close));
        assert!(
            app.tabs
                .as_ref()
                .unwrap()
                .iter()
                .find(|(_, tab)| tab.id == local_tab_id)
                .unwrap()
                .1
                .is_closing()
        );

        let failed = LiveDomainControl::ClosePane {
            domain_id: "domain-control".into(),
            remote_pane_id: 200,
            local_tab_id: usize::MAX,
        };
        app.pending_live_domain_controls.insert(failed.clone());
        app.handle_live_domain_control_completed(failed.clone(), Err("denied".into()));
        assert!(!app.pending_live_domain_controls.contains(&failed));
        assert!(app.toasts.iter().any(|toast| {
            toast.severity == crate::ui::toast::ToastSeverity::Error
                && toast.message.contains("denied")
        }));
    }

    #[test]
    fn topology_handler_applies_only_newer_revisions_and_closes_the_exact_domain() {
        use crate::domain::model::{DomainSnapshot, LivePaneLayout, LiveTab};
        let initial = DomainSnapshot::fixture(
            "domain-topology",
            10,
            vec![LiveTab::fixture(10, LivePaneLayout::pane(100))],
        );
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            1_000,
            1024 * 1024,
        );
        tabs.attach_domain(
            &initial,
            &crate::domain::DomainRelayCommand::local("/bin/true"),
        )
        .unwrap();
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        app.tabs = Some(tabs);
        app.live_domain_authority = Some(crate::domain::DomainAuthority::Local);
        app.live_domain_relay_command = Some(crate::domain::DomainRelayCommand::local("/bin/true"));

        let mut revision_two = initial.clone();
        revision_two.active_tab = 20;
        revision_two.tabs.push(LiveTab::fixture(
            20,
            LivePaneLayout::split(
                44,
                crate::panes::SplitDirection::Vertical,
                500,
                LivePaneLayout::pane(200),
                LivePaneLayout::pane(201),
            ),
        ));
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 2,
                domain: crate::domain::model::DomainTopology::from(&revision_two),
            }),
        );
        assert_eq!(app.live_domain_revisions["domain-topology"], 2);
        assert_eq!(app.tabs.as_ref().unwrap().iter().count(), 2);
        let stable_routes: std::collections::HashMap<_, _> = app
            .tabs
            .as_ref()
            .unwrap()
            .iter()
            .filter_map(|(_, tab)| {
                tab.live_domain_tab_route()
                    .map(|(_, remote)| (remote, tab.id))
            })
            .collect();
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .panes
                .get_dividers()[0]
                .ratio_millis,
            500
        );

        let mut revision_three = revision_two.clone();
        let mut pinned = revision_three.tabs.remove(1);
        pinned.title = "daemon build".into();
        pinned.user_title = Some("daemon build".into());
        pinned.pinned = true;
        revision_three.tabs.insert(0, pinned);
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 3,
                domain: crate::domain::model::DomainTopology::from(&revision_three),
            }),
        );
        let tabs = app.tabs.as_ref().unwrap();
        let order: Vec<_> = tabs
            .iter()
            .filter_map(|(_, tab)| tab.live_domain_tab_route().map(|(_, remote)| remote))
            .collect();
        assert_eq!(order, vec![20, 10]);
        assert_eq!(tabs.active_tab().unwrap().title, "daemon build");
        assert_eq!(
            tabs.active_tab().unwrap().user_title(),
            Some("daemon build")
        );
        assert!(tabs.active_tab().unwrap().pinned);
        for (_, tab) in tabs.iter() {
            let (_, remote) = tab.live_domain_tab_route().unwrap();
            assert_eq!(tab.id, stable_routes[&remote]);
        }
        let active_local_id = tabs.active_tab().unwrap().id;
        let active_pane_id = tabs.active_tab().unwrap().panes.focused_id();
        assert!(app.tabs.as_mut().unwrap().set_title_for_origin(
            crate::tabs::TerminalEventOrigin {
                tab_id: active_local_id,
                pane_id: active_pane_id,
            },
            "relay bypass".into(),
        ));
        assert_eq!(
            app.tabs.as_ref().unwrap().active_tab().unwrap().title,
            "daemon build"
        );

        {
            let tabs = app.tabs.as_mut().unwrap();
            let tab = tabs.active_tab_mut().unwrap();
            tab.set_user_title(Some("optimistic".into()));
            tab.pinned = false;
            assert!(tabs.move_active_to(1));
        }
        app.handle_live_domain_control_completed(
            LiveDomainControl::MoveTab {
                domain_id: "domain-topology".into(),
                remote_tab_id: 20,
                index: 1,
            },
            Err("stale tab order".into()),
        );
        let tabs = app.tabs.as_ref().unwrap();
        let order: Vec<_> = tabs
            .iter()
            .filter_map(|(_, tab)| tab.live_domain_tab_route().map(|(_, remote)| remote))
            .collect();
        assert_eq!(order, vec![20, 10]);
        assert_eq!(tabs.active_tab().unwrap().title, "daemon build");
        assert_eq!(
            tabs.active_tab().unwrap().user_title(),
            Some("daemon build")
        );
        assert!(tabs.active_tab().unwrap().pinned);

        let relay_routes = tabs
            .active_tab()
            .unwrap()
            .live_domain
            .as_ref()
            .unwrap()
            .pane_ids
            .clone();
        let mut revision_four = revision_three.clone();
        revision_four.tabs[0].layout = LivePaneLayout::Split {
            split_id: 55,
            direction: crate::panes::SplitDirection::Horizontal,
            ratio_millis: 500,
            first: Box::new(LivePaneLayout::Pane { pane: 201 }),
            second: Box::new(LivePaneLayout::Pane { pane: 200 }),
        };
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 4,
                domain: crate::domain::model::DomainTopology::from(&revision_four),
            }),
        );
        let active = app.tabs.as_ref().unwrap().active_tab().unwrap();
        assert_eq!(active.live_domain.as_ref().unwrap().pane_ids, relay_routes);
        let divider = active.panes.get_dividers()[0];
        assert_eq!(divider.target, crate::panes::DividerTarget::Domain(55));
        assert!(!divider.vertical);

        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .panes
            .resize_divider_target(crate::panes::DividerTarget::Domain(55), 60.0, 10.0)
            .unwrap();
        app.handle_live_domain_control_completed(
            LiveDomainControl::ResizeSplit {
                domain_id: "domain-topology".into(),
                remote_split_id: 55,
                ratio_millis: 789,
            },
            Err("stale split".into()),
        );
        assert_eq!(
            app.tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .panes
                .get_dividers()[0]
                .ratio_millis,
            500
        );

        // Hiding a tab during a divider preview must finalize through the
        // control path, not silently discard the preview. With an earlier
        // control pending, rejection restores the cached authoritative ratio
        // and does not remove that earlier request.
        let dragged_tab = app.tabs.as_ref().unwrap().active_tab().unwrap().id;
        let pending = LiveDomainControl::ResizeSplit {
            domain_id: "domain-topology".into(),
            remote_split_id: 55,
            ratio_millis: 600,
        };
        for refresh in [true, false] {
            let source_index = app
                .tabs
                .as_ref()
                .unwrap()
                .iter()
                .find_map(|(index, tab)| (tab.id == dragged_tab).then_some(index))
                .unwrap();
            app.tabs.as_mut().unwrap().switch_to(source_index);
            app.tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .panes
                .resize(
                    crate::terminal::geometry::TerminalArea::from_available_pixels(
                        800.0, 480.0, 10.0, 20.0,
                    ),
                );
            app.divider_dragging = Some(DividerDrag {
                tab_id: dragged_tab,
                domain: Some(("domain-topology".into(), 20)),
                target: crate::panes::DividerTarget::Domain(55),
                last_ratio_millis: 500,
            });
            app.apply_divider_drag_position(60.0, 320.0);
            assert_ne!(
                app.divider_dragging.as_ref().unwrap().last_ratio_millis,
                500
            );
            app.pending_live_domain_controls.insert(pending.clone());
            app.tabs.as_mut().unwrap().next_tab();
            if refresh {
                app.refresh_focused_terminal_surfaces();
            } else {
                app.apply_divider_drag_position(60.0, 20.0);
            }
            assert!(app.divider_dragging.is_none());
            assert!(app.take_native_drag_controls().is_empty());
            assert_eq!(app.pending_live_domain_controls.len(), 1);
            assert!(app.pending_live_domain_controls.contains(&pending));
            assert_eq!(
                app.tabs
                    .as_ref()
                    .unwrap()
                    .tab_by_id(dragged_tab)
                    .unwrap()
                    .panes
                    .get_dividers()[0]
                    .ratio_millis,
                500
            );
            app.pending_live_domain_controls.remove(&pending);
        }

        let relays_before: std::collections::HashSet<_> = app
            .tabs
            .as_ref()
            .unwrap()
            .iter()
            .flat_map(|(_, tab)| tab.panes.iter_panes().map(|pane| pane.pty.child_id()))
            .collect();
        let mut revision_five = revision_four.clone();
        let moved = revision_five.tabs[0]
            .panes
            .iter()
            .position(|pane| pane.id == 201)
            .map(|index| revision_five.tabs[0].panes.remove(index))
            .unwrap();
        revision_five.tabs[0].layout = LivePaneLayout::Pane { pane: 200 };
        revision_five.tabs[0].focused_pane = 200;
        revision_five.tabs[1].panes.push(moved);
        revision_five.tabs[1].layout = LivePaneLayout::Split {
            split_id: 56,
            direction: crate::panes::SplitDirection::Vertical,
            ratio_millis: 500,
            first: Box::new(LivePaneLayout::Pane { pane: 100 }),
            second: Box::new(LivePaneLayout::Pane { pane: 201 }),
        };
        revision_five.tabs[1].focused_pane = 201;
        revision_five.active_tab = 10;
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 5,
                domain: crate::domain::model::DomainTopology::from(&revision_five),
            }),
        );
        let tabs = app.tabs.as_ref().unwrap();
        let source = tabs
            .iter()
            .find_map(|(_, tab)| {
                tab.live_domain_tab_route()
                    .and_then(|(_, remote)| (remote == 20).then_some(tab))
            })
            .unwrap();
        assert!(source.panes.pane_count() == 1);
        assert_eq!(
            source.live_domain.as_ref().unwrap().pane_ids,
            std::collections::HashMap::from([(crate::panes::PaneId(0), 200)])
        );
        let destination = tabs.active_tab().unwrap();
        assert_eq!(destination.live_domain_tab_route().unwrap().1, 10);
        let destination_panes = &destination.panes;
        assert_eq!(destination_panes.pane_count(), 2);
        assert!(
            destination_panes
                .iter_panes()
                .all(|pane| relays_before.contains(&pane.pty.child_id())),
            "the existing relay must move instead of being respawned"
        );
        let destination_routes: std::collections::HashSet<_> = destination
            .live_domain
            .as_ref()
            .unwrap()
            .pane_ids
            .values()
            .copied()
            .collect();
        assert_eq!(
            destination_routes,
            std::collections::HashSet::from([100, 201])
        );
        assert_eq!(destination.live_domain_focus_route().unwrap().1, 201);

        app.handle_live_domain_topology("domain-topology".into(), Err("transport reset".into()));
        assert!(app.disconnected_live_domains.contains("domain-topology"));
        let disconnect_warnings = app
            .toasts
            .iter()
            .filter(|toast| {
                toast.severity == crate::ui::toast::ToastSeverity::Warning
                    && toast
                        .message
                        .contains("Live domain domain-topology on this host disconnected")
            })
            .count();
        assert_eq!(disconnect_warnings, 1);
        app.handle_live_domain_topology("domain-topology".into(), Err("same outage".into()));
        assert_eq!(
            app.toasts
                .iter()
                .filter(|toast| {
                    toast.severity == crate::ui::toast::ToastSeverity::Warning
                        && toast
                            .message
                            .contains("Live domain domain-topology on this host disconnected")
                })
                .count(),
            disconnect_warnings,
            "one outage should produce one warning"
        );

        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 5,
                domain: crate::domain::model::DomainTopology::from(&revision_five),
            }),
        );
        assert!(
            !app.disconnected_live_domains.contains("domain-topology"),
            "an equal fresh snapshot must still complete recovery"
        );
        assert!(app.toasts.iter().any(|toast| {
            toast.severity == crate::ui::toast::ToastSeverity::Success
                && toast.message == "Reconnected live domain domain-topology on this host"
        }));

        app.handle_live_domain_topology(
            "domain-topology".into(),
            Err("transport reset again".into()),
        );
        let mut wrong_route = crate::domain::model::DomainTopology::from(&revision_five);
        wrong_route.domain_id = "different-domain".into();
        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 6,
                domain: wrong_route,
            }),
        );
        assert!(
            app.disconnected_live_domains.contains("domain-topology"),
            "a mismatched snapshot must not declare the expected route recovered"
        );
        assert!(app.toasts.iter().any(|toast| {
            toast.severity == crate::ui::toast::ToastSeverity::Error
                && toast.message.contains("Rejected live-domain topology")
        }));

        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Snapshot {
                revision: 1,
                domain: crate::domain::model::DomainTopology::from(&initial),
            }),
        );
        assert_eq!(app.tabs.as_ref().unwrap().iter().count(), 2);

        app.handle_live_domain_topology(
            "domain-topology".into(),
            Ok(crate::domain::DomainTopologyEvent::Closed {
                domain: "domain-topology".into(),
                message: "terminated".into(),
            }),
        );
        assert!(!app.live_domain_revisions.contains_key("domain-topology"));
        assert!(
            !app.disconnected_live_domains.contains("domain-topology"),
            "a definitive close ends the outage without a reconnect notice"
        );
        assert!(
            app.tabs
                .as_ref()
                .unwrap()
                .iter()
                .filter(|(_, tab)| tab.live_domain.is_some())
                .all(|(_, tab)| tab.is_closing())
        );
    }
}
