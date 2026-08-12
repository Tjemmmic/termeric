//! Session startup, splash, and persistence feedback orchestration.

use super::*;

impl App {
    /// Initialize the tab manager with the first tab or restore from session
    pub(super) fn init_tabs(&mut self) -> Result<()> {
        let mut tabs = TabManager::new(
            self.cols,
            self.rows,
            self.config.shell.clone(),
            self.config.scrollback.lines,
        );

        let mut restored = false;
        if self.config.session.enabled
            && self.config.session.auto_restore
            && let Some(latest) = crate::session::list_sessions().into_iter().next()
        {
            match crate::session::restore::restore_named_session(&latest) {
                Ok(Some(session)) => match tabs.new_tab_from_session(&session) {
                    Ok(_) => {
                        restored = true;
                        self.toasts
                            .success(format!("Restored latest session '{}'", latest));
                    }
                    Err(error) => {
                        self.toasts
                            .error(format!("Latest session recovery failed: {error:#}"));
                    }
                },
                Ok(None) => {
                    self.toasts
                        .warning(format!("Latest session '{}' disappeared", latest));
                }
                Err(error) => {
                    self.toasts.error(format!(
                        "Latest session is corrupt or incompatible: {error:#}"
                    ));
                }
            }
        }
        if !restored {
            tabs.new_tab()?;
        }

        self.tabs = Some(tabs);

        // Show splash screen based on configuration
        if !restored {
            self.maybe_show_splash(false);
        }

        log::info!("Tab manager initialized: {}x{}", self.cols, self.rows);

        // Notify plugins that the app is ready
        self.dispatch_plugin_hook(crate::plugin::Hook::AppReady);

        Ok(())
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
                    show_splash(&mut tab.terminal);
                    log::debug!("Splash screen displayed");
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

    /// Save the current session to disk (auto-save path, respects config)
    pub(super) fn save_current_session(&mut self) {
        if !self.config.session.enabled || !self.config.session.auto_save {
            return;
        }

        self.force_save_session();
    }

    /// Unconditionally save the current session (explicit user action)
    pub(super) fn force_save_session(&mut self) {
        let Some(tabs) = &mut self.tabs else { return };
        let save_scrollback = self.config.session.save_scrollback;
        tabs.save_all_sessions(save_scrollback);
        let notices = tabs.drain_persistence_notices();
        self.show_persistence_notices(notices, false);
    }

    pub(super) fn show_persistence_notices(
        &mut self,
        notices: Vec<crate::tabs::PersistenceNotice>,
        automatic: bool,
    ) {
        if notices.is_empty() {
            if !automatic {
                self.toasts.info("No session content to save");
            }
            return;
        }
        let mut saved = 0;
        for notice in notices {
            match notice {
                crate::tabs::PersistenceNotice::Saved { id, truncated } => {
                    saved += 1;
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
            let label = if saved == 1 { "session" } else { "sessions" };
            self.toasts.success(format!("Saved {saved} {label}"));
        }
    }
}
