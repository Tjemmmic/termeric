//! Launch-first restart of an exact plugin owner; unrelated panes survive.
use super::{App, Requester};
use crate::plugin::{Hook, PluginAction};
use crate::tabs::{TabType, TerminalIdentity};

impl App {
    /// Launch a replacement for the exact `target` terminal owned by
    /// `plugin_id`, then retire it.
    pub(super) fn restart_plugin_at(&mut self, plugin_id: &str, target: TerminalIdentity) {
        let Some(tabs) = self.tabs.as_ref() else {
            return;
        };
        let Some(tab) = tabs
            .tab_by_id(target.tab_id)
            .filter(|tab| tab.pane_plugin_id(target.pane_id) == Some(plugin_id))
        else {
            self.toasts.warning(format!(
                "Cannot restart {plugin_id}: its terminal has closed"
            ));
            return;
        };
        let origin = tabs
            .origin_for_pane(target.tab_id, target.pane_id)
            .expect("a live plugin owner has a route");
        if tab.live_domain.is_some() || tabs.origin_read_only(origin) != Some(false) {
            self.toasts.warning(format!(
                "Cannot restart {plugin_id}: its terminal is read-only or belongs to a live domain"
            ));
            return;
        }
        let saved_profile = tab
            .pane_plugin_launch(target.pane_id)
            .and_then(|(_, profile)| profile.map(str::to_owned));
        let mut ctx = self.build_plugin_context();
        ctx.cwd = tab.pane_working_directory(target.pane_id);
        ctx.active_tab = tabs
            .iter()
            .find_map(|(index, tab)| (tab.id == target.tab_id).then_some(index))
            .expect("resolved plugin tab index");
        if let Err(error) = self.validate_plugin_launch_profile(plugin_id, saved_profile.as_deref())
        {
            self.toasts.warning(format!(
                "Cannot restart {plugin_id}; existing terminals were kept: {error}"
            ));
            return;
        }
        let action = self.plugin_manager.dispatch_to(
            plugin_id,
            &Hook::WorkspaceRestore {
                plugin_id: plugin_id.into(),
                profile: saved_profile.clone(),
                cwd: ctx.cwd.clone(),
            },
            &ctx,
        );
        // A notice explains why there is no launch; show it, then refuse.
        if let Some(notice @ PluginAction::ShowNotice { .. }) = action {
            self.execute_plugin_action(notice);
            self.toasts.warning(format!("Cannot restart {plugin_id}: no valid replacement launch; existing terminals were kept"));
            return;
        }
        let launch = match self.check_plugin_terminal_launch(
            plugin_id,
            saved_profile.as_deref(),
            ctx.cwd.as_ref(),
            action,
        ) {
            Ok(launch) => launch,
            Err(error) => {
                self.toasts.warning(format!(
                    "Cannot restart {plugin_id}: no valid replacement launch; existing terminals were kept: {error:#}"
                ));
                return;
            }
        };
        let result = self
            .tabs
            .as_mut()
            .expect("validated tab manager")
            .new_tab_with_command(
                launch
                    .shell
                    .program
                    .as_deref()
                    .expect("a checked plugin launch names its program"),
                &launch.shell.args,
                ctx.cwd.as_deref(),
                launch.title.as_deref(),
                Some(TabType::Plugin {
                    plugin_id: plugin_id.into(),
                }),
                launch.shell.env.into_iter().collect(),
            );
        let replacement = match result {
            Ok(id) => id,
            Err(error) => {
                self.toasts.error(format!(
                    "Could not restart {plugin_id}; existing terminals were kept: {error}"
                ));
                return;
            }
        };
        {
            let tab = self
                .tabs
                .as_mut()
                .unwrap()
                .tab_mut_by_id(replacement)
                .unwrap();
            tab.set_plugin_profile_reference(tab.panes.focused_id(), plugin_id, launch.profile);
        }
        if let Err(error) = self.retire_restarted_plugin_owner(target, plugin_id) {
            self.toasts.warning(format!(
                "Opened a new {plugin_id} terminal, but kept the original: {error}"
            ));
        }
        self.refresh_focused_terminal_surfaces();
        // Run callbacks only after the exact retirement decision. They must not
        // change which existing process this restart is authorized to close.
        self.dispatch_plugin_hook(Hook::TabCreated {
            tab_id: replacement,
        });
    }

    fn retire_restarted_plugin_owner(
        &mut self,
        identity: TerminalIdentity,
        plugin_id: &str,
    ) -> Result<(), String> {
        let tabs = self.tabs.as_ref().ok_or("tab manager disappeared")?;
        let tab = tabs
            .tab_by_id(identity.tab_id)
            .ok_or("original tab disappeared")?;
        if tab.pane_plugin_id(identity.pane_id) != Some(plugin_id) {
            return Err("original plugin owner changed".into());
        }
        if tab.panes.pane_count() > 1 {
            self.close_pane_for(
                Requester::Plugin,
                identity.tab_id as u64,
                identity.pane_id.0,
            )?;
        } else {
            let index = tabs
                .iter()
                .find_map(|(index, tab)| (tab.id == identity.tab_id).then_some(index))
                .ok_or("original tab disappeared")?;
            if !self.tabs.as_mut().unwrap().close_tab(index) {
                return Err("original tab could not be closed".into());
            }
        }
        Ok(())
    }
}
