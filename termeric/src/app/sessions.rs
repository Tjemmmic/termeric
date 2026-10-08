//! Session startup, splash, and persistence feedback orchestration.

use super::*;
use anyhow::Context;

/// `path` as a new terminal's working directory: it must be an existing
/// directory, and valid UTF-8 because the PTY takes it as a string. `what`
/// names it in the error.
fn existing_utf8_dir(path: &std::path::Path, what: &str) -> Result<String> {
    if !path.is_dir() {
        anyhow::bail!("{what} {} does not exist", path.display());
    }
    path.to_str()
        .map(String::from)
        .with_context(|| format!("{what} must be valid UTF-8"))
}

impl App {
    /// Initialize this window from explicit CLI controls, the referenced last
    /// workspace, or one default shell (in that precedence order).
    pub(super) fn init_tabs(&mut self) -> Result<()> {
        let area = self
            .renderer
            .as_ref()
            .zip(self.window.as_ref())
            .map(|(renderer, window)| {
                let size = window.inner_size();
                renderer.terminal_area(size.width, size.height)
            })
            .unwrap_or_else(|| {
                crate::terminal::geometry::TerminalArea::new(
                    crate::terminal::geometry::TerminalGeometry::from_cells(
                        self.cols, self.rows, 1.0, 1.0,
                    ),
                    1.0,
                    1.0,
                )
            });
        let tabs =
            TabManager::new_with_area(area, self.config.shell.clone(), self.terminal_policy());
        self.tabs = Some(tabs);
        if self.launch.adopts_tab {
            // The torn-off tab moves in once the window exists.
            self.dispatch_plugin_hook(crate::plugin::Hook::AppReady);
            return Ok(());
        }

        let explicit_launch = self.launch != crate::cli::WindowLaunch::default();
        let mut restored = false;
        if let Some(target) = self.launch.domain.clone() {
            self.attach_live_domain_window(target)?;
            restored = true;
        } else if let Some(workspace_id) = self.launch.workspace.clone() {
            match self.restore_workspace_archive(&workspace_id) {
                Ok(_) => restored = true,
                Err(error) if self.allow_workspace_fallback => {
                    log::error!(
                        "Automatic workspace {workspace_id:?} could not be restored: {error:#}"
                    );
                    self.toasts.error(format!(
                        "Workspace {workspace_id} could not be restored; opened a shell"
                    ));
                    if let Err(state_error) = self.app_state.forget_workspace(&workspace_id) {
                        log::warn!(
                            "Could not remove failed workspace from startup state: {state_error:#}"
                        );
                    }
                }
                Err(error) => return Err(error),
            }
        } else if self.allow_auto_restore
            && !explicit_launch
            && self.config.session.enabled
            && self.config.session.auto_restore
            && let Some(workspace_id) = self.app_state.last_workspace_id().map(str::to_owned)
        {
            restored = self.restore_saved_workspace(&workspace_id);
        }
        if !restored {
            self.launch_cli_tabs()?;
        }

        // Show splash screen based on configuration
        if !restored && !explicit_launch {
            self.maybe_show_splash(false);
        }

        log::info!("Tab manager initialized: {}x{}", self.cols, self.rows);

        // Notify plugins that the app is ready
        self.dispatch_plugin_hook(crate::plugin::Hook::AppReady);

        Ok(())
    }

    fn launch_cli_tabs(&mut self) -> Result<()> {
        let tab_specs = self.launch.tabs.clone();
        for (tab_index, spec) in tab_specs.iter().enumerate() {
            let cwd = spec
                .working_directory
                .as_deref()
                .map(|path| existing_utf8_dir(path, "working directory"))
                .transpose()?;
            match &spec.target {
                crate::cli::LaunchTarget::Shell => {
                    self.tabs
                        .as_mut()
                        .context("tab manager was lost during startup")?
                        .new_tab_with_cwd(cwd.as_deref(), spec.title.as_deref())?;
                }
                crate::cli::LaunchTarget::Command { program, args } => {
                    self.tabs
                        .as_mut()
                        .context("tab manager was lost during startup")?
                        .launch_command_tab(
                            crate::tabs::CommandLaunch {
                                program: program.clone(),
                                args: args.clone(),
                                working_dir: cwd.clone(),
                                title: spec.title.clone(),
                                tab_type: None,
                                env: spec.environment.clone(),
                                environment_policy: spec.environment_policy.clone(),
                            },
                            false,
                        )?;
                }
                crate::cli::LaunchTarget::TerminalProfile { name } => {
                    let profile = self.config.launch_profile(name)?.clone();
                    if profile.serial_config().is_some() && cwd.is_some() {
                        anyhow::bail!(
                            "serial terminal profile {name:?} does not accept a working directory"
                        );
                    }
                    let profile_cwd = if let Some(cwd) = cwd {
                        Some(cwd)
                    } else {
                        profile
                            .cwd
                            .as_deref()
                            .map(|path| {
                                existing_utf8_dir(
                                    path,
                                    &format!("terminal profile {name:?} working directory"),
                                )
                            })
                            .transpose()?
                    };
                    let title = spec.title.as_deref().or(profile.title.as_deref());
                    let tabs = self
                        .tabs
                        .as_mut()
                        .context("tab manager was lost during startup")?;
                    if let Some(serial) = profile.serial_config() {
                        tabs.new_tab_with_serial_profile(name, serial, title)?;
                    } else {
                        tabs.new_tab_with_profile(
                            name,
                            profile.shell_config(),
                            profile_cwd.as_deref(),
                            title,
                        )?;
                    }
                }
                crate::cli::LaunchTarget::Plugin { plugin_id, profile } => {
                    let title = spec.title.clone().unwrap_or_else(|| plugin_id.clone());
                    let snapshot = crate::workspace::WorkspaceTabSnapshot {
                        id: tab_index as u32,
                        title: title.clone(),
                        user_title: spec.title.clone(),
                        pinned: false,
                        primary_pane: 0,
                        focused_pane: 0,
                        layout: crate::workspace::WorkspacePaneLayout::Pane { pane: 0 },
                        panes: vec![crate::workspace::WorkspacePaneSnapshot {
                            id: 0,
                            launch: crate::workspace::WorkspaceLaunch::Plugin {
                                plugin_id: plugin_id.clone(),
                                profile: Some(profile.clone()),
                            },
                            title: Some(title),
                            working_directory: cwd.clone(),
                            last_command: None,
                            scrollback_file: None,
                            scrollback_truncated: false,
                            scrollback_lines: Vec::new(),
                        }],
                    };
                    let mut tabs = self
                        .tabs
                        .take()
                        .context("tab manager was lost during plugin startup")?;
                    let restore_result = self.restore_workspace_tab(&mut tabs, &snapshot);
                    self.tabs = Some(tabs);
                    restore_result?;
                }
            }
            for &direction in &spec.splits {
                self.split_pane(direction).with_context(|| {
                    format!("could not create requested {} pane", direction.name())
                })?;
            }
        }
        Ok(())
    }

    pub(super) fn open_terminal_profile(&mut self, name: &str) {
        let result = (|| -> anyhow::Result<()> {
            let profile = self.config.launch_profile(name)?.clone();
            if let Some(serial) = profile.serial_config() {
                self.tabs
                    .as_mut()
                    .context("tab manager is unavailable")?
                    .new_tab_with_serial_profile(name, serial, profile.title.as_deref())?;
                return Ok(());
            }
            let cwd = profile
                .cwd
                .as_deref()
                .map(|path| existing_utf8_dir(path, "working directory"))
                .transpose()?;
            self.tabs
                .as_mut()
                .context("tab manager is unavailable")?
                .new_tab_with_profile(
                    name,
                    profile.shell_config(),
                    cwd.as_deref(),
                    profile.title.as_deref(),
                )?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.refresh_focused_terminal_surfaces();
                self.toasts
                    .success(format!("Opened terminal profile {name}"));
            }
            Err(error) => self.toasts.error(format!(
                "Terminal profile {name:?} could not start: {error:#}"
            )),
        }
        self.request_redraw(RedrawReason::Action);
    }

    /// Check splash mode and show splash if appropriate
    ///
    /// `is_new_tab` indicates whether this is a new tab (for NewTab mode)
    pub(super) fn maybe_show_splash(&mut self, is_new_tab: bool) {
        let should_show = match self.config.splash.mode {
            SplashMode::Never => false,
            SplashMode::FirstBoot => {
                // Only show if first boot hasn't been completed
                !self.app_state.first_boot_complete
            }
            SplashMode::NewWindow => {
                // Show for new windows (not tabs)
                !is_new_tab
            }
            SplashMode::NewTab => {
                // Show for every new tab (and new windows)
                true
            }
        };

        if should_show {
            if let Some(tabs) = &mut self.tabs {
                if let Some(tab) = tabs.active_tab_mut() {
                    if let Some(terminal) = tab.focused_terminal_mut() {
                        show_splash(terminal);
                        log::debug!("Splash screen displayed");
                    }
                }
            }

            // Mark first boot as complete if this was a FirstBoot splash
            if self.config.splash.mode == SplashMode::FirstBoot
                && !self.app_state.first_boot_complete
            {
                if let Err(e) = self.app_state.mark_first_boot_complete() {
                    log::warn!("Failed to save first boot state: {}", e);
                }
            }
        }
    }

    /// Report close-time snapshot saves. Successes are silent; only
    /// truncation and failures need the user's attention.
    pub(super) fn show_persistence_notices(
        &mut self,
        notices: Vec<crate::tabs::PersistenceNotice>,
    ) {
        for notice in notices {
            match notice {
                crate::tabs::PersistenceNotice::Saved { id, truncated } => {
                    if truncated {
                        self.toasts.warning(format!(
                            "Closed tab {id} saved, but a pane's scrollback exceeded 5 MB and was truncated"
                        ));
                    }
                }
                crate::tabs::PersistenceNotice::SaveFailed { tab_id, error } => {
                    self.toasts
                        .error(format!("Could not save tab {}: {}", tab_id + 1, error));
                }
            }
        }
    }

    /// Reopen a closed tab's snapshot as a new tab in this window, keeping
    /// the snapshot's ID and creation time so closing the tab again updates
    /// it.
    pub(super) fn restore_closed_tab(&mut self, id: &str) {
        let result = self.reopen_closed_tab(id);
        match result {
            Ok((name, warnings)) => {
                log::info!("Reopened closed tab '{id}'");
                self.toasts.success(format!("Reopened closed tab '{name}'"));
                for warning in warnings {
                    self.toasts.warning(warning);
                }
            }
            Err(error) => {
                log::error!("Failed to reopen closed tab '{id}': {error:#}");
                self.toasts.error(format!("Reopen failed: {error:#}"));
            }
        }
        self.refresh_focused_terminal_surfaces();
    }

    fn reopen_closed_tab(&mut self, id: &str) -> anyhow::Result<(String, Vec<String>)> {
        if self.is_live_domain_window() {
            anyhow::bail!("a live-domain window holds only its domain's tabs");
        }
        let report = crate::workspace::load_workspace(id)?;
        let workspace = report.workspace;
        let [tab] = workspace.tabs.as_slice() else {
            anyhow::bail!("closed-tab snapshot {id:?} does not hold exactly one tab");
        };
        let mut tabs = self
            .tabs
            .take()
            .ok_or_else(|| anyhow::anyhow!("tab manager is unavailable"))?;
        let restored = self.restore_workspace_tab(&mut tabs, tab);
        if let Ok((index, _)) = &restored {
            tabs.set_active_workspace_tab(*index);
            if let Some(tab) = tabs.active_tab_mut() {
                tab.saved_snapshot = Some(crate::tabs::SavedSnapshot {
                    id: workspace.id.clone(),
                    created_at: workspace.created_at.clone(),
                });
            }
        }
        self.tabs = Some(tabs);
        let (_, mut warnings) = restored?;
        warnings.extend(report.warnings);
        Ok((workspace.name, warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn a_working_directory_must_be_an_existing_utf8_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("roots");
        let what = "terminal profile \"dev\" working directory";
        let missing = existing_utf8_dir(&directory, what).unwrap_err();
        assert!(missing.to_string().starts_with(what), "{missing}");

        std::fs::write(&directory, b"").unwrap();
        assert!(
            existing_utf8_dir(&directory, what).is_err(),
            "a file is not a directory"
        );
        std::fs::remove_file(&directory).unwrap();

        std::fs::create_dir(&directory).unwrap();
        assert_eq!(
            existing_utf8_dir(&directory, what).unwrap(),
            directory.to_str().unwrap()
        );

        let not_utf8 = temp.path().join(std::ffi::OsStr::from_bytes(b"r\xffot"));
        std::fs::create_dir(&not_utf8).unwrap();
        let error = existing_utf8_dir(&not_utf8, what).unwrap_err();
        assert_eq!(error.to_string(), format!("{what} must be valid UTF-8"));
    }
}
