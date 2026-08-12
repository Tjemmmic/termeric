//! Plugin command dispatch, context, transcript, export, and diff actions.

use super::*;

impl App {
    /// Build a PluginContext from current app state.
    pub(super) fn build_plugin_context(&self) -> PluginContext {
        let cwd = self
            .tabs
            .as_ref()
            .and_then(|t| t.active_tab())
            .and_then(|tab| tab.working_directory());

        let tab_count = self.tabs.as_ref().map_or(0, |t| t.count());
        let active_tab = self.tabs.as_ref().map_or(0, |t| t.active_index());

        PluginContext {
            cwd,
            tab_count,
            active_tab,
            terminal_size: (self.cols, self.rows),
        }
    }

    /// Dispatch a plugin hook and execute any returned actions.
    pub(super) fn dispatch_plugin_hook(&mut self, hook: crate::plugin::Hook) {
        let ctx = self.build_plugin_context();
        log::debug!(
            "Plugin hook: {:?} (tabs={}, active={}, size={}x{})",
            hook,
            ctx.tab_count,
            ctx.active_tab,
            ctx.terminal_size.0,
            ctx.terminal_size.1
        );
        let actions = self.plugin_manager.dispatch(&hook, &ctx);
        self.execute_plugin_actions(actions);
    }

    /// Execute a plugin command by ID.
    ///
    /// Commands that require direct App state access (review-diff, copy-diff, etc.)
    /// are matched by suffix so they work for any AI plugin (claude-code, codex, etc.).
    pub(super) fn execute_plugin_command(&mut self, command_id: &str) {
        // Extract the suffix after the first '.' for plugin-agnostic matching
        if let Some(suffix) = command_id.split_once('.').map(|(_, s)| s) {
            let plugin_id = command_id.split_once('.').map(|(p, _)| p).unwrap_or("");
            match suffix {
                "review-diff" => {
                    self.open_diff_review();
                    return;
                }
                "copy-diff" => {
                    self.copy_latest_diff();
                    return;
                }
                "insert-context" => {
                    self.open_context_picker_for(plugin_id);
                    return;
                }
                "export-session" => {
                    self.export_plugin_session(plugin_id);
                    return;
                }
                "jump-to-error" => {
                    self.jump_to_last_error(plugin_id);
                    return;
                }
                "select-profile" => {
                    self.execute_plugin_actions(vec![crate::plugin::PluginAction::ListProfiles {
                        plugin_id: plugin_id.to_string(),
                    }]);
                    return;
                }
                _ => {}
            }
        }
        let ctx = self.build_plugin_context();
        let actions = self.plugin_manager.execute_command(command_id, &ctx);
        if !actions.is_empty() {
            self.execute_plugin_actions(actions);
        }
    }

    /// Open the diff review overlay with the latest unreviewed diff
    pub(super) fn open_diff_review(&mut self) {
        if let Some(tabs) = &self.tabs {
            // Find the first tab with unreviewed diffs
            for (i, tab) in tabs.iter() {
                if tab.diff_detector.unreviewed_count() > 0 {
                    if let Some(diff) = tab.diff_detector.latest_unreviewed() {
                        self.diff_review.show(diff.clone(), i);
                        return;
                    }
                }
            }
        }
        log::info!("No unreviewed diffs to show");
        self.toasts.info("No unreviewed diffs");
    }

    /// Copy the latest detected diff to clipboard
    pub(super) fn copy_latest_diff(&mut self) {
        let diff = self.tabs.as_ref().and_then(|tabs| {
            tabs.iter()
                .find_map(|(_, tab)| tab.diff_detector.latest_unreviewed().cloned())
        });
        if let Some(diff) = diff {
            let filename = diff.filename.clone();
            self.copy_to_clipboard(diff.raw, "Copied diff");
            log::info!("Copied diff for '{}' to clipboard", filename);
            return;
        }
        log::info!("No diffs to copy");
        self.toasts.info("No unreviewed diffs to copy");
    }

    /// Open the context picker modal targeting a specific plugin tab
    pub(super) fn open_context_picker_for(&mut self, plugin_id: &str) {
        let plugin_is_open = self
            .tabs
            .as_ref()
            .is_some_and(|tabs| tabs.find_plugin_tab_index(plugin_id).is_some());
        if !plugin_is_open {
            self.toasts
                .warning(format!("Open {plugin_id} before inserting context"));
            return;
        }
        let cwd = if let Some(tabs) = &self.tabs {
            tabs.plugin_tab_cwd(plugin_id).or_else(|| {
                tabs.iter()
                    .find(|(_, t)| !t.closed)
                    .and_then(|(_, t)| t.working_directory())
            })
        } else {
            None
        };
        self.context_picker.target_plugin_id = Some(plugin_id.to_string());
        self.context_picker.open(cwd);
    }

    /// Gather context and inject it into the target plugin tab
    pub(super) fn inject_context(&mut self) {
        // Get terminal output from active shell tab (if selected)
        let terminal_output = if let Some(tabs) = &self.tabs {
            // Get output from the first non-plugin tab
            tabs.iter()
                .find(|(_, t)| matches!(t.tab_type, crate::tabs::TabType::Shell) && !t.closed)
                .map(|(_, tab)| {
                    let lines = tab.terminal.grid().export_scrollback_text();
                    let start = lines.len().saturating_sub(50);
                    lines[start..].join("\n")
                })
        } else {
            None
        };

        let assembled = self.context_picker.gather(terminal_output);

        // Inject into the target plugin tab's PTY
        let Some(target) = self.context_picker.target_plugin_id.clone() else {
            self.toasts.warning("No plugin selected for context");
            self.context_picker.dismiss();
            return;
        };
        self.inject_context_to_plugin(&target, &assembled);

        self.context_picker.dismiss();
    }

    /// Export the session transcript for a plugin tab to a Markdown file.
    ///
    /// Copies the terminal scrollback as text and writes it to a file in the
    /// session data directory. If a clipboard is available, also copies the path.
    pub(super) fn export_plugin_session(&mut self, plugin_id: &str) {
        let Some(tabs) = &self.tabs else {
            log::info!("No {} tab found for export", plugin_id);
            self.toasts.warning(format!("No {plugin_id} tab is open"));
            return;
        };
        let Some(idx) = tabs.find_plugin_tab_index(plugin_id) else {
            log::info!("No {} tab found for export", plugin_id);
            self.toasts.warning(format!("No {plugin_id} tab is open"));
            return;
        };

        // Try JSONL transcript export first (richer structured format)
        let transcript_path = tabs
            .iter()
            .find(|(i, _)| *i == idx)
            .and_then(|(_, t)| t.transcript.as_ref().map(|tr| tr.path().to_path_buf()));

        if let Some(ref path) = transcript_path {
            match crate::session::export::export_to_markdown(path) {
                Ok(md) => {
                    if let Some(base) = crate::session::sessions_base_dir() {
                        if let Err(error) = std::fs::create_dir_all(&base) {
                            self.toasts
                                .error(format!("Could not create session directory: {error}"));
                            return;
                        }
                        let id = crate::session::generate_session_id();
                        let filename = format!("{}-{}-export.md", plugin_id, id);
                        let dest = base.join(&filename);
                        match std::fs::write(&dest, &md) {
                            Ok(()) => {
                                let raw_dest =
                                    base.join(format!("{}-{}-export.jsonl", plugin_id, id));
                                if let Err(error) = std::fs::copy(path, &raw_dest) {
                                    self.toasts.error(format!(
                                        "Markdown exported, but full JSONL copy failed: {error}"
                                    ));
                                    return;
                                }
                                log::info!("Exported transcript to {}", dest.display());
                                self.announce_session_export(&dest, true);
                            }
                            Err(e) => {
                                log::error!("Failed to write export: {}", e);
                                self.toasts.error(format!("Session export failed: {e}"));
                            }
                        }
                        return;
                    } else {
                        self.toasts.error("Session data directory is unavailable");
                        return;
                    }
                }
                Err(e) => log::debug!(
                    "Transcript export failed, falling back to scrollback: {}",
                    e
                ),
            }
        }

        // Fallback: export raw scrollback text
        let content = tabs.iter().find(|(i, _)| *i == idx).map(|(_, t)| {
            let lines = t.terminal.grid().export_scrollback_text();
            lines.join("\n")
        });

        if let Some(text) = content {
            if text.trim().is_empty() {
                log::info!("No session content to export for '{}'", plugin_id);
                self.toasts
                    .info(format!("No {plugin_id} session content to export"));
                return;
            }
            if let Some(base) = crate::session::sessions_base_dir() {
                if let Err(error) = std::fs::create_dir_all(&base) {
                    self.toasts
                        .error(format!("Could not create session directory: {error}"));
                    return;
                }
                let id = crate::session::generate_session_id();
                let filename = format!("{}-{}-export.md", plugin_id, id);
                let path = base.join(&filename);
                let md = format!("# {} Session Export\n\n```\n{}\n```\n", plugin_id, text);
                match std::fs::write(&path, &md) {
                    Ok(()) => {
                        log::info!("Exported session to {}", path.display());
                        self.announce_session_export(&path, false);
                    }
                    Err(e) => {
                        log::error!("Failed to export session: {}", e);
                        self.toasts.error(format!("Session export failed: {e}"));
                    }
                }
            } else {
                self.toasts.error("Session data directory is unavailable");
            }
        }
    }

    pub(super) fn announce_session_export(&mut self, path: &std::path::Path, includes_jsonl: bool) {
        let path_text = path.display().to_string();
        let copied = self
            .clipboard
            .as_mut()
            .is_some_and(|clipboard| clipboard.set_text(path_text.clone()).is_ok());
        let suffix = if copied { " (path copied)" } else { "" };
        let format = if includes_jsonl {
            "Exported complete Markdown + JSONL"
        } else {
            "Exported scrollback Markdown"
        };
        self.toasts
            .success(format!("{format} to {path_text}{suffix}"));
    }

    /// Scroll the plugin tab to the last line containing an error-like pattern.
    ///
    /// First switches to the plugin tab (if not already active), then scans
    /// scrollback for error/failed/panic/exception patterns and scrolls there.
    pub(super) fn jump_to_last_error(&mut self, plugin_id: &str) {
        let Some(tabs) = &mut self.tabs else { return };

        // Switch to the plugin tab so active_tab_mut() works on it
        if let Some(idx) = tabs.find_plugin_tab_index(plugin_id) {
            tabs.switch_to(idx);
        } else {
            log::info!("No '{}' tab found for jump-to-error", plugin_id);
            self.toasts.warning(format!("No {plugin_id} tab is open"));
            return;
        }

        if let Some(tab) = tabs.active_tab_mut() {
            let lines = tab.terminal.grid().export_scrollback_text();
            let error_pos = lines.iter().rposition(|line| {
                let lower = line.to_lowercase();
                lower.contains("error")
                    || lower.contains("failed")
                    || lower.contains("panic")
                    || lower.contains("exception")
            });
            if let Some(pos) = error_pos {
                let offset = lines.len().saturating_sub(pos);
                tab.terminal.grid_mut().set_scroll_offset(offset);
                log::info!("Jumped to error at line {} in '{}'", pos, plugin_id);
            } else {
                log::info!("No errors found in '{}' tab output", plugin_id);
                self.toasts
                    .info(format!("No errors found in {plugin_id} output"));
            }
        }
    }

    /// Execute a list of plugin actions.
    pub(super) fn execute_plugin_actions(&mut self, actions: Vec<crate::plugin::PluginAction>) {
        use crate::plugin::PluginAction;
        use crate::tabs::TabType;

        for action in actions {
            match action {
                PluginAction::OpenTab {
                    program,
                    args,
                    cwd,
                    title,
                    plugin_id,
                    env,
                } => {
                    // Proactively check if the binary actually works before spawning a tab.
                    // We try running it with --version; if the binary doesn't exist or
                    // immediately fails, we show an in-app overlay instead of opening
                    // a ghost tab that exits instantly.
                    let binary_works = std::process::Command::new(&program)
                        .arg("--version")
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);

                    if !binary_works {
                        self.info_overlay.show(
                            format!("Plugin: '{}' not available", program),
                            vec![
                                format!("The '{}' binary could not be found or", program),
                                "is not working correctly.".into(),
                                String::new(),
                                "Please install it and ensure it is in your PATH,".into(),
                                "then try again.".into(),
                            ],
                        );
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        log::warn!(
                            "Plugin binary '{}' not available, showing info overlay",
                            program
                        );
                        continue;
                    }

                    let tab_type = plugin_id.map(|id| TabType::Plugin { plugin_id: id });
                    if let Some(tabs) = &mut self.tabs {
                        match tabs.new_tab_with_command(
                            &program,
                            &args,
                            cwd.as_deref(),
                            title.as_deref(),
                            tab_type,
                            env,
                        ) {
                            Ok(id) => {
                                log::info!("Plugin opened tab {} running '{}'", id, program);
                                self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated {
                                    tab_id: id,
                                });
                            }
                            Err(e) => {
                                self.info_overlay.show(
                                    format!("Failed to launch '{}'", program),
                                    vec![format!("{}", e)],
                                );
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                log::error!("Plugin failed to open tab for '{}': {}", program, e);
                            }
                        }
                    }
                }
                PluginAction::WriteToPty { data } => {
                    self.write_to_pty(&data);
                }
                PluginAction::Notify { title, body } => {
                    self.notifications.notify(&title, &body);
                }
                PluginAction::ShowMessage { message } => {
                    log::info!("Plugin message: {}", message);
                    self.toasts.info(message);
                }
                PluginAction::ShowNotice { title, lines } => {
                    log::info!("Plugin notice [{}]: {}", title, lines.join(" / "));
                    self.info_overlay.show(title, lines);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                PluginAction::ClearPluginTab { plugin_id } => {
                    let cleared = if let Some(tabs) = &mut self.tabs {
                        // Send Ctrl+C then clear
                        tabs.write_to_plugin_tab(&plugin_id, b"\x03")
                            && tabs.write_to_plugin_tab(&plugin_id, b"clear\r")
                    } else {
                        false
                    };
                    if cleared {
                        self.toasts.success(format!("Cleared {plugin_id}"));
                    } else {
                        self.toasts
                            .warning(format!("No writable {plugin_id} tab found"));
                    }
                }
                PluginAction::CopyFromPluginTab { plugin_id, lines } => {
                    let text = self
                        .tabs
                        .as_ref()
                        .and_then(|tabs| tabs.copy_from_plugin_tab(&plugin_id, lines));
                    if let Some(text) = text.filter(|text| !text.is_empty()) {
                        self.copy_to_clipboard(
                            text,
                            &format!("Copied last {lines} lines from {plugin_id}"),
                        );
                    } else {
                        self.toasts
                            .warning(format!("No output available from {plugin_id}"));
                    }
                }
                PluginAction::RestartPlugin { plugin_id } => {
                    // Close existing plugin tab
                    if let Some(tabs) = &mut self.tabs {
                        if let Some(idx) = tabs.find_plugin_tab_index(&plugin_id) {
                            tabs.close_tab(idx);
                        }
                    }
                    // Re-open via plugin command
                    let open_cmd = format!("{}.open", plugin_id);
                    let ctx = self.build_plugin_context();
                    let actions = self.plugin_manager.execute_command(&open_cmd, &ctx);
                    if !actions.is_empty() {
                        self.execute_plugin_actions(actions);
                    } else {
                        self.toasts.warning(format!("{plugin_id} is not enabled"));
                    }
                }
                PluginAction::ApplyDiff { diff_content, cwd } => {
                    self.run_git_apply("apply", &[], &diff_content, cwd.as_deref());
                }
                PluginAction::StageDiff { diff_content, cwd } => {
                    self.run_git_apply("stage", &["--cached"], &diff_content, cwd.as_deref());
                }
                PluginAction::RevertDiff { diff_content, cwd } => {
                    self.run_git_apply("revert", &["--reverse"], &diff_content, cwd.as_deref());
                }
                PluginAction::InjectContext { plugin_id, text } => {
                    self.inject_context_to_plugin(&plugin_id, &text);
                }
                PluginAction::ToggleTranscript => {
                    let has_session = self
                        .tabs
                        .as_ref()
                        .and_then(|tabs| tabs.active_tab())
                        .is_some_and(|tab| tab.ai_session.is_some());
                    if !has_session {
                        self.toasts
                            .warning("Activate an AI plugin tab to toggle its transcript");
                        continue;
                    }
                    if let Some(tabs) = &mut self.tabs {
                        if let Some(tab) = tabs.active_tab_mut() {
                            if let Some(ref mut session) = tab.ai_session {
                                session.toggle_recording();
                                if session.recording {
                                    // Start recording — create a new TranscriptRecorder
                                    if let Some(base) = crate::session::sessions_base_dir() {
                                        let transcript_dir = base.join("transcripts");
                                        let _ = std::fs::create_dir_all(&transcript_dir);
                                        match crate::session::transcript::TranscriptRecorder::new(
                                            &transcript_dir,
                                            session.repo_name.as_deref(),
                                            session.branch.as_deref(),
                                            &session.host.badge_label(),
                                            &session.profile,
                                        ) {
                                            Ok(recorder) => {
                                                let path = recorder.path().display().to_string();
                                                tab.start_transcript(recorder);
                                                log::info!(
                                                    "Transcript recording started: {}",
                                                    path
                                                );
                                                self.info_overlay.show(
                                                    "Transcript Started".to_string(),
                                                    vec![
                                                        "Recording session output to:".into(),
                                                        format!("  {}", path),
                                                        String::new(),
                                                        "Common token prefixes are redacted, but"
                                                            .into(),
                                                        "this is not a complete secret scanner."
                                                            .into(),
                                                    ],
                                                );
                                            }
                                            Err(e) => {
                                                session.recording = false; // revert
                                                log::error!("Failed to start transcript: {}", e);
                                                self.info_overlay.show(
                                                    "Transcript Error".to_string(),
                                                    vec![format!(
                                                        "Failed to start recording: {}",
                                                        e
                                                    )],
                                                );
                                            }
                                        }
                                    } else {
                                        session.recording = false;
                                        self.info_overlay.show(
                                            "Transcript Error".to_string(),
                                            vec!["Session data directory is unavailable".into()],
                                        );
                                    }
                                } else {
                                    // Stop recording — drop the recorder
                                    if let Some(recorder) = tab.stop_transcript() {
                                        let path = recorder.path().display().to_string();
                                        drop(recorder);
                                        log::info!("Transcript recording stopped: {}", path);
                                        self.info_overlay.show(
                                            "Transcript Stopped".to_string(),
                                            vec![
                                                "Transcript saved to:".into(),
                                                format!("  {}", path),
                                            ],
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                PluginAction::ListProfiles { plugin_id } => {
                    // Gather profile names from config
                    let profiles: Vec<String> = match plugin_id.as_str() {
                        "claude-code" => self
                            .config
                            .plugins
                            .claude_code
                            .profiles
                            .keys()
                            .cloned()
                            .collect(),
                        "codex" => self.config.plugins.codex.profiles.keys().cloned().collect(),
                        _ => Vec::new(),
                    };

                    if profiles.is_empty() {
                        self.info_overlay.show(
                            "No Profiles Configured".to_string(),
                            vec![
                                format!("No profiles found for '{}'.", plugin_id),
                                String::new(),
                                "Add profiles in config.toml under:".into(),
                                format!(
                                    "  [plugins.{}.profiles.<name>]",
                                    plugin_id.replace('-', "_")
                                ),
                            ],
                        );
                    } else {
                        let mut lines = vec!["Press a number key to select:".into(), String::new()];
                        for (i, name) in profiles.iter().enumerate() {
                            if i < 9 {
                                lines.push(format!("  {}  {}", i + 1, name));
                            }
                        }
                        // Also show current profile
                        if let Some(tabs) = &self.tabs {
                            if let Some((_, tab)) = tabs.iter().find(|(_, tab)| {
                                matches!(
                                    &tab.tab_type,
                                    crate::tabs::TabType::Plugin { plugin_id: id } if id == &plugin_id
                                )
                            }) {
                                if let Some(ref session) = tab.ai_session {
                                    lines.push(String::new());
                                    lines.push(format!("Current: {}", session.profile));
                                }
                            }
                        }
                        self.info_overlay
                            .show(format!("Select Profile — {}", plugin_id), lines);
                        self.pending_profile_selection = Some((plugin_id, profiles));
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                PluginAction::ApplyProfile {
                    plugin_id,
                    profile_name,
                } => {
                    let applied = self
                        .tabs
                        .as_mut()
                        .and_then(|tabs| tabs.plugin_tab_mut(&plugin_id))
                        .and_then(|tab| tab.ai_session.as_mut())
                        .map(|session| {
                            session.profile = profile_name.clone();
                        })
                        .is_some();
                    if !applied {
                        self.toasts
                            .warning(format!("No active {plugin_id} session found"));
                        continue;
                    }
                    log::info!("Profile for '{}' set to '{}'", plugin_id, profile_name);
                    self.info_overlay.show(
                        "Profile Applied".to_string(),
                        vec![
                            format!("Active profile: {}", profile_name),
                            String::new(),
                            "This profile will be used for the".into(),
                            "next session restart.".into(),
                        ],
                    );
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
        }
    }

    /// Run `git apply` with optional extra args, piping diff_content to stdin.
    pub(super) fn run_git_apply(
        &mut self,
        label: &str,
        extra_args: &[&str],
        diff_content: &str,
        cwd: Option<&str>,
    ) {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut cmd = Command::new("git");
        cmd.arg("apply");
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    if let Err(error) = stdin.write_all(diff_content.as_bytes()) {
                        let _ = child.kill();
                        let _ = child.wait();
                        self.show_git_action_failure(
                            label,
                            format!("Could not send diff: {error}"),
                        );
                        return;
                    }
                }
                match child.wait_with_output() {
                    Ok(output) if output.status.success() => {
                        log::info!("git {} succeeded", label);
                        self.toasts.success(format!("Git {label} succeeded"));
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        log::error!("git {} failed: {}", label, stderr.trim());
                        let detail = if stderr.trim().is_empty() {
                            format!("git apply exited with {}", output.status)
                        } else {
                            stderr.trim().to_string()
                        };
                        self.show_git_action_failure(label, detail);
                    }
                    Err(e) => {
                        log::error!("git {} wait failed: {}", label, e);
                        self.show_git_action_failure(label, e.to_string());
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to spawn git {}: {}", label, e);
                self.show_git_action_failure(label, e.to_string());
            }
        }
    }

    pub(super) fn show_git_action_failure(&mut self, label: &str, detail: String) {
        self.toasts.error(format!("Git {label} failed"));
        let mut lines = vec![format!("git {label} could not complete:"), String::new()];
        lines.extend(detail.lines().take(8).map(str::to_string));
        self.info_overlay.show("Git action failed", lines);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Toggle transcript recording for the active plugin session
    pub(super) fn toggle_transcript(&mut self) {
        self.execute_plugin_actions(vec![crate::plugin::PluginAction::ToggleTranscript]);
    }

    /// Inject context text into the explicitly selected plugin tab.
    pub(super) fn inject_context_to_plugin(&mut self, plugin_id: &str, text: &str) -> bool {
        let injected = format!("\n--- Context ---\n{}\n--- End Context ---\n", text);
        let written = self
            .tabs
            .as_mut()
            .is_some_and(|tabs| tabs.write_to_plugin_tab(plugin_id, injected.as_bytes()));
        if written {
            log::info!(
                "Injected {} chars of context into {}",
                injected.len(),
                plugin_id
            );
            self.toasts.success(format!("Context sent to {plugin_id}"));
        } else {
            self.toasts
                .warning(format!("No writable {plugin_id} tab found"));
        }
        written
    }
}
