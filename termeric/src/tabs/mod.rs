//! Tab management for multiple terminal instances
//!
//! Provides support for tabbed terminal sessions within a single window.

mod plugin_panes;
mod tab;
mod title;

#[cfg(test)]
pub(crate) use tab::SessionCapture;
pub(crate) use tab::{LiveDomainTab, TabAgentView, TabTerminalOwner};
pub use tab::{Tab, TabType};
pub(crate) use title::{
    get_foreground_process, get_process_cwd, presentation_working_directory,
    terminal_working_directory,
};

use crate::config::ShellConfig;
use crate::panes::PaneId;
use crate::session::Session;
use anyhow::Context;
use std::collections::HashMap;
use std::io;
use termeric_core::{Terminal, TerminalEvent, term::XtermKeyboardType};

use crate::terminal::events::MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL;
use crate::terminal::geometry::TerminalGeometry;

/// Stable origin for an app-facing terminal event. Tab IDs are monotonic and
/// pane IDs are stable for the lifetime of a pane, so replies never depend on
/// whichever terminal happens to be focused when App handles the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalEventOrigin {
    Tab { tab_id: usize },
    Pane { tab_id: usize, pane_id: PaneId },
}

/// Long-lived terminal identity for UI work that can outlast a split/collapse.
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
    /// Cell and physical-pixel dimensions for new and single-terminal tabs.
    geometry: TerminalGeometry,
    /// Shell configuration for new tabs
    shell: ShellConfig,
    /// Scrollback limit for new tabs
    scrollback_lines: usize,
    /// Scrollback byte budget for new tabs and panes
    scrollback_bytes: usize,
    /// Host policy for DECRQCRA replies in existing and future terminals.
    screen_checksum_reporting: bool,
    /// Host policy for Kitty persistent-file reads in existing and future terminals.
    kitty_file_transmission: bool,
    /// Configured xterm keyboard profile for existing and future terminals.
    xterm_keyboard_type: XtermKeyboardType,
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
        Self::new_with_geometry(
            TerminalGeometry::from_cells(cols, rows, 1.0, 1.0),
            shell,
            scrollback_lines,
            scrollback_bytes,
        )
    }

    pub(crate) fn new_with_geometry(
        geometry: TerminalGeometry,
        shell: ShellConfig,
        scrollback_lines: usize,
        scrollback_bytes: usize,
    ) -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            geometry,
            shell,
            scrollback_lines,
            scrollback_bytes,
            screen_checksum_reporting: false,
            kitty_file_transmission: false,
            xterm_keyboard_type: XtermKeyboardType::Normal,
            persistence_notices: Vec::new(),
            event_overflow_reported: false,
            reduced_motion: false,
        }
    }

    pub(crate) fn empty_like(&self) -> Self {
        let mut manager = Self::new_with_geometry(
            self.geometry,
            self.shell.clone(),
            self.scrollback_lines,
            self.scrollback_bytes,
        );
        manager.screen_checksum_reporting = self.screen_checksum_reporting;
        manager.kitty_file_transmission = self.kitty_file_transmission;
        manager.xterm_keyboard_type = self.xterm_keyboard_type;
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

        let mut tab = if working_dir.is_none() && title.is_none() {
            Tab::new_with_geometry(
                id,
                self.geometry,
                &self.shell,
                self.scrollback_lines,
                self.scrollback_bytes,
            )?
        } else {
            Tab::new_with_cwd_geometry(
                id,
                self.geometry,
                &self.shell,
                self.scrollback_lines,
                self.scrollback_bytes,
                working_dir,
                title,
            )?
        };
        tab.set_screen_checksum_reporting(self.screen_checksum_reporting);
        tab.set_kitty_file_transmission(self.kitty_file_transmission);
        tab.set_xterm_keyboard_type(self.xterm_keyboard_type, true);
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
        let mut tab = Tab::new_with_cwd_geometry(
            id,
            self.geometry,
            &shell,
            self.scrollback_lines,
            self.scrollback_bytes,
            working_dir,
            title,
        )?;
        tab.tab_type = TabType::Profile {
            name: name.to_string(),
        };
        tab.set_terminal_profile_launch(name, shell);
        tab.set_screen_checksum_reporting(self.screen_checksum_reporting);
        tab.set_kitty_file_transmission(self.kitty_file_transmission);
        tab.set_xterm_keyboard_type(self.xterm_keyboard_type, true);
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
        let mut tab = Tab::new_with_command_geometry(
            id,
            self.geometry,
            &program,
            &args,
            None,
            Some(title),
            self.scrollback_lines,
            self.scrollback_bytes,
            Some(TabType::Serial {
                name: name.to_string(),
            }),
            HashMap::new(),
        )?;
        tab.set_serial_profile_launch(name, title.to_string());
        tab.set_screen_checksum_reporting(self.screen_checksum_reporting);
        tab.set_kitty_file_transmission(self.kitty_file_transmission);
        tab.set_xterm_keyboard_type(self.xterm_keyboard_type, true);
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
        self.new_tab_with_command_environment_policy(
            program,
            args,
            working_dir,
            title,
            tab_type,
            env,
            &termeric_core::pty::EnvironmentPolicy::default(),
        )
    }

    pub(crate) fn new_tab_with_command_environment_policy(
        &mut self,
        program: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        tab_type: Option<TabType>,
        env: HashMap<String, String>,
        policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> anyhow::Result<usize> {
        self.new_tab_with_command_placement(
            program,
            args,
            working_dir,
            title,
            tab_type,
            env,
            policy,
            false,
        )
    }

    /// Background insertion never transiently changes the active owner. An
    /// empty manager necessarily selects its first tab in either mode.
    pub(crate) fn new_tab_with_command_placement(
        &mut self,
        program: &str,
        args: &[String],
        working_dir: Option<&str>,
        title: Option<&str>,
        tab_type: Option<TabType>,
        env: HashMap<String, String>,
        policy: &termeric_core::pty::EnvironmentPolicy,
        background: bool,
    ) -> anyhow::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;

        let mut tab = Tab::new_with_command_environment_policy(
            id,
            self.geometry,
            program,
            args,
            working_dir,
            title,
            self.scrollback_lines,
            self.scrollback_bytes,
            tab_type,
            env,
            policy,
        )?;
        tab.set_split_shell_config(self.shell.clone());
        tab.set_screen_checksum_reporting(self.screen_checksum_reporting);
        tab.set_kitty_file_transmission(self.kitty_file_transmission);
        tab.set_xterm_keyboard_type(self.xterm_keyboard_type, true);
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
        if snapshot.workspace.tabs.is_empty() {
            anyhow::bail!("live domain has no tabs");
        }
        for remote_tab in &snapshot.workspace.tabs {
            self.attach_domain_tab(&snapshot.id, remote_tab, relay_command)?;
        }
        self.tabs.sort_by_key(|tab| !tab.pinned);
        self.active = self
            .tabs
            .iter()
            .position(|tab| {
                tab.live_domain.as_ref().is_some_and(|live| {
                    live.domain_id == snapshot.id && live.tab_id == snapshot.workspace.active_tab
                })
            })
            .context("live domain active tab is absent from its snapshot")?;
        Ok(())
    }

    fn attach_domain_tab(
        &mut self,
        domain_id: &str,
        remote_tab: &crate::domain::model::LiveTab,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<usize> {
        let mut remote_panes = remote_tab.panes.clone();
        remote_panes.sort_by_key(|pane| pane.id);
        let primary = remote_panes
            .first()
            .ok_or_else(|| anyhow::anyhow!("live domain tab {} has no panes", remote_tab.id))?;
        self.new_tab_with_command(
            relay_command.program(),
            &relay_command.pane_arguments(domain_id, primary.id),
            None,
            Some(&remote_tab.title),
            None,
            HashMap::new(),
        )?;
        let index = self.tabs.len() - 1;
        let tab = &mut self.tabs[index];
        tab.set_graphics_relay_snapshot_allowed(true);
        tab.live_domain = Some(LiveDomainTab {
            domain_id: domain_id.to_string(),
            tab_id: remote_tab.id,
            pane_ids: remote_panes
                .iter()
                .enumerate()
                .map(|(index, pane)| (PaneId(index as u32), pane.id))
                .collect(),
        });
        tab.set_kitty_file_transmission(false);
        tab.set_live_domain_metadata(
            &remote_tab.title,
            remote_tab.user_title.as_deref(),
            remote_tab.pinned,
        );
        if remote_panes.len() > 1 {
            let owner = tab
                .take_terminal_owner()
                .ok_or_else(|| anyhow::anyhow!("domain relay tab could not enter pane mode"))?;
            let cell_width = f32::from(self.geometry.pixel_width) / self.geometry.cols as f32;
            let cell_height = f32::from(self.geometry.pixel_height) / self.geometry.rows as f32;
            tab.pane_manager = Some(crate::panes::PaneManager::from_domain(
                owner,
                self.shell.clone(),
                self.scrollback_lines,
                self.scrollback_bytes,
                f32::from(self.geometry.pixel_width),
                f32::from(self.geometry.pixel_height),
                cell_width,
                cell_height,
                domain_id,
                remote_tab,
                relay_command,
            )?);
        }
        Ok(index)
    }

    fn attach_topology_tab(
        &mut self,
        domain_id: &str,
        remote_tab: &crate::domain::model::LiveTopologyTab,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<usize> {
        let primary = remote_tab
            .panes
            .iter()
            .min_by_key(|pane| pane.id)
            .ok_or_else(|| anyhow::anyhow!("live domain tab {} has no panes", remote_tab.id))?;
        self.new_tab_with_command(
            relay_command.program(),
            &relay_command.pane_arguments(domain_id, primary.id),
            None,
            Some(&remote_tab.title),
            None,
            HashMap::new(),
        )?;
        let index = self.tabs.len() - 1;
        self.tabs[index].set_graphics_relay_snapshot_allowed(true);
        self.tabs[index].live_domain = Some(LiveDomainTab {
            domain_id: domain_id.to_string(),
            tab_id: remote_tab.id,
            pane_ids: HashMap::from([(PaneId(0), primary.id)]),
        });
        self.tabs[index].set_kitty_file_transmission(false);
        self.tabs[index].set_live_domain_metadata(
            &remote_tab.title,
            remote_tab.user_title.as_deref(),
            remote_tab.pinned,
        );
        Ok(index)
    }

    /// Apply one whole-domain daemon revision while preserving every relay PTY
    /// whose stable remote identity still exists.
    pub(crate) fn reconcile_live_domain(
        &mut self,
        snapshot: &crate::domain::model::DomainTopology,
        relay_command: &crate::domain::DomainRelayCommand,
    ) -> anyhow::Result<bool> {
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
        let mut changed = false;
        for index in stale.into_iter().rev() {
            changed |= self.close_tab(index);
        }

        changed |= self.relocate_live_domain_relays(snapshot)?;

        for remote_tab in &snapshot.tabs {
            let index = self.tabs.iter().position(|tab| {
                tab.live_domain.as_ref().is_some_and(|live| {
                    live.domain_id == snapshot.domain_id && live.tab_id == remote_tab.id
                })
            });
            let index = match index {
                Some(index) => index,
                None => {
                    changed = true;
                    self.attach_topology_tab(&snapshot.domain_id, remote_tab, relay_command)?
                }
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
        Ok(changed || !snapshot.tabs.is_empty())
    }

    /// Move an existing relay ownership bundle between local tabs before the
    /// per-tab reconciliation pass. The daemon pane identity is global to the
    /// domain, so a cross-tab topology revision must not respawn a relay merely
    /// because its tab-local mapping changed.
    fn relocate_live_domain_relays(
        &mut self,
        snapshot: &crate::domain::model::DomainTopology,
    ) -> anyhow::Result<bool> {
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

        let shell = self.shell.clone();
        let scrollback_lines = self.scrollback_lines;
        let scrollback_bytes = self.scrollback_bytes;
        let total_width = f32::from(self.geometry.pixel_width);
        let total_height = f32::from(self.geometry.pixel_height);
        let cell_width = total_width / self.geometry.cols as f32;
        let cell_height = total_height / self.geometry.rows as f32;
        let mut changed = false;
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
            if self.tabs[source_index]
                .pane_manager
                .as_ref()
                .is_none_or(|panes| panes.pane_count() <= 1)
            {
                // A skipped intermediate revision can make a direct transfer
                // impossible. The ordinary reconciliation path will replace
                // only that relay and still converge to daemon state.
                continue;
            }

            let (source, destination) = if source_index < destination_index {
                let (before, after) = self.tabs.split_at_mut(destination_index);
                (&mut before[source_index], &mut after[0])
            } else {
                let (before, after) = self.tabs.split_at_mut(source_index);
                (&mut after[0], &mut before[destination_index])
            };
            if destination.pane_manager.is_none() {
                let owner = destination
                    .take_terminal_owner()
                    .context("live-domain destination tab lost its terminal owner")?;
                destination.pane_manager = Some(crate::panes::PaneManager::from_existing(
                    owner,
                    shell.clone(),
                    scrollback_lines,
                    scrollback_bytes,
                    total_width,
                    total_height,
                    cell_width,
                    cell_height,
                ));
            }
            let pane = source
                .pane_manager
                .as_mut()
                .and_then(|panes| panes.detach(source_local))
                .context("live-domain source relay could not be detached")?;
            source
                .live_domain
                .as_mut()
                .context("live-domain source tab lost its stable route")?
                .pane_ids
                .remove(&source_local);
            let destination_local = destination
                .pane_manager
                .as_mut()
                .expect("destination pane manager was installed")
                .attach(pane, crate::panes::SplitDirection::Vertical);
            destination
                .live_domain
                .as_mut()
                .context("live-domain destination tab lost its stable route")?
                .pane_ids
                .insert(destination_local, remote_pane);
            changed = true;
        }
        Ok(changed)
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
        let cell_width = f32::from(self.geometry.pixel_width) / self.geometry.cols as f32;
        let cell_height = f32::from(self.geometry.pixel_height) / self.geometry.rows as f32;
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

        let result = (|| {
            if remote_tab.panes.len() > 1 && tab.pane_manager.is_none() {
                let owner = tab
                    .take_terminal_owner()
                    .context("live-domain tab lost its terminal owner")?;
                tab.pane_manager = Some(crate::panes::PaneManager::from_existing(
                    owner,
                    self.shell.clone(),
                    self.scrollback_lines,
                    self.scrollback_bytes,
                    f32::from(self.geometry.pixel_width),
                    f32::from(self.geometry.pixel_height),
                    cell_width,
                    cell_height,
                ));
            }

            if let Some(panes) = &mut tab.pane_manager {
                panes.reconcile_domain(&mut pane_ids, domain_id, remote_tab, relay_command)?;
                if remote_tab.panes.len() == 1 {
                    let remaining_local = panes.focused_id();
                    let remaining_remote = pane_ids.get(&remaining_local).copied();
                    let mut panes = tab.pane_manager.take().unwrap();
                    let owner = panes
                        .take_remaining_pane()
                        .context("live-domain pane reconciliation did not leave one owner")?;
                    tab.restore_terminal_owner(owner);
                    pane_ids.clear();
                    if let Some(remote) = remaining_remote {
                        pane_ids.insert(remaining_local, remote);
                    }
                }
            } else {
                let remote = remote_tab
                    .panes
                    .first()
                    .context("live domain tab has no panes")?;
                pane_ids.clear();
                pane_ids.insert(tab.single_pane_id(), remote.id);
            }
            Ok(())
        })();
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
            anyhow::bail!("GUI tab {tab_id} is closing");
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
            anyhow::bail!("GUI tab {tab_id} is closing");
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

    /// Adopt a complete tab under a fresh destination-local ID. Pinned tabs
    /// remain in the leading pinned prefix and become active after transfer.
    pub(crate) fn attach_window_transferred_tab(&mut self, mut tab: Tab) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        tab.id = id;
        tab.has_activity = false;
        if let Some(panes) = &mut tab.pane_manager {
            let cell_width = f32::from(self.geometry.pixel_width) / self.geometry.cols as f32;
            let cell_height = f32::from(self.geometry.pixel_height) / self.geometry.rows as f32;
            panes.resize(
                f32::from(self.geometry.pixel_width),
                f32::from(self.geometry.pixel_height),
                cell_width,
                cell_height,
            );
        } else if let Err(error) = tab.resize_geometry(self.geometry) {
            log::warn!("Transferred tab {id} could not be resized immediately: {error:#}");
        }
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
    /// receives a fresh destination-local stable ID and becomes active.
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
        if let Some(panes) = &mut tab.pane_manager {
            let cell_width = f32::from(self.geometry.pixel_width) / self.geometry.cols as f32;
            let cell_height = f32::from(self.geometry.pixel_height) / self.geometry.rows as f32;
            panes.resize(
                f32::from(self.geometry.pixel_width),
                f32::from(self.geometry.pixel_height),
                cell_width,
                cell_height,
            );
        } else if let Err(error) = tab.resize_geometry(self.geometry) {
            log::warn!("Transferred tab {id} could not be resized immediately: {error:#}");
        }
        self.tabs.insert(target, tab);
        self.active = target;
        Ok(id)
    }

    /// Move the active pane through the same prevalidated path as exact-ID moves.
    pub(crate) fn move_focused_pane_to_next_tab(
        &mut self,
        cwd_mode: crate::config::PaneCwdMode,
    ) -> anyhow::Result<PaneId> {
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
            cwd_mode,
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
            anyhow::bail!("GUI tab {tab_id} is closing");
        }
        if tab.live_domain.is_some() {
            anyhow::bail!(
                "Live-domain panes are daemon-owned and require an acknowledged topology change"
            );
        }
        let manager = tab
            .pane_manager
            .as_ref()
            .context("Split the source tab before moving a pane")?;
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
            anyhow::bail!("GUI tab {tab_id} is closing");
        }
        if tab.live_domain.is_some() {
            anyhow::bail!("A local pane cannot be attached to a live-domain tab");
        }
        if !tab.allows_pane_splits() {
            anyhow::bail!("A pane cannot be attached to a serial terminal tab");
        }
        if let Some(manager) = &tab.pane_manager {
            manager.validate_attach_target(target)
        } else if target != tab.single_pane_id() {
            anyhow::bail!(
                "GUI pane {} does not exist in unsplit tab {tab_id}; its sole pane ID is {}",
                target.0,
                tab.single_pane_id().0
            )
        } else {
            tab.validate_single_pane_attachment()
        }
    }

    pub(crate) fn prepare_pane_transfer_destination_by_id(
        &mut self,
        tab_id: usize,
        target: PaneId,
        cwd_mode: crate::config::PaneCwdMode,
    ) -> anyhow::Result<()> {
        self.validate_pane_transfer_destination_by_id(tab_id, target)?;
        let destination = self
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .expect("validated destination tab remains present");
        if self.tabs[destination].pane_manager.is_none() {
            let cell_width = f32::from(self.geometry.pixel_width) / self.geometry.cols as f32;
            let cell_height = f32::from(self.geometry.pixel_height) / self.geometry.rows as f32;
            let split_shell = self.tabs[destination].split_shell_config();
            let owner = self.tabs[destination]
                .take_terminal_owner()
                .context("Destination tab does not own a terminal")?;
            let mut manager = crate::panes::PaneManager::from_existing(
                owner,
                split_shell,
                self.scrollback_lines,
                self.scrollback_bytes,
                f32::from(self.geometry.pixel_width),
                f32::from(self.geometry.pixel_height),
                cell_width,
                cell_height,
            );
            manager.set_cwd_mode(cwd_mode);
            self.tabs[destination].pane_manager = Some(manager);
        }
        Ok(())
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
            .pane_manager
            .as_mut()
            .and_then(|manager| manager.detach(pane_id))
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
            .pane_manager
            .as_mut()
            .context("Destination tab was not prepared for pane attachment")?
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
        cwd_mode: crate::config::PaneCwdMode,
    ) -> anyhow::Result<PaneId> {
        if source_tab_id == destination_tab_id {
            anyhow::bail!("Moving a pane within the same tab requires a layout-tree rearrangement");
        }
        self.validate_pane_transfer_source_by_id(source_tab_id, pane_id)?;
        self.validate_pane_transfer_destination_by_id(destination_tab_id, target)?;
        self.prepare_pane_transfer_destination_by_id(destination_tab_id, target, cwd_mode)?;
        let pane = self
            .detach_pane_for_transfer_by_id(source_tab_id, pane_id)
            .expect("prevalidated source pane remains detachable");
        self.attach_transferred_pane_by_id(destination_tab_id, target, pane, direction)
            .context("prevalidated destination pane remains attachable")
    }

    /// Normalize only after validating the same limits used by exact-ID moves.
    pub(crate) fn prepare_window_pane_transfer_destination(
        &mut self,
        cwd_mode: crate::config::PaneCwdMode,
    ) -> anyhow::Result<()> {
        let target = self
            .active_tab()
            .context("No destination tab is active")?
            .focused_terminal_identity();
        self.prepare_pane_transfer_destination_by_id(target.tab_id, target.pane_id, cwd_mode)
    }

    /// Detach the validated focused pane while retaining its PTY, reader,
    /// terminal state, Harvest state, and process identity.
    pub(crate) fn detach_focused_pane_for_window_transfer(
        &mut self,
    ) -> anyhow::Result<crate::panes::Pane> {
        self.validate_window_pane_transfer_source()?;
        let pane = self
            .active_tab_mut()
            .and_then(|tab| tab.pane_manager.as_mut())
            .and_then(crate::panes::PaneManager::detach_focused)
            .context("The focused pane could not be detached")?;
        self.active_tab_mut()
            .expect("validated source tab remains active")
            .reconcile_primary_pane_launch();
        Ok(pane)
    }

    /// Attach a detached owner to the already prepared active destination.
    pub(crate) fn attach_window_transferred_pane(&mut self, pane: crate::panes::Pane) -> PaneId {
        let manager = self.tabs[self.active]
            .pane_manager
            .as_mut()
            .expect("window transfer destination must be prepared");
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

    pub(crate) fn new_shell_tab_from_workspace(
        &mut self,
        snapshot: &crate::workspace::WorkspaceTabSnapshot,
        pane_launches: &HashMap<
            u32,
            (
                crate::workspace::WorkspaceLaunch,
                crate::config::ShellConfig,
            ),
        >,
    ) -> anyhow::Result<(usize, Vec<String>)> {
        let primary = snapshot
            .panes
            .iter()
            .find(|pane| pane.id == snapshot.primary_pane)
            .ok_or_else(|| anyhow::anyhow!("workspace tab {} has no primary pane", snapshot.id))?;
        let mut warnings = Vec::new();
        let cwd = usable_workspace_cwd(
            primary.working_directory.as_deref(),
            snapshot.id,
            0,
            &mut warnings,
        );
        let id = self.next_id;
        self.next_id += 1;
        let tab = Tab::new_with_cwd_geometry(
            id,
            self.geometry,
            &self.shell,
            self.scrollback_lines,
            self.scrollback_bytes,
            cwd.as_deref(),
            Some(&snapshot.title),
        )?;
        let mut tab = tab;
        tab.set_screen_checksum_reporting(self.screen_checksum_reporting);
        tab.set_kitty_file_transmission(self.kitty_file_transmission);
        tab.set_xterm_keyboard_type(self.xterm_keyboard_type, true);
        tab.user_title.clone_from(&snapshot.user_title);
        tab.pinned = snapshot.pinned;
        if let Some(title) = &tab.user_title {
            tab.title.clone_from(title);
        }
        self.tabs.push(tab);
        let index = self.tabs.len() - 1;
        self.active = index;
        warnings.extend(self.apply_workspace_layout(index, snapshot, pane_launches)?);
        Ok((index, warnings))
    }

    pub(crate) fn new_profile_tab_from_workspace(
        &mut self,
        snapshot: &crate::workspace::WorkspaceTabSnapshot,
        name: &str,
        shell: ShellConfig,
        pane_launches: &HashMap<
            u32,
            (
                crate::workspace::WorkspaceLaunch,
                crate::config::ShellConfig,
            ),
        >,
    ) -> anyhow::Result<(usize, Vec<String>)> {
        let primary = snapshot
            .panes
            .iter()
            .find(|pane| pane.id == snapshot.primary_pane)
            .ok_or_else(|| anyhow::anyhow!("workspace tab {} has no primary pane", snapshot.id))?;
        let mut warnings = Vec::new();
        let cwd = usable_workspace_cwd(
            primary.working_directory.as_deref(),
            snapshot.id,
            0,
            &mut warnings,
        );
        self.new_tab_with_profile(name, shell, cwd.as_deref(), Some(&snapshot.title))?;
        let index = self.tabs.len() - 1;
        warnings.extend(self.apply_workspace_layout(index, snapshot, pane_launches)?);
        Ok((index, warnings))
    }

    pub(crate) fn apply_workspace_layout(
        &mut self,
        index: usize,
        snapshot: &crate::workspace::WorkspaceTabSnapshot,
        pane_launches: &HashMap<
            u32,
            (
                crate::workspace::WorkspaceLaunch,
                crate::config::ShellConfig,
            ),
        >,
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
        let primary = snapshot
            .panes
            .iter()
            .find(|pane| pane.id == snapshot.primary_pane)
            .ok_or_else(|| anyhow::anyhow!("workspace tab {} has no primary pane", snapshot.id))?;
        tab.last_command.clone_from(&primary.last_command);

        if snapshot.panes.len() == 1 {
            tab.set_single_pane_id(PaneId(snapshot.primary_pane));
            if let Some(terminal) = tab.focused_terminal_mut() {
                replay_workspace_scrollback(terminal, &primary.scrollback_lines);
            }
            if let crate::workspace::WorkspaceLaunch::Plugin { plugin_id, profile } = pane_launches
                .get(&snapshot.primary_pane)
                .map_or(&snapshot.launch, |(launch, _)| launch)
            {
                tab.set_plugin_profile_reference(
                    PaneId(snapshot.primary_pane),
                    plugin_id,
                    profile.clone(),
                );
            }
            return Ok(Vec::new());
        }

        let split_shell = tab.split_shell_config();
        let owner = tab
            .take_terminal_owner()
            .ok_or_else(|| anyhow::anyhow!("workspace tab owner could not enter pane mode"))?;
        let cell_width = f32::from(self.geometry.pixel_width) / self.geometry.cols as f32;
        let cell_height = f32::from(self.geometry.pixel_height) / self.geometry.rows as f32;
        let (panes, warnings) = crate::panes::PaneManager::from_workspace(
            owner,
            split_shell,
            self.scrollback_lines,
            self.scrollback_bytes,
            f32::from(self.geometry.pixel_width),
            f32::from(self.geometry.pixel_height),
            cell_width,
            cell_height,
            snapshot,
            pane_launches,
        )?;
        tab.pane_manager = Some(panes);
        if let crate::workspace::WorkspaceLaunch::Plugin { plugin_id, profile } = pane_launches
            .get(&snapshot.primary_pane)
            .map_or(&snapshot.launch, |(launch, _)| launch)
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
        let Some(panes) = &mut tab.pane_manager else {
            return false;
        };
        if !panes.close(local_id) {
            return false;
        }
        if let Some(live) = &mut tab.live_domain {
            live.pane_ids.remove(&local_id);
        }

        if panes.pane_count() == 1 {
            let remaining_local = panes.focused_id();
            let remaining_remote = tab
                .live_domain
                .as_ref()
                .and_then(|live| live.pane_ids.get(&remaining_local).copied());
            let mut panes = tab.pane_manager.take().unwrap();
            if let Some(remaining) = panes.take_remaining_pane() {
                tab.restore_terminal_owner(remaining);
            }
            if let Some(live) = &mut tab.live_domain {
                live.pane_ids.clear();
                if let Some(remote) = remaining_remote {
                    live.pane_ids.insert(remaining_local, remote);
                }
            }
            if let Err(error) = tab.resize_geometry(self.geometry) {
                log::error!(
                    "Failed to resize live-domain tab {} after pane close: {error}",
                    tab.id
                );
            }
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
            if let Some(panes) = &mut tab.pane_manager {
                let mut saw_output = false;
                let (pane_changed, pane_events) = panes.poll_events(|_, data| {
                    saw_output |= !data.is_empty();
                });
                if saw_output {
                    tab.update_title();
                }
                if pane_changed {
                    if i != self.active {
                        tab.has_activity = true;
                    }
                    changed = true;
                }
                for pane_event in pane_events {
                    if all_events.len() < MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL {
                        all_events.push(RoutedTerminalEvent {
                            origin: TerminalEventOrigin::Pane {
                                tab_id,
                                pane_id: pane_event.pane_id,
                            },
                            event: pane_event.event,
                        });
                    } else {
                        dropped_events += 1;
                    }
                }
            } else {
                let (tab_changed, events) = tab.poll_events();
                if tab_changed && i != self.active {
                    tab.has_activity = true;
                }
                changed |= tab_changed;
                for event in events {
                    if all_events.len() < MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL {
                        all_events.push(RoutedTerminalEvent {
                            origin: TerminalEventOrigin::Tab { tab_id },
                            event,
                        });
                    } else {
                        dropped_events += 1;
                    }
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

    /// Resize all tabs
    ///
    /// Skips tabs with active pane managers — those terminals are at pane
    /// dimensions and are managed by PaneManager::resize() instead.
    /// Whether the active tab already has `geometry`, so fitting it would
    /// change nothing.
    pub(crate) fn active_has_geometry(
        &self,
        geometry: TerminalGeometry,
        content_size: (f32, f32),
        cell_size: (f32, f32),
    ) -> bool {
        self.geometry == geometry
            && self
                .tabs
                .get(self.active)
                .is_none_or(|tab| match &tab.pane_manager {
                    Some(panes) => panes.is_laid_out_for(
                        content_size.0,
                        content_size.1,
                        cell_size.0,
                        cell_size.1,
                    ),
                    None => tab.has_geometry(geometry),
                })
    }

    /// Fits only the active tab to `geometry`, which new tabs then take too.
    /// Other tabs keep their size until they are shown, so a change that
    /// differs from tab to tab, such as the agent view's dock, never
    /// reflows or signals programs in tabs nobody is looking at.
    pub(crate) fn resize_active_geometry(
        &mut self,
        geometry: TerminalGeometry,
        content_size: (f32, f32),
        cell_size: (f32, f32),
    ) {
        self.geometry = geometry;
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.pane_manager {
            Some(panes) => panes.resize(content_size.0, content_size.1, cell_size.0, cell_size.1),
            None => {
                if let Err(error) = tab.resize_geometry(geometry) {
                    log::error!("Failed to resize tab {}: {error}", tab.id);
                }
            }
        }
    }

    pub(crate) fn resize_geometry(&mut self, geometry: TerminalGeometry) {
        self.geometry = geometry;

        for tab in &mut self.tabs {
            if tab.pane_manager.is_some() {
                continue; // Pane terminals are resized via PaneManager::resize()
            }
            if let Err(e) = tab.resize_geometry(geometry) {
                log::error!("Failed to resize tab {}: {}", tab.id, e);
            }
        }
    }

    /// Detect plugin tabs that are about to be cleaned up but exited suspiciously
    /// fast (within 5 seconds of creation). Returns a list of (plugin_id, output_lines)
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

            // Was it a quick exit? (< 5 seconds from creation)
            if tab.created_at.elapsed().as_secs() >= 5 {
                continue;
            }

            // Extract the terminal's visible content for the error message
            let Some(terminal) = tab.focused_terminal() else {
                continue;
            };
            let grid = terminal.grid();
            let mut lines: Vec<String> = Vec::new();
            for row in 0..grid.rows() {
                let mut line = String::new();
                for col in 0..grid.cols() {
                    if let Some(cell) = grid.cell(row, col) {
                        if cell.is_wide_char_spacer()
                            || (cell.multicell.is_present() && !cell.multicell.is_leader())
                        {
                            continue;
                        }
                        if cell.content.is_empty() {
                            line.push(' ');
                        } else {
                            cell.content.push_to(&mut line);
                        }
                    }
                }
                let trimmed = line.trim_end().to_string();
                if !trimmed.is_empty() || !lines.is_empty() {
                    lines.push(trimmed);
                }
            }
            // Trim trailing empty lines
            while lines.last().is_some_and(|l| l.is_empty()) {
                lines.pop();
            }

            results.push((plugin_id, lines));
        }

        results
    }

    /// Remove closed panes and dissolve a one-pane manager only after App has
    /// handled the routed terminal events from the current poll. This ordering
    /// keeps app-mediated replies addressable by their exact pane ID.
    fn cleanup_pane_topology(&mut self) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            let Some(panes) = &mut tab.pane_manager else {
                continue;
            };
            let before = panes.pane_count();
            panes.cleanup_closed();
            changed |= panes.pane_count() != before;

            if let Some(live) = &mut tab.live_domain {
                live.pane_ids.retain(|local, _| panes.contains(*local));
            }

            let all_closed = panes.all_closed();
            if all_closed || panes.pane_count() <= 1 {
                let remaining_route = (!all_closed).then(|| {
                    let local = panes.focused_id();
                    let remote = tab
                        .live_domain
                        .as_ref()
                        .and_then(|live| live.pane_ids.get(&local).copied());
                    (local, remote)
                });
                let mut panes = tab.pane_manager.take().unwrap();
                if all_closed {
                    tab.closed = true;
                    if let Some(live) = &mut tab.live_domain {
                        live.pane_ids.clear();
                    }
                } else if let Some(remaining) = panes.take_remaining_pane() {
                    tab.restore_terminal_owner(remaining);
                    if let Some((local, Some(remote))) = remaining_route
                        && let Some(live) = &mut tab.live_domain
                    {
                        live.pane_ids.clear();
                        live.pane_ids.insert(local, remote);
                    }
                }
                if let Err(error) = tab.resize_geometry(self.geometry) {
                    log::error!(
                        "Failed to resize tab {} after pane close: {}",
                        tab.id,
                        error
                    );
                }
                log::info!("Tab {} exited pane mode after pane cleanup", tab.id);
                changed = true;
            } else {
                tab.reconcile_primary_pane_launch();
            }
        }
        changed
    }

    /// Remove closed tabs and tabs with completed close animations.
    /// Before removing, saves each non-empty tab when auto-save is enabled.
    /// Returns true if tab or pane topology changed.
    pub fn cleanup_closed(&mut self, auto_save: bool, save_scrollback: bool) -> bool {
        // If every pane exited in the same poll, topology cleanup has no live
        // owner to collapse back into the tab and must drop the pane manager.
        // Capture its still-addressable focused pane before that destructive
        // transition so close-time persistence captures the same focused
        // terminal as an ordinary tab close.
        let mut handled_before_topology_cleanup = Vec::new();
        if auto_save {
            for tab in &mut self.tabs {
                if !tab
                    .pane_manager
                    .as_ref()
                    .is_some_and(crate::panes::PaneManager::all_closed)
                {
                    continue;
                }
                handled_before_topology_cleanup.push(tab.id);
                if tab.live_domain.is_some() {
                    log::debug!(
                        "Skipping legacy snapshot for closing live-domain relay tab {}",
                        tab.id
                    );
                } else if tab.has_content() {
                    match tab.save_as_session(save_scrollback) {
                        Ok(outcome) => {
                            log::info!(
                                "Auto-saved closing split tab {} as session '{}' before pane cleanup",
                                tab.id,
                                outcome.id
                            );
                            self.persistence_notices.push(PersistenceNotice::Saved {
                                id: outcome.id,
                                truncated: outcome.scrollback_truncated,
                            });
                        }
                        Err(error) => {
                            log::error!(
                                "Failed to auto-save closing split tab {}: {error:#}",
                                tab.id
                            );
                            self.persistence_notices
                                .push(PersistenceNotice::SaveFailed {
                                    tab_id: tab.id,
                                    error: format!("{error:#}"),
                                });
                        }
                    }
                } else {
                    log::debug!("Skipping empty closing split tab {} (no content)", tab.id);
                }
            }
        }
        let pane_topology_changed = self.cleanup_pane_topology();
        let initial_count = self.tabs.len();

        // Save sessions for tabs about to be removed
        for tab in self.tabs.iter_mut() {
            if tab.closed || tab.is_close_complete() || (self.reduced_motion && tab.is_closing()) {
                if handled_before_topology_cleanup.contains(&tab.id) {
                    continue;
                } else if auto_save && tab.live_domain.is_some() {
                    log::debug!(
                        "Skipping legacy snapshot for closing live-domain relay tab {}",
                        tab.id
                    );
                } else if auto_save && tab.has_content() {
                    match tab.save_as_session(save_scrollback) {
                        Ok(outcome) => {
                            log::info!(
                                "Auto-saved closing tab {} as session '{}'",
                                tab.id,
                                outcome.id
                            );
                            self.persistence_notices.push(PersistenceNotice::Saved {
                                id: outcome.id,
                                truncated: outcome.scrollback_truncated,
                            });
                        }
                        Err(error) => {
                            log::error!("Failed to auto-save closing tab {}: {error:#}", tab.id);
                            self.persistence_notices
                                .push(PersistenceNotice::SaveFailed {
                                    tab_id: tab.id,
                                    error: format!("{error:#}"),
                                });
                        }
                    }
                } else if auto_save {
                    log::debug!("Skipping empty closing tab {} (no content)", tab.id);
                }
            }
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
        let tab_id = match origin {
            TerminalEventOrigin::Tab { tab_id } | TerminalEventOrigin::Pane { tab_id, .. } => {
                tab_id
            }
        };
        let tab = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "terminal tab no longer exists")
            })?;

        match origin {
            TerminalEventOrigin::Tab { .. } => {
                if tab.pane_manager.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "originating tab terminal is dormant in pane mode",
                    ));
                }
                tab.write_protocol_reply(data)
            }
            TerminalEventOrigin::Pane { pane_id, .. } => tab
                .pane_manager
                .as_mut()
                .and_then(|panes| panes.get_pane_mut(pane_id))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "originating pane no longer exists")
                })?
                .write_protocol_reply(data),
        }
    }

    /// Write captured user input to one exact terminal. Unlike
    /// `write_to_origin`, this path enforces the pane-local read-only policy.
    pub(crate) fn write_user_to_origin(
        &mut self,
        origin: TerminalEventOrigin,
        data: &[u8],
    ) -> io::Result<()> {
        let tab_id = match origin {
            TerminalEventOrigin::Tab { tab_id } | TerminalEventOrigin::Pane { tab_id, .. } => {
                tab_id
            }
        };
        let tab = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "terminal tab no longer exists")
            })?;
        if tab.closed || tab.is_closing() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "terminal tab is closed or closing",
            ));
        }
        match origin {
            TerminalEventOrigin::Tab { .. } => {
                if tab.pane_manager.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "originating tab terminal is dormant in pane mode",
                    ));
                }
                tab.write(data)
            }
            TerminalEventOrigin::Pane { pane_id, .. } => {
                tab.pane_manager
                    .as_mut()
                    .and_then(|panes| panes.get_pane_mut(pane_id))
                    .filter(|pane| !pane.closed)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, "originating pane no longer exists")
                    })?
                    .write(data)?;
                Ok(())
            }
        }
    }

    /// Capture a live event route before retaining it across asynchronous work.
    /// A tab route names the current single owner only at capture time.
    pub(crate) fn identity_for_origin(
        &self,
        origin: TerminalEventOrigin,
    ) -> Option<TerminalIdentity> {
        let (tab_id, pane_id) = match origin {
            TerminalEventOrigin::Tab { tab_id } => {
                let tab = self.tab_by_id(tab_id)?;
                if tab.pane_manager.is_some() {
                    return None;
                }
                (tab_id, tab.single_pane_id())
            }
            TerminalEventOrigin::Pane { tab_id, pane_id } => (tab_id, pane_id),
        };
        (self.origin_for_pane(tab_id, pane_id)? == origin)
            .then_some(TerminalIdentity { tab_id, pane_id })
    }

    /// Resolve a stable tab-local pane identity to its current owner. Unlike
    /// an event's mode-specific route, this survives split/collapse, but never
    /// crosses tab namespaces or substitutes a different single survivor.
    pub(crate) fn origin_for_pane(
        &self,
        tab_id: usize,
        pane_id: PaneId,
    ) -> Option<TerminalEventOrigin> {
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        if tab.closed || tab.is_closing() {
            return None;
        }
        if let Some(panes) = &tab.pane_manager {
            panes.get_pane(pane_id).filter(|pane| !pane.closed)?;
            Some(TerminalEventOrigin::Pane { tab_id, pane_id })
        } else if tab.single_pane_id() == pane_id && tab.focused_terminal().is_some() {
            Some(TerminalEventOrigin::Tab { tab_id })
        } else {
            None
        }
    }

    pub(crate) fn origin_read_only(&self, origin: TerminalEventOrigin) -> Option<bool> {
        let tab_id = match origin {
            TerminalEventOrigin::Tab { tab_id } | TerminalEventOrigin::Pane { tab_id, .. } => {
                tab_id
            }
        };
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        if tab.closed || tab.is_closing() {
            return None;
        }
        match origin {
            TerminalEventOrigin::Tab { .. } if tab.pane_manager.is_none() => {
                tab.pane_read_only(tab.single_pane_id())
            }
            TerminalEventOrigin::Pane { pane_id, .. } => tab
                .pane_manager
                .as_ref()?
                .get_pane(pane_id)
                .filter(|pane| !pane.closed)
                .map(|pane| pane.read_only),
            TerminalEventOrigin::Tab { .. } => None,
        }
    }

    pub(crate) fn terminal_for_origin(&self, origin: TerminalEventOrigin) -> Option<&Terminal> {
        let tab_id = match origin {
            TerminalEventOrigin::Tab { tab_id } | TerminalEventOrigin::Pane { tab_id, .. } => {
                tab_id
            }
        };
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        match origin {
            TerminalEventOrigin::Tab { .. } => tab
                .pane_manager
                .is_none()
                .then_some(())
                .and_then(|()| tab.focused_terminal()),
            TerminalEventOrigin::Pane { pane_id, .. } => tab
                .pane_manager
                .as_ref()?
                .get_pane(pane_id)
                .map(|pane| &pane.terminal),
        }
    }

    pub(crate) fn terminal_for_origin_mut(
        &mut self,
        origin: TerminalEventOrigin,
    ) -> Option<&mut Terminal> {
        let tab_id = match origin {
            TerminalEventOrigin::Tab { tab_id } | TerminalEventOrigin::Pane { tab_id, .. } => {
                tab_id
            }
        };
        let tab = self.tabs.iter_mut().find(|tab| tab.id == tab_id)?;
        match origin {
            TerminalEventOrigin::Tab { .. } => tab
                .pane_manager
                .is_none()
                .then_some(())
                .and_then(|()| tab.focused_terminal_mut()),
            TerminalEventOrigin::Pane { pane_id, .. } => tab
                .pane_manager
                .as_mut()?
                .get_pane_mut(pane_id)
                .map(|pane| &mut pane.terminal),
        }
    }

    pub(crate) fn set_title_for_origin(
        &mut self,
        origin: TerminalEventOrigin,
        title: String,
    ) -> bool {
        let tab_id = match origin {
            TerminalEventOrigin::Tab { tab_id } | TerminalEventOrigin::Pane { tab_id, .. } => {
                tab_id
            }
        };
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return false;
        };
        if tab.live_domain.is_some() {
            // The daemon parses this same relay output and publishes the
            // authoritative title. Applying it locally would bypass explicit
            // remote rename precedence and briefly diverge across windows.
            return true;
        }
        if let TerminalEventOrigin::Pane { pane_id, .. } = origin {
            let Some(pane) = tab
                .pane_manager
                .as_mut()
                .and_then(|panes| panes.get_pane_mut(pane_id))
            else {
                return false;
            };
            pane.title = (!title.is_empty()).then(|| title.clone());
        }
        tab.set_osc_title(title);
        true
    }

    /// Iterate over all tabs with their indices
    pub fn iter(&self) -> impl Iterator<Item = (usize, &Tab)> {
        self.tabs.iter().enumerate()
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

    /// Find the index of the first tab owned by a given plugin
    #[cfg(test)]
    pub fn find_plugin_tab_index(&self, plugin_id: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            !tab.closed
                && !tab.is_closing()
                && matches!(&tab.tab_type, TabType::Plugin { plugin_id: id } if id == plugin_id)
        })
    }

    /// Prefer the active tab, then other live tabs in order. Within each tab,
    /// prefer its focused matching owner, then its first matching pane in
    /// visual order. Tab presentation is not launch identity.
    #[cfg(test)]
    pub(crate) fn plugin_terminal_identity(&self, plugin_id: &str) -> Option<TerminalIdentity> {
        // Prefer the active tab's matching owner before searching other tabs.
        // Otherwise moving a focused plugin beside an earlier same-plugin tab
        // would make subsequent profile/restart commands target that sibling.
        self.active_tab()
            .into_iter()
            .chain(self.tabs.iter())
            .find_map(|tab| {
                tab.plugin_pane_id(plugin_id)
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
                unreviewed_diff_count: tab.unreviewed_diff_count(),
                agent_badge: tab.agent_badge(),
            })
            .collect()
    }

    /// Update scrollback line and byte budgets for existing and future terminals.
    pub fn set_scrollback_limits(&mut self, lines: usize, bytes: usize) {
        self.scrollback_lines = lines;
        self.scrollback_bytes = bytes;
        for tab in &mut self.tabs {
            if let Some(panes) = &mut tab.pane_manager {
                panes.set_scrollback_limits(lines, bytes);
            } else if let Some(terminal) = tab.focused_terminal_mut() {
                terminal.set_scrollback_limits(lines, bytes);
            }
        }
    }

    /// Update the screen-report policy for all terminals and future tabs/panes.
    pub fn set_screen_checksum_reporting(&mut self, allowed: bool) {
        self.screen_checksum_reporting = allowed;
        for tab in &mut self.tabs {
            tab.set_screen_checksum_reporting(allowed);
        }
    }

    /// Update the Kitty persistent-file policy for all terminals and future
    /// tabs/panes.
    pub fn set_kitty_file_transmission(&mut self, allowed: bool) {
        self.kitty_file_transmission = allowed;
        for tab in &mut self.tabs {
            tab.set_kitty_file_transmission(allowed);
        }
    }

    /// Update the configured xterm keyboard profile for every terminal and
    /// for future tabs and pane splits. `apply_current` distinguishes an
    /// explicit profile change from reapplying unrelated configuration.
    pub fn set_xterm_keyboard_type(
        &mut self,
        keyboard_type: XtermKeyboardType,
        apply_current: bool,
    ) {
        self.xterm_keyboard_type = keyboard_type;
        for tab in &mut self.tabs {
            tab.set_xterm_keyboard_type(keyboard_type, apply_current);
        }
    }

    /// Update the shell used by tabs created after a config reload.
    /// Existing PTYs are intentionally left running.
    pub fn set_shell_config(&mut self, shell: ShellConfig) {
        self.shell = shell;
    }

    /// Create a tab from a saved session (single-tab) and add it
    ///
    /// The new tab inherits the session's ID so subsequent saves update
    /// the same session entry.
    pub fn new_tab_from_session(&mut self, session: &Session) -> anyhow::Result<usize> {
        let lines = crate::session::restore::load_snapshot_scrollback(session)
            .map_err(|error| anyhow::anyhow!("Snapshot could not be recovered: {error:#}"))?;
        let id = self.next_id;
        self.next_id += 1;

        let mut tab = Tab::new_with_cwd_geometry(
            id,
            self.geometry,
            &self.shell,
            self.scrollback_lines,
            self.scrollback_bytes,
            session.working_directory.as_deref(),
            Some(&session.title),
        )?;
        tab.set_screen_checksum_reporting(self.screen_checksum_reporting);
        tab.set_kitty_file_transmission(self.kitty_file_transmission);
        tab.set_xterm_keyboard_type(self.xterm_keyboard_type, true);

        // Link this tab to its session
        tab.session_id = Some(session.id.clone());

        // Restore last_command so the session title is preserved across open/close cycles
        tab.last_command = session.last_command.clone();

        // Replay scrollback
        if !lines.is_empty() {
            // Join lines with \r\n and add a trailing \r\n so the shell's
            // new prompt appears on a fresh line below the scrollback. Saved
            // lines stop before the old final prompt (`restorable_scrollback`).
            let text = lines.join("\r\n") + "\r\n";
            if let Some(terminal) = tab.focused_terminal_mut() {
                terminal.process(text.as_bytes());
            }
            log::info!(
                "Replayed {} scrollback lines into restored tab",
                lines.len()
            );
        }

        self.tabs.push(tab);
        // Switch to the newly restored tab
        self.active = self.tabs.len() - 1;

        log::info!("Restored tab {} from session '{}'", id, session.id);
        Ok(id)
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

fn replay_workspace_scrollback(terminal: &mut Terminal, lines: &[String]) {
    if !lines.is_empty() {
        terminal.process((lines.join("\r\n") + "\r\n").as_bytes());
    }
}

fn usable_workspace_cwd(
    cwd: Option<&str>,
    tab_id: u32,
    pane_id: u32,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let cwd = cwd?;
    if std::path::Path::new(cwd).is_dir() {
        Some(cwd.to_string())
    } else {
        warnings.push(format!(
            "tab {tab_id} pane {pane_id} cwd {cwd:?} is unavailable; used the process default"
        ));
        None
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::{PaneManager, SplitDirection};
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn runtime_scrollback_limits_bound_existing_panes_future_splits_and_future_tabs() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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

        let owner = manager
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut panes = PaneManager::from_existing(
            owner,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            160.0,
            10.0,
            20.0,
        );
        panes.split(SplitDirection::Vertical).unwrap();
        manager.active_tab_mut().unwrap().pane_manager = Some(panes);

        const LINE_LIMIT: usize = 32;
        const BYTE_LIMIT: usize = 4 * 1024;
        manager.set_scrollback_limits(LINE_LIMIT, BYTE_LIMIT);
        manager
            .active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap()
            .split(SplitDirection::Horizontal)
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
            if let Some(panes) = &mut tab.pane_manager {
                let pane_ids: Vec<_> = panes.iter_with_layout().map(|(pane, _)| pane.id).collect();
                for pane_id in pane_ids {
                    let terminal = &mut panes.get_pane_mut(pane_id).unwrap().terminal;
                    for _ in 0..64 {
                        terminal.process(line.as_bytes());
                    }
                }
            } else {
                let terminal = tab.focused_terminal_mut().unwrap();
                for _ in 0..64 {
                    terminal.process(line.as_bytes());
                }
            }
        }

        let usages: Vec<_> = manager
            .tabs
            .iter()
            .flat_map(|tab| {
                if let Some(panes) = &tab.pane_manager {
                    panes
                        .iter_with_layout()
                        .map(|(pane, _)| pane.terminal.memory_usage())
                        .collect::<Vec<_>>()
                } else {
                    vec![tab.focused_terminal().unwrap().memory_usage()]
                }
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

    #[cfg(unix)]
    #[test]
    fn simultaneous_pane_exit_autosaves_the_focused_source_before_cleanup() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let session_root = tempfile::tempdir().unwrap();
        crate::session::set_test_sessions_base_dir(Some(session_root.path().to_path_buf()));
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
        let owner = tab.take_terminal_owner().unwrap();
        let mut panes = PaneManager::from_existing(
            owner,
            ShellConfig::default(),
            1_000,
            8 * 1024 * 1024,
            800.0,
            160.0,
            10.0,
            20.0,
        );
        let right = panes.split(SplitDirection::Vertical).unwrap();
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
        tab.pane_manager = Some(panes);

        assert!(manager.cleanup_closed(true, true));
        assert!(manager.is_empty());
        let notices = manager.drain_persistence_notices();
        let [PersistenceNotice::Saved { id, truncated }] = notices.as_slice() else {
            panic!("close-time focused pane was not saved: {notices:?}");
        };
        assert!(!truncated);
        let restored = crate::session::restore::restore_named_session(id)
            .unwrap()
            .unwrap();
        assert_eq!(
            restored.capture_scope,
            crate::session::SessionCaptureScope::FocusedPane
        );
        assert_eq!(restored.last_command.as_deref(), Some("right-command"));
        let history = crate::session::restore::load_snapshot_scrollback(&restored)
            .unwrap()
            .join("\n");
        assert!(history.contains("right-history"));
        assert!(!history.contains("left-history"));
        crate::session::set_test_sessions_base_dir(None);
    }

    #[cfg(unix)]
    #[test]
    fn first_snapshot_failure_does_not_assign_session_identity() {
        let root = tempfile::tempdir().unwrap();
        let blocked = root.path().join("not-a-directory");
        std::fs::write(&blocked, "sentinel").unwrap();
        crate::session::set_test_sessions_base_dir(Some(blocked));
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
        let result = tab.save_as_session(false);
        crate::session::set_test_sessions_base_dir(None);
        assert!(result.is_err());
        assert!(tab.session_id.is_none());
    }

    #[test]
    fn stale_snapshot_restore_does_not_allocate_or_spawn_a_tab() {
        let root = tempfile::tempdir().unwrap();
        crate::session::set_test_sessions_base_dir(Some(root.path().to_owned()));
        let mut saved = Session::with_id("stale".into(), "old".into(), None);
        crate::session::save::save_snapshot(&saved, &["old text".into()]).unwrap();
        let selected = crate::session::restore::restore_named_session("stale")
            .unwrap()
            .unwrap();
        saved.title = "new".into();
        crate::session::save::save_snapshot(&saved, &["new text".into()]).unwrap();
        let shell = ShellConfig {
            program: Some("/bin/cat".into()),
            ..Default::default()
        };
        let mut manager = TabManager::for_test(shell);
        let next_id = manager.next_id;
        let result = manager.new_tab_from_session(&selected);
        crate::session::set_test_sessions_base_dir(None);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("changed since selection")
        );
        assert!(manager.is_empty());
        assert_eq!(manager.next_id, next_id);
    }

    #[cfg(unix)]
    #[test]
    fn screen_checksum_policy_updates_existing_and_future_tabs() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let mut manager = TabManager::for_test(ShellConfig::default());
        manager.set_screen_checksum_reporting(true);
        manager.set_kitty_file_transmission(true);
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

        manager.set_screen_checksum_reporting(false);
        manager.set_kitty_file_transmission(false);
        assert!(manager.tabs.iter().all(|tab| {
            tab.focused_terminal().is_some_and(|terminal| {
                !terminal.screen_checksum_reporting() && !terminal.kitty_file_transmission()
            })
        }));
    }

    #[cfg(unix)]
    #[test]
    fn xterm_keyboard_profile_updates_existing_and_future_tabs() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let mut manager = TabManager::for_test(ShellConfig::default());
        manager.set_xterm_keyboard_type(XtermKeyboardType::Legacy, true);
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
        manager.set_xterm_keyboard_type(XtermKeyboardType::Legacy, false);
        assert_eq!(
            manager
                .active_tab()
                .unwrap()
                .focused_terminal()
                .unwrap()
                .xterm_keyboard_type(),
            XtermKeyboardType::Vt220
        );

        manager.set_xterm_keyboard_type(XtermKeyboardType::Normal, true);
        let terminal = manager.active_tab().unwrap().focused_terminal().unwrap();
        assert_eq!(terminal.xterm_keyboard_type(), XtermKeyboardType::Normal);
        assert_eq!(
            terminal.initial_xterm_keyboard_type(),
            XtermKeyboardType::Normal
        );
    }

    #[cfg(unix)]
    #[test]
    fn rename_reorder_and_pin_preserve_stable_tab_identity() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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

    #[cfg(unix)]
    #[test]
    fn exact_tab_reorder_preserves_selection_and_rejects_pinned_boundary_crossing() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn focused_osc7_directory_drives_title_but_remote_paths_stay_display_only() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
        let owner = manager
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut panes = PaneManager::from_existing(
            owner,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        let remote = panes.split(SplitDirection::Vertical).unwrap();
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
        tab.pane_manager = Some(panes);

        tab.pane_manager.as_mut().unwrap().focus(PaneId(0));
        tab.update_title();
        assert_eq!(tab.title, "local-cwd");
        assert_eq!(tab.working_directory().as_deref(), local.to_str());

        tab.pane_manager.as_mut().unwrap().focus(remote);
        tab.update_title();
        assert_eq!(tab.title, "remote-project");
        assert_ne!(
            tab.working_directory().as_deref(),
            Some("/srv/remote-project")
        );
    }

    #[cfg(unix)]
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
        let owner = source
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut panes =
            PaneManager::from_existing(owner, shell, 100, 1024 * 1024, 800.0, 480.0, 10.0, 20.0);
        panes.split(SplitDirection::Vertical).unwrap();
        panes.focus(PaneId(0));
        source.active_tab_mut().unwrap().pane_manager = Some(panes);
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
        let moved = source
            .move_focused_pane_to_next_tab(crate::config::PaneCwdMode::Focused)
            .unwrap();
        // Drop the destination's unrelated original pane so the tracked owner
        // collapses back into a single-terminal tab.
        source
            .active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap()
            .get_pane_mut(PaneId(0))
            .unwrap()
            .closed = true;
        source.cleanup_closed(false, false);
        assert!(source.active_tab().unwrap().pane_manager.is_none());
        assert_eq!(source.active_tab().unwrap().single_pane_id(), moved);
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

    #[cfg(unix)]
    #[test]
    fn moving_any_pane_between_tabs_preserves_child_and_restore_identity() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
        let owner = manager
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut panes = PaneManager::from_existing(
            owner,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        let movable = panes.split(SplitDirection::Vertical).unwrap();
        panes.focus(PaneId(0));
        let child = panes.get_pane(PaneId(0)).unwrap().pty.child_id();
        manager.active_tab_mut().unwrap().pane_manager = Some(panes);

        let attached = manager
            .move_focused_pane_to_next_tab(crate::config::PaneCwdMode::Focused)
            .unwrap();
        assert_eq!(manager.active_index(), 1);
        assert_eq!(
            manager.tabs[0].pane_manager.as_ref().unwrap().pane_count(),
            1
        );
        let source_snapshot = manager.tabs[0].capture_workspace_tab(0, false).unwrap();
        source_snapshot.validate().unwrap();
        assert_eq!(source_snapshot.primary_pane, movable.0);
        assert_eq!(source_snapshot.focused_pane, movable.0);
        let destination = manager.active_tab().unwrap().pane_manager.as_ref().unwrap();
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
        assert!(restored_tab.pane_manager.is_none());
        assert_eq!(restored_tab.single_pane_id(), movable);
        let round_trip = restored_tab.capture_workspace_tab(0, false).unwrap();
        assert_eq!(round_trip.primary_pane, movable.0);
        assert_eq!(round_trip.focused_pane, movable.0);
    }

    #[cfg(unix)]
    #[test]
    fn exact_cross_tab_pane_transfer_uses_source_and_destination_ids() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
        let owner = manager.tabs[0].take_terminal_owner().unwrap();
        let mut panes = PaneManager::from_existing(
            owner,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        let remaining = panes.split(SplitDirection::Vertical).unwrap();
        panes.focus(remaining);
        let child = panes.get_pane(PaneId(0)).unwrap().pty.child_id();
        manager.tabs[0].pane_manager = Some(panes);

        let same_tab_error = manager
            .transfer_pane_by_id(
                0,
                PaneId(0),
                0,
                remaining,
                SplitDirection::Horizontal,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap_err()
            .to_string();
        assert!(same_tab_error.contains("layout-tree"), "{same_tab_error}");

        manager.tabs[1].tab_type = TabType::Serial {
            name: "board".into(),
        };
        let serial_error = manager
            .transfer_pane_by_id(
                0,
                PaneId(0),
                1,
                PaneId(0),
                SplitDirection::Horizontal,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap_err()
            .to_string();
        assert!(serial_error.contains("serial terminal"), "{serial_error}");
        assert!(
            manager.tabs[0]
                .pane_manager
                .as_ref()
                .unwrap()
                .get_pane(PaneId(0))
                .is_some()
        );
        assert!(manager.tabs[1].pane_manager.is_none());
        manager.tabs[1].tab_type = TabType::Shell;

        let attached = manager
            .transfer_pane_by_id(
                0,
                PaneId(0),
                1,
                PaneId(0),
                SplitDirection::Horizontal,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        assert_eq!(manager.active_index(), 1);
        assert_eq!(
            manager.tabs[0].pane_manager.as_ref().unwrap().pane_ids(),
            vec![remaining]
        );
        let destination = manager.tabs[1].pane_manager.as_ref().unwrap();
        assert_eq!(destination.focused_id(), attached);
        assert_eq!(
            destination.get_pane(attached).unwrap().pty.child_id(),
            child
        );
        let layout = destination.calculate_layout();
        assert!(layout[&attached].y > layout[&PaneId(0)].y);
    }

    #[cfg(unix)]
    #[test]
    fn moved_profile_pane_keeps_launch_identity_through_restore_and_collapse() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
        let owner = manager
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut profile_panes = PaneManager::from_existing(
            owner,
            profile_shell.clone(),
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        profile_panes.split(SplitDirection::Vertical).unwrap();
        manager.active_tab_mut().unwrap().pane_manager = Some(profile_panes);
        manager.new_tab_with_cwd(None, Some("shell")).unwrap();
        manager.switch_to(0);

        let attached = manager
            .move_focused_pane_to_next_tab(crate::config::PaneCwdMode::Focused)
            .unwrap();
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
                .and_then(|pane| pane.launch.as_ref()),
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
                .pane_manager
                .as_ref()
                .unwrap()
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
        let panes = tab.pane_manager.as_mut().unwrap();
        panes.focus(PaneId(0));
        assert!(panes.close_focused());
        let mut panes = tab.pane_manager.take().unwrap();
        let remaining = panes.take_remaining_pane().unwrap();
        assert_eq!(remaining.pane.id, attached);
        tab.restore_terminal_owner(remaining);
        let collapsed = tab.capture_workspace_tab(1, false).unwrap();
        assert_eq!(
            collapsed.launch,
            crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() }
        );
        assert!(matches!(tab.tab_type, TabType::Profile { ref name } if name == "dev"));

        let owner = tab.take_terminal_owner().unwrap();
        let mut panes = PaneManager::from_existing(
            owner,
            tab.split_shell_config(),
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        let new_pane = panes.split(SplitDirection::Vertical).unwrap();
        assert_eq!(
            panes.get_pane(new_pane).unwrap().launch,
            crate::workspace::WorkspaceLaunch::Profile { name: "dev".into() }
        );
        assert_eq!(
            panes.get_pane(new_pane).unwrap().launch_shell.args,
            profile_shell.args
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_primary_and_ordinary_sibling_can_move_without_relaunching() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
        let owner = manager
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut panes = PaneManager::from_existing(
            owner,
            shell,
            1_000,
            8 * 1024 * 1024,
            800.0,
            480.0,
            10.0,
            20.0,
        );
        let ordinary = panes.split(SplitDirection::Vertical).unwrap();
        panes.focus(PaneId(0));
        manager.active_tab_mut().unwrap().pane_manager = Some(panes);
        manager.new_tab().unwrap();
        manager.switch_to(0);

        let moved_plugin = manager
            .move_focused_pane_to_next_tab(crate::config::PaneCwdMode::Focused)
            .unwrap();
        assert_eq!(manager.tabs[1].pane_plugin_id(moved_plugin), Some("fake"));
        manager
            .move_focused_pane_to_next_tab(crate::config::PaneCwdMode::Focused)
            .unwrap();
        assert_eq!(
            manager.tabs[0].pane_manager.as_ref().unwrap().pane_count(),
            2
        );

        manager.tabs[0]
            .pane_manager
            .as_mut()
            .unwrap()
            .focus(ordinary);
        manager
            .move_focused_pane_to_next_tab(crate::config::PaneCwdMode::Focused)
            .unwrap();
        let moved = manager
            .active_tab()
            .unwrap()
            .pane_manager
            .as_ref()
            .unwrap()
            .focused_pane()
            .unwrap();
        assert_eq!(moved.launch, crate::workspace::WorkspaceLaunch::Shell);
    }

    #[cfg(unix)]
    #[test]
    fn tab_spawn_and_pixel_only_resize_reach_kernel_winsize() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let initial = TerminalGeometry::from_cells(80, 24, 9.25, 18.5);
        let mut manager =
            TabManager::new_with_geometry(initial, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
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
            manager.active_tab().unwrap().single_pty_size(),
            Some(initial.pty_size())
        );

        let pixel_only = TerminalGeometry::from_cells(80, 24, 10.0, 20.0);
        manager.resize_geometry(pixel_only);
        assert_eq!(
            manager.active_tab().unwrap().single_pty_size(),
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
        manager.resize_geometry(TerminalGeometry::from_cells(8, 5, 2.0, 3.0));
        crate::test_support::seed_pixel_geometry_retention_probe(
            manager
                .active_tab_mut()
                .unwrap()
                .focused_terminal_mut()
                .unwrap(),
        );
        let scaled = TerminalGeometry::from_cells(8, 2, 1.0, 1.0);
        manager.resize_geometry(scaled);
        let tab = manager.active_tab().unwrap();
        assert_eq!(tab.single_pty_size(), Some(scaled.pty_size()));
        crate::test_support::assert_pixel_geometry_retention_probe(tab.focused_terminal().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn inactive_tab_panes_poll_replies_and_tag_app_events_without_stealing_focus() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
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
        let mut panes = PaneManager::new(
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
        panes.split(SplitDirection::Vertical).unwrap();
        let pane_ids: HashSet<PaneId> = panes.iter_with_layout().map(|(pane, _)| pane.id).collect();
        manager.tabs[0].pane_manager = Some(panes);

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
                let TerminalEventOrigin::Pane { tab_id, pane_id } = routed.origin else {
                    continue;
                };
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
            let all_replied = manager.tabs[0]
                .pane_manager
                .as_ref()
                .unwrap()
                .iter_with_layout()
                .all(|(pane, _)| {
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

    #[cfg(unix)]
    #[test]
    fn domain_attachment_reconstructs_nested_layout_and_stable_focus_mapping() {
        if !std::path::Path::new("/bin/true").exists() {
            return;
        }
        use crate::domain::model::{
            DomainSnapshot, LivePane, LivePaneLayout, LiveTab, LiveWorkspace,
        };
        let panes = [5, 9, 12]
            .into_iter()
            .map(|id| LivePane {
                id,
                title: format!("remote-{id}"),
                working_directory: None,
                command: vec!["shell".into()],
                cols: 80,
                rows: 24,
                closed: false,
                attachments: 0,
            })
            .collect();
        let snapshot = DomainSnapshot {
            id: "domain-layout".into(),
            name: "layout".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            workspace: LiveWorkspace {
                id: "live-domain-layout".into(),
                name: "layout".into(),
                active_tab: 7,
                tabs: vec![LiveTab {
                    id: 7,
                    title: "nested".into(),
                    user_title: None,
                    pinned: false,
                    focused_pane: 12,
                    layout: LivePaneLayout::Split {
                        split_id: 70,
                        direction: SplitDirection::Vertical,
                        ratio_millis: 400,
                        first: Box::new(LivePaneLayout::Pane { pane: 5 }),
                        second: Box::new(LivePaneLayout::Split {
                            split_id: 71,
                            direction: SplitDirection::Horizontal,
                            ratio_millis: 600,
                            first: Box::new(LivePaneLayout::Pane { pane: 9 }),
                            second: Box::new(LivePaneLayout::Pane { pane: 12 }),
                        }),
                    },
                    panes,
                }],
            },
        };
        let mut manager = TabManager::new(80, 24, ShellConfig::default(), 1_000, 1024 * 1024);
        manager.set_kitty_file_transmission(true);
        manager
            .attach_domain(
                &snapshot,
                &crate::domain::DomainRelayCommand::local("/bin/true"),
            )
            .unwrap();
        let local_tab_id = {
            let tab = manager.active_tab().unwrap();
            assert_eq!(tab.pane_manager.as_ref().unwrap().pane_count(), 3);
            let domain_panes = tab.pane_manager.as_ref().unwrap();
            assert!(domain_panes.pane_ids().into_iter().all(|id| {
                domain_panes
                    .get_pane(id)
                    .is_some_and(|pane| !pane.terminal.kitty_file_transmission())
            }));
            assert_eq!(tab.live_domain_focus_route(), Some(("domain-layout", 12)));

            assert_eq!(tab.live_domain_tab_route(), Some(("domain-layout", 7)));
            assert!(matches!(
                tab.pane_manager.as_ref().unwrap().workspace_layout(),
                crate::workspace::WorkspacePaneLayout::Split { ratio, .. }
                    if (ratio - 0.4).abs() < f32::EPSILON
            ));
            let mut split_ids: Vec<_> = tab
                .pane_manager
                .as_ref()
                .unwrap()
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

        // The primary owner transferred into pane mode and every later pane
        // must accept exactly one daemon snapshot. An invalid private end
        // consumes that permission and clears a deliberately seeded image.
        for pane in manager
            .active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap()
            .all_panes_mut()
        {
            pane.terminal
                .process(b"\x1b_Ga=T,f=32,s=1,v=1,i=9,c=1,r=1,C=1;AQIDBA==\x1b\\");
            assert!(pane.terminal.has_images());
            pane.terminal.process(b"\x1b_TG;end\x1b\\");
            assert!(!pane.terminal.has_images());
        }
        manager.set_kitty_file_transmission(true);
        let domain_panes = manager.active_tab().unwrap().pane_manager.as_ref().unwrap();
        assert!(domain_panes.pane_ids().into_iter().all(|id| {
            domain_panes
                .get_pane(id)
                .is_some_and(|pane| !pane.terminal.kitty_file_transmission())
        }));
        assert!(manager.reconcile_closed_live_domain_pane(local_tab_id, 9));
        let tab = manager.active_tab().unwrap();
        assert_eq!(tab.pane_manager.as_ref().unwrap().pane_count(), 2);
        assert_eq!(tab.live_domain_focus_route(), Some(("domain-layout", 12)));

        assert!(manager.reconcile_closed_live_domain_pane(local_tab_id, 12));
        let tab = manager.active_tab().unwrap();
        assert!(tab.pane_manager.is_none());
        assert_eq!(tab.live_domain_focus_route(), Some(("domain-layout", 5)));
        assert!(
            !manager.reconcile_closed_live_domain_pane(local_tab_id, 12),
            "duplicate completion must be idempotent"
        );
    }

    #[cfg(unix)]
    #[test]
    fn domain_topology_revisions_incrementally_add_split_collapse_and_remove_relays() {
        if !std::path::Path::new("/bin/true").exists() {
            return;
        }
        use crate::domain::model::{
            DomainSnapshot, LivePane, LivePaneLayout, LiveTab, LiveWorkspace,
        };
        let remote_pane = |id| LivePane {
            id,
            title: format!("remote-{id}"),
            working_directory: None,
            command: vec!["shell".into()],
            cols: 80,
            rows: 24,
            closed: false,
            attachments: 0,
        };
        let mut snapshot = DomainSnapshot {
            id: "domain-revisions".into(),
            name: "revisions".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            workspace: LiveWorkspace {
                id: "live-domain-revisions".into(),
                name: "revisions".into(),
                active_tab: 7,
                tabs: vec![LiveTab {
                    id: 7,
                    title: "first".into(),
                    user_title: None,
                    pinned: false,
                    focused_pane: 5,
                    layout: LivePaneLayout::Pane { pane: 5 },
                    panes: vec![remote_pane(5)],
                }],
            },
        };
        let mut manager = TabManager::new(80, 24, ShellConfig::default(), 1_000, 1024 * 1024);
        manager
            .attach_domain(
                &snapshot,
                &crate::domain::DomainRelayCommand::local("/bin/true"),
            )
            .unwrap();
        let stable_first_tab = manager.active_tab().unwrap().id;

        snapshot.workspace.tabs[0].focused_pane = 9;
        snapshot.workspace.tabs[0].layout = LivePaneLayout::Split {
            split_id: 70,
            direction: SplitDirection::Vertical,
            ratio_millis: 350,
            first: Box::new(LivePaneLayout::Pane { pane: 5 }),
            second: Box::new(LivePaneLayout::Pane { pane: 9 }),
        };
        snapshot.workspace.tabs[0].panes.push(remote_pane(9));
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
        assert_eq!(first.pane_manager.as_ref().unwrap().pane_count(), 2);
        assert_eq!(
            first.live_domain_focus_route(),
            Some(("domain-revisions", 9))
        );

        snapshot.workspace.tabs.push(LiveTab {
            id: 8,
            title: "second".into(),
            user_title: None,
            pinned: false,
            focused_pane: 15,
            layout: LivePaneLayout::Pane { pane: 15 },
            panes: vec![remote_pane(15)],
        });
        snapshot.workspace.active_tab = 8;
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

        snapshot.workspace.tabs[0].focused_pane = 5;
        snapshot.workspace.tabs[0].layout = LivePaneLayout::Pane { pane: 5 };
        snapshot.workspace.tabs[0].panes.retain(|pane| pane.id == 5);
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
        assert!(first.pane_manager.is_none());
        assert_eq!(
            first.live_domain_focus_route(),
            Some(("domain-revisions", 5))
        );

        snapshot.workspace.tabs.remove(0);
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

    #[cfg(unix)]
    #[test]
    fn relay_driven_full_tab_exit_removes_the_tab_instead_of_leaving_an_empty_owner() {
        if !std::path::Path::new("/bin/echo").exists() {
            return;
        }
        use crate::domain::model::{
            DomainSnapshot, LivePane, LivePaneLayout, LiveTab, LiveWorkspace,
        };
        let pane = |id| LivePane {
            id,
            title: format!("remote-{id}"),
            working_directory: None,
            command: vec!["shell".into()],
            cols: 80,
            rows: 24,
            closed: false,
            attachments: 0,
        };
        let snapshot = DomainSnapshot {
            id: "domain-exit".into(),
            name: "exit".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            workspace: LiveWorkspace {
                id: "live-domain-exit".into(),
                name: "exit".into(),
                active_tab: 3,
                tabs: vec![LiveTab {
                    id: 3,
                    title: "closing".into(),
                    user_title: None,
                    pinned: false,
                    focused_pane: 8,
                    layout: LivePaneLayout::Split {
                        split_id: 30,
                        direction: SplitDirection::Vertical,
                        ratio_millis: 500,
                        first: Box::new(LivePaneLayout::Pane { pane: 4 }),
                        second: Box::new(LivePaneLayout::Pane { pane: 8 }),
                    },
                    panes: vec![pane(4), pane(8)],
                }],
            },
        };
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
            "relay output must never become a legacy session snapshot"
        );
        crate::session::set_test_sessions_base_dir(None);
    }

    #[cfg(unix)]
    #[test]
    fn relay_driven_split_dissolution_remaps_focus_to_the_surviving_remote_pane() {
        use crate::domain::model::{
            DomainSnapshot, LivePane, LivePaneLayout, LiveTab, LiveWorkspace,
        };
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let relay = temporary.path().join("fake-relay");
        std::fs::write(
            &relay,
            "#!/bin/sh\nif [ \"$4\" = \"8\" ]; then exit 0; fi\nsleep 5\n",
        )
        .unwrap();
        std::fs::set_permissions(&relay, std::fs::Permissions::from_mode(0o700)).unwrap();
        let pane = |id| LivePane {
            id,
            title: format!("remote-{id}"),
            working_directory: None,
            command: vec!["shell".into()],
            cols: 80,
            rows: 24,
            closed: false,
            attachments: 0,
        };
        let snapshot = DomainSnapshot {
            id: "domain-partial-exit".into(),
            name: "partial exit".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            workspace: LiveWorkspace {
                id: "live-domain-partial-exit".into(),
                name: "partial exit".into(),
                active_tab: 3,
                tabs: vec![LiveTab {
                    id: 3,
                    title: "closing split".into(),
                    user_title: None,
                    pinned: false,
                    focused_pane: 8,
                    layout: LivePaneLayout::Split {
                        split_id: 30,
                        direction: SplitDirection::Vertical,
                        ratio_millis: 500,
                        first: Box::new(LivePaneLayout::Pane { pane: 4 }),
                        second: Box::new(LivePaneLayout::Pane { pane: 8 }),
                    },
                    panes: vec![pane(4), pane(8)],
                }],
            },
        };
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
            .is_some_and(|tab| tab.pane_manager.is_some())
            && std::time::Instant::now() < deadline
        {
            manager.poll_events();
            manager.cleanup_closed(false, false);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let tab = manager.active_tab().unwrap();
        assert!(tab.pane_manager.is_none());
        assert_eq!(
            tab.live_domain_focus_route(),
            Some(("domain-partial-exit", 4))
        );
    }
}
