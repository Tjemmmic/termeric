//! Broadcast input: arming a target set of panes, then writing keys, pastes,
//! and bytes to every target.

use super::*;

impl App {
    pub(super) fn toggle_broadcast(&mut self) {
        const ARM_WINDOW: Duration = Duration::from_secs(10);

        if matches!(self.broadcast, BroadcastState::Active { .. }) {
            self.broadcast = BroadcastState::Inactive;
            self.toasts.success("Broadcast input cancelled");
            return;
        }
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            self.toasts.warning("No tab is active");
            return;
        };
        let Some(manager) = &tab.pane_manager else {
            self.toasts.warning("Broadcast requires at least two panes");
            return;
        };
        let tab_id = tab.id;
        let targets: Vec<_> = manager
            .pane_ids()
            .into_iter()
            .filter(|id| manager.get_pane(*id).is_some_and(|pane| !pane.read_only))
            .collect();
        if targets.len() < 2 {
            self.toasts
                .warning("Broadcast requires at least two writable panes");
            return;
        }
        self.broadcast = match self.broadcast.clone() {
            BroadcastState::Inactive => {
                self.ime.clear_preedit();
                self.input_line_buffer.clear();
                self.git_completions_visible = false;
                self.git_completions.clear();
                self.git_completion_index = None;
                self.toasts.warning(
                    "Broadcast armed for 10 seconds: click panes or focus one and press Space to change targets; press Enter or invoke Toggle Broadcast Input again to confirm",
                );
                BroadcastState::Armed {
                    tab_id,
                    expires: Instant::now() + ARM_WINDOW,
                    targets,
                }
            }
            BroadcastState::Armed {
                tab_id: armed_tab,
                expires,
                targets: selected,
            } if armed_tab == tab_id && Instant::now() <= expires => {
                if selected.len() < 2 {
                    self.toasts.warning(format!(
                        "Select at least two broadcast targets ({} selected)",
                        selected.len()
                    ));
                    BroadcastState::Armed {
                        tab_id,
                        expires: Instant::now() + ARM_WINDOW,
                        targets: selected,
                    }
                } else {
                    self.toasts.warning(format!(
                        "BROADCAST ACTIVE to {} panes — press Escape to cancel immediately",
                        selected.len()
                    ));
                    BroadcastState::Active {
                        tab_id,
                        targets: selected,
                    }
                }
            }
            BroadcastState::Active { .. } => unreachable!("active state returned above"),
            BroadcastState::Armed { .. } => {
                self.toasts
                    .warning("Broadcast confirmation expired; armed again for 10 seconds");
                BroadcastState::Armed {
                    tab_id,
                    expires: Instant::now() + ARM_WINDOW,
                    targets,
                }
            }
        };
    }

    pub(super) fn toggle_armed_broadcast_target(&mut self, target: PaneId) -> bool {
        const ARM_WINDOW: Duration = Duration::from_secs(10);

        let BroadcastState::Armed {
            tab_id,
            expires,
            mut targets,
        } = self.broadcast.clone()
        else {
            return false;
        };
        if Instant::now() > expires {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast arming expired; input was not sent");
            return true;
        }
        let target_state = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_manager.as_ref())
            .and_then(|manager| manager.get_pane(target))
            .map(|pane| pane.read_only);
        let Some(target_read_only) = target_state else {
            return false;
        };
        if target_read_only {
            self.toasts
                .warning("Read-only panes cannot receive broadcast input");
            return true;
        }
        if let Some(index) = targets.iter().position(|candidate| *candidate == target) {
            targets.remove(index);
        } else {
            targets.push(target);
        }
        targets.sort_by_key(|id| id.0);
        self.toasts.info(format!(
            "Broadcast armed: {} target{} selected",
            targets.len(),
            if targets.len() == 1 { "" } else { "s" }
        ));
        self.broadcast = BroadcastState::Armed {
            tab_id,
            expires: Instant::now() + ARM_WINDOW,
            targets,
        };
        true
    }

    pub(super) fn handle_broadcast_arming_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let BroadcastState::Armed { expires, .. } = self.broadcast else {
            return false;
        };
        if event.state != winit::event::ElementState::Pressed {
            return true;
        }
        if Instant::now() > expires {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast arming expired; input was not sent");
            return true;
        }

        use crate::input::keybindings::BindableAction;
        use winit::keyboard::{Key, NamedKey};
        let plain = !self.modifiers.control_key()
            && !self.modifiers.alt_key()
            && !self.modifiers.super_key();
        if plain && matches!(event.logical_key, Key::Named(NamedKey::Enter)) {
            self.toggle_broadcast();
            return true;
        }
        if plain && matches!(event.logical_key, Key::Named(NamedKey::Space)) {
            if let Some(target) = self
                .tabs
                .as_ref()
                .and_then(TabManager::active_tab)
                .and_then(|tab| tab.pane_manager.as_ref())
                .map(PaneManager::focused_id)
            {
                self.toggle_armed_broadcast_target(target);
            }
            return true;
        }
        if let Some(action) = self
            .shortcut_registry
            .check(&event.logical_key, self.modifiers)
            .filter(|action| {
                matches!(
                    action,
                    BindableAction::ToggleBroadcast
                        | BindableAction::NavigatePaneUp
                        | BindableAction::NavigatePaneDown
                        | BindableAction::NavigatePaneLeft
                        | BindableAction::NavigatePaneRight
                )
            })
        {
            self.handle_action(action.to_action());
        }
        true
    }

    pub(super) fn cancel_broadcast_on_escape(&mut self, event: &winit::event::KeyEvent) -> bool {
        if matches!(self.broadcast, BroadcastState::Inactive)
            || event.state != winit::event::ElementState::Pressed
            || event.logical_key != winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
        {
            return false;
        }
        self.broadcast = BroadcastState::Inactive;
        self.toasts.success("Broadcast input cancelled");
        true
    }

    pub(super) fn broadcast_targets(&self) -> Option<(usize, Vec<PaneId>)> {
        let BroadcastState::Active { tab_id, targets } = &self.broadcast else {
            return None;
        };
        let tab = self.tabs.as_ref()?.active_tab()?;
        (tab.id == *tab_id).then(|| (*tab_id, targets.clone()))
    }

    /// Broadcast input is the person's typing in every pane it reached.
    fn note_broadcast_input(
        &mut self,
        tab_id: usize,
        panes: &[PaneId],
        input: super::agents::PersonInput,
    ) {
        for pane_id in panes {
            self.note_person_input(
                crate::tabs::TerminalIdentity {
                    tab_id,
                    pane_id: *pane_id,
                },
                input,
            );
        }
    }

    fn finish_broadcast_writes(
        &mut self,
        tab_id: usize,
        mut remaining: Vec<PaneId>,
        failed: usize,
    ) {
        remaining.sort_by_key(|id| id.0);
        remaining.dedup();
        if remaining.len() < 2 {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast cancelled because fewer than two targets remain");
        } else {
            self.broadcast = BroadcastState::Active {
                tab_id,
                targets: remaining,
            };
            if failed > 0 {
                self.toasts.warning(format!(
                    "Broadcast dropped {failed} unavailable target{}",
                    if failed == 1 { "" } else { "s" }
                ));
            }
        }
    }

    pub(super) fn broadcast_render_targets(
        &self,
    ) -> Option<(usize, Vec<PaneId>, crate::renderer::BroadcastTargetState)> {
        let (tab_id, targets, state) = match &self.broadcast {
            BroadcastState::Armed {
                tab_id,
                expires,
                targets,
            } if Instant::now() <= *expires => (
                *tab_id,
                targets.clone(),
                crate::renderer::BroadcastTargetState::Armed,
            ),
            BroadcastState::Active { tab_id, targets } => (
                *tab_id,
                targets.clone(),
                crate::renderer::BroadcastTargetState::Active,
            ),
            _ => return None,
        };
        let tab = self.tabs.as_ref()?.active_tab()?;
        (tab.id == tab_id).then_some((tab_id, targets, state))
    }

    pub(super) fn reconcile_broadcast_state(&mut self) {
        let active_tab = self.tabs.as_ref().and_then(TabManager::active_tab);
        let active_id = active_tab.map(|tab| tab.id);
        match self.broadcast.clone() {
            BroadcastState::Inactive => {}
            BroadcastState::Armed { tab_id, .. } if active_id != Some(tab_id) => {
                self.broadcast = BroadcastState::Inactive;
                self.toasts
                    .info("Broadcast arming cancelled after focus changed");
            }
            BroadcastState::Armed { expires, .. } if Instant::now() > expires => {
                self.broadcast = BroadcastState::Inactive;
                self.toasts.info("Broadcast arming expired");
            }
            BroadcastState::Armed {
                tab_id,
                expires,
                mut targets,
            } => {
                let manager = active_tab
                    .filter(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.pane_manager.as_ref());
                if manager.is_none_or(|manager| manager.pane_ids().len() < 2) {
                    self.broadcast = BroadcastState::Inactive;
                    self.toasts
                        .warning("Broadcast arming cancelled because fewer than two panes remain");
                } else {
                    targets.retain(|id| {
                        manager.is_some_and(|manager| {
                            manager.get_pane(*id).is_some_and(|pane| !pane.read_only)
                        })
                    });
                    self.broadcast = BroadcastState::Armed {
                        tab_id,
                        expires,
                        targets,
                    };
                }
            }
            BroadcastState::Active {
                tab_id,
                mut targets,
            } => {
                let manager = active_tab
                    .filter(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.pane_manager.as_ref());
                targets.retain(|id| {
                    manager.is_some_and(|manager| {
                        manager.get_pane(*id).is_some_and(|pane| !pane.read_only)
                    })
                });
                if targets.len() < 2 {
                    self.broadcast = BroadcastState::Inactive;
                    self.toasts
                        .warning("Broadcast cancelled because its target set changed");
                } else {
                    self.broadcast = BroadcastState::Active { tab_id, targets };
                }
            }
        }
    }

    pub(super) fn write_broadcast_bytes(&mut self, data: &[u8]) -> bool {
        let Some((tab_id, targets)) = self.broadcast_targets() else {
            if matches!(self.broadcast, BroadcastState::Active { .. }) {
                self.broadcast = BroadcastState::Inactive;
                self.toasts
                    .warning("Broadcast cancelled because the active tab changed");
            }
            return false;
        };
        let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) else {
            return false;
        };
        debug_assert_eq!(tab.id, tab_id);
        let Some(manager) = &mut tab.pane_manager else {
            self.broadcast = BroadcastState::Inactive;
            return false;
        };
        let mut remaining = targets;
        let mut failed = 0;
        remaining.retain(|id| {
            let written = manager.write_to(*id, data).is_ok();
            failed += usize::from(!written);
            written
        });
        self.note_broadcast_input(tab_id, &remaining, super::agents::PersonInput::of(data));
        self.finish_broadcast_writes(tab_id, remaining, failed);
        true
    }

    pub(super) fn write_broadcast_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let Some((tab_id, targets)) = self.broadcast_targets() else {
            if matches!(self.broadcast, BroadcastState::Active { .. }) {
                self.broadcast = BroadcastState::Inactive;
                self.toasts
                    .warning("Broadcast cancelled because the active tab changed");
            }
            return false;
        };
        let Some(manager) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_manager.as_ref())
        else {
            self.broadcast = BroadcastState::Inactive;
            return true;
        };
        let existing = targets
            .iter()
            .filter(|id| manager.get_pane(**id).is_some())
            .count();
        if existing < 2 {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast cancelled because fewer than two targets remain");
            return true;
        }
        let encoded: Vec<_> = targets
            .iter()
            .filter_map(|id| {
                let terminal = &manager.get_pane(*id)?.terminal;
                crate::input::key_to_bytes_for_terminal(
                    event,
                    self.modifiers,
                    self.alt_graph_active,
                    terminal,
                )
                .map(|bytes| (*id, bytes))
            })
            .collect();
        let Some(manager) = self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .and_then(|tab| tab.pane_manager.as_mut())
        else {
            self.broadcast = BroadcastState::Inactive;
            return true;
        };
        let mut remaining = targets;
        let mut failed = 0;
        let mut input = super::agents::PersonInput::Navigation;
        for (id, bytes) in encoded {
            if manager.write_to(id, &bytes).is_err() {
                remaining.retain(|target| *target != id);
                failed += 1;
            } else {
                // One key, encoded per pane; any pane's reading will do.
                input = super::agents::PersonInput::of(&bytes);
            }
        }
        self.note_broadcast_input(tab_id, &remaining, input);
        self.finish_broadcast_writes(tab_id, remaining, failed);
        true
    }

    pub(super) fn write_broadcast_paste(&mut self, text: &str) -> bool {
        let Some((tab_id, targets)) = self.broadcast_targets() else {
            if matches!(self.broadcast, BroadcastState::Active { .. }) {
                self.broadcast = BroadcastState::Inactive;
                self.toasts
                    .warning("Broadcast cancelled because the active tab changed");
            }
            return false;
        };
        let framed: Vec<_> = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .filter(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_manager.as_ref())
            .map(|manager| {
                targets
                    .iter()
                    .filter_map(|id| {
                        let mode = manager.get_pane(*id)?.terminal.mode();
                        Some((*id, crate::input::encode_paste(mode, text)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if framed.len() < 2 {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast cancelled because fewer than two targets remain");
            return true;
        }
        let Some(manager) = self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .and_then(|tab| tab.pane_manager.as_mut())
        else {
            self.broadcast = BroadcastState::Inactive;
            return true;
        };
        let mut remaining = targets;
        let mut failed = 0;
        for (id, bytes) in framed {
            if manager.write_to(id, &bytes).is_err() {
                remaining.retain(|target| *target != id);
                failed += 1;
            }
        }
        self.note_broadcast_input(tab_id, &remaining, super::agents::PersonInput::Answer);
        self.finish_broadcast_writes(tab_id, remaining, failed);
        self.input_line_buffer.clear();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_broadcast_targets_are_pruned_and_the_two_target_floor_is_enforced() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.broadcast = BroadcastState::Active {
            tab_id: 7,
            targets: vec![PaneId(1), PaneId(2), PaneId(3)],
        };
        app.finish_broadcast_writes(7, vec![PaneId(1), PaneId(3)], 1);
        assert!(matches!(
            &app.broadcast,
            BroadcastState::Active { tab_id: 7, targets }
                if targets == &[PaneId(1), PaneId(3)]
        ));
        app.finish_broadcast_writes(7, vec![PaneId(3)], 1);
        assert!(matches!(app.broadcast, BroadcastState::Inactive));
    }

    #[test]
    fn broadcast_targeting_excludes_and_rejects_read_only_panes() {
        use std::collections::HashMap;

        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let shell = crate::test_support::sleeping_shell();
        let mut tabs = TabManager::new(40, 8, shell.clone(), 100, 1024 * 1024);
        tabs.new_tab_with_command(
            "/bin/sh",
            &shell.args,
            None,
            Some("read-only-broadcast"),
            None,
            HashMap::new(),
        )
        .unwrap();
        let owner = tabs
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut panes =
            PaneManager::from_existing(owner, shell, 100, 1024 * 1024, 400.0, 160.0, 10.0, 20.0);
        panes.split(SplitDirection::Vertical).unwrap();
        let locked = panes.split(SplitDirection::Horizontal).unwrap();
        panes.get_pane_mut(locked).unwrap().read_only = true;
        tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);

        app.toggle_broadcast();
        let BroadcastState::Armed { targets, .. } = &app.broadcast else {
            panic!("two writable panes should arm broadcast");
        };
        assert_eq!(targets.len(), 2);
        assert!(!targets.contains(&locked));
        assert!(app.toggle_armed_broadcast_target(locked));
        let BroadcastState::Armed { targets, .. } = &app.broadcast else {
            panic!("rejected read-only target should leave broadcast armed");
        };
        assert_eq!(targets.len(), 2);
        assert!(!targets.contains(&locked));
    }

    #[test]
    fn selected_broadcast_reaches_only_targets_and_closure_reconciles_it() {
        use std::collections::HashMap;
        use std::time::{Duration, Instant};

        if !std::path::Path::new("/bin/sh").is_file() {
            return;
        }
        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec![
                "-c".into(),
                "stty raw -echo; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 1".into(),
            ],
            env: Default::default(),
        };
        let mut tabs = TabManager::new(
            40,
            8,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command(
            "/bin/sh",
            &shell.args,
            None,
            Some("broadcast-routing"),
            None,
            HashMap::new(),
        )
        .unwrap();
        let owner = tabs
            .active_tab_mut()
            .unwrap()
            .take_terminal_owner()
            .unwrap();
        let mut panes =
            PaneManager::from_existing(owner, shell, 100, 1024 * 1024, 400.0, 160.0, 10.0, 20.0);
        panes.split(SplitDirection::Vertical).unwrap();
        panes.split(SplitDirection::Horizontal).unwrap();
        tabs.active_tab_mut().unwrap().pane_manager = Some(panes);
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);

        app.toggle_broadcast();
        assert!(matches!(app.broadcast, BroadcastState::Armed { .. }));
        let (pane_ids, excluded) = {
            let manager = app
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap();
            (manager.pane_ids(), manager.focused_id())
        };
        assert!(app.toggle_armed_broadcast_target(excluded));
        assert!(matches!(
            app.broadcast,
            BroadcastState::Armed { ref targets, .. }
                if targets.len() == 2 && !targets.contains(&excluded)
        ));
        let temporarily_excluded = pane_ids.iter().copied().find(|id| *id != excluded).unwrap();
        assert!(app.toggle_armed_broadcast_target(temporarily_excluded));
        app.toggle_broadcast();
        assert!(matches!(
            app.broadcast,
            BroadcastState::Armed { ref targets, .. } if targets.len() == 1
        ));
        assert!(app.toggle_armed_broadcast_target(temporarily_excluded));
        app.write_to_pty(b"z");
        app.write_paste_text("z");
        app.handle_ime_event(winit::event::Ime::Commit("z".into()));
        app.toggle_broadcast();
        assert!(matches!(
            app.broadcast,
            BroadcastState::Active { ref targets, .. }
                if targets.len() == 2 && !targets.contains(&excluded)
        ));
        assert!(app.write_broadcast_bytes(b"x"));

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.process_pty_events();
            let manager = app
                .tabs
                .as_ref()
                .unwrap()
                .active_tab()
                .unwrap()
                .pane_manager
                .as_ref()
                .unwrap();
            let reached = pane_ids.iter().all(|id| {
                let output = manager.get_pane(*id).unwrap().terminal.rows_text(0, 7);
                output.contains("78") == (*id != excluded)
            });
            if reached || Instant::now() >= deadline {
                assert!(
                    reached,
                    "broadcast byte did not reach exactly the selected targets"
                );
                break;
            }
            std::thread::yield_now();
        }

        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .pane_manager
            .as_mut()
            .unwrap()
            .close_focused();
        app.reconcile_broadcast_state();
        assert!(matches!(
            app.broadcast,
            BroadcastState::Active { ref targets, .. } if targets.len() == 2
        ));

        app.close_focused_pane();
        assert!(matches!(app.broadcast, BroadcastState::Inactive));
    }
}
