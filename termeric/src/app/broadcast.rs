//! Broadcast input: arming a target set of panes, then writing keys, pastes,
//! and bytes to every target.

use super::agents::PersonInput;
use super::*;
use crate::ui::text::plural;

/// How long an armed broadcast waits for confirmation; changing the targets
/// starts it again.
const ARM_WINDOW: Duration = Duration::from_secs(10);

impl App {
    pub(super) fn toggle_broadcast(&mut self) {
        if matches!(self.broadcast, BroadcastState::Active { .. }) {
            self.broadcast = BroadcastState::Inactive;
            self.toasts.success("Broadcast input cancelled");
            return;
        }
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            self.toasts.warning("No tab is active");
            return;
        };
        let Some(manager) = tab.split_panes() else {
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
                self.dismiss_git_completion_popup();
                self.toasts.warning(format!(
                    "Broadcast armed for {} seconds: click panes or focus one and press Space to change targets; press Enter or invoke Toggle Broadcast Input again to confirm",
                    ARM_WINDOW.as_secs()
                ));
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
                self.toasts.warning(format!(
                    "Broadcast confirmation expired; armed again for {} seconds",
                    ARM_WINDOW.as_secs()
                ));
                BroadcastState::Armed {
                    tab_id,
                    expires: Instant::now() + ARM_WINDOW,
                    targets,
                }
            }
        };
    }

    pub(super) fn toggle_armed_broadcast_target(&mut self, target: PaneId) -> bool {
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
            .and_then(|tab| tab.split_panes())
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
            "Broadcast armed: {} selected",
            plural(targets.len(), "target", "targets")
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
        self.handle_broadcast_arming_press(&event.logical_key);
        true
    }

    /// A key pressed while broadcast is armed: Enter confirms, Space toggles
    /// the focused pane, and only broadcast and pane navigation shortcuts
    /// run. Every other key is swallowed.
    fn handle_broadcast_arming_press(&mut self, key: &winit::keyboard::Key) {
        use crate::input::keybindings::BindableAction;
        use winit::keyboard::{Key, NamedKey};
        // Shift may be held, but not AltGr: there the key types something
        // else, even on layouts that report no modifier with it.
        let plain = !self.alt_graph_active
            && !self.modifiers.control_key()
            && !self.modifiers.alt_key()
            && !self.modifiers.super_key();
        if plain && matches!(key, Key::Named(NamedKey::Enter)) {
            self.toggle_broadcast();
            return;
        }
        if plain && matches!(key, Key::Named(NamedKey::Space)) {
            if let Some(target) = self
                .tabs
                .as_ref()
                .and_then(TabManager::active_tab)
                .and_then(|tab| tab.split_panes())
                .map(PaneManager::focused_id)
            {
                self.toggle_armed_broadcast_target(target);
            }
            return;
        }
        if let Some(action) = self
            .shortcut_registry
            .check(key, self.modifiers)
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
    fn note_broadcast_input(&mut self, tab_id: usize, panes: &[PaneId], input: PersonInput) {
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
                    "Broadcast dropped {}",
                    plural(failed, "unavailable target", "unavailable targets")
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
                    .and_then(|tab| tab.split_panes());
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
                    .and_then(|tab| tab.split_panes());
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
        self.broadcast_write(|_| Some(data.to_vec()), PersonInput::of)
    }

    pub(super) fn write_broadcast_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let (modifiers, alt_graph_active) = (self.modifiers, self.alt_graph_active);
        self.broadcast_write(
            |terminal| {
                crate::input::key_to_bytes_for_terminal(
                    event,
                    modifiers,
                    alt_graph_active,
                    terminal,
                )
            },
            PersonInput::of,
        )
    }

    pub(super) fn write_broadcast_paste(&mut self, text: &str) -> bool {
        let handled = self.broadcast_write(
            |terminal| Some(crate::input::encode_paste(terminal.mode(), text)),
            |_| PersonInput::Answer,
        );
        if handled {
            self.input_line_buffer.clear();
        }
        handled
    }

    /// Write one input to every broadcast target. `encode` gives the bytes
    /// for a target's terminal, which can differ by its modes (or nothing,
    /// for a key that types nothing there), and `input` what written bytes
    /// say to an agent. Returns false when no broadcast is active in the
    /// active tab, so the input goes to the focused terminal instead.
    ///
    /// With fewer than two writable targets left the broadcast is cancelled
    /// before anything is written: broadcast input never lands in just one
    /// pane. A target that is gone or refuses the write is dropped.
    fn broadcast_write(
        &mut self,
        encode: impl Fn(&termeric_core::Terminal) -> Option<Vec<u8>>,
        input: impl Fn(&[u8]) -> PersonInput,
    ) -> bool {
        let Some((tab_id, targets)) = self.broadcast_targets() else {
            if matches!(self.broadcast, BroadcastState::Active { .. }) {
                self.broadcast = BroadcastState::Inactive;
                self.toasts
                    .warning("Broadcast cancelled because the active tab changed");
            }
            return false;
        };
        // `broadcast_targets` found the broadcast's tab active.
        let Some(manager) = self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .and_then(crate::tabs::Tab::split_panes_mut)
            .filter(|manager| {
                let writable = targets
                    .iter()
                    .filter(|id| manager.get_pane(**id).is_some_and(|pane| !pane.read_only))
                    .count();
                writable >= 2
            })
        else {
            self.broadcast = BroadcastState::Inactive;
            self.toasts
                .warning("Broadcast cancelled because fewer than two targets remain");
            return true;
        };
        let mut remaining = Vec::with_capacity(targets.len());
        let mut failed = 0;
        let mut written_input = PersonInput::Navigation;
        for id in targets {
            let Some(pane) = manager.get_pane(id) else {
                failed += 1;
                continue;
            };
            let Some(bytes) = encode(&pane.terminal) else {
                remaining.push(id);
                continue;
            };
            if manager.write_to(id, &bytes).is_ok() {
                // One input, encoded per pane; any pane's reading will do.
                written_input = input(&bytes);
                remaining.push(id);
            } else {
                failed += 1;
            }
        }
        self.note_broadcast_input(tab_id, &remaining, written_input);
        self.finish_broadcast_writes(tab_id, remaining, failed);
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
        let panes = tabs
            .active_tab_mut()
            .unwrap()
            .panes_for_test(shell, 400.0, 160.0, 10.0, 20.0);
        panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        let locked = panes
            .split(
                SplitDirection::Horizontal,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        panes.get_pane_mut(locked).unwrap().read_only = true;
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
        let panes = tabs
            .active_tab_mut()
            .unwrap()
            .panes_for_test(shell, 400.0, 160.0, 10.0, 20.0);
        panes
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        panes
            .split(
                SplitDirection::Horizontal,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);

        app.toggle_broadcast();
        assert!(matches!(app.broadcast, BroadcastState::Armed { .. }));
        let (pane_ids, excluded) = {
            let manager = &app.tabs.as_ref().unwrap().active_tab().unwrap().panes;
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
            let manager = &app.tabs.as_ref().unwrap().active_tab().unwrap().panes;
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
            .panes
            .close_focused();
        app.reconcile_broadcast_state();
        assert!(matches!(
            app.broadcast,
            BroadcastState::Active { ref targets, .. } if targets.len() == 2
        ));

        app.close_focused_pane();
        assert!(matches!(app.broadcast, BroadcastState::Inactive));
    }

    #[test]
    fn altgr_enter_and_space_neither_confirm_nor_retarget_an_armed_broadcast() {
        use winit::keyboard::{Key, NamedKey};

        let mut app = App::new_headless_running("sleep 2", "altgr-arming");
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .panes_for_test(
                crate::test_support::sleeping_shell(),
                400.0,
                160.0,
                10.0,
                20.0,
            )
            .split(
                SplitDirection::Vertical,
                crate::config::PaneCwdMode::Focused,
            )
            .unwrap();
        app.toggle_broadcast();
        let armed_targets = |app: &App| match &app.broadcast {
            BroadcastState::Armed { targets, .. } => Some(targets.len()),
            _ => None,
        };
        assert_eq!(armed_targets(&app), Some(2));

        app.alt_graph_active = true;
        for key in [NamedKey::Space, NamedKey::Enter] {
            app.handle_broadcast_arming_press(&Key::Named(key));
        }
        assert_eq!(armed_targets(&app), Some(2));

        // Without AltGr, Space drops the focused pane from the targets.
        app.alt_graph_active = false;
        app.handle_broadcast_arming_press(&Key::Named(NamedKey::Space));
        assert_eq!(armed_targets(&app), Some(1));
    }

    #[test]
    fn broadcast_with_one_writable_target_left_cancels_before_writing_anything() {
        let probe = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), crate::test_support::FIRST_BYTE_PROBE.into()],
            env: Default::default(),
        };
        for path in ["bytes", "paste"] {
            let mut app =
                App::new_headless_running(crate::test_support::FIRST_BYTE_PROBE, "broadcast-floor");
            let tab_id = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
            let panes = app
                .tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .panes_for_test(probe.clone(), 400.0, 160.0, 10.0, 20.0);
            let writable = panes.focused_id();
            let locked = panes
                .split(
                    SplitDirection::Vertical,
                    crate::config::PaneCwdMode::Focused,
                )
                .unwrap();
            let identity = |pane_id| crate::tabs::TerminalIdentity { tab_id, pane_id };
            for pane in [writable, locked] {
                app.wait_for_terminal_text(identity(pane), "READY");
            }
            app.broadcast = BroadcastState::Active {
                tab_id,
                targets: vec![writable, locked],
            };
            // The pane turns read-only between frames, before reconciling.
            app.active_panes_mut()
                .get_pane_mut(locked)
                .unwrap()
                .read_only = true;

            let handled = if path == "bytes" {
                app.write_broadcast_bytes(b"x")
            } else {
                app.write_broadcast_paste("x")
            };
            assert!(handled, "{path}: broadcast input must not fall through");
            assert!(matches!(app.broadcast, BroadcastState::Inactive), "{path}");
            // The writable pane's first byte is this one, not the broadcast's.
            app.active_panes_mut().write_to(writable, b"Z").unwrap();
            app.wait_for_terminal_text(identity(writable), "5a");
        }
    }
}
