//! Session startup, splash, and persistence feedback orchestration.

use super::*;
use anyhow::Context;

impl App {
    /// Initialize this window from explicit CLI controls, the referenced last
    /// workspace, or one default shell (in that precedence order).
    pub(super) fn init_tabs(&mut self) -> Result<()> {
        let geometry = self
            .renderer
            .as_ref()
            .zip(self.window.as_ref())
            .map(|(renderer, window)| {
                let size = window.inner_size();
                renderer.calculate_grid_geometry(size.width, size.height)
            })
            .unwrap_or_else(|| {
                crate::terminal::geometry::TerminalGeometry::from_cells(
                    self.cols, self.rows, 1.0, 1.0,
                )
            });
        let mut tabs = TabManager::new_with_geometry(
            geometry,
            self.config.shell.clone(),
            self.config.scrollback.lines,
            self.config.scrollback.max_memory_bytes(),
        );
        tabs.set_screen_checksum_reporting(self.config.security.allow_screen_checksum_reports);
        tabs.set_kitty_file_transmission(self.config.security.allow_kitty_file_transfers);
        tabs.set_xterm_keyboard_type(self.config.keyboard.xterm_type.terminal_type(), true);
        self.tabs = Some(tabs);

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
            && let Some(workspace_id) = self.app_state.last_workspace_id.clone()
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
                        .new_tab_with_command(
                            program,
                            args,
                            cwd.as_deref(),
                            spec.title.as_deref(),
                            None,
                            std::collections::HashMap::new(),
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
                        launch: crate::workspace::WorkspaceLaunch::Plugin {
                            plugin_id: plugin_id.clone(),
                            profile: Some(profile.clone()),
                        },
                        primary_pane: 0,
                        focused_pane: 0,
                        layout: crate::workspace::WorkspacePaneLayout::Pane { pane: 0 },
                        panes: vec![crate::workspace::WorkspacePaneSnapshot {
                            id: 0,
                            launch: Some(crate::workspace::WorkspaceLaunch::Plugin {
                                plugin_id: plugin_id.clone(),
                                profile: Some(profile.clone()),
                            }),
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
            for split in &spec.splits {
                let direction = match split {
                    crate::cli::CliSplitDirection::Horizontal => SplitDirection::Horizontal,
                    crate::cli::CliSplitDirection::Vertical => SplitDirection::Vertical,
                };
                let before = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.active_tab())
                    .and_then(|tab| tab.pane_manager.as_ref())
                    .map_or(1, PaneManager::pane_count);
                self.split_pane(direction);
                let after = self
                    .tabs
                    .as_ref()
                    .and_then(|tabs| tabs.active_tab())
                    .and_then(|tab| tab.pane_manager.as_ref())
                    .map_or(1, PaneManager::pane_count);
                if after != before + 1 {
                    anyhow::bail!("could not create requested {split:?} pane");
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
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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

    /// Unconditionally save the current session (explicit user action)
    pub(super) fn force_save_session(&mut self) {
        self.save_sessions(true, false);
    }

    fn save_sessions(&mut self, include_empty: bool, automatic: bool) {
        let Some(tabs) = &mut self.tabs else { return };
        if tabs.has_live_domain_tabs() {
            if !automatic {
                self.toasts.warning(
                    "Live-domain relay tabs cannot be saved as relaunchable session snapshots",
                );
            }
            return;
        }
        let save_scrollback = self.config.session.save_scrollback;
        tabs.save_all_sessions(save_scrollback, include_empty);
        let notices = tabs.drain_persistence_notices();
        self.show_persistence_notices(notices, automatic);
    }

    pub(super) fn show_persistence_notices(
        &mut self,
        notices: Vec<crate::tabs::PersistenceNotice>,
        automatic: bool,
    ) {
        if notices.is_empty() {
            if !automatic {
                self.toasts.info("No legacy snapshot content to save");
            }
            return;
        }
        let mut saved = 0;
        let mut saved_focused_pane = false;
        for notice in notices {
            match notice {
                crate::tabs::PersistenceNotice::Saved {
                    id,
                    truncated,
                    capture_scope,
                } => {
                    saved += 1;
                    saved_focused_pane |=
                        capture_scope == crate::session::SessionCaptureScope::FocusedPane;
                    if truncated {
                        self.toasts.warning(format!(
                            "Session {id} saved, but scrollback exceeded 5 MB and was truncated"
                        ));
                    }
                }
                crate::tabs::PersistenceNotice::SaveFailed { tab_id, error } => {
                    self.toasts
                        .error(format!("Could not save tab {}: {}", tab_id + 1, error));
                }
            }
        }
        if saved > 0 && !automatic {
            let label = if saved == 1 { "snapshot" } else { "snapshots" };
            let scope = if saved_focused_pane {
                "; split tabs captured their focused pane only"
            } else {
                ""
            };
            self.toasts.success(format!("Saved {saved} {label}{scope}"));
        }
    }

    /// Restore every interactive session entry point through the same loader,
    /// compatibility checks, tab construction, and user-facing semantics.
    pub(super) fn restore_saved_session(&mut self, name: &str) {
        let result = self
            .tabs
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("tab manager is unavailable"))
            .and_then(|tabs| restore_session_into(tabs, name));
        match result {
            Ok(Some(scope)) => {
                log::info!("Loaded session '{}'", name);
                self.toasts.success(restored_session_message(name, scope));
            }
            Ok(None) => {
                log::warn!("Session '{}' not found", name);
                self.toasts.warning(format!("Session '{}' not found", name));
            }
            Err(error) => {
                log::error!("Failed to load session: {error:#}");
                self.toasts.error(format!("Load failed: {error:#}"));
            }
        }
    }
}

fn restore_session_into(
    tabs: &mut TabManager,
    name: &str,
) -> anyhow::Result<Option<crate::session::SessionCaptureScope>> {
    let Some(session) = crate::session::restore::restore_named_session(name)? else {
        return Ok(None);
    };
    let scope = session.capture_scope;
    tabs.new_tab_from_session(&session)?;
    Ok(Some(scope))
}

fn restored_session_message(name: &str, scope: crate::session::SessionCaptureScope) -> String {
    let source = match scope {
        crate::session::SessionCaptureScope::SingleTerminal => "session snapshot",
        crate::session::SessionCaptureScope::FocusedPane => "focused-pane snapshot",
    };
    format!("Opened new shell from {source} '{name}' (metadata and text history only)")
}
