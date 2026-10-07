//! Writing to the focused terminal: raw and protocol input, saved commands,
//! editable-line aliases, and accepting a Git completion.

use super::*;

impl App {
    pub(super) fn active_terminal_origin(&self) -> Option<crate::tabs::TerminalEventOrigin> {
        let tab = self.tabs.as_ref().and_then(TabManager::active_tab)?;
        Some(crate::tabs::TerminalEventOrigin {
            tab_id: tab.id,
            pane_id: tab.focused_terminal_identity().pane_id,
        })
    }

    /// Write interactive input with saved aliases and Git branch suggestions.
    pub(super) fn write_to_pty(&mut self, data: &[u8]) {
        self.write_editable_input(data, true);
    }

    /// Explicitly chosen command text is already resolved. Preserve normal input
    /// policy, but do not reinterpret it through aliases or Git suggestions.
    pub(super) fn write_saved_command(&mut self, command: &str) {
        for chunk in command_keystrokes(command) {
            self.write_editable_input(&chunk, false);
        }
    }

    /// Tab and Enter must use the same literal encoding and focused-owner route.
    /// True means a selected completion was handled (including safe refusal).
    pub(super) fn accept_git_completion(&mut self, submit: bool) -> bool {
        if !self.git_completion_has_room() {
            self.dismiss_git_completion_popup();
            return false;
        }
        let Some(branch) = self
            .git_completion
            .take()
            .map(|popup| popup.selected_branch().to_owned())
        else {
            return false;
        };
        if self.focused_pane_read_only() || !matches!(self.broadcast, BroadcastState::Inactive) {
            self.toasts
                .warning("Completion was not sent while input is locked or broadcasting");
            return true;
        }
        let integration = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.focused_pane())
            .map(|pane| &pane.shell_integration);
        use crate::terminal::shell_integration::ShellIntegrationSupport;
        let quoting = match integration {
            Some(ShellIntegrationSupport::Enabled("bash" | "zsh")) => {
                Some(crate::git::BranchQuoting::Posix)
            }
            Some(ShellIntegrationSupport::Enabled("fish")) => Some(crate::git::BranchQuoting::Fish),
            _ => None,
        };
        let edit = match crate::git::branch_edit(&self.input_line_buffer, &branch, quoting, submit)
        {
            Ok(edit) => edit,
            Err(message) => {
                self.toasts.warning(message);
                return true;
            }
        };
        if let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) {
            match tab.write(&edit.wire) {
                Ok(()) => {
                    if submit {
                        self.input_line_buffer.clear();
                        self.git_watcher.invalidate();
                    } else {
                        self.input_line_buffer = edit.line;
                    }
                }
                Err(error) => self
                    .toasts
                    .error(format!("Completion was not sent: {error}")),
            }
        }
        true
    }

    fn write_editable_input(&mut self, data: &[u8], interactive_assistance: bool) {
        if matches!(self.broadcast, BroadcastState::Armed { .. }) {
            self.toasts
                .warning("Input was not sent while broadcast is armed");
            return;
        }
        if self.focused_pane_read_only() {
            self.clear_terminal_input_tracking();
            return;
        }
        // Reset cursor blink so cursor is fully visible when typing
        self.reset_cursor_blink();

        // Broadcast deliberately bypasses alias expansion and completion.
        if self.write_broadcast_bytes(data) {
            self.dismiss_git_completion_popup();
            return;
        }

        // Track the input line buffer for alias expansion + git completion
        if data == b"\r" {
            // If git completions are visible and a selection is active, accept it
            if interactive_assistance && self.accept_git_completion(true) {
                return;
            }
            self.dismiss_git_completion_popup();

            // Enter pressed — check if first word matches a custom command name or alias
            let trimmed = self.input_line_buffer.trim().to_string();
            let (first_word, rest) = match trimmed.find(' ') {
                Some(pos) => (trimmed[..pos].to_string(), trimmed[pos..].to_string()),
                None => (trimmed.clone(), String::new()),
            };
            let matched_command = if interactive_assistance {
                self.config
                    .commands
                    .iter()
                    .find(|cmd| cmd.name == first_word || cmd.alias.as_deref() == Some(&first_word))
                    .map(|cmd| cmd.command.clone())
            } else {
                None
            };
            let expanded = matched_command.map(|command| format!("{command}{rest}"));
            let input = expanded.as_deref().unwrap_or(&trimmed);
            let rewrite = expanded
                .as_ref()
                .map(|command| format!("\x15{command}\r").into_bytes());

            // If a git command was run, invalidate the watcher to refresh branch state
            if input.starts_with("git ") {
                self.git_watcher.invalidate();
            }

            self.input_line_buffer.clear();

            if let Some(rewrite) = rewrite {
                // Clear the typed alias from the terminal line (Ctrl+U)
                // then send the real command + Enter
                if !self.write_typed_to_focused_pane(&rewrite) {
                    return;
                }
                self.note_person_typed(data);
                return;
            }
        } else if data == b"\x7f" {
            // Backspace — remove last char from buffer
            self.input_line_buffer.pop();
        } else if data == b"\x15" {
            // Ctrl+U — clear buffer
            self.input_line_buffer.clear();
        } else if data == b"\x17" {
            // Ctrl+W — delete last word
            let trimmed = self.input_line_buffer.trim_end();
            if let Some(pos) = trimmed.rfind(' ') {
                self.input_line_buffer.truncate(pos + 1);
            } else {
                self.input_line_buffer.clear();
            }
        } else if data.len() == 1 && data[0] < 0x20 {
            // Other control characters (Ctrl+C, Ctrl+D, etc.) — reset buffer
            self.input_line_buffer.clear();
        } else if data.len() >= 2 && data[0] == 0x1b {
            // Escape sequences (arrow keys, etc.) — can affect cursor position, reset buffer
            self.input_line_buffer.clear();
        } else {
            // Regular characters — append to buffer
            if let Ok(s) = std::str::from_utf8(data) {
                self.input_line_buffer.push_str(s);
            }
        }

        // Explicit Run text is already resolved; it must neither build nor
        // accept interactive suggestions between its edit and submit chunks.
        if interactive_assistance
            && self.git_completion_has_room()
            && let Some(partial) = crate::git::detect_git_completion(&self.input_line_buffer)
        {
            self.git_completion = crate::app::git_completion::GitCompletionPopup::new(
                self.git_watcher.complete_branch(partial),
            );
        } else {
            self.dismiss_git_completion_popup();
        }

        if !self.write_typed_to_focused_pane(data) {
            return;
        }
        self.note_person_typed(data);
    }

    /// Write typed bytes to the focused pane's PTY, scrolled back to its live
    /// screen. Returns whether the PTY took them; a failed write is logged, and
    /// the caller must not record input the shell never received.
    fn write_typed_to_focused_pane(&mut self, data: &[u8]) -> bool {
        // The tab routes to its focused pane's terminal and PTY.
        let Some(tab) = self.tabs.as_mut().and_then(TabManager::active_tab_mut) else {
            return false;
        };
        if let Some(terminal) = tab.focused_terminal_mut() {
            terminal.grid_mut().set_scroll_offset(0);
        }
        match tab.write(data) {
            Ok(()) => true,
            Err(error) => {
                log::error!("Failed to write to focused PTY: {error}");
                false
            }
        }
    }

    /// Clear state derived from a user's editable terminal line. This must run
    /// whenever a route rejects user input before the bytes reach the PTY so a
    /// later unlock cannot revive aliases or completions the shell never
    /// received.
    pub(super) fn clear_terminal_input_tracking(&mut self) {
        self.input_line_buffer.clear();
        self.dismiss_git_completion_popup();
    }

    /// Write terminal-protocol bytes without treating them as editable shell
    /// input. Mouse/focus/paste framing must not mutate alias or completion state.
    pub(super) fn write_terminal_protocol_input(&mut self, data: &[u8]) -> bool {
        let result = self
            .tabs
            .as_mut()
            .and_then(TabManager::active_tab_mut)
            .map(|tab| tab.write(data));
        match result {
            Some(Ok(())) => true,
            Some(Err(error)) => {
                log::error!("Failed to write terminal protocol input: {}", error);
                false
            }
            None => false,
        }
    }

    /// Write pointer-protocol bytes (wheel reports and alternate-scroll keys)
    /// to the exact terminal under the pointer, which need not have focus.
    /// As with typing, a read-only pane refuses them, and no pane receives
    /// them while broadcast targets are being chosen.
    pub(super) fn write_pointer_protocol_input(
        &mut self,
        target: crate::tabs::TerminalIdentity,
        data: &[u8],
    ) -> bool {
        if matches!(self.broadcast, BroadcastState::Armed { .. }) {
            return false;
        }
        let Some(tabs) = &mut self.tabs else {
            return false;
        };
        let Some(origin) = tabs.origin_for_pane(target.tab_id, target.pane_id) else {
            return false;
        };
        match tabs.write_user_to_origin(origin, data) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
            Err(error) => {
                log::error!("Failed to write pointer input to {target:?}: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_command_app(count: usize) -> App {
        let mut tabs = TabManager::new(
            256,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab_with_command("/bin/sh", &[
            "-c".into(), format!("stty raw -echo; printf READY; dd bs=1 count={count} 2>/dev/null | od -An -tx1 -v; sleep 0.2"),
        ], None, Some("saved-command-bytes"), None, Default::default()).unwrap();
        let mut app = App::new_headless(Config::default()).unwrap();
        app.tabs = Some(tabs);
        wait_command_output(&mut app, "READY");
        app.config.commands = vec![crate::config::CustomCommand {
            name: "B".into(),
            command: "X".into(),
            alias: Some("alias".into()),
        }];
        app
    }

    fn wait_command_output(app: &mut App, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut last = String::new();
        loop {
            app.process_pty_events();
            let Some(terminal) = app.active_terminal() else {
                panic!("reader exited before {expected:?}; last output: {last:?}");
            };
            last = terminal.rows_text(0, 23);
            // od can wrap bytes onto multiple rows; preserve order but not whitespace.
            if last
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains(expected)
            {
                return;
            }
            assert!(Instant::now() < deadline, "missing {expected:?}: {last:?}");
            std::thread::yield_now();
        }
    }

    fn assert_command_bytes(app: &mut App, expected: &[u8]) {
        let expected = expected
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        wait_command_output(app, &expected);
    }

    fn seed_git_completion_cache(app: &mut App) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(directory.path()).unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature =
            git2::Signature::new("Fixture", "fixture@example.invalid", &git2::Time::new(1, 0))
                .unwrap();
        repo.commit(
            Some("refs/heads/main"),
            &signature,
            &signature,
            "inert fixture",
            &tree,
            &[],
        )
        .unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            app.git_watcher.update(directory.path().to_str());
            if app.git_watcher.complete_branch("ma") == ["main"] {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Git fixture scan did not publish: {:?}",
                app.git_watcher.take_error()
            );
            std::thread::yield_now();
        }
        drop(tree);
        drop(repo);
        directory
    }

    #[test]
    fn explicit_run_preserves_git_text_even_with_live_branch_suggestions() {
        let command = "git checkout ma";
        let expected = format!("\x15{command}\rZ");
        for keybinding in [false, true] {
            let mut app = raw_command_app(expected.len());
            let _repository = seed_git_completion_cache(&mut app);
            if keybinding {
                assert!(
                    app.handle_key_action(crate::input::keybindings::KeyAction::RunCommand {
                        command: command.into()
                    })
                );
            } else {
                app.run_custom_command(command);
            }
            assert!(app.git_completion.is_none());
            assert!(app.input_line_buffer.is_empty());
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, expected.as_bytes());
        }
    }

    #[test]
    fn explicit_git_run_in_an_agent_tab_sends_the_exact_text_without_a_completion_rewrite() {
        let command = "git checkout rm -rf /synthetic-never-executed ma";
        let expected = format!("\x15{command}\rZ");
        let mut app = raw_command_app(expected.len());
        let _repository = seed_git_completion_cache(&mut app);
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .tab_type = crate::tabs::TabType::Plugin {
            plugin_id: "inert-explicit-git-guard".into(),
        };
        app.run_custom_command(command);
        assert!(app.git_completion.is_none());
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected.as_bytes());
    }

    #[test]
    fn ordinary_typed_git_input_still_builds_and_accepts_real_cached_suggestions() {
        let expected = b"git checkout ma\x7f\x7fmain\rZ";
        let mut app = raw_command_app(expected.len());
        let _repository = seed_git_completion_cache(&mut app);
        app.write_to_pty(b"git checkout ma");
        let popup = app.git_completion.as_ref().unwrap();
        assert_eq!(&**popup.completions(), ["main"]);
        assert_eq!(popup.selected(), 0);
        app.write_to_pty(b"\r");
        assert!(app.git_completion.is_none());
        assert!(app.input_line_buffer.is_empty());
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
    }

    #[test]
    fn git_completion_enter_quotes_repository_text_before_pty_delivery() {
        let expected = b"git checkout to\x7f\x7f'topic;printf${IFS}UNEXPECTED'\rZ";
        let mut app = raw_command_app(expected.len());
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .focused_pane_mut()
            .unwrap()
            .shell_integration =
            crate::terminal::shell_integration::ShellIntegrationSupport::Enabled("bash");
        app.write_to_pty(b"git checkout to");
        app.git_completion = crate::app::git_completion::GitCompletionPopup::new(vec![
            "topic;printf${IFS}UNEXPECTED".into(),
        ]);
        app.write_to_pty(b"\r");
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
    }

    #[test]
    fn git_completion_tab_keeps_quoted_text_for_later_enter() {
        let expected = b"git checkout a\x7f'a'\\''b'\rZ";
        let mut app = raw_command_app(expected.len());
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .focused_pane_mut()
            .unwrap()
            .shell_integration =
            crate::terminal::shell_integration::ShellIntegrationSupport::Enabled("bash");
        app.write_to_pty(b"git checkout a");
        app.git_completion =
            crate::app::git_completion::GitCompletionPopup::new(vec!["a'b".into()]);
        assert!(app.handle_git_completion_key(&winit::keyboard::Key::Named(
            winit::keyboard::NamedKey::Tab,
        )));
        assert_eq!(app.input_line_buffer, "git checkout 'a'\\''b'");
        assert!(app.git_completion.is_none());
        app.write_to_pty(b"\r");
        assert!(app.input_line_buffer.is_empty());
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
    }

    #[test]
    fn completion_popup_does_not_steal_control_tab_or_write_edits() {
        use winit::keyboard::{Key, NamedKey};
        for read_only in [false, true] {
            let typed = "git checkout ma";
            let mut app = raw_command_app(typed.len() + 1);
            app.write_to_pty(typed.as_bytes());
            app.tabs.as_mut().unwrap().new_tab_with_command("/bin/sh", &[
                "-c".into(), "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1; sleep 0.2".into(),
            ], None, Some("shortcut-target-reader"), None, Default::default()).unwrap();
            wait_command_output(&mut app, "READY");
            app.tabs.as_mut().unwrap().switch_to(0);
            if read_only {
                app.toggle_focused_pane_read_only();
            }
            app.input_line_buffer = typed.into();
            app.git_completion =
                crate::app::git_completion::GitCompletionPopup::new(vec!["main".into()]);
            app.modifiers = ModifiersState::CONTROL;
            let key = Key::Named(NamedKey::Tab);
            assert!(!app.handle_git_completion_key(&key));
            let action = app.shortcut_registry.check(&key, app.modifiers).unwrap();
            assert_eq!(action, crate::input::keybindings::BindableAction::NextTab);
            assert!(app.handle_action(action.to_action()));
            assert_eq!(app.tabs.as_ref().unwrap().active_index(), 1);
            app.modifiers = ModifiersState::empty();
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, b"Z");
            app.tabs.as_mut().unwrap().switch_to(0);
            if read_only {
                app.toggle_focused_pane_read_only();
            }
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, format!("{typed}Z").as_bytes());
        }
    }

    #[test]
    fn modified_enter_cannot_accept_a_completion_through_legacy_carriage_return() {
        use winit::keyboard::{Key, NamedKey};
        let expected = b"git checkout ma\rZ";
        let mut app = raw_command_app(expected.len());
        app.write_to_pty(b"git checkout ma");
        app.git_completion =
            crate::app::git_completion::GitCompletionPopup::new(vec!["main".into()]);
        app.modifiers = ModifiersState::CONTROL;
        assert!(!app.handle_git_completion_key(&Key::Named(NamedKey::Enter)));
        // Even if the terminal's keyboard mode encodes this key as legacy CR,
        // only the user's original line is submitted, without a branch rewrite.
        app.write_to_pty(b"\r");
        app.modifiers = ModifiersState::empty();
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
    }

    #[test]
    fn git_completion_refusal_emits_neither_edits_nor_enter() {
        for submit in [false, true] {
            for (typed, branch) in [
                ("git checkout a", "a;b"),
                ("git checkout 'a", "main"),
                ("git checkout a", "a\nb"),
                ("git checkout a", "-flag"),
            ] {
                let expected = format!("{typed}Z");
                let mut app = raw_command_app(expected.len());
                app.write_to_pty(typed.as_bytes());
                app.git_completion =
                    crate::app::git_completion::GitCompletionPopup::new(vec![branch.into()]);
                assert!(app.accept_git_completion(submit));
                assert_eq!(app.input_line_buffer, typed);
                assert!(app.git_completion.is_none());
                app.write_to_pty(b"Z");
                assert_command_bytes(&mut app, expected.as_bytes());
            }
        }
    }

    #[test]
    fn git_completion_tab_cannot_bypass_read_only_or_broadcast_arming() {
        for read_only in [true, false] {
            let expected = b"git checkout maZ";
            let mut app = raw_command_app(expected.len());
            app.write_to_pty(b"git checkout ma");
            app.git_completion =
                crate::app::git_completion::GitCompletionPopup::new(vec!["main".into()]);
            if read_only {
                app.toggle_focused_pane_read_only();
                // Exercise the mutation boundary even with a stale visible list.
                app.git_completion =
                    crate::app::git_completion::GitCompletionPopup::new(vec!["main".into()]);
            } else {
                app.broadcast = BroadcastState::Armed {
                    tab_id: 0,
                    targets: vec![],
                    expires: Instant::now() + Duration::from_secs(5),
                };
            }
            assert!(app.accept_git_completion(false));
            if read_only {
                app.toggle_focused_pane_read_only();
            } else {
                app.broadcast = BroadcastState::Inactive;
            }
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, expected);
        }
    }

    #[test]
    fn git_completion_uses_focused_pane_dialect_and_destination() {
        use crate::terminal::shell_integration::ShellIntegrationSupport;
        let expected = b"git checkout a\x7f'a\\'b'\rZ";
        let mut app = raw_command_app(1);
        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec![
                "-c".into(),
                format!(
                    "stty raw -echo; printf READY; dd bs=1 count={} 2>/dev/null | od -An -tx1 -v; sleep 0.2",
                    expected.len()
                ),
            ],
            env: Default::default(),
        };
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        tab.focused_pane_mut().unwrap().shell_integration =
            ShellIntegrationSupport::Enabled("bash");
        let panes = tab.panes_for_test(shell, 1600.0, 480.0, 8.0, 16.0);
        let original = panes.focused_id();
        panes.split(SplitDirection::Vertical).unwrap();
        let second = panes
            .pane_ids()
            .into_iter()
            .find(|id| *id != original)
            .unwrap();
        panes.focus(second);
        wait_command_output(&mut app, "READY");
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        tab.panes.focused_pane_mut().unwrap().shell_integration =
            ShellIntegrationSupport::Enabled("fish");
        app.write_to_pty(b"git checkout a");
        app.git_completion =
            crate::app::git_completion::GitCompletionPopup::new(vec!["a'b".into()]);
        app.write_to_pty(b"\r");
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, expected);
        app.tabs
            .as_mut()
            .unwrap()
            .active_tab_mut()
            .unwrap()
            .panes
            .focus(original);
        app.write_to_pty(b"Z");
        assert_command_bytes(&mut app, b"Z");
    }

    #[test]
    fn explicit_run_preserves_text_when_saved_names_or_aliases_collide() {
        for (command, expected) in [
            ("B", b"\x15B\r".as_slice()),
            ("B tail", b"\x15B tail\r".as_slice()),
            ("alias tail\n", b"\x15alias tail\r".as_slice()),
        ] {
            let mut app = raw_command_app(expected.len() + 1);
            app.run_custom_command(command);
            assert!(app.input_line_buffer.is_empty());
            app.write_to_pty(b"Z");
            // A following byte fences the operation, rejecting duplicate trailing input.
            assert_command_bytes(&mut app, &[expected, b"Z"].concat());
        }
    }

    #[test]
    fn ordinary_typed_names_and_aliases_still_expand_once() {
        for (name, guarded) in [("B", false), ("alias", false), ("B", true), ("alias", true)] {
            let typed = format!("{name} tail");
            let expected = format!("{typed}\x15X tail\r");
            let mut app = raw_command_app(expected.len() + 1);
            if guarded {
                app.tabs
                    .as_mut()
                    .unwrap()
                    .active_tab_mut()
                    .unwrap()
                    .tab_type = crate::tabs::TabType::Plugin {
                    plugin_id: "inert-safe-alias".into(),
                };
            }
            app.write_to_pty(typed.as_bytes());
            app.write_to_pty(b"\r");
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, &[expected.as_bytes(), b"Z"].concat());
        }
    }

    #[test]
    fn typed_and_pasted_input_in_agent_tabs_reaches_the_terminal_without_confirmation() {
        // Only automated input (control protocol, plugins) is confirmed; what
        // a person types, expands, completes, or pastes is sent as-is.
        for kind in ["typed", "alias", "paste"] {
            let typed = if kind == "alias" {
                "alias /synthetic-never-executed"
            } else {
                "rm -rf /synthetic-never-executed"
            };
            let expected = match kind {
                "alias" => format!("{typed}\x15rm -rf /synthetic-never-executed\rZ"),
                "paste" => format!("{typed}\rZ"),
                _ => format!("{typed}\rZ"),
            };
            let mut app = raw_command_app(expected.len());
            app.config.commands[0].command = "rm -rf".into();
            app.tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .tab_type = crate::tabs::TabType::Plugin {
                plugin_id: "inert-agent-input".into(),
            };
            if kind == "paste" {
                app.write_paste_text(&format!("{typed}\r"));
            } else {
                app.write_to_pty(typed.as_bytes());
                app.write_to_pty(b"\r");
            }
            assert!(!app.guardrail.is_awaiting_current(), "{kind}");
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, expected.as_bytes());
        }
    }

    #[test]
    fn git_completion_safe_enter_and_no_popup_fallback_preserve_exact_bytes() {
        for (shown, expected) in [
            (true, b"git checkout ma\x7f\x7fmain\rZ".as_slice()),
            (false, b"git checkout ma\rZ".as_slice()),
        ] {
            let mut app = raw_command_app(expected.len());
            app.tabs
                .as_mut()
                .unwrap()
                .active_tab_mut()
                .unwrap()
                .tab_type = crate::tabs::TabType::Plugin {
                plugin_id: "inert-safe-completion".into(),
            };
            app.write_to_pty(b"git checkout ma");
            app.git_completion =
                crate::app::git_completion::GitCompletionPopup::new(vec!["main".into()])
                    .filter(|_| shown);
            app.write_to_pty(b"\r");
            assert!(app.input_line_buffer.is_empty());
            assert!(app.git_completion.is_none());
            app.write_to_pty(b"Z");
            assert_command_bytes(&mut app, expected);
        }
    }

    #[test]
    fn a_failed_alias_write_is_not_recorded_as_typed_input() {
        let mut app = raw_command_app(2);
        app.write_to_pty(b"alias");
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let pane_id = tab.focused_terminal_identity().pane_id;
        tab.pane_integration_mut(pane_id).unwrap().last_person_input = None;
        // The PTY stops taking writes, so the alias's rewrite fails.
        drop(tab.focused_pane_mut().unwrap().pty.take_writer());

        app.write_to_pty(b"\r");
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        assert_eq!(
            tab.pane_integration_mut(pane_id).unwrap().last_person_input,
            None
        );
    }

    #[test]
    fn explicit_run_keeps_read_only_and_armed_broadcast_guards() {
        let mut app = raw_command_app(1);
        app.toggle_focused_pane_read_only();
        app.input_line_buffer = "stale".into();
        app.git_completion =
            crate::app::git_completion::GitCompletionPopup::new(vec!["stale".into()]);
        app.run_custom_command("B");
        assert!(app.input_line_buffer.is_empty());
        assert!(app.git_completion.is_none());
        app.toggle_focused_pane_read_only();
        let tab = app.tabs.as_ref().unwrap().active_tab().unwrap();
        app.broadcast = BroadcastState::Armed {
            tab_id: tab.id,
            targets: vec![tab.panes.focused_id()],
            expires: Instant::now() + Duration::from_secs(10),
        };
        app.run_custom_command("B");
        app.broadcast = BroadcastState::Inactive;
        app.write_to_pty(b"Z");
        // Even the line-clear prefix must not reach a refused destination.
        assert_command_bytes(&mut app, b"Z");
    }

    #[test]
    fn explicit_run_still_broadcasts_literal_text_to_selected_panes() {
        let mut app = raw_command_app(4);
        let shell = crate::config::ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "stty raw -echo; printf READY; dd bs=1 count=4 2>/dev/null | od -An -tx1; sleep 0.2".into()],
            env: Default::default(),
        };
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        let panes = tab.panes_for_test(shell, 800.0, 480.0, 8.0, 16.0);
        panes.split(SplitDirection::Vertical).unwrap();
        let targets = panes.pane_ids();
        let tab_id = tab.id;
        let wait_all = |app: &mut App, expected: &str| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                app.process_pty_events();
                let panes = app
                    .tabs
                    .as_ref()
                    .unwrap()
                    .active_tab()
                    .unwrap()
                    .split_panes()
                    .expect("both raw readers must remain live until observed");
                let outputs: Vec<_> = targets
                    .iter()
                    .map(|id| panes.get_pane(*id).unwrap().terminal.rows_text(0, 23))
                    .collect();
                if outputs.iter().all(|output| output.contains(expected)) {
                    return;
                }
                assert!(Instant::now() < deadline, "missing {expected}: {outputs:?}");
                std::thread::yield_now();
            }
        };
        wait_all(&mut app, "READY");
        app.broadcast = BroadcastState::Active {
            tab_id,
            targets: targets.clone(),
        };
        app.run_custom_command("B");
        app.write_to_pty(b"Z");
        wait_all(&mut app, "15 42 0d 5a");
    }
}
