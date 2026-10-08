//! Complete workspace persistence and relaunch orchestration.

use super::*;
use crate::tabs::TabType;
use crate::ui::info_overlay::push_keyless_choice;
use crate::ui::text::plural;
use crate::workspace::{
    WorkspaceConfigReference, WorkspaceLaunch, WorkspaceRestoreOutcome, WorkspaceSnapshot,
    WorkspaceTabSnapshot,
};

/// Which workspace a person's save puts this window in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceSave {
    /// **Save Workspace**: back into the workspace this window belongs to,
    /// else into a new one.
    Update,
    /// **Save Workspace As…**: always into a new workspace.
    New,
}

/// The archive a save writes over: its identity and how it was named.
struct SaveTarget {
    id: String,
    name: String,
    created_at: String,
    automatic: bool,
}

const AUTOMATIC_WORKSPACE_NAME: &str = "Last workspace";
const WORKSPACE_OPEN_CHOICE: &str = "workspace-open";
const WORKSPACE_DELETE_CHOICE: &str = "workspace-delete";
const WORKSPACE_CANCEL_CHOICE: &str = "workspace-cancel";
/// Longest workspace name a confirmation quotes.
const CONFIRMATION_NAME_CHARS: usize = 40;

impl App {
    /// Save this window as the person asked, and say how it went.
    pub(super) fn force_save_workspace(&mut self, mode: WorkspaceSave) {
        match self.save_workspace(mode) {
            Ok(report) => {
                let suffix = if report.truncated_panes.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; scrollback was shortened in {} to fit {}",
                        plural(report.truncated_panes.len(), "pane", "panes"),
                        saved_scrollback_limits()
                    )
                };
                self.toasts
                    .success(format!("Saved workspace {}{}", report.id, suffix));
            }
            Err(error) => self
                .toasts
                .error(format!("Workspace save failed: {error:#}")),
        }
    }

    /// Save this window into its own workspace (for `Update`) or a new one,
    /// which it then belongs to. A workspace saved by the person is theirs:
    /// an automatic "Last workspace" saved this way takes the name a new
    /// save would, and is never pruned.
    pub(super) fn save_workspace(
        &mut self,
        mode: WorkspaceSave,
    ) -> anyhow::Result<crate::workspace::WorkspaceSaveReport> {
        let target = match mode {
            WorkspaceSave::Update => self.current_save_target(),
            WorkspaceSave::New => None,
        };
        let name = match &target {
            Some(target) if !target.automatic => target.name.clone(),
            _ => self.new_workspace_name(),
        };
        let (id, report) = self.save_window_into(name, target.as_ref(), false)?;
        if let Err(error) = self.remember_workspace(&id) {
            log::warn!(
                "Workspace was saved, but its last-workspace reference could not be persisted: {error:#}"
            );
        }
        Ok(report)
    }

    /// The name a new workspace takes from the active tab.
    fn new_workspace_name(&self) -> String {
        self.tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .map(|tab| {
                const SUFFIX: &str = " workspace";
                let title = crate::safe_text::truncate_with_ellipsis(
                    &tab.title,
                    crate::workspace::MAX_WORKSPACE_NAME_CHARS - SUFFIX.len(),
                );
                format!("{title}{SUFFIX}")
            })
            .unwrap_or_else(|| "Termeric workspace".into())
    }

    /// The workspace this window belongs to, while its archive still reads.
    fn current_save_target(&self) -> Option<SaveTarget> {
        let id = self.current_workspace_id.as_deref()?;
        match crate::workspace::load_workspace(id) {
            Ok(report) => Some(SaveTarget {
                id: report.workspace.id,
                name: report.workspace.name,
                created_at: report.workspace.created_at,
                automatic: report.workspace.automatic,
            }),
            Err(error) => {
                log::warn!("Workspace {id:?} could not be read; saving a new one: {error:#}");
                None
            }
        }
    }

    /// Capture this window and write it over `target`, keeping its ID and
    /// creation time, or as a new workspace. Every save of a window, the
    /// person's or automatic, goes through here. Returns the saved ID.
    fn save_window_into(
        &self,
        name: String,
        target: Option<&SaveTarget>,
        automatic: bool,
    ) -> anyhow::Result<(String, crate::workspace::WorkspaceSaveReport)> {
        let mut workspace = self.capture_workspace(name)?;
        if let Some(target) = target {
            workspace.id = target.id.clone();
            workspace.created_at = target.created_at.clone();
        }
        workspace.automatic = automatic;
        let report = crate::workspace::save_workspace(&mut workspace)?;
        Ok((workspace.id, report))
    }

    /// Whether closing this window saves it, as **Auto Save Workspace**
    /// asks.
    pub(super) fn auto_saves(&self) -> bool {
        self.automatic_persistence && self.config.session.enabled && self.config.session.auto_save
    }

    pub(super) fn auto_save_workspace(&mut self) {
        if !self.auto_saves() {
            return;
        }
        // A window closed while others stay open is not restored. Unless it
        // belongs to a saved workspace, an archive of it would only pile up.
        if !self.retain_workspace_on_exit && self.current_workspace_id.is_none() {
            return;
        }
        if let Err(error) = self.save_back_to_current_workspace() {
            log::warn!("Automatic workspace save failed: {error:#}");
        }
    }

    /// Save this window back into the workspace it belongs to, as closing
    /// it does, or into a new automatic "Last workspace".
    pub(super) fn save_back_to_current_workspace(&mut self) -> anyhow::Result<()> {
        let target = self.current_save_target();
        self.save_back_into(target)
    }

    /// [`Self::save_back_to_current_workspace`] into `target`, the window's
    /// workspace as already read.
    fn save_back_into(&mut self, target: Option<SaveTarget>) -> anyhow::Result<()> {
        let (name, automatic) = target.as_ref().map_or_else(
            || (AUTOMATIC_WORKSPACE_NAME.to_owned(), true),
            |target| (target.name.clone(), target.automatic),
        );
        let (id, _) = self.save_window_into(name, target.as_ref(), automatic)?;
        let state_result = if self.retain_workspace_on_exit {
            self.remember_workspace(&id)
        } else {
            self.current_workspace_id = Some(id.clone());
            self.app_state.forget_workspace(&id)
        };
        if let Err(error) = state_result {
            log::warn!("Could not update automatic workspace state: {error:#}");
        }
        Ok(())
    }

    fn capture_workspace(&self, name: String) -> anyhow::Result<WorkspaceSnapshot> {
        let tabs = self
            .tabs
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("tab manager is unavailable"))?;
        if tabs.has_live_domain_tabs() {
            anyhow::bail!(
                "live domains already preserve processes and layout; relay tabs cannot be saved as snapshot workspaces"
            );
        }
        tabs.capture_workspace(
            name,
            self.config.session.save_scrollback,
            WorkspaceConfigReference {
                theme: Some(self.config.theme.name.clone()),
                shell_program: self.config.shell.program.clone(),
            },
        )
    }

    fn remember_workspace(&mut self, id: &str) -> anyhow::Result<()> {
        if let Some(previous) = self.current_workspace_id.replace(id.to_string())
            && previous != id
            && let Err(error) = self.app_state.forget_workspace(&previous)
        {
            log::warn!(
                "Could not remove replaced workspace {previous:?} from startup state: {error:#}"
            );
        }
        self.app_state.remember_workspace(id)
    }

    pub(super) fn restore_saved_workspace(&mut self, id: &str) -> bool {
        match self.restore_workspace_archive(id) {
            Ok(outcome) => {
                for warning in &outcome.warnings {
                    log::warn!("Workspace restore: {warning}");
                }
                if outcome.warnings.is_empty() {
                    self.toasts.success(format!(
                        "Restored workspace ({})",
                        plural(outcome.restored_tabs, "tab", "tabs")
                    ));
                } else {
                    self.toasts.warning(format!(
                        "Restored {}; {} attention",
                        plural(outcome.restored_tabs, "tab", "tabs"),
                        plural(outcome.warnings.len(), "item needs", "items need")
                    ));
                }
                true
            }
            Err(error) => {
                log::error!("Workspace restore failed: {error:#}");
                self.toasts
                    .error(format!("Workspace restore failed: {error:#}"));
                false
            }
        }
    }

    pub(super) fn restore_workspace_archive(
        &mut self,
        id: &str,
    ) -> anyhow::Result<WorkspaceRestoreOutcome> {
        if self.is_live_domain_window() {
            anyhow::bail!("a live-domain window holds only its domain's tabs");
        }
        let report = crate::workspace::load_workspace(id)?;
        let original = self
            .tabs
            .take()
            .ok_or_else(|| anyhow::anyhow!("tab manager is unavailable"))?;
        let mut replacement = original.empty_like();
        let mut outcome = WorkspaceRestoreOutcome {
            warnings: report.warnings,
            ..WorkspaceRestoreOutcome::default()
        };
        if let Some(theme) = report.workspace.config.theme.as_deref()
            && theme != self.config.theme.name
        {
            outcome.warnings.push(format!(
                "workspace references theme {theme:?}; kept current theme {:?}",
                self.config.theme.name
            ));
        }
        if let Some(shell) = report.workspace.config.shell_program.as_deref()
            && self.config.shell.program.as_deref() != Some(shell)
        {
            outcome.warnings.push(format!(
                "workspace references shell {shell:?}; relaunched with the current shell configuration"
            ));
        }
        if report.recovered_from_backup {
            outcome
                .warnings
                .push("workspace metadata was recovered from its backup".into());
        }

        // Restored tabs by their runtime IDs: a pinned tab restored after an
        // unpinned one moves into the pinned group, shifting earlier indices.
        let mut restored_ids = vec![None; report.workspace.tabs.len()];
        for (source_index, snapshot) in report.workspace.tabs.iter().enumerate() {
            match self.restore_workspace_tab(&mut replacement, snapshot) {
                Ok((index, warnings)) => {
                    restored_ids[source_index] = Some(replacement.all_tabs()[index].id);
                    outcome.restored_tabs += 1;
                    outcome.warnings.extend(warnings);
                }
                Err(error) => {
                    outcome.skipped_tabs += 1;
                    let launch = snapshot
                        .launch()
                        .map_or_else(|_| "unknown launch".into(), ToString::to_string);
                    outcome.warnings.push(format!(
                        "tab {} ({launch}) was skipped: {error:#}",
                        snapshot.id
                    ));
                }
            }
        }

        if outcome.restored_tabs == 0 {
            self.tabs = Some(original);
            anyhow::bail!("none of the workspace's tabs could be relaunched");
        }
        if let Some(index) = restored_ids
            .get(report.workspace.active_tab)
            .copied()
            .flatten()
            .and_then(|id| replacement.all_tabs().iter().position(|tab| tab.id == id))
        {
            replacement.set_active_workspace_tab(index);
        } else {
            outcome
                .warnings
                .push("the saved focused tab was unavailable; focused the first tab".into());
            replacement.set_active_workspace_tab(0);
        }
        // The old tabs' conversations end with them.
        self.retire_agent_view_tabs();
        self.tabs = Some(replacement);
        if let Err(error) = self.remember_workspace(id) {
            outcome.warnings.push(format!(
                "workspace restored, but startup state could not remember it: {error:#}"
            ));
        }
        self.refresh_focused_terminal_surfaces();
        self.request_redraw(RedrawReason::Action);
        Ok(outcome)
    }

    pub(super) fn restore_workspace_tab(
        &mut self,
        tabs: &mut TabManager,
        snapshot: &WorkspaceTabSnapshot,
    ) -> anyhow::Result<(usize, Vec<String>)> {
        snapshot.validate()?;
        let count = tabs.count();
        let active = tabs.active_index();
        match self.restore_workspace_tab_inner(tabs, snapshot) {
            Ok((index, warnings)) => Ok((tabs.settle_restored_workspace_tab(index), warnings)),
            Err(error) => {
                tabs.rollback_workspace_tab(count, active);
                Err(error)
            }
        }
    }

    fn restore_workspace_tab_inner(
        &mut self,
        tabs: &mut TabManager,
        snapshot: &WorkspaceTabSnapshot,
    ) -> anyhow::Result<(usize, Vec<String>)> {
        // Resolve every identity before spawning any PTY for this tab.
        let pane_launches = self.resolve_workspace_pane_launches(snapshot, tabs)?;
        match snapshot.launch()? {
            WorkspaceLaunch::Shell => tabs.new_shell_tab_from_workspace(snapshot, &pane_launches),
            WorkspaceLaunch::Profile { name } => {
                let profile = self.config.launch_profile(name)?.clone();
                if let Some(serial) = profile.serial_config() {
                    if snapshot.panes.len() != 1 {
                        anyhow::bail!(
                            "serial terminal profile {name:?} cannot restore a split pane layout"
                        );
                    }
                    tabs.new_tab_with_serial_profile(name, serial, Some(&snapshot.title))?;
                    let index = tabs
                        .last_tab_index()
                        .ok_or_else(|| anyhow::anyhow!("serial tab was not created"))?;
                    let warnings = tabs.apply_workspace_layout(index, snapshot, &pane_launches)?;
                    Ok((index, warnings))
                } else {
                    tabs.new_profile_tab_from_workspace(
                        snapshot,
                        name,
                        profile.shell_config(),
                        &pane_launches,
                    )
                }
            }
            WorkspaceLaunch::Plugin { plugin_id, .. } => {
                let mut warnings = Vec::new();
                let cwd = crate::panes::usable_primary_workspace_cwd(snapshot, &mut warnings)?;
                let (_, shell) = &pane_launches[&snapshot.primary_pane];
                tabs.new_tab_with_command(
                    shell.program.as_deref().expect("validated plugin program"),
                    &shell.args,
                    cwd.as_deref(),
                    Some(&snapshot.title),
                    Some(TabType::Plugin {
                        plugin_id: plugin_id.clone(),
                    }),
                    shell
                        .env
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                )
                .map_err(|error| {
                    anyhow::anyhow!("required plugin executable is missing or unusable: {error:#}")
                })?;
                let index = tabs
                    .last_tab_index()
                    .ok_or_else(|| anyhow::anyhow!("plugin tab was not created"))?;
                warnings.extend(tabs.apply_workspace_layout(index, snapshot, &pane_launches)?);
                Ok((index, warnings))
            }
        }
    }

    fn resolve_workspace_pane_launches(
        &mut self,
        snapshot: &WorkspaceTabSnapshot,
        tabs: &TabManager,
    ) -> anyhow::Result<crate::panes::WorkspacePaneLaunches> {
        let mut launches = crate::panes::WorkspacePaneLaunches::new();
        for pane in &snapshot.panes {
            let launch = &pane.launch;
            let (resolved, shell) = match launch {
                WorkspaceLaunch::Shell => (launch.clone(), self.config.shell.clone()),
                WorkspaceLaunch::Profile { name } => {
                    let profile = self.config.launch_profile(name)?;
                    if profile.serial_config().is_some() {
                        if pane.id != snapshot.primary_pane || snapshot.panes.len() != 1 {
                            anyhow::bail!(
                                "workspace tab {} pane {} cannot clone serial profile {name:?}",
                                snapshot.id,
                                pane.id
                            );
                        }
                        (launch.clone(), crate::config::ShellConfig::default())
                    } else {
                        (launch.clone(), profile.shell_config())
                    }
                }
                WorkspaceLaunch::Plugin { plugin_id, profile } => {
                    // The normal workspace cwd fallback is reported by the
                    // owner constructor. Never borrow a sibling/focused cwd.
                    let cwd = pane
                        .working_directory
                        .as_ref()
                        .filter(|cwd| std::path::Path::new(cwd).is_dir())
                        .cloned();
                    let ctx = PluginContext {
                        window_id: self.control_window_id,
                        cwd: cwd.clone(),
                        tab_count: tabs.count(),
                        active_tab: tabs.active_index(),
                        active_tab_id: tabs.active_tab_id(),
                        terminal_size: (self.cols, self.rows),
                    };
                    self.resolve_plugin_terminal_launch(
                        &crate::plugin::Hook::WorkspaceRestore {
                            plugin_id: plugin_id.clone(),
                            profile: profile.clone(),
                            cwd,
                        },
                        &ctx,
                    )
                    .map_err(|error| {
                        anyhow::anyhow!("workspace tab {} pane {}: {error:#}", snapshot.id, pane.id)
                    })?
                }
            };
            launches.insert(pane.id, (resolved, shell));
        }
        Ok(launches)
    }
}

impl App {
    /// Whether this window is attached to a live domain. Its tabs are the
    /// domain's relays, which keep their processes and layout in the domain,
    /// so a local tab here could be neither saved nor restored.
    pub(super) fn is_live_domain_window(&self) -> bool {
        self.launch.domain.is_some()
    }

    /// Why no tab from another window may move into this one: a
    /// live-domain window holds only its domain's tabs, and a local tab
    /// there could be neither saved nor restored. Domain tabs never leave
    /// their window, which their tab manager refuses.
    pub(super) fn live_domain_tab_move_refusal(&self) -> Option<&'static str> {
        self.is_live_domain_window()
            .then_some("a live-domain window holds only its domain's tabs")
    }

    /// Open workspace `id` in place of this window's tabs. A window that
    /// belongs to a named workspace and saves itself on close is first saved
    /// back into it, as closing it would. Otherwise, while any tab runs a
    /// program besides an idle shell, those tabs close only once the person
    /// agrees. An automatic "Last workspace" is no place to keep them: once
    /// the window opens another workspace, nothing reopens it, and the next
    /// start prunes it.
    pub(super) fn request_open_workspace(&mut self, id: &str) {
        if self.is_live_domain_window() {
            self.toasts.warning(
                "A live-domain window can't open a workspace; open it from a local window",
            );
            return;
        }
        let named_target = self
            .auto_saves()
            .then(|| self.current_save_target())
            .flatten()
            .filter(|target| !target.automatic);
        if let Some(target) = named_target {
            if let Err(error) = self.save_back_into(Some(target)) {
                self.toasts.error(format!(
                    "This window could not be saved, so the workspace was not opened: {error:#}"
                ));
                return;
            }
            self.restore_saved_workspace(id);
            return;
        }
        let tabs = self.tabs.as_ref().map_or(0, TabManager::count);
        let running = self.tabs.as_ref().map_or(0, |tabs| {
            tabs.iter()
                .filter(|(_, tab)| tab.has_running_program())
                .count()
        });
        if running == 0 {
            self.restore_saved_workspace(id);
            return;
        }
        let name = confirmation_name(id);
        let tabs = plural(tabs, "tab", "tabs");
        let close = format!("Close {tabs} and open {name}");
        let mut lines = vec![
            format!("Open {name} in this window?"),
            format!("It replaces the {tabs} here, and the programs running"),
            "in them end. Save the window first to keep them.".into(),
            String::new(),
        ];
        let mut choices = Vec::new();
        push_keyless_choice(&mut lines, &mut choices, WORKSPACE_OPEN_CHOICE, close);
        push_keyless_choice(&mut lines, &mut choices, WORKSPACE_CANCEL_CHOICE, "Cancel");
        let request_id = self
            .info_overlay
            .show_choices("Open Workspace", lines, choices);
        self.overlay_flow = Some(OverlayFlow::WorkspaceOpen {
            request_id,
            workspace_id: id.to_owned(),
        });
    }

    /// Ask before deleting workspace (or closed-tab snapshot) `id`, which
    /// can't be undone.
    pub(super) fn confirm_workspace_delete(&mut self, id: &str, closed_tab: bool) {
        let name = confirmation_name(id);
        let mut lines = vec![
            format!("Delete {name}?"),
            "Its layout and saved scrollback are removed from disk.".into(),
            "This can't be undone.".into(),
            String::new(),
        ];
        let mut choices = Vec::new();
        push_keyless_choice(&mut lines, &mut choices, WORKSPACE_DELETE_CHOICE, "Delete");
        push_keyless_choice(&mut lines, &mut choices, WORKSPACE_CANCEL_CHOICE, "Cancel");
        let request_id = self.info_overlay.show_choices(
            if closed_tab {
                "Delete Tab Snapshot"
            } else {
                "Delete Workspace"
            },
            lines,
            choices,
        );
        self.overlay_flow = Some(OverlayFlow::WorkspaceDelete {
            request_id,
            workspace_id: id.to_owned(),
            closed_tab,
        });
    }

    /// Answer an open or delete confirmation. Returns whether the choice was
    /// its.
    pub(super) fn handle_workspace_choice(&mut self, request_id: u64, choice_id: &str) -> bool {
        let pending = matches!(
            self.overlay_flow,
            Some(
                OverlayFlow::WorkspaceOpen { request_id: pending, .. }
                    | OverlayFlow::WorkspaceDelete { request_id: pending, .. }
            ) if pending == request_id
        );
        if !pending
            || !matches!(
                choice_id,
                WORKSPACE_OPEN_CHOICE | WORKSPACE_DELETE_CHOICE | WORKSPACE_CANCEL_CHOICE
            )
        {
            return false;
        }
        self.info_overlay.dismiss(request_id);
        match (self.overlay_flow.take(), choice_id) {
            (Some(OverlayFlow::WorkspaceOpen { workspace_id, .. }), WORKSPACE_OPEN_CHOICE) => {
                self.restore_saved_workspace(&workspace_id);
            }
            (
                Some(OverlayFlow::WorkspaceDelete {
                    workspace_id,
                    closed_tab,
                    ..
                }),
                WORKSPACE_DELETE_CHOICE,
            ) => self.delete_saved_workspace(&workspace_id, closed_tab),
            _ => {}
        }
        self.request_redraw(RedrawReason::Action);
        true
    }

    fn delete_saved_workspace(&mut self, id: &str, closed_tab: bool) {
        let noun = if closed_tab {
            "closed tab"
        } else {
            "workspace"
        };
        match crate::workspace::delete_workspace(id) {
            Ok(()) => {
                log::info!("Deleted {noun} '{id}'");
                self.toasts.success(format!("Deleted {noun} '{id}'"));
                if !closed_tab {
                    if self.current_workspace_id.as_deref() == Some(id) {
                        self.current_workspace_id = None;
                    }
                    if let Err(error) = self.app_state.forget_workspace(id) {
                        log::warn!("Could not clear deleted workspace from state: {error:#}");
                    }
                }
            }
            Err(error) => {
                log::error!("Failed to delete {noun} '{id}': {error:#}");
                self.toasts.error(format!("Delete failed: {error:#}"));
            }
        }
        if self.workspace_panel.is_open() {
            self.workspace_panel
                .refresh(crate::session::listing::WorkspaceListing::load(
                    self.workspace_panel.is_workspace_mode(),
                ));
        }
    }
}

/// Workspace `id`'s name, quoted and bounded for a confirmation.
fn confirmation_name(id: &str) -> String {
    let name = crate::workspace::describe_workspace(id)
        .map_or_else(|_| id.to_owned(), |workspace| workspace.name);
    format!(
        "\u{201c}{}\u{201d}",
        crate::safe_text::truncate_with_ellipsis(&name, CONFIRMATION_NAME_CHARS)
    )
}

/// The limits that shorten saved scrollback, for the notices that report it:
/// each pane keeps its newest lines up to a per-pane size, and the panes of
/// one workspace or closed tab share a total.
pub(super) fn saved_scrollback_limits() -> String {
    const MIB: usize = 1024 * 1024;
    format!(
        "{} MiB per pane and {} MiB in all",
        crate::workspace::MAX_WORKSPACE_SCROLLBACK_BYTES_PER_PANE / MIB,
        crate::workspace::MAX_WORKSPACE_SCROLLBACK_BYTES_TOTAL / MIB
    )
}
