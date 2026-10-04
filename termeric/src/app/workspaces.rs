//! Complete workspace persistence and relaunch orchestration.

use super::*;
use crate::tabs::TabType;
use crate::workspace::{
    WorkspaceConfigReference, WorkspaceLaunch, WorkspaceRestoreOutcome, WorkspaceSnapshot,
    WorkspaceTabSnapshot,
};

impl App {
    pub(super) fn force_save_workspace(&mut self) {
        let name = self
            .tabs
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
            .unwrap_or_else(|| "Termeric workspace".into());
        match self.save_workspace_as(name) {
            Ok(report) => {
                let suffix = if report.truncated_panes.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; {} pane(s) had bounded scrollback",
                        report.truncated_panes.len()
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

    pub(super) fn save_workspace_as(
        &mut self,
        name: String,
    ) -> anyhow::Result<crate::workspace::WorkspaceSaveReport> {
        let mut workspace = self.capture_workspace(name)?;
        let report = crate::workspace::save_workspace(&mut workspace)?;
        if let Err(error) = self.remember_workspace(&workspace.id) {
            log::warn!(
                "Workspace was saved, but its last-workspace reference could not be persisted: {error:#}"
            );
        }
        Ok(report)
    }

    pub(super) fn auto_save_workspace(&mut self) {
        if !self.automatic_persistence
            || !self.config.session.enabled
            || !self.config.session.auto_save
        {
            return;
        }
        // A window closed while others stay open is not restored. Unless it
        // belongs to a saved workspace, an archive of it would only pile up.
        if !self.retain_workspace_on_exit && self.current_workspace_id.is_none() {
            return;
        }
        let existing = self
            .current_workspace_id
            .as_deref()
            .and_then(|id| crate::workspace::load_workspace(id).ok())
            .map(|report| report.workspace);
        let name = existing
            .as_ref()
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| "Last workspace".into());
        let mut workspace = match self.capture_workspace(name) {
            Ok(workspace) => workspace,
            Err(error) => {
                log::warn!("Automatic workspace capture failed: {error:#}");
                return;
            }
        };
        match existing {
            Some(existing) => {
                workspace.id = existing.id;
                workspace.created_at = existing.created_at;
                workspace.automatic = existing.automatic;
            }
            None => workspace.automatic = true,
        }
        match crate::workspace::save_workspace(&mut workspace) {
            Ok(_) => {
                let state_result = if self.retain_workspace_on_exit {
                    self.remember_workspace(&workspace.id)
                } else {
                    self.current_workspace_id = Some(workspace.id.clone());
                    self.app_state.forget_workspace(&workspace.id)
                };
                if let Err(error) = state_result {
                    log::warn!("Could not update automatic workspace state: {error:#}");
                }
            }
            Err(error) => log::warn!("Automatic workspace save failed: {error:#}"),
        }
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
                        "Restored workspace ({0} {1})",
                        outcome.restored_tabs,
                        if outcome.restored_tabs == 1 {
                            "tab"
                        } else {
                            "tabs"
                        }
                    ));
                } else {
                    self.toasts.warning(format!(
                        "Restored {} tab(s); {} item(s) need attention",
                        outcome.restored_tabs,
                        outcome.warnings.len()
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
                let primary = snapshot
                    .panes
                    .iter()
                    .find(|pane| pane.id == snapshot.primary_pane);
                let cwd = crate::panes::usable_workspace_cwd(
                    primary.and_then(|pane| pane.working_directory.as_deref()),
                    snapshot.id,
                    snapshot.primary_pane,
                    &mut warnings,
                );
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
    ) -> anyhow::Result<std::collections::HashMap<u32, (WorkspaceLaunch, crate::config::ShellConfig)>>
    {
        let mut launches = std::collections::HashMap::new();
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
