//! Launch-first restart of an exact plugin owner; unrelated panes survive.
use super::App;
use crate::plugin::{Hook, PluginAction};
use crate::tabs::{TabType, TerminalIdentity};

impl App {
    pub(super) fn restart_plugin(&mut self, plugin_id: &str) {
        let Some(tabs) = self.tabs.as_ref() else {
            return;
        };
        let target = tabs.plugin_terminal_identity(plugin_id);
        let mut ctx = self.build_plugin_context();
        let mut saved_profile = None;
        if let Some(identity) = target {
            let tab = tabs
                .tab_by_id(identity.tab_id)
                .expect("resolved plugin tab");
            let origin = tabs
                .origin_for_pane(identity.tab_id, identity.pane_id)
                .expect("resolved live plugin owner");
            if tab.live_domain.is_some() || tabs.origin_read_only(origin) != Some(false) {
                self.toasts.warning(format!("Cannot restart {plugin_id}: its terminal is read-only or belongs to a live domain"));
                return;
            }
            saved_profile = tab
                .pane_plugin_launch(identity.pane_id)
                .and_then(|(_, profile)| profile.map(str::to_owned));
            ctx.cwd = tab.pane_working_directory(identity.pane_id);
            ctx.active_tab = tabs
                .iter()
                .find_map(|(index, tab)| (tab.id == identity.tab_id).then_some(index))
                .expect("resolved plugin tab index");
        }
        if let Err(error) = self.validate_plugin_launch_profile(plugin_id, saved_profile.as_deref())
        {
            self.toasts.warning(format!(
                "Cannot restart {plugin_id}; existing terminals were kept: {error}"
            ));
            return;
        }
        let actions = if target.is_some() {
            self.plugin_manager.dispatch_to(
                plugin_id,
                &Hook::WorkspaceRestore {
                    plugin_id: plugin_id.into(),
                    profile: saved_profile.clone(),
                    cwd: ctx.cwd.clone(),
                },
                &ctx,
            )
        } else {
            self.plugin_manager
                .execute_command(&format!("{plugin_id}.open"), &ctx)
        };
        let mut launch = None;
        let mut notices = Vec::new();
        let mut invalid = false;
        for action in actions {
            match action {
                PluginAction::OpenTab {
                    program,
                    args,
                    cwd,
                    title,
                    plugin_id: owner,
                    profile,
                    env,
                } => {
                    if launch.is_some()
                        || program.is_empty()
                        || owner.as_deref() != Some(plugin_id)
                        || (target.is_some() && cwd != ctx.cwd)
                        || self
                            .validate_plugin_launch_profile(plugin_id, profile.as_deref())
                            .is_err()
                        || saved_profile
                            .as_ref()
                            .is_some_and(|saved| profile.as_ref() != Some(saved))
                    {
                        invalid = true;
                    }
                    launch = Some((program, args, cwd, title, profile, env));
                }
                notice @ PluginAction::ShowNotice { .. } => {
                    invalid |= target.is_some();
                    notices.push(notice);
                }
                _ => invalid = true,
            }
        }
        self.execute_plugin_actions(notices);
        let Some((program, args, cwd, title, profile, env)) = launch.filter(|_| !invalid) else {
            self.toasts.warning(format!("Cannot restart {plugin_id}: no valid replacement launch; existing terminals were kept"));
            return;
        };
        // Legacy PTY setup tolerates a missing PWD. A replacement must not
        // retire its source after knowingly launching in a fallback directory.
        if cwd
            .as_ref()
            .is_some_and(|directory| !std::path::Path::new(directory).is_dir())
        {
            self.toasts.warning(format!("Cannot restart {plugin_id}: replacement working directory is unavailable; existing terminals were kept"));
            return;
        }
        let result = self
            .tabs
            .as_mut()
            .expect("validated tab manager")
            .new_tab_with_command(
                &program,
                &args,
                cwd.as_deref(),
                title.as_deref(),
                Some(TabType::Plugin {
                    plugin_id: plugin_id.into(),
                }),
                // The validated owner cwd must also win over profile PWD.
                env.into_iter().filter(|(key, _)| key != "PWD").collect(),
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
            tab.set_plugin_profile_reference(tab.single_pane_id(), plugin_id, profile);
        }
        if let Some(identity) = target
            && let Err(error) = self.retire_restarted_plugin_owner(identity, plugin_id)
        {
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
        if tab
            .pane_manager
            .as_ref()
            .is_some_and(|panes| panes.pane_count() > 1)
        {
            self.close_plugin_pane(identity.tab_id as u64, identity.pane_id.0)?;
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
