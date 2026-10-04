//! Session startup, splash, and persistence feedback orchestration.

use super::*;
use anyhow::Context;

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
        let tabs = TabManager::new_with_area(
            area,
            self.config.shell.clone(),
            crate::terminal::policy::TerminalPolicy::from_config(&self.config),
        );
        self.tabs = Some(tabs);
        if self.launch.adopts_tab {
            // The torn-off tab moves in once the window exists.
            self.dispatch_plugin_hook(crate::plugin::Hook::AppReady);
            return Ok(());
        }

        let explicit_launch = self.launch != crate::cli::WindowLaunch::default();
        let mut restored = false;
        if let Some(target) = self.launch.domain.clone() {
            let domain_id = target.domain_id;
            let domain = if let Some(domain) = self.launch.domain_snapshot.take() {
                if domain.id != domain_id {
                    anyhow::bail!("prepared domain snapshot does not match its launch target");
                }
                domain
            } else {
                let response = target.authority.client()?.request(
                    crate::domain::protocol::Request::Describe {
                        domain: domain_id.clone(),
                    },
                )?;
                let crate::domain::protocol::Response::Domain { domain } = response else {
                    anyhow::bail!("domain server returned an invalid describe response");
                };
                domain
            };
            let executable = std::env::current_exe().context("locate Termeric relay executable")?;
            let executable = executable
                .to_str()
                .context("Termeric relay executable path must be valid UTF-8")?
                .to_string();
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
            self.start_live_domain_topology_subscription(
                target.authority.clone(),
                domain_id.clone(),
            )?;
            // A live domain is not a snapshot workspace. Never auto-save relay
            // processes as if they were relaunchable shell sessions.
            self.automatic_persistence = false;
            restored = true;
            log::info!(
                "Attached window to persistent domain {domain_id} via {:?}",
                target.authority
            );
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

    fn start_live_domain_topology_subscription(
        &mut self,
        authority: crate::domain::DomainAuthority,
        domain_id: String,
    ) -> Result<()> {
        let window_id = self
            .window
            .as_ref()
            .map(|window| window.id())
            .context("live-domain topology requires a native window")?;
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
                                .send_event(AppEvent::LiveDomainTopology {
                                    window_id,
                                    domain_id: domain_id.clone(),
                                    result: Ok(event),
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
                            .send_event(AppEvent::LiveDomainTopology {
                                window_id,
                                domain_id: domain_id.clone(),
                                result: Err(format!("{error:#}")),
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

    fn launch_cli_tabs(&mut self) -> Result<()> {
        let tab_specs = self.launch.tabs.clone();
        for (tab_index, spec) in tab_specs.iter().enumerate() {
            let cwd = spec
                .working_directory
                .as_deref()
                .map(|path| {
                    if !path.is_dir() {
                        anyhow::bail!("working directory {} does not exist", path.display());
                    }
                    path.to_str()
                        .map(String::from)
                        .context("working directory must be valid UTF-8")
                })
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
                                if !path.is_dir() {
                                    anyhow::bail!(
                                        "terminal profile {name:?} working directory {} does not exist",
                                        path.display()
                                    );
                                }
                                path.to_str().map(String::from).context(
                                    "terminal profile working directory must be valid UTF-8",
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
                let before = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.active_tab())
                    .map_or(1, |tab| tab.panes.pane_count());
                self.split_pane(direction);
                let after = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.active_tab())
                    .map_or(1, |tab| tab.panes.pane_count());
                if after != before + 1 {
                    anyhow::bail!("could not create requested {} pane", direction.name());
                }
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
                .map(|path| {
                    if !path.is_dir() {
                        anyhow::bail!("working directory {} does not exist", path.display());
                    }
                    path.to_str()
                        .map(String::from)
                        .context("working directory must be valid UTF-8")
                })
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
