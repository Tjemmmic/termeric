//! Tab management for multiple terminal instances
//!
//! Provides support for tabbed terminal sessions within a single window.

mod plugin_panes;
#[cfg(test)]
mod real_pty_tests;
mod tab;
mod title;

pub use tab::{CommandLaunch, SavedSnapshot, Tab, TabType};
pub(crate) use tab::{LiveDomainTab, TabAgentView};

use crate::config::ShellConfig;
use crate::panes::{PaneId, WorkspacePaneLaunches, usable_primary_workspace_cwd};
use anyhow::Context;
use std::collections::HashMap;
use std::io;
use termeric_core::{Terminal, TerminalEvent};

use crate::terminal::events::MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL;
use crate::terminal::geometry::TerminalArea;
use crate::terminal::policy::TerminalPolicy;

/// A plugin tab that closes this soon after it was created most likely failed
/// to start, so the person is shown what it printed.
pub(crate) const QUICK_EXIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

/// The exact terminal an app-facing event came from: its tab's stable ID and
/// its tab-local pane ID. Both are stable for the terminal's lifetime, so a
/// reply never depends on whichever terminal is focused when App handles the
/// event. Holding one means the terminal was live when it was captured;
/// every use resolves it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalEventOrigin {
    pub tab_id: usize,
    pub pane_id: PaneId,
}

/// Long-lived terminal identity for UI work that can outlast the terminal.
/// Resolve it to an event/write route only at the point of use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalIdentity {
    pub tab_id: usize,
    pub pane_id: PaneId,
}

#[derive(Debug, Clone)]
pub struct RoutedTerminalEvent {
    pub origin: TerminalEventOrigin,
    pub event: TerminalEvent,
}

#[derive(Debug, Clone)]
pub enum PersistenceNotice {
    Saved { id: String, truncated: bool },
    SaveFailed { tab_id: usize, error: String },
}

/// Manager for multiple tabs
pub struct TabManager {
    /// All tabs
    tabs: Vec<Tab>,
    /// Currently active tab index
    active: usize,
    /// Next tab ID to assign
    next_id: usize,
    /// The terminal area every tab's panes divide, and new tabs fill.
    area: TerminalArea,
    /// Shell configuration for new tabs
    shell: ShellConfig,
    /// Host settings of every terminal, inherited by terminals created later.
    policy: TerminalPolicy,
    persistence_notices: Vec<PersistenceNotice>,
    event_overflow_reported: bool,
    reduced_motion: bool,
}

impl TabManager {
    /// An 80×24 manager keeping 1,000 lines or 8 MiB of scrollback, the size
    /// most tests use.
    #[cfg(test)]
    pub(crate) fn for_test(shell: ShellConfig) -> Self {
        Self::new(80, 24, shell, 1_000, 8 * 1024 * 1024)
    }

    /// Create a new tab manager
    #[cfg(test)]
    pub fn new(
        cols: usize,
        rows: usize,
        shell: ShellConfig,
        scrollback_lines: usize,
        scrollback_bytes: usize,
    ) -> Self {
        Self::new_with_area(
            TerminalArea::new(
                crate::terminal::geometry::TerminalGeometry::from_cells(cols, rows, 1.0, 1.0),
                1.0,
                1.0,
            ),
            shell,
            TerminalPolicy::with_scrollback(scrollback_lines, scrollback_bytes),
        )
    }

    pub(crate) fn new_with_area(
        area: TerminalArea,
        shell: ShellConfig,
        policy: TerminalPolicy,
    ) -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            area,
            shell,
            policy,
            persistence_notices: Vec::new(),
            event_overflow_reported: false,
            reduced_motion: false,
        }
    }

    pub(crate) fn empty_like(&self) -> Self {
        let mut manager = Self::new_with_area(self.area, self.shell.clone(), self.policy);
        manager.reduced_motion = self.reduced_motion;
        manager
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }

    /// Create a new tab and make it active
    pub fn new_tab(&mut self) -> anyhow::Result<usize> {
        self.new_tab_with_cwd(None, None)
    }

    /// Create a shell tab with explicit startup cwd/title overrides.
    pub(crate) fn new_tab_with_cwd(
        &mut self,
        working_dir: Option<&str>,
        title: Option<&str>,
    ) -> anyhow::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;

        let tab = Tab::new_shell(id, self.area, &self.shell, &self.policy, working_dir, title)?;
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;

        log::info!("Created new tab {}", id);
        Ok(id)
    }

    /// Create a tab from a validated reusable terminal profile. The profile's
    /// executable, argv, and environment are frozen for future pane splits.
    pub(crate) fn new_tab_with_profile(
        &mut self,
        name: &str,
        shell: ShellConfig,
        working_dir: Option<&str>,
        title: Option<&str>,
    ) -> anyhow::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;
        let mut tab = Tab::new_shell(id, self.area, &shell, &self.policy, working_dir, title)?;
        tab.tab_type = TabType::Profile {
            name: name.to_string(),
        };
        tab.set_terminal_profile_launch(name, shell);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        log::info!("Created terminal profile tab {} from {:?}", id, name);
        Ok(id)
    }

    /// Open one validated native serial profile through Termeric's bounded
    /// internal bridge. Serial tabs deliberately do not inherit pane splits.
    pub(crate) fn new_tab_with_serial_profile(
        &mut self,
        name: &str,
        serial: &crate::config::SerialProfileConfig,
        title: Option<&str>,
    ) -> anyhow::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;
        let (program, args) = crate::serial::bridge_command(serial)?;
        let title = title.unwrap_or(name);
        let launch = CommandLaunch {
            program,
            args,
            title: Some(title.to_string()),
            tab_type: Some(TabType::Serial {
                name: name.to_string(),
            }),
            ..CommandLaunch::default()
        };
        let mut tab = Tab::new_command(id, self.area, &launch, &self.policy)?;
        tab.set_serial_profile_launch(name, title.to_string());
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        log::info!("Opened serial terminal profile {} from {:?}", id, name);
        Ok(id)
    }

    /// Create a new tab running a specific command and make it active.
    ///
    /// The program runs directly — no shell prompt or command echo.
    pub fn new_tab_with_command(
        &mut self,
        program: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        tab_type: Option<TabType>,
        env: HashMap<String, String>,
    ) -> anyhow::Result<usize> {
        self.launch_command_tab(
            CommandLaunch {
                program: program.to_string(),
                args: args.to_vec(),
                working_dir: working_dir.map(str::to_string),
                title: title.map(str::to_string),
                tab_type,
                env,
                ..CommandLaunch::default()
            },
            false,
        )
    }

    /// Open a command tab. A `background` tab never transiently changes the
    /// active owner, though an empty manager necessarily selects its first.
    pub(crate) fn launch_command_tab(
        &mut self,
        launch: CommandLaunch,
        background: bool,
    ) -> anyhow::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;

        let mut tab = Tab::new_command(id, self.area, &launch, &self.policy)?;
        tab.set_split_shell_config(self.shell.clone());
        self.tabs.push(tab);
        if !background || self.tabs.len() == 1 {
            self.active = self.tabs.len() - 1;
        }

        log::info!("Created command tab {}", id);
        Ok(id)
    }

    /// Reconstruct a live domain with one transparent relay command per pane.
    /// The daemon remains the only owner of the real PTYs.
    pub(crate) fn attach_domain(
        &mut self,
        snapshot: &crate::domain::model::DomainSnapshot,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<()> {
        if snapshot.tabs.is_empty() {
            anyhow::bail!("live domain has no tabs");
        }
        for remote_tab in &snapshot.tabs {
            self.attach_domain_tab(&snapshot.id, remote_tab, relay_command)?;
        }
        self.tabs.sort_by_key(|tab| !tab.pinned);
        self.active = self
            .tabs
            .iter()
            .position(|tab| {
                tab.live_domain.as_ref().is_some_and(|live| {
                    live.domain_id == snapshot.id && live.tab_id == snapshot.active_tab
                })
            })
            .context("live domain active tab is absent from its snapshot")?;
        Ok(())
    }

    /// Attach `remote_tab` with every one of its panes laid out as the
    /// daemon has them; local pane IDs number its panes in ascending order.
    fn attach_domain_tab(
        &mut self,
        domain_id: &str,
        remote_tab: &crate::domain::model::LiveTab,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<usize> {
        let index = self.attach_live_tab(
            domain_id,
            &crate::domain::model::LiveTopologyTab::from(remote_tab),
            relay_command,
        )?;
        if remote_tab.panes.len() > 1 {
            let mut remote_panes: Vec<u32> = remote_tab.panes.iter().map(|pane| pane.id).collect();
            remote_panes.sort_unstable();
            let tab = &mut self.tabs[index];
            tab.live_domain
                .as_mut()
                .expect("an attached live-domain tab has its route")
                .pane_ids = remote_panes
                .into_iter()
                .enumerate()
                .map(|(index, remote)| (PaneId(index as u32), remote))
                .collect();
            tab.panes
                .adopt_domain_layout(domain_id, remote_tab, relay_command)?;
        }
        Ok(index)
    }

    /// Open a tab showing `remote_tab` through one relay, local pane 0, to
    /// its lowest-numbered pane, and make it active. Returns its index.
    fn attach_live_tab(
        &mut self,
        domain_id: &str,
        remote_tab: &crate::domain::model::LiveTopologyTab,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<usize> {
        let primary = remote_tab
            .panes
            .iter()
            .map(|pane| pane.id)
            .min()
            .ok_or_else(|| anyhow::anyhow!("live domain tab {} has no panes", remote_tab.id))?;
        let id = self.next_id;
        self.next_id += 1;
        let policy = self.policy.for_relay();
        let relay = crate::panes::Pane::new_relay(
            PaneId(0),
            self.area.geometry,
            relay_command,
            domain_id,
            primary,
            &policy,
            None,
        )?;
        let mut tab = Tab::new_live_domain(
            id,
            self.area,
            relay,
            &policy,
            LiveDomainTab {
                domain_id: domain_id.to_string(),
                tab_id: remote_tab.id,
                pane_ids: HashMap::from([(PaneId(0), primary)]),
            },
            self.shell.clone(),
        );
        tab.set_live_domain_metadata(
            &remote_tab.title,
            remote_tab.user_title.as_deref(),
            remote_tab.pinned,
        );
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        log::info!("Attached live-domain tab {} as tab {}", remote_tab.id, id);
        Ok(self.active)
    }

    /// Apply one whole-domain daemon revision while preserving every relay PTY
    /// whose stable remote identity still exists.
    pub(crate) fn reconcile_live_domain(
        &mut self,
        snapshot: &crate::domain::model::DomainTopology,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<()> {
        if snapshot.tabs.is_empty() {
            anyhow::bail!("live domain has no tabs");
        }
        let remote_tabs: std::collections::HashSet<_> =
            snapshot.tabs.iter().map(|tab| tab.id).collect();
        if remote_tabs.len() != snapshot.tabs.len() {
            anyhow::bail!("live domain contains duplicate tab identifiers");
        }
        let mut reached_unpinned = false;
        for tab in &snapshot.tabs {
            reached_unpinned |= !tab.pinned;
            if reached_unpinned && tab.pinned {
                anyhow::bail!("live domain contains a pinned tab after the unpinned group");
            }
        }

        let stale: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| {
                tab.live_domain.as_ref().and_then(|live| {
                    (live.domain_id == snapshot.domain_id && !remote_tabs.contains(&live.tab_id))
                        .then_some(index)
                })
            })
            .collect();
        for index in stale.into_iter().rev() {
            self.close_tab(index);
        }

        self.relocate_live_domain_relays(snapshot)?;

        for remote_tab in &snapshot.tabs {
            let index = self.tabs.iter().position(|tab| {
                tab.live_domain.as_ref().is_some_and(|live| {
                    live.domain_id == snapshot.domain_id && live.tab_id == remote_tab.id
                })
            });
            let index = match index {
                Some(index) => index,
                None => self.attach_live_tab(&snapshot.domain_id, remote_tab, relay_command)?,
            };
            self.reconcile_live_domain_tab(index, &snapshot.domain_id, remote_tab, relay_command)?;
        }
        self.tabs.sort_by_key(|tab| !tab.pinned);
        self.reorder_live_domain_tabs(&snapshot.domain_id, &snapshot.tabs)?;
        self.active = self
            .tabs
            .iter()
            .position(|tab| {
                tab.live_domain.as_ref().is_some_and(|live| {
                    live.domain_id == snapshot.domain_id && live.tab_id == snapshot.active_tab
                })
            })
            .context("live domain active tab is absent from its topology")?;
        Ok(())
    }

    /// Move an existing relay ownership bundle between local tabs before the
    /// per-tab reconciliation pass. The daemon pane identity is global to the
    /// domain, so a cross-tab topology revision must not respawn a relay merely
    /// because its tab-local mapping changed.
    fn relocate_live_domain_relays(
        &mut self,
        snapshot: &crate::domain::model::DomainTopology,
    ) -> anyhow::Result<()> {
        let mut desired_tabs = HashMap::new();
        for tab in &snapshot.tabs {
            for pane in &tab.panes {
                if desired_tabs.insert(pane.id, tab.id).is_some() {
                    anyhow::bail!("live domain contains duplicate pane identifier {}", pane.id);
                }
            }
        }

        let mut moves = Vec::new();
        for tab in &self.tabs {
            let Some(live) = tab
                .live_domain
                .as_ref()
                .filter(|live| live.domain_id == snapshot.domain_id)
            else {
                continue;
            };
            for remote_pane in live.pane_ids.values().copied() {
                if let Some(destination_tab) = desired_tabs.get(&remote_pane).copied()
                    && destination_tab != live.tab_id
                {
                    moves.push((remote_pane, live.tab_id, destination_tab));
                }
            }
        }

        for (remote_pane, source_tab_id, destination_tab_id) in moves {
            let source_index = self.tabs.iter().position(|tab| {
                tab.live_domain.as_ref().is_some_and(|live| {
                    live.domain_id == snapshot.domain_id && live.tab_id == source_tab_id
                })
            });
            let destination_index = self.tabs.iter().position(|tab| {
                tab.live_domain.as_ref().is_some_and(|live| {
                    live.domain_id == snapshot.domain_id && live.tab_id == destination_tab_id
                })
            });
            let (Some(source_index), Some(destination_index)) = (source_index, destination_index)
            else {
                continue;
            };
            if source_index == destination_index {
                continue;
            }
            let source_local = self.tabs[source_index]
                .live_domain
                .as_ref()
                .and_then(|live| {
                    live.pane_ids
                        .iter()
                        .find_map(|(local, remote)| (*remote == remote_pane).then_some(*local))
                });
            let Some(source_local) = source_local else {
                continue;
            };
            if self.tabs[source_index].panes.pane_count() <= 1 {
                // A skipped intermediate revision can make a direct transfer
                // impossible. The ordinary reconciliation path will replace
                // only that relay and still converge to daemon state.
                continue;
            }
            // The daemon's layout replaces the destination's tree below, so
            // any relay there that can take a neighbour anchors the move;
            // without one, reconciliation replaces the relay as above.
            let destination_panes = &self.tabs[destination_index].panes;
            let Some(anchor) = destination_panes
                .pane_ids()
                .into_iter()
                .find(|id| destination_panes.validate_attach_target(*id).is_ok())
            else {
                continue;
            };

            let (source, destination) = if source_index < destination_index {
                let (before, after) = self.tabs.split_at_mut(destination_index);
                (&mut before[source_index], &mut after[0])
            } else {
                let (before, after) = self.tabs.split_at_mut(source_index);
                (&mut after[0], &mut before[destination_index])
            };
            let pane = source
                .panes
                .detach(source_local)
                .context("live-domain source relay could not be detached")?;
            source
                .live_domain
                .as_mut()
                .context("live-domain source tab lost its stable route")?
                .pane_ids
                .remove(&source_local);
            let destination_local = destination
                .panes
                .attach_target(pane, anchor, crate::panes::SplitDirection::Vertical)
                .context("live-domain relay could not join its destination tab")?;
            destination
                .live_domain
                .as_mut()
                .context("live-domain destination tab lost its stable route")?
                .pane_ids
                .insert(destination_local, remote_pane);
        }
        Ok(())
    }

    fn reorder_live_domain_tabs(
        &mut self,
        domain_id: &str,
        remote_tabs: &[crate::domain::model::LiveTopologyTab],
    ) -> anyhow::Result<()> {
        let expected: std::collections::HashSet<_> = remote_tabs.iter().map(|tab| tab.id).collect();
        let local_routes: Vec<_> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                tab.live_domain.as_ref().and_then(|live| {
                    (live.domain_id == domain_id && expected.contains(&live.tab_id))
                        .then_some(live.tab_id)
                })
            })
            .collect();
        let unique_local: std::collections::HashSet<_> = local_routes.iter().copied().collect();
        if local_routes.len() != remote_tabs.len() || unique_local != expected {
            anyhow::bail!("live domain local tab routes do not match the authoritative order");
        }
        let mut extracted = HashMap::new();
        let mut slots = Vec::with_capacity(self.tabs.len());
        for tab in std::mem::take(&mut self.tabs) {
            let remote_id = tab.live_domain.as_ref().and_then(|live| {
                (live.domain_id == domain_id && expected.contains(&live.tab_id))
                    .then_some(live.tab_id)
            });
            if let Some(remote_id) = remote_id {
                debug_assert!(!extracted.contains_key(&remote_id));
                extracted.insert(remote_id, tab);
                slots.push(None);
            } else {
                slots.push(Some(tab));
            }
        }
        let mut ordered = remote_tabs.iter();
        for slot in slots.iter_mut().filter(|slot| slot.is_none()) {
            let remote = ordered
                .next()
                .expect("validated live-domain route count matches remote order");
            *slot = Some(
                extracted
                    .remove(&remote.id)
                    .expect("validated live-domain route remains available during reorder"),
            );
        }
        debug_assert!(ordered.next().is_none());
        debug_assert!(extracted.is_empty());
        self.tabs = slots
            .into_iter()
            .map(|tab| tab.expect("every live-domain reorder slot was filled"))
            .collect();
        Ok(())
    }

    fn reconcile_live_domain_tab(
        &mut self,
        index: usize,
        domain_id: &str,
        remote_tab: &crate::domain::model::LiveTopologyTab,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<()> {
        let tab = &mut self.tabs[index];
        tab.set_live_domain_metadata(
            &remote_tab.title,
            remote_tab.user_title.as_deref(),
            remote_tab.pinned,
        );
        let mut pane_ids = std::mem::take(
            &mut tab
                .live_domain
                .as_mut()
                .context("live-domain tab lost its stable route")?
                .pane_ids,
        );

        let result =
            tab.panes
                .reconcile_domain(&mut pane_ids, domain_id, remote_tab, relay_command);
        tab.live_domain.as_mut().unwrap().pane_ids = pane_ids;
        result
    }

    /// Start closing a tab by index (begins shrink animation)
    pub fn close_tab(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() {
            return false;
        }

        // Don't close if already closing
        if self.tabs[index].is_closing() {
            return false;
        }

        self.tabs[index].start_closing();
        log::info!("Starting close animation for tab {}", self.tabs[index].id);

        // If closing the active tab, switch to another tab
        if index == self.active {
            // Find the next non-closing tab to switch to
            let next_active = self.find_next_active_tab(index);
            if let Some(new_active) = next_active {
                self.active = new_active;
            }
        }

        true
    }

    /// Find the next suitable active tab when the current one is closing
    fn find_next_active_tab(&self, closing_index: usize) -> Option<usize> {
        // Try to find a non-closing tab, preferring ones after the current
        let non_closing: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(i, tab)| *i != closing_index && !tab.is_closing())
            .map(|(i, _)| i)
            .collect();

        if non_closing.is_empty() {
            return None;
        }

        // Prefer tab after current, otherwise before
        non_closing
            .iter()
            .find(|&&i| i > closing_index)
            .or_else(|| non_closing.last())
            .copied()
    }

    /// Get the active tab
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// Get the active tab mutably
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub(crate) fn tab_mut_by_id(&mut self, tab_id: usize) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == tab_id)
    }

    pub(crate) fn tab_by_id(&self, tab_id: usize) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == tab_id)
    }

    pub fn active_tab_id(&self) -> Option<usize> {
        self.active_tab().map(|tab| tab.id)
    }

    /// Move the active tab one visual position, respecting the pinned prefix.
    pub fn move_active_by(&mut self, delta: isize) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };
        let pinned = tab.pinned;
        let pinned_count = self.tabs.iter().take_while(|tab| tab.pinned).count();
        let minimum = if pinned { 0 } else { pinned_count };
        let maximum = if pinned {
            pinned_count.saturating_sub(1)
        } else {
            self.tabs.len().saturating_sub(1)
        };
        let target = self
            .active
            .saturating_add_signed(delta)
            .clamp(minimum, maximum);
        self.move_active_to(target)
    }

    pub fn move_active_to(&mut self, target: usize) -> bool {
        if self.tabs.is_empty() || target >= self.tabs.len() || target == self.active {
            return false;
        }
        if self.tabs[target].pinned != self.tabs[self.active].pinned {
            return false;
        }
        let active_id = self.tabs[self.active].id;
        let tab = self.tabs.remove(self.active);
        self.tabs.insert(target, tab);
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.id == active_id)
            .expect("moved tab remains present");
        true
    }

    /// A pointer drag stays bound to the tab selected at button-down. Keyboard
    /// selection changes or source closure must not transfer it to another tab.
    /// Pinned boundaries remain enforced; the caller checks live-domain target
    /// membership before permitting a preview move.
    pub(crate) fn move_dragged_tab_to(&mut self, tab_id: usize, target: usize) -> bool {
        if self
            .active_tab()
            .is_none_or(|tab| tab.id != tab_id || tab.closed || tab.is_closing())
        {
            return false;
        }
        self.move_active_to(target)
    }

    /// Move one exact local tab to a final visual index without changing which
    /// tab is selected. Pinned and unpinned tabs cannot cross their boundary.
    pub(crate) fn move_tab_by_id_to(
        &mut self,
        tab_id: usize,
        target: usize,
    ) -> anyhow::Result<bool> {
        let source = self
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .with_context(|| format!("GUI tab {tab_id} does not exist in the source window"))?;
        let tab = &self.tabs[source];
        if tab.is_closing() || tab.closed {
            anyhow::bail!("GUI tab {tab_id} is already closing");
        }
        if tab.live_domain.is_some() {
            anyhow::bail!(
                "Live-domain tab movement requires an acknowledged daemon topology change"
            );
        }
        if target >= self.tabs.len() {
            anyhow::bail!(
                "Destination tab index {target} is outside this window's {} tabs",
                self.tabs.len()
            );
        }
        let pinned_count = self.tabs.iter().take_while(|tab| tab.pinned).count();
        if (tab.pinned && target >= pinned_count) || (!tab.pinned && target < pinned_count) {
            anyhow::bail!("Destination tab index {target} crosses the pinned-tab boundary");
        }
        if source == target {
            return Ok(false);
        }

        let active_id = self.active_tab_id();
        let tab = self.tabs.remove(source);
        self.tabs.insert(target, tab);
        self.active = active_id
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
            .unwrap_or(0);
        Ok(true)
    }

    pub fn toggle_pin_active(&mut self) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        let active_id = self.tabs[self.active].id;
        let was_pinned = self.tabs[self.active].pinned;
        self.tabs[self.active].pinned = !was_pinned;
        let tab = self.tabs.remove(self.active);
        let pinned_count = self.tabs.iter().take_while(|tab| tab.pinned).count();
        self.tabs.insert(pinned_count, tab);
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.id == active_id)
            .expect("pinned tab remains present");
        true
    }

    pub(crate) fn validate_window_tab_transfer_source(&self) -> anyhow::Result<()> {
        let tab = self.active_tab().context("No source tab is active")?;
        if tab.is_closing() || tab.closed {
            anyhow::bail!("The source tab is closing");
        }
        if tab.live_domain.is_some() {
            anyhow::bail!(
                "Live-domain tabs are topology-owned and cannot be moved between local windows"
            );
        }
        Ok(())
    }

    /// Validate an exact whole-tab transfer and return its pin state. This is
    /// separate from detachment so a destination can be proven safe first.
    pub(crate) fn validate_window_tab_transfer_source_by_id(
        &self,
        tab_id: usize,
    ) -> anyhow::Result<bool> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .with_context(|| format!("GUI tab {tab_id} does not exist in the source window"))?;
        if tab.is_closing() || tab.closed {
            anyhow::bail!("GUI tab {tab_id} is already closing");
        }
        if tab.live_domain.is_some() {
            anyhow::bail!(
                "Live-domain tabs are topology-owned and cannot be moved between local windows"
            );
        }
        Ok(tab.pinned)
    }

    /// Prove a cross-window insertion is representable before the source tab
    /// is detached. `target` is the final zero-based index after insertion.
    pub(crate) fn validate_window_tab_transfer_destination(
        &self,
        pinned: bool,
        target: usize,
    ) -> anyhow::Result<()> {
        if self.tabs.len() >= crate::control::protocol::MAX_TABS_PER_WINDOW {
            anyhow::bail!(
                "GUI window tab limit of {} reached",
                crate::control::protocol::MAX_TABS_PER_WINDOW
            );
        }
        if target > self.tabs.len() {
            anyhow::bail!(
                "Destination tab index {target} is outside the {} available insertion positions",
                self.tabs.len() + 1
            );
        }
        let pinned_count = self.tabs.iter().take_while(|tab| tab.pinned).count();
        if (pinned && target > pinned_count) || (!pinned && target < pinned_count) {
            anyhow::bail!("Destination tab index {target} crosses the pinned-tab boundary");
        }
        if self.next_id.checked_add(1).is_none()
            || self.tabs.iter().any(|tab| tab.id == self.next_id)
        {
            anyhow::bail!("Destination window exhausted its stable tab ID space");
        }
        Ok(())
    }

    /// Detach the active whole-tab owner without closing any child process.
    pub(crate) fn detach_active_tab_for_window_transfer(&mut self) -> anyhow::Result<Tab> {
        self.validate_window_tab_transfer_source()?;
        let tab = self.tabs.remove(self.active);
        if self.tabs.is_empty() {
            self.active = 0;
        } else {
            self.active = self.active.min(self.tabs.len() - 1);
            self.tabs[self.active].has_activity = false;
        }
        Ok(tab)
    }

    /// Detach one validated tab by stable ID while preserving selection when a
    /// different tab was active.
    pub(crate) fn detach_tab_for_window_transfer(&mut self, tab_id: usize) -> anyhow::Result<Tab> {
        self.validate_window_tab_transfer_source_by_id(tab_id)?;
        let source = self
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .expect("validated source tab remains present");
        let active_id = self.active_tab_id();
        let tab = self.tabs.remove(source);
        if self.tabs.is_empty() {
            self.active = 0;
        } else if active_id == Some(tab_id) {
            self.active = source.min(self.tabs.len() - 1);
            self.tabs[self.active].has_activity = false;
        } else {
            self.active = active_id
                .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
                .expect("unmoved active tab remains present");
        }
        Ok(tab)
    }

    /// Adopt a complete tab under a fresh destination-local ID and this
    /// manager's host settings. Pinned tabs remain in the leading pinned
    /// prefix and become active after transfer.
    pub(crate) fn attach_window_transferred_tab(&mut self, mut tab: Tab) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        tab.id = id;
        tab.has_activity = false;
        tab.panes.resize(self.area);
        tab.apply_terminal_policy(&self.policy, false);
        let insertion = if tab.pinned {
            self.tabs.iter().take_while(|tab| tab.pinned).count()
        } else {
            self.tabs.len()
        };
        self.tabs.insert(insertion, tab);
        self.active = insertion;
        id
    }

    /// Adopt a transferred tab at one prevalidated final visual index. The tab
    /// receives a fresh destination-local stable ID and this manager's host
    /// settings, and becomes active.
    pub(crate) fn attach_window_transferred_tab_at(
        &mut self,
        mut tab: Tab,
        target: usize,
    ) -> anyhow::Result<usize> {
        self.validate_window_tab_transfer_destination(tab.pinned, target)?;
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("Destination window exhausted its stable tab ID space")?;
        tab.id = id;
        tab.has_activity = false;
        tab.panes.resize(self.area);
        tab.apply_terminal_policy(&self.policy, false);
        self.tabs.insert(target, tab);
        self.active = target;
        Ok(id)
    }

    /// Move the active pane through the same prevalidated path as exact-ID moves.
    pub(crate) fn move_focused_pane_to_next_tab(&mut self) -> anyhow::Result<PaneId> {
        if self.tabs.len() < 2 {
            anyhow::bail!("A second tab is required");
        }
        let source = self
            .active_tab()
            .context("No source tab is active")?
            .focused_terminal_identity();
        let destination = (1..self.tabs.len())
            .map(|offset| (self.active + offset) % self.tabs.len())
            .find(|index| {
                !self.tabs[*index].is_closing()
                    && !self.tabs[*index].closed
                    && self.tabs[*index].allows_pane_splits()
            })
            .context("No destination tab is available")?;
        let target = self.tabs[destination].focused_terminal_identity();
        self.transfer_pane_by_id(
            source.tab_id,
            source.pane_id,
            target.tab_id,
            target.pane_id,
            crate::panes::SplitDirection::Vertical,
        )
    }

    /// Validate the source half of a process-local cross-window pane move.
    pub(crate) fn validate_window_pane_transfer_source(&self) -> anyhow::Result<()> {
        let source = self
            .active_tab()
            .context("No source tab is active")?
            .focused_terminal_identity();
        self.validate_pane_transfer_source_by_id(source.tab_id, source.pane_id)
    }

    pub(crate) fn validate_pane_transfer_source_by_id(
        &self,
        tab_id: usize,
        pane_id: PaneId,
    ) -> anyhow::Result<()> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .with_context(|| format!("GUI tab {tab_id} does not exist in the source window"))?;
        if tab.is_closing() || tab.closed {
            anyhow::bail!("GUI tab {tab_id} is already closing");
        }
        if tab.live_domain.is_some() {
            anyhow::bail!(
                "Live-domain panes are daemon-owned and require an acknowledged topology change"
            );
        }
        let manager = &tab.panes;
        if manager.pane_count() == 1 {
            anyhow::bail!("Split the source tab before moving a pane");
        }
        let pane = manager
            .get_pane(pane_id)
            .with_context(|| format!("GUI pane {} does not exist in tab {tab_id}", pane_id.0))?;
        if pane.closed {
            anyhow::bail!("GUI pane {} in tab {tab_id} is closed", pane_id.0);
        }
        if manager.open_pane_count() <= 1 {
            anyhow::bail!("The last pane in a tab cannot be moved");
        }
        Ok(())
    }

    pub(crate) fn validate_pane_transfer_destination_by_id(
        &self,
        tab_id: usize,
        target: PaneId,
    ) -> anyhow::Result<()> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .with_context(|| {
                format!("GUI tab {tab_id} does not exist in the destination window")
            })?;
        if tab.is_closing() || tab.closed {
            anyhow::bail!("GUI tab {tab_id} is already closing");
        }
        if tab.live_domain.is_some() {
            anyhow::bail!("A local pane cannot be attached to a live-domain tab");
        }
        if !tab.allows_pane_splits() {
            anyhow::bail!("A pane cannot be attached to a serial terminal tab");
        }
        tab.panes.validate_attach_target(target)
    }

    pub(crate) fn detach_pane_for_transfer_by_id(
        &mut self,
        tab_id: usize,
        pane_id: PaneId,
    ) -> anyhow::Result<crate::panes::Pane> {
        self.validate_pane_transfer_source_by_id(tab_id, pane_id)?;
        let tab = self
            .tab_mut_by_id(tab_id)
            .expect("validated source tab remains present");
        let pane = tab
            .panes
            .detach(pane_id)
            .context("The selected pane could not be detached")?;
        tab.reconcile_primary_pane_launch();
        Ok(pane)
    }

    pub(crate) fn attach_transferred_pane_by_id(
        &mut self,
        tab_id: usize,
        target: PaneId,
        pane: crate::panes::Pane,
        direction: crate::panes::SplitDirection,
    ) -> anyhow::Result<PaneId> {
        let destination = self
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .with_context(|| {
                format!("GUI tab {tab_id} does not exist in the destination window")
            })?;
        let id = self.tabs[destination]
            .panes
            .attach_target(pane, target, direction)?;
        self.active = destination;
        Ok(id)
    }

    pub(crate) fn transfer_pane_by_id(
        &mut self,
        source_tab_id: usize,
        pane_id: PaneId,
        destination_tab_id: usize,
        target: PaneId,
        direction: crate::panes::SplitDirection,
    ) -> anyhow::Result<PaneId> {
        if source_tab_id == destination_tab_id {
            anyhow::bail!("Moving a pane within the same tab requires a layout-tree rearrangement");
        }
        self.validate_pane_transfer_source_by_id(source_tab_id, pane_id)?;
        self.validate_pane_transfer_destination_by_id(destination_tab_id, target)?;
        let pane = self
            .detach_pane_for_transfer_by_id(source_tab_id, pane_id)
            .expect("prevalidated source pane remains detachable");
        self.attach_transferred_pane_by_id(destination_tab_id, target, pane, direction)
            .context("prevalidated destination pane remains attachable")
    }

    /// Validate the destination half of a cross-window pane move: beside the
    /// active tab's focused pane, under the same limits as exact-ID moves.
    pub(crate) fn validate_window_pane_transfer_destination(&self) -> anyhow::Result<()> {
        let target = self
            .active_tab()
            .context("No destination tab is active")?
            .focused_terminal_identity();
        self.validate_pane_transfer_destination_by_id(target.tab_id, target.pane_id)
    }

    /// Detach the validated focused pane while retaining its PTY, reader,
    /// terminal state, Harvest state, and process identity.
    pub(crate) fn detach_focused_pane_for_window_transfer(
        &mut self,
    ) -> anyhow::Result<crate::panes::Pane> {
        self.validate_window_pane_transfer_source()?;
        let pane = self
            .active_tab_mut()
            .and_then(|tab| tab.panes.detach_focused())
            .context("The focused pane could not be detached")?;
        self.active_tab_mut()
            .expect("validated source tab remains active")
            .reconcile_primary_pane_launch();
        Ok(pane)
    }

    /// Attach a detached owner to the already validated active destination.
    pub(crate) fn attach_window_transferred_pane(&mut self, pane: crate::panes::Pane) -> PaneId {
        let manager = &mut self.tabs[self.active].panes;
        manager
            .attach_target(
                pane,
                manager.focused_id(),
                crate::panes::SplitDirection::Vertical,
            )
            .expect("prevalidated destination remains attachable")
    }

    /// Get mutable access to all tabs (for resize propagation)
    pub fn all_tabs_mut(&mut self) -> &mut [Tab] {
        &mut self.tabs
    }

    /// Read tabs in their visible order without exposing mutable manager state.
    pub(crate) fn all_tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Switch to a specific tab
    pub fn switch_to(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            // Clear activity indicator when switching to tab
            if let Some(tab) = self.tabs.get_mut(index) {
                tab.has_activity = false;
            }
        }
    }

    /// Switch to next tab
    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.switch_to((self.active + 1) % self.tabs.len());
        }
    }

    /// Switch to previous tab
    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.switch_to(self.active.checked_sub(1).unwrap_or(self.tabs.len() - 1));
        }
    }

    /// Get number of tabs
    pub fn count(&self) -> usize {
        self.tabs.len()
    }

    /// Get active tab index
    pub fn active_index(&self) -> usize {
        self.active
    }

    pub(crate) fn set_active_workspace_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    pub(crate) fn last_tab_index(&self) -> Option<usize> {
        self.tabs.len().checked_sub(1)
    }

    /// Remove only the uncommitted tail created by one workspace-tab restore.
    /// Dropping it tears down its PTYs immediately, without close animations,
    /// automatic snapshots, or disturbing already-restored/existing owners.
    pub(crate) fn rollback_workspace_tab(&mut self, count: usize, active: usize) {
        assert!(count <= self.tabs.len());
        self.tabs.truncate(count);
        self.active = active.min(self.tabs.len().saturating_sub(1));
    }

    /// Move the tab a workspace restore just appended at `index` into the
    /// leading pinned group when it is pinned, as pinning it would, keeping
    /// the same tab active. Returns the tab's index.
    pub(crate) fn settle_restored_workspace_tab(&mut self, index: usize) -> usize {
        let boundary = self.tabs[..index]
            .iter()
            .take_while(|tab| tab.pinned)
            .count();
        if !self.tabs[index].pinned || boundary == index {
            return index;
        }
        let active_id = self.active_tab_id();
        let tab = self.tabs.remove(index);
        self.tabs.insert(boundary, tab);
        self.active = active_id
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
            .unwrap_or(boundary);
        boundary
    }

    pub(crate) fn new_shell_tab_from_workspace(
        &mut self,
        snapshot: &crate::workspace::WorkspaceTabSnapshot,
        pane_launches: &WorkspacePaneLaunches,
    ) -> anyhow::Result<(usize, Vec<String>)> {
        let mut warnings = Vec::new();
        let cwd = usable_primary_workspace_cwd(snapshot, &mut warnings)?;
        self.new_tab_with_cwd(cwd.as_deref(), Some(&snapshot.title))?;
        let index = self.tabs.len() - 1;
        warnings.extend(self.apply_workspace_layout(index, snapshot, pane_launches)?);
        Ok((index, warnings))
    }

    pub(crate) fn new_profile_tab_from_workspace(
        &mut self,
        snapshot: &crate::workspace::WorkspaceTabSnapshot,
        name: &str,
        shell: ShellConfig,
        pane_launches: &WorkspacePaneLaunches,
    ) -> anyhow::Result<(usize, Vec<String>)> {
        let mut warnings = Vec::new();
        let cwd = usable_primary_workspace_cwd(snapshot, &mut warnings)?;
        self.new_tab_with_profile(name, shell, cwd.as_deref(), Some(&snapshot.title))?;
        let index = self.tabs.len() - 1;
        warnings.extend(self.apply_workspace_layout(index, snapshot, pane_launches)?);
        Ok((index, warnings))
    }

    /// Give the tab at `index` the saved tab's titles, pin, and pane
    /// layout, relaunching its other panes.
    pub(crate) fn apply_workspace_layout(
        &mut self,
        index: usize,
        snapshot: &crate::workspace::WorkspaceTabSnapshot,
        pane_launches: &WorkspacePaneLaunches,
    ) -> anyhow::Result<Vec<String>> {
        let tab = self
            .tabs
            .get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("workspace target tab no longer exists"))?;
        tab.title.clone_from(&snapshot.title);
        tab.user_title.clone_from(&snapshot.user_title);
        tab.pinned = snapshot.pinned;
        if let Some(title) = &tab.user_title {
            tab.title.clone_from(title);
        }
        let warnings = tab
            .panes
            .restore_workspace_layout(snapshot, pane_launches)?;
        if let crate::workspace::WorkspaceLaunch::Plugin { plugin_id, profile } = pane_launches
            .get(&snapshot.primary_pane)
            .map_or(snapshot.launch()?, |(launch, _)| launch)
        {
            tab.set_plugin_profile_reference(
                PaneId(snapshot.primary_pane),
                plugin_id,
                profile.clone(),
            );
        }
        Ok(warnings)
    }

    pub(crate) fn capture_workspace(
        &self,
        name: String,
        save_scrollback: bool,
        config: crate::workspace::WorkspaceConfigReference,
    ) -> anyhow::Result<crate::workspace::WorkspaceSnapshot> {
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| !tab.closed && !tab.is_closing())
            .enumerate()
            .map(|(index, tab)| tab.capture_workspace_tab(index as u32, save_scrollback))
            .collect::<anyhow::Result<_>>()?;
        if tabs.is_empty() {
            anyhow::bail!("there are no live tabs to save as a workspace");
        }
        let active_runtime_id = self.active_tab().map(|tab| tab.id);
        let active_tab = self
            .tabs
            .iter()
            .filter(|tab| !tab.closed && !tab.is_closing())
            .position(|tab| Some(tab.id) == active_runtime_id)
            .unwrap_or(0);
        let id = format!("workspace-{}", crate::session::generate_session_id());
        let mut workspace = crate::workspace::WorkspaceSnapshot::new(id, name, tabs);
        workspace.active_tab = active_tab;
        workspace.config = config;
        workspace.validate()?;
        Ok(workspace)
    }

    /// Check if there are any tabs
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub(crate) fn has_live_domain_tabs(&self) -> bool {
        self.tabs.iter().any(|tab| tab.live_domain.is_some())
    }

    pub(crate) fn live_domain_tab_count(&self, domain_id: &str) -> usize {
        self.tabs
            .iter()
            .filter(|tab| {
                tab.live_domain
                    .as_ref()
                    .is_some_and(|live| live.domain_id == domain_id)
            })
            .count()
    }

    pub(crate) fn close_tab_by_id(&mut self, tab_id: usize) -> bool {
        self.tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .is_some_and(|index| self.close_tab(index))
    }

    pub(crate) fn close_live_domain_tabs(&mut self, domain_id: &str) -> usize {
        let indices: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| {
                tab.live_domain
                    .as_ref()
                    .is_some_and(|live| live.domain_id == domain_id)
                    .then_some(index)
            })
            .collect();
        indices
            .into_iter()
            .rev()
            .filter(|index| self.close_tab(*index))
            .count()
    }

    /// Apply a successful daemon pane closure to the corresponding relay tree.
    /// This is idempotent because relay cleanup may win the response race.
    pub(crate) fn reconcile_closed_live_domain_pane(
        &mut self,
        tab_id: usize,
        remote_pane_id: u32,
    ) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return false;
        };
        let Some(local_id) = tab.live_domain.as_ref().and_then(|live| {
            live.pane_ids
                .iter()
                .find_map(|(local, remote)| (*remote == remote_pane_id).then_some(*local))
        }) else {
            return false;
        };
        if !tab.panes.close(local_id) {
            return false;
        }
        if let Some(live) = &mut tab.live_domain {
            live.pane_ids.remove(&local_id);
        }
        true
    }

    /// Whether tab chrome is currently sliding in or shrinking out.
    pub fn has_active_animation(&self) -> bool {
        if self.reduced_motion {
            return false;
        }
        self.tabs
            .iter()
            .any(|tab| tab.slide_progress() < 1.0 || tab.is_closing())
    }

    /// Poll every tab and pane, including inactive/unfocused terminals. Protocol
    /// replies are already consumed by their originating PTY owner; returned
    /// events retain a stable route for any app-mediated reply.
    pub fn poll_events(&mut self) -> (bool, Vec<RoutedTerminalEvent>) {
        let mut changed = false;
        let mut all_events = Vec::new();
        let mut dropped_events = 0usize;

        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let tab_id = tab.id;
            let poll = tab.poll_events();
            let tab_changed = poll.changed;
            let events = poll
                .events
                .into_iter()
                .map(|pane_event| RoutedTerminalEvent {
                    origin: TerminalEventOrigin {
                        tab_id,
                        pane_id: pane_event.pane_id,
                    },
                    event: pane_event.event,
                });
            if poll.saw_output {
                tab.update_title();
            }
            if tab_changed && i != self.active {
                tab.has_activity = true;
            }
            changed |= tab_changed;
            for event in events {
                if all_events.len() < MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL {
                    all_events.push(event);
                } else {
                    dropped_events += 1;
                }
            }
        }

        if dropped_events > 0 && !self.event_overflow_reported {
            self.event_overflow_reported = true;
            log::warn!(
                "Tab manager dropped {} app-facing terminal events after reaching the per-poll limit of {}",
                dropped_events,
                MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL
            );
        }

        (changed || !all_events.is_empty(), all_events)
    }

    /// Whether the active tab is already laid out for `area`, so fitting it
    /// would change nothing.
    pub(crate) fn active_has_area(&self, area: TerminalArea) -> bool {
        self.area == area
            && self
                .tabs
                .get(self.active)
                .is_none_or(|tab| tab.panes.is_laid_out_for(area))
    }

    /// Fits only the active tab to `area`, which new tabs then take too.
    /// Other tabs keep their size until they are shown, so a change that
    /// differs from tab to tab, such as the agent view's dock, never
    /// reflows or signals programs in tabs nobody is looking at.
    pub(crate) fn resize_active(&mut self, area: TerminalArea) {
        self.area = area;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.panes.resize(area);
        }
    }

    /// Fit every tab, and the tabs created later, to `area`.
    pub(crate) fn resize(&mut self, area: TerminalArea) {
        self.area = area;
        for tab in &mut self.tabs {
            tab.panes.resize(area);
        }
    }

    /// Detect plugin tabs that are about to be cleaned up but exited suspiciously
    /// fast (within `QUICK_EXIT_WINDOW` of creation). Returns a list of (plugin_id, output_lines)
    /// for each quick-exiting plugin tab, so the caller can show an error overlay.
    ///
    /// Must be called BEFORE `cleanup_closed`, since that removes the tabs.
    pub fn drain_quick_exit_plugin_info(&self) -> Vec<(String, Vec<String>)> {
        let mut results = Vec::new();

        for tab in &self.tabs {
            // Only interested in plugin tabs that just closed
            if !tab.closed {
                continue;
            }
            let plugin_id = match &tab.tab_type {
                TabType::Plugin { plugin_id } => plugin_id.clone(),
                TabType::Shell | TabType::Profile { .. } | TabType::Serial { .. } => continue,
            };

            if tab.created_at.elapsed() >= QUICK_EXIT_WINDOW {
                continue;
            }

            // The visible screen, without its leading and trailing blank rows,
            // explains why the plugin exited.
            let Some(terminal) = tab.focused_terminal() else {
                continue;
            };
            let last_row = i32::try_from(terminal.grid().rows()).unwrap_or(i32::MAX) - 1;
            let mut lines: Vec<String> = terminal
                .rows_text(0, last_row)
                .lines()
                .skip_while(|line| line.is_empty())
                .map(String::from)
                .collect();
            while lines.last().is_some_and(|line| line.is_empty()) {
                lines.pop();
            }

            results.push((plugin_id, lines));
        }

        results
    }

    /// Remove exited panes from tabs that stay open, only after App has
    /// handled the routed terminal events from the current poll. This
    /// ordering keeps app-mediated replies addressable by their exact pane ID.
    /// A tab whose every pane exited closes with all of them, so its closing
    /// snapshot saves the whole tab.
    fn cleanup_pane_topology(&mut self) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            if tab.panes.all_closed() {
                tab.closed = true;
            }
            if tab.closed {
                continue;
            }
            let before = tab.panes.pane_count();
            tab.panes.cleanup_closed();
            if let Some(live) = &mut tab.live_domain {
                live.pane_ids.retain(|local, _| tab.panes.contains(*local));
            }
            if tab.panes.pane_count() != before {
                tab.reconcile_primary_pane_launch();
                changed = true;
            }
        }
        changed
    }

    /// Remove closed tabs and tabs with completed close animations.
    /// Before removing, saves each non-empty tab when auto-save is enabled.
    /// Returns true if tab or pane topology changed.
    pub fn cleanup_closed(&mut self, auto_save: bool, save_scrollback: bool) -> bool {
        let pane_topology_changed = self.cleanup_pane_topology();
        let initial_count = self.tabs.len();

        // Save sessions for tabs about to be removed
        let mut saved_closed_tab = false;
        for tab in self.tabs.iter_mut() {
            let removing =
                tab.closed || tab.is_close_complete() || (self.reduced_motion && tab.is_closing());
            if auto_save && removing {
                saved_closed_tab |=
                    auto_save_closing_tab(tab, save_scrollback, &mut self.persistence_notices);
            }
        }

        if saved_closed_tab {
            crate::workspace::prune_closed_tabs();
        }

        // Now remove them, keeping the same tab active: removing tabs before
        // it would otherwise shift a different tab under the active index.
        let active_id = self.tabs.get(self.active).map(|tab| tab.id);
        self.tabs.retain(|tab| {
            !(tab.closed || tab.is_close_complete() || self.reduced_motion && tab.is_closing())
        });

        if self.tabs.len() != initial_count {
            if let Some(index) =
                active_id.and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
            {
                self.active = index;
            } else if self.active >= self.tabs.len() && !self.tabs.is_empty() {
                self.active = self.tabs.len() - 1;
            }
            true
        } else {
            pane_topology_changed
        }
    }

    /// Write an app-mediated response to the exact terminal that requested it.
    /// No focused-terminal fallback is allowed: a stale/missing origin fails
    /// explicitly rather than leaking clipboard or protocol data elsewhere.
    pub fn write_to_origin(&mut self, origin: TerminalEventOrigin, data: &[u8]) -> io::Result<()> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == origin.tab_id)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "terminal tab no longer exists")
            })?
            .pane_mut(origin.pane_id)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "originating pane no longer exists")
            })?
            .write_protocol_reply(data)
    }

    /// Write captured user input to one exact terminal. Unlike
    /// `write_to_origin`, this path enforces the pane-local read-only policy.
    pub(crate) fn write_user_to_origin(
        &mut self,
        origin: TerminalEventOrigin,
        data: &[u8],
    ) -> io::Result<()> {
        let tab = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == origin.tab_id)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "terminal tab no longer exists")
            })?;
        if tab.closed || tab.is_closing() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "terminal tab is closed or closing",
            ));
        }
        tab.pane_mut(origin.pane_id)
            .filter(|pane| !pane.closed)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "originating pane no longer exists")
            })?
            .write(data)
    }

    /// The identity of `origin`'s terminal, while that terminal is still live,
    /// for retaining it across asynchronous work.
    pub(crate) fn identity_for_origin(
        &self,
        origin: TerminalEventOrigin,
    ) -> Option<TerminalIdentity> {
        self.origin_for_pane(origin.tab_id, origin.pane_id)
            .map(|origin| TerminalIdentity {
                tab_id: origin.tab_id,
                pane_id: origin.pane_id,
            })
    }

    /// Resolve a stable tab-local pane identity to a route for its live
    /// terminal. It never crosses tab namespaces or substitutes another pane.
    pub(crate) fn origin_for_pane(
        &self,
        tab_id: usize,
        pane_id: PaneId,
    ) -> Option<TerminalEventOrigin> {
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        if tab.closed || tab.is_closing() {
            return None;
        }
        tab.pane(pane_id).filter(|pane| !pane.closed)?;
        Some(TerminalEventOrigin { tab_id, pane_id })
    }

    pub(crate) fn origin_read_only(&self, origin: TerminalEventOrigin) -> Option<bool> {
        let tab = self.tabs.iter().find(|tab| tab.id == origin.tab_id)?;
        if tab.closed || tab.is_closing() {
            return None;
        }
        tab.pane(origin.pane_id)
            .filter(|pane| !pane.closed)
            .map(|pane| pane.read_only)
    }

    pub(crate) fn terminal_for_origin(&self, origin: TerminalEventOrigin) -> Option<&Terminal> {
        self.tab_by_id(origin.tab_id)?.pane_terminal(origin.pane_id)
    }

    pub(crate) fn terminal_for_origin_mut(
        &mut self,
        origin: TerminalEventOrigin,
    ) -> Option<&mut Terminal> {
        self.tab_mut_by_id(origin.tab_id)?
            .pane_mut(origin.pane_id)
            .map(|pane| &mut pane.terminal)
    }

    pub(crate) fn set_title_for_origin(
        &mut self,
        origin: TerminalEventOrigin,
        title: String,
    ) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == origin.tab_id) else {
            return false;
        };
        if tab.live_domain.is_some() {
            // The daemon parses this same relay output and publishes the
            // authoritative title. Applying it locally would bypass explicit
            // remote rename precedence and briefly diverge across windows.
            return true;
        }
        if !tab.set_pane_osc_title(origin.pane_id, &title) {
            return false;
        }
        tab.set_osc_title(title);
        true
    }

    /// Iterate over all tabs with their indices
    pub fn iter(&self) -> impl Iterator<Item = (usize, &Tab)> {
        self.tabs.iter().enumerate()
    }

    /// The tabs a live domain shows here, in tab-bar order: the order its
    /// tab-move index counts in. Closed tabs keep their place until the
    /// domain removes them.
    pub(crate) fn live_domain_tabs<'a>(
        &'a self,
        domain_id: &'a str,
    ) -> impl Iterator<Item = &'a Tab> + 'a {
        self.tabs.iter().filter(move |tab| {
            tab.live_domain_tab_route()
                .is_some_and(|(candidate, _)| candidate == domain_id)
        })
    }

    /// Where the tab showing `remote_tab_id` sits among
    /// [`live_domain_tabs`](Self::live_domain_tabs).
    pub(crate) fn live_domain_tab_index(
        &self,
        domain_id: &str,
        remote_tab_id: u32,
    ) -> Option<usize> {
        self.live_domain_tabs(domain_id).position(|tab| {
            tab.live_domain_tab_route()
                .is_some_and(|(_, remote)| remote == remote_tab_id)
        })
    }

    /// Re-identify the coding agent in every terminal. Returns whether any
    /// terminal changed.
    pub(crate) fn refresh_agents(&mut self, catalog: &crate::agents::AgentCatalog) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            changed |= tab.refresh_agents(catalog);
        }
        changed
    }

    /// Forget every terminal's agent, its reported status, and its diffs,
    /// as when agent features are off. Returns whether any terminal changed.
    pub(crate) fn forget_agents(&mut self) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            changed |= tab.forget_agents();
        }
        changed
    }

    /// The open terminal whose processes inherited `terminal_id`.
    pub(crate) fn terminal_with_id(&self, terminal_id: &str) -> Option<TerminalIdentity> {
        self.tabs
            .iter()
            .filter(|tab| !tab.closed && !tab.is_closing())
            .find_map(|tab| {
                tab.pane_with_terminal_id(terminal_id)
                    .map(|pane_id| TerminalIdentity {
                        tab_id: tab.id,
                        pane_id,
                    })
            })
    }

    /// Get tab info for display (index, title, is_active, has_activity)
    pub fn tab_info(&self) -> Vec<TabInfo> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| TabInfo {
                title: tab.display_title().to_owned(),
                pinned: tab.pinned,
                broadcast_armed: false,
                broadcast_active: false,
                read_only: tab.focused_pane_read_only(),
                is_active: i == self.active && !tab.is_closing(),
                has_activity: tab.has_activity,
                slide_progress: if self.reduced_motion {
                    1.0
                } else {
                    tab.slide_progress()
                },
                close_progress: if self.reduced_motion && tab.is_closing() {
                    1.0
                } else {
                    tab.close_progress()
                },
                tab_type: tab.tab_type.clone(),
                // Diff review is an agent feature.
                unreviewed_diff_count: if self.policy.agent_features {
                    tab.unreviewed_diff_count()
                } else {
                    0
                },
                agent_badge: tab.agent_badge(),
                rename: None,
            })
            .collect()
    }

    /// Apply host settings to every terminal and to terminals created later.
    /// `apply_keyboard_type` is as in [`TerminalPolicy::apply`].
    pub(crate) fn set_terminal_policy(
        &mut self,
        policy: TerminalPolicy,
        apply_keyboard_type: bool,
    ) {
        self.policy = policy;
        for tab in &mut self.tabs {
            tab.apply_terminal_policy(&policy, apply_keyboard_type);
        }
    }

    /// Update the shell used by tabs created after a config reload.
    /// Existing PTYs are intentionally left running.
    pub fn set_shell_config(&mut self, shell: ShellConfig) {
        self.shell = shell;
    }

    pub fn drain_persistence_notices(&mut self) -> Vec<PersistenceNotice> {
        std::mem::take(&mut self.persistence_notices)
    }

    pub fn drain_user_notices(&mut self) -> Vec<String> {
        self.tabs
            .iter_mut()
            .flat_map(Tab::drain_user_notices)
            .collect()
    }
}

/// Saves a closing tab as a closed-tab snapshot when it has content and is not a
/// live-domain relay, recording the outcome for the person. Returns whether a
/// snapshot was saved.
fn auto_save_closing_tab(
    tab: &mut Tab,
    save_scrollback: bool,
    notices: &mut Vec<PersistenceNotice>,
) -> bool {
    if tab.live_domain.is_some() {
        log::debug!(
            "Skipping closed-tab snapshot for live-domain relay tab {}",
            tab.id
        );
        return false;
    }
    if !tab.has_content() {
        log::debug!("Skipping empty closing tab {} (no content)", tab.id);
        return false;
    }
    match tab.save_closed_tab(save_scrollback) {
        Ok(outcome) => {
            log::info!(
                "Auto-saved closing tab {} as closed tab '{}'",
                tab.id,
                outcome.id
            );
            notices.push(PersistenceNotice::Saved {
                id: outcome.id,
                truncated: outcome.scrollback_truncated,
            });
            true
        }
        Err(error) => {
            log::error!("Failed to auto-save closing tab {}: {error:#}", tab.id);
            notices.push(PersistenceNotice::SaveFailed {
                tab_id: tab.id,
                error: format!("{error:#}"),
            });
            false
        }
    }
}

/// Tab information for display
#[derive(Debug, Clone)]
pub struct TabInfo {
    pub title: String,
    pub pinned: bool,
    pub broadcast_armed: bool,
    pub broadcast_active: bool,
    pub read_only: bool,
    pub is_active: bool,
    pub has_activity: bool,
    /// Slide-in animation progress (0.0 = just created, 1.0 = fully visible)
    pub slide_progress: f32,
    /// Close animation progress (0.0 = not closing, 1.0 = fully closed)
    pub close_progress: f32,
    /// What type of tab this is
    pub tab_type: TabType,
    /// Number of unreviewed diffs detected in this tab
    pub unreviewed_diff_count: usize,
    /// The most urgent state reported by an agent in this tab.
    pub agent_badge: Option<crate::agents::AgentBadge>,
    /// While this tab is being renamed, where the draft's caret sits and
    /// whether the whole draft is selected; `title` is then the draft.
    pub rename: Option<TabRenameView>,
}

/// The caret and selection of a tab's rename draft, as the tab bar draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabRenameView {
    /// The caret, as a byte offset into the draft.
    pub cursor: usize,
    pub all_selected: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::{PaneManager, SplitDirection};
    use crate::terminal::geometry::TerminalGeometry;
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    #[test]
    fn a_lone_pane_shows_the_tab_title_and_a_split_pane_keeps_its_own() {
        let mut manager = TabManager::for_test(crate::test_support::sleeping_shell());
        let tab_id = manager.new_tab().unwrap();
        let origin = TerminalEventOrigin {
            tab_id,
            pane_id: PaneId(0),
        };
        assert!(manager.set_title_for_origin(origin, "lone".into()));
        let tab = manager.active_tab_mut().unwrap();
        assert_eq!(tab.title, "lone");
        assert_eq!(tab.panes.get_pane(PaneId(0)).unwrap().title, None);

        let right = tab
            .panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        assert!(manager.set_title_for_origin(origin, "split".into()));
        let tab = manager.active_tab_mut().unwrap();
        assert_eq!(
            tab.panes.get_pane(PaneId(0)).unwrap().title.as_deref(),
            Some("split")
        );

        assert!(tab.panes.close(right));
        assert_eq!(tab.panes.get_pane(PaneId(0)).unwrap().title, None);
        let saved = tab.capture_workspace_tab(0, false).unwrap();
        assert_eq!(saved.panes[0].title.as_deref(), Some("split"));
        assert_eq!(
            saved.layout,
            crate::workspace::WorkspacePaneLayout::Pane { pane: 0 }
        );
    }

    #[test]
    fn titles_longer_than_a_workspace_holds_are_cut_when_saved() {
        let root = tempfile::tempdir().unwrap();
        crate::workspace::set_test_workspaces_base_dir(Some(root.path().to_path_buf()));
        let shell = crate::test_support::sleeping_shell();
        let mut manager = TabManager::for_test(shell.clone());
        let tab_id = manager.new_tab().unwrap();
        // OSC titles may be up to 4096 bytes; this one is 300 characters.
        let long = "界".repeat(300);
        let cut = format!(
            "{}…",
            "界".repeat(crate::workspace::MAX_WORKSPACE_TITLE_CHARS - 1)
        );
        let lone = TerminalEventOrigin {
            tab_id,
            pane_id: PaneId(0),
        };
        assert!(manager.set_title_for_origin(lone, long.clone()));
        let right = manager
            .active_tab_mut()
            .unwrap()
            .panes_for_test(shell, 800.0, 160.0, 10.0, 20.0)
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        let split = TerminalEventOrigin {
            tab_id,
            pane_id: right,
        };
        assert!(manager.set_title_for_origin(split, long.clone()));
        assert_eq!(manager.active_tab().unwrap().title, long);

        let workspace = manager
            .capture_workspace("Long titles".into(), false, Default::default())
            .unwrap();
        let tab = &workspace.tabs[0];
        assert_eq!(tab.title, cut);
        for pane in &tab.panes {
            assert_eq!(
                pane.title.as_deref(),
                Some(cut.as_str()),
                "pane {}",
                pane.id
            );
        }

        // A profile's title becomes the tab's own title, and has no limit.
        manager
            .active_tab_mut()
            .unwrap()
            .set_user_title(Some(long.clone()));
        let workspace = manager
            .capture_workspace("Long titles".into(), false, Default::default())
            .unwrap();
        assert_eq!(workspace.tabs[0].user_title.as_deref(), Some(cut.as_str()));

        let tab = manager.active_tab_mut().unwrap();
        for pane in tab.panes.all_panes_mut() {
            pane.last_command = Some("build".into());
            pane.closed = true;
        }
        assert!(manager.cleanup_closed(true, false));
        let notices = manager.drain_persistence_notices();
        let [PersistenceNotice::Saved { id, .. }] = notices.as_slice() else {
            panic!("the closed tab with long titles was not saved: {notices:?}");
        };
        let saved = crate::workspace::load_workspace(id).unwrap().workspace;
        crate::workspace::set_test_workspaces_base_dir(None);
        assert_eq!(saved.tabs[0].title, cut);
        assert_eq!(saved.tabs[0].user_title.as_deref(), Some(cut.as_str()));
    }

    #[test]
    fn removing_closed_tabs_keeps_the_same_tab_active() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 5".into()],
            env: Default::default(),
        };
        let mut manager = TabManager::new(80, 24, shell, 100, 1024 * 1024);
        for _ in 0..3 {
            manager.new_tab().unwrap();
        }
        let ids: Vec<usize> = manager.iter().map(|(_, tab)| tab.id).collect();
        manager.switch_to(1);
        assert!(manager.close_tab_by_id(ids[0]));
        manager.set_reduced_motion(true);
        manager.cleanup_closed(false, false);
        assert_eq!(manager.active_tab_id(), Some(ids[1]));
    }

    #[test]
    fn test_tab_manager_navigation() {
        let manager = TabManager::new(
            80,
            24,
            ShellConfig::default(),
            10_000,
            termeric_core::DEFAULT_SCROLLBACK_MEMORY_BYTES,
        );

        // Can't test much without actual PTY, but test navigation logic
        assert!(manager.is_empty());
        assert_eq!(manager.count(), 0);
    }

    #[test]
    fn runtime_scrollback_limits_bound_existing_panes_future_splits_and_future_tabs() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 5".into()],
            env: Default::default(),
        };
        let mut manager = TabManager::new(80, 8, shell.clone(), 1_000, 8 * 1024 * 1024);
        manager
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 5".into()],
                None,
                Some("existing-panes"),
                None,
                HashMap::new(),
            )
            .unwrap();

        let panes = manager
            .active_tab_mut()
            .unwrap()
            .panes_for_test(shell, 800.0, 160.0, 10.0, 20.0);
        panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();

        const LINE_LIMIT: usize = 32;
        const BYTE_LIMIT: usize = 4 * 1024;
        manager.set_terminal_policy(
            TerminalPolicy::with_scrollback(LINE_LIMIT, BYTE_LIMIT),
            false,
        );
        manager
            .active_tab_mut()
            .unwrap()
            .panes
            .split(
                SplitDirection::Horizontal,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        manager
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 5".into()],
                None,
                Some("future-tab"),
                None,
                HashMap::new(),
            )
            .unwrap();

        let line = format!("{}\r\n", "bounded-history".repeat(32));
        for tab in &mut manager.tabs {
            for pane in tab.panes.all_panes_mut() {
                for _ in 0..64 {
                    pane.terminal.process(line.as_bytes());
                }
            }
        }

        let usages: Vec<_> = manager
            .tabs
            .iter()
            .flat_map(|tab| {
                tab.panes
                    .iter_panes()
                    .map(|pane| pane.terminal.memory_usage())
            })
            .collect();
        assert_eq!(usages.len(), 4);
        for usage in usages {
            assert_eq!(usage.scrollback_line_limit, LINE_LIMIT);
            assert_eq!(usage.scrollback_byte_limit, BYTE_LIMIT);
            assert!(usage.scrollback_lines <= LINE_LIMIT);
            assert!(usage.scrollback_bytes <= BYTE_LIMIT);
        }
    }

    #[test]
    fn simultaneous_pane_exit_saves_the_whole_closed_tab_before_cleanup() {
        let root = tempfile::tempdir().unwrap();
        crate::workspace::set_test_workspaces_base_dir(Some(root.path().to_path_buf()));
        let mut manager = TabManager::new(80, 8, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
        manager
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 5".into()],
                None,
                Some("close-save"),
                None,
                HashMap::new(),
            )
            .unwrap();

        let tab = manager.active_tab_mut().unwrap();
        let panes = tab.panes_for_test(ShellConfig::default(), 800.0, 160.0, 10.0, 20.0);
        let right = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        for (id, marker, command) in [
            (PaneId(0), "left-history", "left-command"),
            (right, "right-history", "right-command"),
        ] {
            let pane = panes.get_pane_mut(id).unwrap();
            for index in 0..20 {
                pane.terminal
                    .process(format!("{marker}-{index}\r\n").as_bytes());
            }
            pane.last_command = Some(command.into());
            pane.closed = true;
        }
        panes.focus(right);

        assert!(manager.cleanup_closed(true, true));
        assert!(manager.is_empty());
        let notices = manager.drain_persistence_notices();
        let [PersistenceNotice::Saved { id, truncated }] = notices.as_slice() else {
            panic!("the closing split tab was not saved: {notices:?}");
        };
        assert!(!truncated);
        let saved = crate::workspace::load_workspace(id).unwrap().workspace;
        let listed = crate::workspace::list_closed_tabs();
        crate::workspace::set_test_workspaces_base_dir(None);
        assert!(saved.closed_tab);
        assert_eq!(listed.len(), 1);
        assert!(saved.name.ends_with("| right-command"), "{}", saved.name);
        let [tab] = saved.tabs.as_slice() else {
            panic!("a closed tab is one tab");
        };
        assert_eq!(tab.focused_pane, right.0);
        for (pane, history) in [(0, "left-history"), (right.0, "right-history")] {
            let pane = tab.panes.iter().find(|saved| saved.id == pane).unwrap();
            assert!(pane.scrollback_lines.join("\n").contains(history));
        }
    }

    #[test]
    fn a_split_tab_is_worth_saving_before_anything_runs_in_it() {
        let shell = crate::test_support::sleeping_shell();
        let mut manager = TabManager::for_test(shell.clone());
        manager.new_tab().unwrap();
        let tab = manager.active_tab_mut().unwrap();
        assert!(!tab.has_content(), "a pristine lone pane is not kept");
        tab.panes_for_test(shell, 800.0, 160.0, 10.0, 20.0)
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        assert!(tab.has_content(), "its layout is worth having back");
    }

    #[test]
    fn output_in_any_pane_makes_a_closing_tab_worth_saving() {
        let root = tempfile::tempdir().unwrap();
        crate::workspace::set_test_workspaces_base_dir(Some(root.path().to_path_buf()));
        let shell = crate::test_support::sleeping_shell();
        let mut manager = TabManager::for_test(shell.clone());
        manager.new_tab().unwrap();
        let tab = manager.active_tab_mut().unwrap();
        let panes = tab.panes_for_test(shell, 800.0, 160.0, 10.0, 20.0);
        let right = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        // The unfocused pane scrolled output away; the focused one is idle.
        let left = panes.get_pane_mut(PaneId(0)).unwrap();
        for index in 0..40 {
            left.terminal
                .process(format!("line {index}\r\n").as_bytes());
        }
        panes.focus(right);
        assert!(tab.has_content());

        assert!(manager.close_tab(0));
        manager.set_reduced_motion(true);
        manager.cleanup_closed(true, false);
        let notices = manager.drain_persistence_notices();
        let listed = crate::workspace::list_closed_tabs();
        crate::workspace::set_test_workspaces_base_dir(None);
        assert!(
            matches!(notices.as_slice(), [PersistenceNotice::Saved { .. }]),
            "{notices:?}"
        );
        assert_eq!(listed.len(), 1);
    }

    #[test]
    fn a_missing_primary_directory_warning_names_the_primary_pane() {
        let snapshot = crate::workspace::WorkspaceTabSnapshot::single_pane_for_test(
            4,
            "moved",
            crate::workspace::WorkspacePaneSnapshot {
                working_directory: Some("/definitely/missing/termeric-cwd".into()),
                ..crate::workspace::WorkspacePaneSnapshot::shell_for_test(3)
            },
        );
        let mut manager = TabManager::for_test(crate::test_support::sleeping_shell());
        let (_, warnings) = manager
            .new_shell_tab_from_workspace(&snapshot, &HashMap::new())
            .unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.starts_with("tab 4 pane 3 cwd")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_tab_holds_as_many_panes_as_a_workspace_saves_and_no_more() {
        use crate::panes::MAX_PANES_PER_TAB;
        let shell = crate::test_support::sleeping_shell();
        let mut manager = TabManager::for_test(shell.clone());
        manager.new_tab().unwrap();
        let mut other = TabManager::for_test(shell.clone());
        other.new_tab().unwrap();
        let other_panes =
            other
                .active_tab_mut()
                .unwrap()
                .panes_for_test(shell.clone(), 800.0, 160.0, 10.0, 20.0);
        other_panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        let moved = other_panes.detach_focused().unwrap();
        let tab = manager.active_tab_mut().unwrap();
        let panes = tab.panes_for_test(shell.clone(), 6_400.0, 6_400.0, 10.0, 20.0);
        for index in 1..MAX_PANES_PER_TAB {
            let direction = if index % 2 == 0 {
                SplitDirection::Horizontal
            } else {
                SplitDirection::Vertical
            };
            panes
                .split(direction, crate::config::PaneCwdMode::Focused)
                .unwrap();
            panes.balance();
        }
        assert_eq!(panes.pane_count(), MAX_PANES_PER_TAB);

        // Neither a split, by key or by control request, nor a pane moved in
        // from another tab goes past it.
        let focused = panes.focused_id();
        assert!(
            panes
                .split(
                    SplitDirection::Vertical,
                    crate::config::PaneCwdMode::Focused
                )
                .is_err()
        );
        assert!(
            panes
                .split_target(
                    focused,
                    SplitDirection::Vertical,
                    crate::config::PaneCwdMode::Focused
                )
                .is_err()
        );
        assert!(panes.validate_attach_target(focused).is_err());
        assert!(
            panes
                .attach_target(moved, focused, SplitDirection::Vertical)
                .is_err()
        );
        assert_eq!(panes.pane_count(), MAX_PANES_PER_TAB);
        assert_eq!(panes.focused_id(), focused);

        // The full tab saves, as a workspace and as a closed tab.
        let tab = manager.active_tab().unwrap();
        tab.capture_workspace_tab(0, false)
            .unwrap()
            .validate()
            .unwrap();
        tab.closed_tab_snapshot(false).unwrap().validate().unwrap();
    }

    #[test]
    fn a_failed_first_save_leaves_the_tab_without_a_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let blocked = root.path().join("not-a-directory");
        std::fs::write(&blocked, "sentinel").unwrap();
        crate::workspace::set_test_workspaces_base_dir(Some(blocked));
        let mut manager = TabManager::for_test(ShellConfig::default());
        manager
            .new_tab_with_command(
                "/bin/cat",
                &[],
                None,
                Some("snapshot"),
                None,
                HashMap::new(),
            )
            .unwrap();
        let tab = manager.active_tab_mut().unwrap();
        let result = tab.save_closed_tab(false);
        crate::workspace::set_test_workspaces_base_dir(None);
        assert!(result.is_err());
        assert!(tab.saved_snapshot.is_none());
    }

    #[test]
    fn screen_checksum_policy_updates_existing_and_future_tabs() {
        let mut manager = TabManager::for_test(ShellConfig::default());
        let allowed = TerminalPolicy {
            screen_checksum_reporting: true,
            kitty_file_transmission: true,
            ..TerminalPolicy::with_scrollback(1_000, 8 * 1024 * 1024)
        };
        manager.set_terminal_policy(allowed, false);
        manager
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 1".into()],
                None,
                Some("checksum-policy"),
                None,
                HashMap::new(),
            )
            .unwrap();
        assert!(
            manager
                .active_tab()
                .and_then(Tab::focused_terminal)
                .is_some_and(Terminal::screen_checksum_reporting)
        );
        assert!(
            manager
                .active_tab()
                .and_then(Tab::focused_terminal)
                .is_some_and(Terminal::kitty_file_transmission)
        );

        manager.set_terminal_policy(
            TerminalPolicy::with_scrollback(1_000, 8 * 1024 * 1024),
            false,
        );
        assert!(manager.tabs.iter().all(|tab| {
            tab.focused_terminal().is_some_and(|terminal| {
                !terminal.screen_checksum_reporting() && !terminal.kitty_file_transmission()
            })
        }));
    }

    #[test]
    fn xterm_keyboard_profile_updates_existing_and_future_tabs() {
        use termeric_core::term::XtermKeyboardType;
        let keyboard = |xterm_keyboard_type| TerminalPolicy {
            xterm_keyboard_type,
            ..TerminalPolicy::with_scrollback(1_000, 8 * 1024 * 1024)
        };
        let mut manager = TabManager::for_test(ShellConfig::default());
        manager.set_terminal_policy(keyboard(XtermKeyboardType::Legacy), true);
        manager
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 1".into()],
                None,
                Some("keyboard-profile"),
                None,
                HashMap::new(),
            )
            .unwrap();
        let terminal = manager.active_tab().unwrap().focused_terminal().unwrap();
        assert_eq!(terminal.xterm_keyboard_type(), XtermKeyboardType::Legacy);
        assert_eq!(
            terminal.initial_xterm_keyboard_type(),
            XtermKeyboardType::Legacy
        );

        manager
            .active_tab_mut()
            .unwrap()
            .focused_terminal_mut()
            .unwrap()
            .process(b"\x1b[?1061h");
        manager.set_terminal_policy(keyboard(XtermKeyboardType::Legacy), false);
        assert_eq!(
            manager
                .active_tab()
                .unwrap()
                .focused_terminal()
                .unwrap()
                .xterm_keyboard_type(),
            XtermKeyboardType::Vt220
        );

        manager.set_terminal_policy(keyboard(XtermKeyboardType::Normal), true);
        let terminal = manager.active_tab().unwrap().focused_terminal().unwrap();
        assert_eq!(terminal.xterm_keyboard_type(), XtermKeyboardType::Normal);
        assert_eq!(
            terminal.initial_xterm_keyboard_type(),
            XtermKeyboardType::Normal
        );
    }

    #[test]
    fn rename_reorder_and_pin_preserve_stable_tab_identity() {
        let mut manager = TabManager::for_test(ShellConfig::default());
        for title in ["one", "two", "three"] {
            manager
                .new_tab_with_command(
                    "/bin/sh",
                    &["-c".into(), "sleep 1".into()],
                    None,
                    Some(title),
                    None,
                    HashMap::new(),
                )
                .unwrap();
        }
        let stable = manager.active_tab_id().unwrap();
        manager
            .active_tab_mut()
            .unwrap()
            .set_user_title(Some("mine".into()));
        manager
            .active_tab_mut()
            .unwrap()
            .set_osc_title("remote".into());
        assert_eq!(manager.active_tab().unwrap().title, "mine");

        assert!(manager.toggle_pin_active());
        assert_eq!(manager.active_index(), 0);
        assert_eq!(manager.active_tab_id(), Some(stable));
        assert!(manager.active_tab().unwrap().pinned);
        assert!(!manager.move_active_by(-1));

        assert!(manager.toggle_pin_active());
        assert!(!manager.active_tab().unwrap().pinned);
        assert!(manager.move_active_by(1));
        assert_eq!(manager.active_tab_id(), Some(stable));
        assert_eq!(manager.tab_info()[manager.active_index()].title, "mine");
    }

    #[test]
    fn exact_tab_reorder_preserves_selection_and_rejects_pinned_boundary_crossing() {
        let mut manager = TabManager::for_test(ShellConfig::default());
        for title in ["one", "two", "three"] {
            manager
                .new_tab_with_command(
                    "/bin/sh",
                    &["-c".into(), "sleep 1".into()],
                    None,
                    Some(title),
                    None,
                    HashMap::new(),
                )
                .unwrap();
        }
        let selected = manager.active_tab_id().unwrap();
        assert!(manager.move_tab_by_id_to(0, 2).unwrap());
        assert_eq!(manager.active_tab_id(), Some(selected));
        assert_eq!(
            manager.iter().map(|(_, tab)| tab.id).collect::<Vec<_>>(),
            vec![1, 2, 0]
        );
        assert!(!manager.move_tab_by_id_to(0, 2).unwrap());

        manager.tab_mut_by_id(1).unwrap().pinned = true;
        let error = manager.move_tab_by_id_to(0, 0).unwrap_err().to_string();
        assert!(error.contains("pinned-tab boundary"), "{error}");
        assert_eq!(
            manager.iter().map(|(_, tab)| tab.id).collect::<Vec<_>>(),
            vec![1, 2, 0]
        );
    }

    #[test]
    fn tab_drag_reorder_requires_the_original_selected_owner() {
        let mut manager = TabManager::for_test(ShellConfig::default());
        for _ in 0..3 {
            manager
                .new_tab_with_command(
                    "/bin/sh",
                    &["-c".into(), "sleep 1".into()],
                    None,
                    None,
                    None,
                    HashMap::new(),
                )
                .unwrap();
        }
        let dragged = manager.active_tab_id().unwrap();
        let original: Vec<_> = manager.iter().map(|(_, tab)| tab.id).collect();
        manager.switch_to(0);
        assert!(!manager.move_dragged_tab_to(dragged, 1));
        assert_eq!(
            manager.iter().map(|(_, tab)| tab.id).collect::<Vec<_>>(),
            original
        );
        assert_eq!(manager.active_tab_id(), Some(original[0]));
        manager.switch_to(2);
        assert!(manager.move_dragged_tab_to(dragged, 1));
        assert_eq!(manager.active_tab_id(), Some(dragged));
        assert_eq!(
            manager.iter().map(|(_, tab)| tab.id).collect::<Vec<_>>(),
            vec![original[0], dragged, original[1]]
        );
        manager.switch_to(0);
        assert!(manager.toggle_pin_active());
        manager.switch_to(1);
        assert!(!manager.move_dragged_tab_to(dragged, 0));
        assert!(!manager.move_dragged_tab_to(usize::MAX, 2));
        manager.tab_mut_by_id(dragged).unwrap().closed = true;
        assert!(!manager.move_dragged_tab_to(dragged, 2));
        manager.tab_mut_by_id(dragged).unwrap().closed = false;
        manager.tab_mut_by_id(dragged).unwrap().start_closing();
        assert!(!manager.move_dragged_tab_to(dragged, 2));
    }

    #[test]
    fn focused_osc7_directory_drives_title_but_remote_paths_stay_display_only() {
        let temporary = tempfile::tempdir().unwrap();
        let local = temporary.path().join("local-cwd");
        std::fs::create_dir(&local).unwrap();
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut manager = TabManager::for_test(shell.clone());
        manager
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 1".into()],
                None,
                Some("seed"),
                None,
                HashMap::new(),
            )
            .unwrap();
        let panes = manager
            .active_tab_mut()
            .unwrap()
            .panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
        let remote = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        panes
            .get_pane_mut(PaneId(0))
            .unwrap()
            .terminal
            .process(format!("\x1b]7;file://{}\x07", local.display()).as_bytes());
        panes
            .get_pane_mut(remote)
            .unwrap()
            .terminal
            .process(b"\x1b]7;file://remote.example/srv/remote-project\x07");
        let tab = manager.active_tab_mut().unwrap();

        tab.panes.focus(PaneId(0));
        tab.update_title();
        assert_eq!(tab.title, "local-cwd");
        assert_eq!(tab.working_directory().as_deref(), local.to_str());

        tab.panes.focus(remote);
        tab.update_title();
        assert_eq!(tab.title, "remote-project");
        assert_ne!(
            tab.working_directory().as_deref(),
            Some("/srv/remote-project")
        );
    }

    #[test]
    fn launched_child_observer_follows_split_pane_transfer_collapse_and_window_transfer() {
        use termeric_core::pty::ChildExitStatus;
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "read value".into()],
            env: Default::default(),
        };
        let mut source = TabManager::new(80, 24, shell.clone(), 100, 1024 * 1024);
        source
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "read value; exit 19".into()],
                None,
                None,
                None,
                HashMap::new(),
            )
            .unwrap();
        let observer = source.active_tab().unwrap().child_observer().unwrap();
        let panes = source
            .active_tab_mut()
            .unwrap()
            .panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
        panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        panes.focus(PaneId(0));
        source
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "read value; exit 27".into()],
                None,
                None,
                None,
                HashMap::new(),
            )
            .unwrap();
        source.switch_to(0);
        let moved = source.move_focused_pane_to_next_tab().unwrap();
        // Drop the destination's unrelated original pane so the tracked owner
        // is left as the tab's only pane.
        source
            .active_tab_mut()
            .unwrap()
            .panes
            .get_pane_mut(PaneId(0))
            .unwrap()
            .closed = true;
        source.cleanup_closed(false, false);
        assert!(source.active_tab().unwrap().panes.pane_count() == 1);
        assert_eq!(source.active_tab().unwrap().panes.focused_id(), moved);
        let mut destination = source.empty_like();
        let tab = source.detach_active_tab_for_window_transfer().unwrap();
        destination.attach_window_transferred_tab(tab);
        drop(source);
        assert_eq!(observer.poll().unwrap().exit, None);
        assert!(!observer.poll().unwrap().terminal_closed);
        destination.active_tab_mut().unwrap().write(b"\n").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            destination.poll_events();
            destination.cleanup_closed(false, false);
            let result = observer.poll().unwrap();
            if result.terminal_closed && result.exit.is_some() {
                assert_eq!(
                    result.exit,
                    Some(ChildExitStatus {
                        exit_code: 19,
                        success: false
                    })
                );
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn moving_any_pane_between_tabs_preserves_child_and_restore_identity() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut manager = TabManager::for_test(shell.clone());
        for title in ["source", "destination"] {
            manager
                .new_tab_with_command(
                    "/bin/sh",
                    &["-c".into(), "sleep 1".into()],
                    None,
                    Some(title),
                    None,
                    HashMap::new(),
                )
                .unwrap();
        }
        manager.switch_to(0);
        let panes = manager
            .active_tab_mut()
            .unwrap()
            .panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
        let movable = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        panes.focus(PaneId(0));
        let child = panes.get_pane(PaneId(0)).unwrap().pty.child_id();

        let attached = manager.move_focused_pane_to_next_tab().unwrap();
        assert_eq!(manager.active_index(), 1);
        assert_eq!(manager.tabs[0].panes.pane_count(), 1);
        let source_snapshot = manager.tabs[0].capture_workspace_tab(0, false).unwrap();
        source_snapshot.validate().unwrap();
        assert_eq!(source_snapshot.primary_pane, movable.0);
        assert_eq!(source_snapshot.focused_pane, movable.0);
        let destination = &manager.active_tab().unwrap().panes;
        assert_eq!(destination.pane_count(), 2);
        assert_eq!(destination.focused_id(), attached);
        assert_eq!(
            destination.get_pane(attached).unwrap().pty.child_id(),
            child
        );

        let mut restored = manager.empty_like();
        restored
            .new_shell_tab_from_workspace(&source_snapshot, &HashMap::new())
            .unwrap();
        let restored_tab = restored.active_tab().unwrap();
        assert!(restored_tab.panes.pane_count() == 1);
        assert_eq!(restored_tab.panes.focused_id(), movable);
        let round_trip = restored_tab.capture_workspace_tab(0, false).unwrap();
        assert_eq!(round_trip.primary_pane, movable.0);
        assert_eq!(round_trip.focused_pane, movable.0);
    }

    #[test]
    fn exact_cross_tab_pane_transfer_uses_source_and_destination_ids() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 1".into()],
            env: Default::default(),
        };
        let mut manager = TabManager::for_test(shell.clone());
        for title in ["source", "destination"] {
            manager
                .new_tab_with_command(
                    "/bin/sh",
                    &["-c".into(), "sleep 1".into()],
                    None,
                    Some(title),
                    None,
                    HashMap::new(),
                )
                .unwrap();
        }
        manager.switch_to(0);
        let panes = manager.tabs[0].panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
        let remaining = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        panes.focus(remaining);
        let child = panes.get_pane(PaneId(0)).unwrap().pty.child_id();

        let same_tab_error = manager
            .transfer_pane_by_id(0, PaneId(0), 0, remaining, SplitDirection::Horizontal)
            .unwrap_err()
            .to_string();
        assert!(same_tab_error.contains("layout-tree"), "{same_tab_error}");

        manager.tabs[1].tab_type = TabType::Serial {
            name: "board".into(),
        };
        let serial_error = manager
            .transfer_pane_by_id(0, PaneId(0), 1, PaneId(0), SplitDirection::Horizontal)
            .unwrap_err()
            .to_string();
        assert!(serial_error.contains("serial terminal"), "{serial_error}");
        assert!(manager.tabs[0].panes.get_pane(PaneId(0)).is_some());
        assert!(manager.tabs[1].panes.pane_count() == 1);
        manager.tabs[1].tab_type = TabType::Shell;

        let attached = manager
            .transfer_pane_by_id(0, PaneId(0), 1, PaneId(0), SplitDirection::Horizontal)
            .unwrap();
        assert_eq!(manager.active_index(), 1);
        assert_eq!(manager.tabs[0].panes.pane_ids(), vec![remaining]);
        let destination = &manager.tabs[1].panes;
        assert_eq!(destination.focused_id(), attached);
        assert_eq!(
            destination.get_pane(attached).unwrap().pty.child_id(),
            child
        );
        let layout = destination.calculate_layout();
        assert!(layout[&attached].y > layout[&PaneId(0)].y);
    }

    #[test]
    fn moved_profile_pane_keeps_launch_identity_through_restore_and_collapse() {
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "printf 'SHELL_OWNER\\n'; sleep 2".into()],
            env: Default::default(),
        };
        let profile_shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "printf 'PROFILE_OWNER\\n'; sleep 2".into()],
            env: Default::default(),
        };
        let mut manager = TabManager::for_test(shell.clone());
        manager
            .new_tab_with_profile("dev", profile_shell.clone(), None, Some("profile"))
            .unwrap();
        let profile_panes = manager.active_tab_mut().unwrap().panes_for_test(
            profile_shell.clone(),
            800.0,
            480.0,
            10.0,
            20.0,
        );
        profile_panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        manager.new_tab_with_cwd(None, Some("shell")).unwrap();
        manager.switch_to(0);

        let attached = manager.move_focused_pane_to_next_tab().unwrap();
        let snapshot = manager
            .active_tab()
            .unwrap()
            .capture_workspace_tab(0, false)
            .unwrap();
        snapshot.validate().unwrap();
        assert_eq!(
            snapshot
                .panes
                .iter()
                .find(|pane| pane.id == attached.0)
                .map(|pane| &pane.launch),
            Some(&crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() })
        );

        let launches = HashMap::from([(
            attached.0,
            (
                crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() },
                profile_shell.clone(),
            ),
        )]);
        let mut restored = manager.empty_like();
        restored
            .new_shell_tab_from_workspace(&snapshot, &launches)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            restored.poll_events();
            let text = restored
                .active_tab()
                .unwrap()
                .panes
                .get_pane(attached)
                .unwrap()
                .terminal
                .grid()
                .plain_lines()
                .join("\n");
            if text.contains("PROFILE_OWNER") {
                break;
            }
            assert!(Instant::now() < deadline, "profile pane never relaunched");
            std::thread::sleep(Duration::from_millis(2));
        }

        let tab = manager.active_tab_mut().unwrap();
        tab.panes.focus(PaneId(0));
        assert!(tab.panes.close_focused());
        tab.reconcile_primary_pane_launch();
        assert_eq!(tab.panes.lone_pane().unwrap().id, attached);
        let collapsed = tab.capture_workspace_tab(1, false).unwrap();
        assert_eq!(
            collapsed.launch().unwrap(),
            &crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() }
        );
        assert!(matches!(tab.tab_type, TabType::Profile { ref name } if name == "dev"));

        let new_pane = tab
            .panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        let panes = &tab.panes;
        assert_eq!(
            panes.get_pane(new_pane).unwrap().launch,
            crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() }
        );
        assert_eq!(
            panes.get_pane(new_pane).unwrap().launch_shell.args,
            profile_shell.args
        );
    }

    #[test]
    fn plugin_primary_and_ordinary_sibling_can_move_without_relaunching() {
        let shell = crate::test_support::sleeping_shell();
        let mut manager = TabManager::for_test(shell.clone());
        manager
            .new_tab_with_command(
                "/bin/sh",
                &crate::test_support::sleeping_args(),
                None,
                Some("fake plugin"),
                Some(TabType::Plugin {
                    plugin_id: "fake".into(),
                }),
                HashMap::new(),
            )
            .unwrap();
        let panes = manager
            .active_tab_mut()
            .unwrap()
            .panes_for_test(shell, 800.0, 480.0, 10.0, 20.0);
        let ordinary = panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        panes.focus(PaneId(0));
        manager.new_tab().unwrap();
        manager.switch_to(0);

        let moved_plugin = manager.move_focused_pane_to_next_tab().unwrap();
        assert_eq!(manager.tabs[1].pane_plugin_id(moved_plugin), Some("fake"));
        manager.move_focused_pane_to_next_tab().unwrap();
        assert_eq!(manager.tabs[0].panes.pane_count(), 2);

        manager.tabs[0].panes.focus(ordinary);
        manager.move_focused_pane_to_next_tab().unwrap();
        let moved = manager.active_tab().unwrap().panes.focused_pane().unwrap();
        assert_eq!(moved.launch, crate::workspace::WorkspaceLaunch::Shell);
    }

    #[test]
    fn tab_spawn_and_pixel_only_resize_reach_kernel_winsize() {
        let initial = TerminalGeometry::from_cells(80, 24, 9.25, 18.5);
        let mut manager = TabManager::new_with_area(
            TerminalArea::new(initial, 9.25, 18.5),
            ShellConfig::default(),
            TerminalPolicy::with_scrollback(1_000, 8 * 1024 * 1024),
        );
        manager
            .new_tab_with_command(
                "/bin/sh",
                &["-c".into(), "sleep 1".into()],
                None,
                Some("winsize"),
                None,
                HashMap::new(),
            )
            .unwrap();
        assert_eq!(
            manager
                .active_tab()
                .unwrap()
                .panes
                .lone_pane()
                .and_then(|pane| pane.pty.size().ok()),
            Some(initial.pty_size())
        );

        let pixel_only = TerminalGeometry::from_cells(80, 24, 10.0, 20.0);
        manager.resize(TerminalArea::new(pixel_only, 10.0, 20.0));
        assert_eq!(
            manager
                .active_tab()
                .unwrap()
                .panes
                .lone_pane()
                .and_then(|pane| pane.pty.size().ok()),
            Some(pixel_only.pty_size())
        );
        assert_eq!(
            manager
                .active_tab()
                .unwrap()
                .focused_terminal()
                .unwrap()
                .grid()
                .cols(),
            80
        );
        manager.resize(TerminalArea::new(
            TerminalGeometry::from_cells(8, 5, 2.0, 3.0),
            2.0,
            3.0,
        ));
        crate::test_support::seed_pixel_geometry_retention_probe(
            manager
                .active_tab_mut()
                .unwrap()
                .focused_terminal_mut()
                .unwrap(),
        );
        let scaled = TerminalGeometry::from_cells(8, 2, 1.0, 1.0);
        manager.resize(TerminalArea::new(scaled, 1.0, 1.0));
        let tab = manager.active_tab().unwrap();
        assert_eq!(
            tab.panes.lone_pane().and_then(|pane| pane.pty.size().ok()),
            Some(scaled.pty_size())
        );
        crate::test_support::assert_pixel_geometry_retention_probe(tab.focused_terminal().unwrap());
    }

    #[test]
    fn inactive_tab_panes_poll_replies_and_tag_app_events_without_stealing_focus() {
        let mut manager = TabManager::for_test(ShellConfig::default());
        let dormant_tab_id = manager
            .new_tab_with_command(
                "/bin/sh",
                &crate::test_support::sleeping_args(),
                None,
                Some("pane-owner"),
                None,
                HashMap::new(),
            )
            .unwrap();
        let script = concat!(
            "stty raw -echo; ",
            "printf '\\007\\033]9;pane-event\\007\\033[6n\\033[c\\033[>c\\033_Ga=q,f=32,s=1,v=1,i=77;AQIDBA==\\033\\\\'; ",
            "dsr=$(dd bs=1 count=6 2>/dev/null); ",
            "da1=$(dd bs=1 count=7 2>/dev/null); ",
            "da2=$(dd bs=1 count=9 2>/dev/null); ",
            "kitty=$(dd bs=1 count=12 2>/dev/null); ",
            "printf '\\rdsr='; printf '%s' \"$dsr\" | od -An -tx1; ",
            "printf '\\r\\nda1='; printf '%s' \"$da1\" | od -An -tx1; ",
            "printf '\\r\\nda2='; printf '%s' \"$da2\" | od -An -tx1; ",
            "printf '\\r\\nkitty='; printf '%s' \"$kitty\" | od -An -tx1; ",
            "printf '\\r\\n'"
        );
        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), script.into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::for_test(
            80,
            24,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        )
        .unwrap();
        panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        let pane_ids: HashSet<PaneId> = panes.iter_with_layout().map(|(pane, _)| pane.id).collect();
        manager.tabs[0].panes = panes;

        manager
            .new_tab_with_command(
                "/bin/sh",
                &crate::test_support::sleeping_args(),
                None,
                Some("active"),
                None,
                HashMap::new(),
            )
            .unwrap();
        assert_eq!(manager.active_index(), 1);

        let mut bells = HashSet::new();
        let mut notifications = HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let (_, events) = manager.poll_events();
            for routed in events {
                let TerminalEventOrigin { tab_id, pane_id } = routed.origin;
                assert_eq!(tab_id, dormant_tab_id);
                match routed.event {
                    TerminalEvent::Bell => {
                        bells.insert(pane_id);
                    }
                    TerminalEvent::Notification(_) => {
                        notifications.insert(pane_id);
                    }
                    TerminalEvent::PtyWrite(_) => panic!("protocol reply escaped pane owner"),
                    _ => {}
                }
            }
            let all_replied = manager.tabs[0].panes.iter_with_layout().all(|(pane, _)| {
                let text = pane.terminal.grid().plain_lines().join("\n");
                text.contains("1b 5b 31 3b 31 52")
                    && text.contains("1b 5b 3f 31 3b 32 63")
                    && text.contains("1b 5b 3e 30 3b 31 3b 30 63")
                    && text.contains("1b 5f 47 69 3d 37 37 3b 4f 4b 1b 5c")
            });
            if all_replied && bells == pane_ids && notifications == pane_ids {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        assert_eq!(bells, pane_ids);
        assert_eq!(notifications, pane_ids);
        assert_eq!(manager.active_index(), 1);
        let active_text = manager.tabs[1]
            .focused_terminal()
            .unwrap()
            .grid()
            .plain_lines()
            .join("\n");
        assert!(!active_text.contains("reply="));
    }

    #[test]
    fn domain_attachment_reconstructs_nested_layout_and_stable_focus_mapping() {
        use crate::domain::model::{DomainSnapshot, LivePaneLayout, LiveTab};
        let layout = LivePaneLayout::split(
            70,
            SplitDirection::Vertical,
            400,
            LivePaneLayout::pane(5),
            LivePaneLayout::split(
                71,
                SplitDirection::Horizontal,
                600,
                LivePaneLayout::pane(9),
                LivePaneLayout::pane(12),
            ),
        );
        let snapshot =
            DomainSnapshot::fixture("domain-layout", 7, vec![LiveTab::fixture(7, layout)]);
        let mut manager = TabManager::new(80, 24, ShellConfig::default(), 1_000, 1024 * 1024);
        let kitty_files = TerminalPolicy {
            kitty_file_transmission: true,
            ..TerminalPolicy::with_scrollback(1_000, 1024 * 1024)
        };
        manager.set_terminal_policy(kitty_files, false);
        manager
            .attach_domain(
                &snapshot,
                &crate::domain::DomainRelayCommand::local("/bin/true"),
            )
            .unwrap();
        let local_tab_id = {
            let tab = manager.active_tab().unwrap();
            assert_eq!(tab.panes.pane_count(), 3);
            let domain_panes = &tab.panes;
            assert!(domain_panes.pane_ids().into_iter().all(|id| {
                domain_panes
                    .get_pane(id)
                    .is_some_and(|pane| !pane.terminal.kitty_file_transmission())
            }));
            assert_eq!(tab.live_domain_focus_route(), Some(("domain-layout", 12)));

            assert_eq!(tab.live_domain_tab_route(), Some(("domain-layout", 7)));
            assert!(matches!(
                tab.panes.workspace_layout(),
                crate::workspace::WorkspacePaneLayout::Split { ratio, .. }
                    if (ratio - 0.4).abs() < f32::EPSILON
            ));
            let mut split_ids: Vec<_> = tab
                .panes
                .get_dividers()
                .into_iter()
                .filter_map(|divider| match divider.target {
                    crate::panes::DividerTarget::Domain(id) => Some(id),
                    crate::panes::DividerTarget::Local(_) => None,
                })
                .collect();
            split_ids.sort_unstable();
            assert_eq!(split_ids, vec![70, 71]);
            tab.id
        };

        // The first relay and every later one must accept exactly one
        // daemon snapshot. An invalid private end
        // consumes that permission and clears a deliberately seeded image.
        for pane in manager.active_tab_mut().unwrap().panes.all_panes_mut() {
            pane.terminal
                .process(b"\x1b_Ga=T,f=32,s=1,v=1,i=9,c=1,r=1,C=1;AQIDBA==\x1b\\");
            assert!(pane.terminal.has_images());
            pane.terminal.process(b"\x1b_TG;end\x1b\\");
            assert!(!pane.terminal.has_images());
        }
        manager.set_terminal_policy(kitty_files, false);
        let domain_panes = &manager.active_tab().unwrap().panes;
        assert!(domain_panes.pane_ids().into_iter().all(|id| {
            domain_panes
                .get_pane(id)
                .is_some_and(|pane| !pane.terminal.kitty_file_transmission())
        }));
        assert!(manager.reconcile_closed_live_domain_pane(local_tab_id, 9));
        let tab = manager.active_tab().unwrap();
        assert_eq!(tab.panes.pane_count(), 2);
        assert_eq!(tab.live_domain_focus_route(), Some(("domain-layout", 12)));

        assert!(manager.reconcile_closed_live_domain_pane(local_tab_id, 12));
        let tab = manager.active_tab().unwrap();
        assert!(tab.panes.pane_count() == 1);
        assert_eq!(tab.live_domain_focus_route(), Some(("domain-layout", 5)));
        assert!(
            !manager.reconcile_closed_live_domain_pane(local_tab_id, 12),
            "duplicate completion must be idempotent"
        );
    }

    #[test]
    fn a_tabs_first_relay_starts_like_every_later_one() {
        use crate::domain::model::{DomainSnapshot, DomainTopology, LivePaneLayout, LiveTab};
        let relay = crate::domain::DomainRelayCommand::local("/bin/true");
        let snapshot = DomainSnapshot::fixture(
            "relays",
            7,
            vec![LiveTab::fixture(
                7,
                LivePaneLayout::split(
                    70,
                    SplitDirection::Vertical,
                    500,
                    LivePaneLayout::pane(5),
                    LivePaneLayout::pane(9),
                ),
            )],
        );
        let kitty_files = TerminalPolicy {
            kitty_file_transmission: true,
            ..TerminalPolicy::with_scrollback(1_000, 1024 * 1024)
        };
        let manager = || {
            let mut manager = TabManager::for_test(crate::test_support::sleeping_shell());
            manager.set_terminal_policy(kitty_files, false);
            manager
        };
        // Attached whole, and grown by a topology revision from one relay.
        let mut attached = manager();
        attached.attach_domain(&snapshot, &relay).unwrap();
        let mut revised = manager();
        revised
            .reconcile_live_domain(&DomainTopology::from(&snapshot), &relay)
            .unwrap();
        for manager in [&mut attached, &mut revised] {
            let tab = manager.active_tab_mut().unwrap();
            assert_eq!(tab.panes.pane_count(), 2);
            for pane in tab.panes.all_panes_mut() {
                let id = pane.id;
                assert!(pane.suppress_protocol_replies, "{id:?}");
                assert_eq!(
                    pane.shell_integration,
                    crate::terminal::shell_integration::ShellIntegrationSupport::Unavailable(
                        "Persistent-domain relays do not install a local shell hook".into()
                    ),
                    "{id:?}"
                );
                assert_eq!(
                    pane.last_command.as_deref(),
                    Some("termeric domain relay"),
                    "{id:?}"
                );
                assert_eq!(pane.launch, crate::workspace::WorkspaceLaunch::Shell);
                assert_eq!(pane.launch_shell, ShellConfig::default(), "{id:?}");
                assert!(!pane.terminal.kitty_file_transmission(), "{id:?}");
                // A relay takes the daemon's graphics snapshot, which an
                // invalid one consumes, clearing what it seeded.
                pane.terminal
                    .process(b"\x1b_Ga=T,f=32,s=1,v=1,i=9,c=1,r=1,C=1;AQIDBA==\x1b\\");
                assert!(pane.terminal.has_images(), "{id:?}");
                pane.terminal.process(b"\x1b_TG;end\x1b\\");
                assert!(!pane.terminal.has_images(), "{id:?}");
            }
        }
    }

    #[test]
    fn domain_topology_revisions_incrementally_add_split_collapse_and_remove_relays() {
        use crate::domain::model::{DomainSnapshot, LivePaneLayout, LiveTab};
        let mut snapshot = DomainSnapshot::fixture(
            "domain-revisions",
            7,
            vec![LiveTab::fixture(7, LivePaneLayout::pane(5))],
        );
        let mut manager = TabManager::new(80, 24, ShellConfig::default(), 1_000, 1024 * 1024);
        manager
            .attach_domain(
                &snapshot,
                &crate::domain::DomainRelayCommand::local("/bin/true"),
            )
            .unwrap();
        let stable_first_tab = manager.active_tab().unwrap().id;

        snapshot.tabs[0] = LiveTab::fixture(
            7,
            LivePaneLayout::split(
                70,
                SplitDirection::Vertical,
                350,
                LivePaneLayout::pane(5),
                LivePaneLayout::pane(9),
            ),
        );
        manager
            .reconcile_live_domain(
                &crate::domain::model::DomainTopology::from(&snapshot),
                &crate::domain::DomainRelayCommand::local("/bin/true"),
            )
            .unwrap();
        let first = manager
            .tabs
            .iter()
            .find(|tab| tab.id == stable_first_tab)
            .unwrap();
        assert_eq!(first.panes.pane_count(), 2);
        assert_eq!(
            first.live_domain_focus_route(),
            Some(("domain-revisions", 9))
        );

        snapshot
            .tabs
            .push(LiveTab::fixture(8, LivePaneLayout::pane(15)));
        snapshot.active_tab = 8;
        manager
            .reconcile_live_domain(
                &crate::domain::model::DomainTopology::from(&snapshot),
                &crate::domain::DomainRelayCommand::local("/bin/true"),
            )
            .unwrap();
        assert_eq!(manager.tabs.len(), 2);
        assert_eq!(
            manager.active_tab().unwrap().live_domain_tab_route(),
            Some(("domain-revisions", 8))
        );

        snapshot.tabs[0].focused_pane = 5;
        snapshot.tabs[0].layout = LivePaneLayout::Pane { pane: 5 };
        snapshot.tabs[0].panes.retain(|pane| pane.id == 5);
        manager
            .reconcile_live_domain(
                &crate::domain::model::DomainTopology::from(&snapshot),
                &crate::domain::DomainRelayCommand::local("/bin/true"),
            )
            .unwrap();
        let first = manager
            .tabs
            .iter()
            .find(|tab| tab.id == stable_first_tab)
            .unwrap();
        assert!(first.panes.pane_count() == 1);
        assert_eq!(
            first.live_domain_focus_route(),
            Some(("domain-revisions", 5))
        );

        snapshot.tabs.remove(0);
        manager
            .reconcile_live_domain(
                &crate::domain::model::DomainTopology::from(&snapshot),
                &crate::domain::DomainRelayCommand::local("/bin/true"),
            )
            .unwrap();
        assert!(
            manager
                .tabs
                .iter()
                .find(|tab| tab.id == stable_first_tab)
                .unwrap()
                .is_closing()
        );
    }

    /// One domain tab split into remote panes 4 and 8, focused on 8.
    fn split_domain_fixture(id: &str) -> crate::domain::model::DomainSnapshot {
        use crate::domain::model::{DomainSnapshot, LivePaneLayout, LiveTab};
        let layout = LivePaneLayout::split(
            30,
            SplitDirection::Vertical,
            500,
            LivePaneLayout::pane(4),
            LivePaneLayout::pane(8),
        );
        DomainSnapshot::fixture(id, 3, vec![LiveTab::fixture(3, layout)])
    }

    #[test]
    fn relay_driven_full_tab_exit_removes_the_tab_instead_of_leaving_an_empty_owner() {
        let snapshot = split_domain_fixture("domain-exit");
        let session_root = tempfile::tempdir().unwrap();
        crate::session::set_test_sessions_base_dir(Some(session_root.path().to_path_buf()));
        let mut manager = TabManager::new(80, 24, ShellConfig::default(), 1_000, 1024 * 1024);
        manager
            .attach_domain(
                &snapshot,
                &crate::domain::DomainRelayCommand::local("/bin/echo"),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !manager.is_empty() && std::time::Instant::now() < deadline {
            manager.poll_events();
            manager.cleanup_closed(true, true);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            manager.is_empty(),
            "a tab whose relays all exited must not retain an ownerless shell"
        );
        assert!(manager.drain_persistence_notices().is_empty());
        assert_eq!(
            std::fs::read_dir(session_root.path())
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            0,
            "relay output must never become a closed-tab snapshot"
        );
        crate::session::set_test_sessions_base_dir(None);
    }

    #[test]
    fn relay_driven_split_dissolution_remaps_focus_to_the_surviving_remote_pane() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let relay = temporary.path().join("fake-relay");
        std::fs::write(
            &relay,
            "#!/bin/sh\nif [ \"$4\" = \"8\" ]; then exit 0; fi\nsleep 5\n",
        )
        .unwrap();
        std::fs::set_permissions(&relay, std::fs::Permissions::from_mode(0o700)).unwrap();
        let snapshot = split_domain_fixture("domain-partial-exit");
        let mut manager = TabManager::new(80, 24, ShellConfig::default(), 1_000, 1024 * 1024);
        manager
            .attach_domain(
                &snapshot,
                &crate::domain::DomainRelayCommand::local(relay.to_string_lossy()),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while manager
            .active_tab()
            .is_some_and(|tab| tab.panes.pane_count() > 1)
            && std::time::Instant::now() < deadline
        {
            manager.poll_events();
            manager.cleanup_closed(false, false);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let tab = manager.active_tab().unwrap();
        assert!(tab.panes.pane_count() == 1);
        assert_eq!(
            tab.live_domain_focus_route(),
            Some(("domain-partial-exit", 4))
        );
    }
}
