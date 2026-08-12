//! Terminal-event bridges for clipboard, notifications, links, and PTY input.

use super::*;

impl App {
    /// Handle a terminal event
    pub(super) fn handle_terminal_event(&mut self, event: TerminalEvent) {
        match event {
            TerminalEvent::Bell => {
                self.bell.ring();
            }
            TerminalEvent::Notification(req) => {
                let title = req.title.as_deref().unwrap_or("Terminal");
                self.notifications.notify(title, &req.body);
            }
            TerminalEvent::TitleChanged(_title) => {
                // Title is handled via shell cwd detection
            }
            TerminalEvent::ClipboardRequest(_which) => {
                // OSC 52 read: echo the clipboard contents back to the PTY
                // as a base64-encoded OSC 52 reply so the requesting app
                // doesn't time out. If the clipboard is unavailable we
                // still emit an empty reply so the app fails fast.
                let text = self
                    .clipboard
                    .as_mut()
                    .and_then(|c| c.get_text().ok())
                    .unwrap_or_default();
                let encoded = base64_encode_standard(text.as_bytes());
                let reply = format!("\x1b]52;c;{}\x07", encoded);
                self.write_to_pty(reply.as_bytes());
            }
            TerminalEvent::ClipboardWrite(text) => {
                if let Some(clipboard) = &mut self.clipboard {
                    if let Err(e) = clipboard.set_text(&text) {
                        log::error!("Failed to set clipboard: {}", e);
                    }
                }
            }
            TerminalEvent::HyperlinkActivated(url) => {
                if let Err(e) = crate::terminal::url::UrlDetector::open_url(&url) {
                    log::error!("Failed to open URL: {}", e);
                }
            }
            TerminalEvent::CursorVisibilityChanged(_) => {
                // Cursor visibility is handled in render
            }
            TerminalEvent::PtyWrite(_) => {
                // Handled in Tab::process_events (written back to tab PTY)
            }
            TerminalEvent::Exit => {
                // Individual tab exit is handled by Tab::process_events
            }
        }
    }

    /// Write input to the active tab's PTY, with custom command alias expansion
    pub(super) fn write_to_pty(&mut self, data: &[u8]) {
        // Reset cursor blink so cursor is fully visible when typing
        self.reset_cursor_blink();

        // Track the input line buffer for alias expansion + git completion
        if data == b"\r" {
            // If git completions are visible and a selection is active, accept it
            if self.git_completions_visible {
                if let Some(idx) = self.git_completion_index {
                    if let Some(branch) = self.git_completions.get(idx).cloned() {
                        // Determine how many chars of partial text to erase
                        if let Some(partial) =
                            crate::git::detect_git_completion(&self.input_line_buffer)
                        {
                            let partial_len = partial.len();
                            // Send backspaces to erase the partial, then type the branch name
                            if let Some(tabs) = &mut self.tabs {
                                if let Some(tab) = tabs.active_tab_mut() {
                                    for _ in 0..partial_len {
                                        let _ = tab.write(b"\x7f"); // backspace
                                    }
                                    let _ = tab.write(branch.as_bytes());
                                    let _ = tab.write(b"\r"); // then Enter
                                }
                            }
                            self.input_line_buffer.clear();
                            self.git_completions_visible = false;
                            self.git_completions.clear();
                            self.git_completion_index = None;
                            return;
                        }
                    }
                }
                // Dismiss completions
                self.git_completions_visible = false;
                self.git_completions.clear();
                self.git_completion_index = None;
            }

            // Enter pressed — guardrail check for plugin tabs
            let is_plugin_tab = self
                .tabs
                .as_ref()
                .and_then(|tabs| {
                    tabs.active_tab()
                        .map(|t| matches!(t.tab_type, crate::tabs::TabType::Plugin { .. }))
                })
                .unwrap_or(false);

            if is_plugin_tab {
                let input = self.input_line_buffer.trim().to_string();
                if let Some(matched) = crate::terminal::guardrails::check_command(&input) {
                    log::warn!(
                        "Guardrail triggered: {} — {}",
                        matched.command,
                        matched.reason
                    );
                    self.guardrail.show(matched, data.to_vec());
                    self.input_line_buffer.clear();
                    return;
                }
            }

            // Enter pressed — check if first word matches a custom command name or alias
            let trimmed = self.input_line_buffer.trim().to_string();
            let (first_word, rest) = match trimmed.find(' ') {
                Some(pos) => (trimmed[..pos].to_string(), trimmed[pos..].to_string()),
                None => (trimmed.clone(), String::new()),
            };
            let matched_command = self
                .config
                .commands
                .iter()
                .find(|cmd| cmd.name == first_word || cmd.alias.as_deref() == Some(&first_word))
                .map(|cmd| cmd.command.clone());

            // If a git command was run, invalidate the watcher to refresh branch state
            if trimmed.starts_with("git ") {
                self.git_watcher.invalidate();
            }

            self.input_line_buffer.clear();

            if let Some(real_command) = matched_command {
                // Build expanded command: real_command + any trailing args
                let expanded = format!("{}{}", real_command, rest);
                // Clear the typed alias from the terminal line (Ctrl+U)
                // then send the real command + Enter
                if let Some(tabs) = &mut self.tabs {
                    if let Some(tab) = tabs.active_tab_mut() {
                        tab.terminal.grid_mut().set_scroll_offset(0);
                        let _ = tab.write(b"\x15"); // Ctrl+U: clear line
                        let cmd_with_enter = format!("{}\r", expanded);
                        let _ = tab.write(cmd_with_enter.as_bytes());
                    }
                }
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

        // Update git completions based on current input
        if let Some(partial) = crate::git::detect_git_completion(&self.input_line_buffer) {
            let completions = self.git_watcher.complete_branch(partial);
            if !completions.is_empty() {
                self.git_completions = completions;
                self.git_completion_index = Some(0);
                self.git_completions_visible = true;
            } else {
                self.git_completions_visible = false;
                self.git_completions.clear();
                self.git_completion_index = None;
            }
        } else {
            self.git_completions_visible = false;
            self.git_completions.clear();
            self.git_completion_index = None;
        }

        // Route keyboard input to the active tab — through pane manager if split,
        // otherwise directly to the tab's PTY
        if let Some(tabs) = &mut self.tabs {
            if let Some(tab) = tabs.active_tab_mut() {
                if let Some(ref mut pm) = tab.pane_manager {
                    if let Some(pane) = pm.focused_pane_mut() {
                        pane.terminal.grid_mut().set_scroll_offset(0);
                        if let Err(e) = pane.write(data) {
                            log::error!("Failed to write to pane PTY: {}", e);
                        }
                    }
                } else {
                    tab.terminal.grid_mut().set_scroll_offset(0);
                    if let Err(e) = tab.write(data) {
                        log::error!("Failed to write to PTY: {}", e);
                    }
                }
            }
        }
    }

    /// Copy `text` to the clipboard, emit a success toast with `label`.
    pub(super) fn copy_to_clipboard(&mut self, text: String, label: &str) {
        if let Some(clipboard) = &mut self.clipboard {
            match clipboard.set_text(&text) {
                Ok(()) => {
                    self.copy_flash_start = Some(Instant::now());
                    self.toasts.success(label.to_string());
                }
                Err(e) => {
                    log::error!("Clipboard write failed: {}", e);
                    self.toasts.error(format!("Copy failed: {}", e));
                }
            }
        } else {
            self.toasts.warning("Clipboard not available");
        }
    }
}
