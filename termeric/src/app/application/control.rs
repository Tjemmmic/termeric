//! The running-GUI control plane: answering control requests and the window,
//! tab, pane, and text operations they name.

use super::*;

impl Application {
    pub(super) fn handle_control_request(
        &mut self,
        event_loop: &ActiveEventLoop,
        mut request: crate::control::protocol::Request,
        peer_process: Option<u32>,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        use crate::control::protocol::{Request, Response};

        request.validate()?;
        request.canonicalize();
        if let Some(replay) = self.control_replays.replay(&request) {
            return replay;
        }
        if let Request::LaunchStatus { launch_request_id } = &request {
            let (state, terminal_closed) = self.control_launches.query(launch_request_id)?;
            return Ok(Response::LaunchStatus {
                process_id: std::process::id(),
                launch_request_id: launch_request_id.clone(),
                state,
                terminal_closed,
            });
        }
        if request
            .request_id()
            .is_some_and(|id| self.control_launches.contains(id))
        {
            return Err("Launch request ID remains retained but its mutation replay expired; query launch-status or use a new request ID".into());
        }
        if request == Request::Inventory {
            return Ok(Response::Inventory(self.control_inventory()));
        }
        if let Request::ReadText {
            window_id,
            tab_id,
            pane_id,
            history_lines,
            max_bytes,
        } = request
        {
            return self.read_control_text(window_id, tab_id, pane_id, history_lines, max_bytes);
        }
        if let Request::ListAgents { supervisor } = &request {
            let mut claimed = supervisor.is_none();
            let mut agents = Vec::new();
            for (native_id, control_id) in &self.control_window_ids {
                let Some(window) = self
                    .windows
                    .get(native_id)
                    .filter(|window| !window.should_exit)
                else {
                    continue;
                };
                match supervisor {
                    None => agents.extend(window.control_agents(*control_id)),
                    Some(token) if window.owns_supervisor_token(token) => {
                        claimed = true;
                        agents.extend(window.supervised_agents(*control_id));
                    }
                    Some(_) => {}
                }
            }
            if !claimed {
                return Err("That supervisor is no longer running".into());
            }
            agents.sort_by_key(|agent| (agent.window_id, agent.tab_id, agent.pane_id));
            let truncated = crate::control::protocol::fit_agent_summaries(&mut agents);
            return Ok(Response::Agents {
                process_id: std::process::id(),
                agents,
                truncated,
            });
        }
        if let Request::ReadConversation {
            window_id,
            session_id,
        } = request
        {
            let native_id = self.control_native_window_id(window_id)?;
            let (text, truncated) = self
                .live_window(native_id, window_id)?
                .read_agent_view_conversation(session_id)?;
            return Ok(Response::Conversation {
                process_id: std::process::id(),
                window_id,
                session_id,
                text,
                truncated,
            });
        }
        if let Request::AgentEvent {
            terminal_id,
            agent_id,
            event,
            detail,
            last_message,
            transcript_path,
        } = request
        {
            let report = crate::agents::AgentReport::from_wire(
                agent_id,
                event,
                detail,
                last_message,
                transcript_path,
            );
            let owned = self.windows.values_mut().any(|window| {
                window.record_agent_report(&terminal_id, report.clone(), peer_process)
            });
            self.remove_closed_windows(event_loop);
            return if owned {
                Ok(Response::AgentEventRecorded {
                    process_id: std::process::id(),
                })
            } else {
                Err("No terminal in this process has that identity".into())
            };
        }

        let result = match &request {
            Request::CreateWindow {
                request_id,
                source_window,
            } => self.create_control_window(event_loop, request_id, *source_window),
            Request::JoinLaunch {
                request_id,
                display,
                activation_token,
                build,
                tabs,
            } => self.join_launch(
                event_loop,
                request_id,
                display,
                build,
                activation_token.clone(),
                tabs,
            ),
            Request::LaunchWindow {
                request_id,
                source_window,
                program,
                args,
                working_directory,
                title,
                environment,
                environment_policy,
            } => self.launch_control_window(
                event_loop,
                request_id,
                *source_window,
                program,
                args,
                working_directory.as_deref(),
                title.as_deref(),
                environment,
                environment_policy,
            ),
            Request::CreateTab {
                request_id,
                window_id,
            } => self.create_control_tab(request_id, *window_id),
            Request::LaunchTab {
                background,
                request_id,
                window_id,
                program,
                args,
                working_directory,
                title,
                environment,
                environment_policy,
            } => self.launch_control_tab(
                *background,
                request_id,
                *window_id,
                program,
                args,
                working_directory.as_deref(),
                title.as_deref(),
                environment,
                environment_policy,
            ),
            Request::SplitPane {
                request_id,
                window_id,
                tab_id,
                pane_id,
                direction,
            } => self.create_control_pane(request_id, *window_id, *tab_id, *pane_id, *direction),
            Request::FocusWindow {
                request_id,
                window_id,
            } => self.focus_control_window(request_id, *window_id),
            Request::SetQuickAccessVisibility {
                request_id,
                window_id,
                visible,
            } => self.set_control_quick_access_visibility(request_id, *window_id, *visible),
            Request::SelectTab {
                request_id,
                window_id,
                tab_id,
            } => self.select_control_tab(request_id, *window_id, *tab_id),
            Request::FocusPane {
                request_id,
                window_id,
                tab_id,
                pane_id,
            } => self.focus_control_pane(request_id, *window_id, *tab_id, *pane_id),
            Request::CloseWindow {
                request_id,
                window_id,
            } => self.close_control_window(request_id, *window_id),
            Request::CloseTab {
                request_id,
                window_id,
                tab_id,
            } => self.close_control_tab(request_id, *window_id, *tab_id),
            Request::ClosePane {
                request_id,
                window_id,
                tab_id,
                pane_id,
            } => self.close_control_pane(request_id, *window_id, *tab_id, *pane_id),
            Request::MoveTab {
                request_id,
                source_window_id,
                tab_id,
                destination_window_id,
                destination_index,
            } => self.move_control_tab(
                request_id,
                *source_window_id,
                *tab_id,
                *destination_window_id,
                *destination_index,
            ),
            Request::MovePane {
                request_id,
                source_window_id,
                source_tab_id,
                pane_id,
                destination_window_id,
                destination_tab_id,
                target_pane_id,
                direction,
            } => self.move_control_pane(
                request_id,
                *source_window_id,
                *source_tab_id,
                *pane_id,
                *destination_window_id,
                *destination_tab_id,
                *target_pane_id,
                *direction,
            ),
            Request::ResizeWindow {
                request_id,
                window_id,
                width,
                height,
            } => self.resize_control_window(request_id, *window_id, *width, *height),
            Request::ResizeDivider {
                request_id,
                window_id,
                tab_id,
                divider_id,
                ratio_millis,
            } => self.resize_control_divider(
                request_id,
                *window_id,
                *tab_id,
                *divider_id,
                *ratio_millis,
            ),
            Request::SendText {
                request_id,
                window_id,
                tab_id,
                pane_id,
                text,
                paste,
                enter,
            } => self.send_control_text(
                request_id, *window_id, *tab_id, *pane_id, text, *paste, *enter,
            ),
            Request::SubmitToAgent {
                request_id,
                window_id,
                tab_id,
                pane_id,
                agent_id,
                text,
                supervisor,
            } => self.submit_control_text_to_agent(
                request_id,
                *window_id,
                *tab_id,
                *pane_id,
                agent_id,
                text,
                supervisor.as_deref(),
                peer_process,
            ),
            Request::AnswerApproval {
                request_id,
                window_id,
                session_id,
                approval_id,
                allow,
                reason,
                supervisor,
            } => self.answer_control_approval(
                request_id,
                *window_id,
                *session_id,
                approval_id,
                *allow,
                reason,
                supervisor.as_deref(),
                peer_process,
            ),
            Request::SubmitToSession {
                request_id,
                window_id,
                session_id,
                text,
                supervisor,
            } => self.submit_control_text_to_session(
                request_id,
                *window_id,
                *session_id,
                text,
                supervisor.as_deref(),
                peer_process,
            ),
            Request::Inventory
            | Request::ReadText { .. }
            | Request::LaunchStatus { .. }
            | Request::ListAgents { .. }
            | Request::ReadConversation { .. }
            | Request::AgentEvent { .. } => {
                unreachable!("read-only and agent requests returned before mutation dispatch")
            }
        }
        .map_err(|error| bounded_control_error(&error));
        self.control_replays.record(request, result.clone());
        result
    }

    pub(super) fn create_control_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        request_id: &str,
        source_window: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let total_tabs = process_tab_count(&self.windows);
        validate_control_window_creation_capacity(
            self.windows.len(),
            self.pending.len(),
            total_tabs,
        )?;
        let source_native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| {
                (*control_id == source_window).then_some(*native_id)
            })
            .ok_or_else(|| format!("GUI window {source_window} does not exist"))?;
        let source = self.live_window(source_native_id, source_window)?;
        let config = source.config.clone();
        let cwd = source
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .and_then(|tab| tab.working_directory());
        let result_index = self.pending.len();
        self.queue_default_window(config, cwd)
            .map_err(|error| format!("Could not prepare a new window: {error:#}"))?;
        let creation = self
            .create_pending_windows(event_loop)
            .into_iter()
            .nth(result_index)
            .ok_or_else(|| "New window creation produced no result".to_string())?;
        let window_id = creation?.window_id;
        Ok(crate::control::protocol::Response::WindowCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
        })
    }

    /// Opens the window a new launch asked for, so it shares this process
    /// with the windows already open.
    pub(super) fn join_launch(
        &mut self,
        event_loop: &ActiveEventLoop,
        request_id: &str,
        display: &str,
        build: &str,
        activation_token: Option<String>,
        tabs: &[crate::control::protocol::JoinTab],
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        // Only the same build joins: a rebuilt or different Termeric runs
        // its own code in a process of its own.
        if crate::control::build_identity().as_deref() != Some(build) {
            return Err("This Termeric is a different build".into());
        }
        let result_index = self.prepare_join_launch(
            crate::control::display_identity().as_deref(),
            display,
            activation_token,
            tabs,
        )?;
        let creation = self
            .create_pending_windows(event_loop)
            .into_iter()
            .nth(result_index)
            .ok_or_else(|| "New window creation produced no result".to_string())?;
        let window_id = creation?.window_id;
        Ok(crate::control::protocol::Response::WindowCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
        })
    }

    /// Queues the window a joining launch asked for, when it runs on this
    /// process's display (`own_display`). Returns its place among the
    /// windows waiting to be created.
    pub(super) fn prepare_join_launch(
        &mut self,
        own_display: Option<&str>,
        display: &str,
        activation_token: Option<String>,
        tabs: &[crate::control::protocol::JoinTab],
    ) -> std::result::Result<usize, String> {
        // Windows of one process share its display connection.
        if own_display != Some(display) {
            return Err("This Termeric runs on another display".into());
        }
        let total_tabs = process_tab_count(&self.windows);
        validate_control_window_creation_capacity(
            self.windows.len(),
            self.pending.len(),
            total_tabs,
        )?;
        // Settings are the process's, as for any window it opens.
        let config = self
            .window_order
            .iter()
            .find_map(|native_id| self.windows.get(native_id))
            .map(|window| window.config.clone())
            .ok_or("This Termeric has no open window to join")?;
        let mut launch = WindowLaunch {
            force_new: true,
            tabs: tabs
                .iter()
                .map(|tab| crate::cli::TabLaunch {
                    working_directory: tab.working_directory.as_ref().map(Into::into),
                    title: tab.title.clone(),
                    target: match &tab.program {
                        Some(program) => crate::cli::LaunchTarget::Command {
                            program: program.clone(),
                            args: tab.args.clone(),
                        },
                        None => crate::cli::LaunchTarget::Shell,
                    },
                    ..crate::cli::TabLaunch::default()
                })
                .collect(),
            ..WindowLaunch::default()
        };
        if launch.tabs.is_empty() {
            launch.tabs.push(crate::cli::TabLaunch::default());
        }
        let result_index = self.pending.len();
        self.queue_window(config, launch)
            .map_err(|error| format!("Could not prepare a new window: {error:#}"))?;
        if let Some(window) = self.pending.last_mut() {
            window.activation_token = activation_token;
        }
        Ok(result_index)
    }

    pub(super) fn launch_control_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        request_id: &str,
        source_window: u64,
        program: &str,
        args: &[String],
        working_directory: Option<&str>,
        title: Option<&str>,
        environment: &[crate::control::protocol::EnvironmentVariable],
        environment_policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        self.control_launches.prepare(request_id)?;
        let result_index = self.prepare_control_launch_window(
            source_window,
            program,
            args,
            working_directory,
            title,
            environment,
            environment_policy,
        )?;
        let creation = self
            .create_pending_windows(event_loop)
            .into_iter()
            .nth(result_index)
            .ok_or_else(|| "New command window creation produced no result".to_string())??;
        self.record_control_launch(request_id, creation.window_id, creation.initial_tab_id);
        Ok(crate::control::protocol::Response::WindowLaunched {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id: creation.window_id,
            tab_id: creation.initial_tab_id,
        })
    }

    pub(super) fn record_control_launch(&mut self, request_id: &str, window_id: u64, tab_id: u64) {
        let native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native, id)| (*id == window_id).then_some(native))
            .expect("successfully launched window has a control ID");
        let child = self
            .windows
            .get(native_id)
            .and_then(|window| window.tabs.as_ref())
            .and_then(|tabs| tabs.all_tabs().iter().find(|tab| tab.id as u64 == tab_id))
            .and_then(|tab| tab.child_observer())
            .expect("successfully launched command has an initial child");
        self.control_launches.record(request_id, child);
    }

    pub(super) fn prepare_control_launch_window(
        &mut self,
        source_window: u64,
        program: &str,
        args: &[String],
        working_directory: Option<&str>,
        title: Option<&str>,
        environment: &[crate::control::protocol::EnvironmentVariable],
        environment_policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> std::result::Result<usize, String> {
        let total_tabs = process_tab_count(&self.windows);
        validate_control_window_creation_capacity(
            self.windows.len(),
            self.pending.len(),
            total_tabs,
        )?;
        let source_native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| {
                (*control_id == source_window).then_some(*native_id)
            })
            .ok_or_else(|| format!("GUI window {source_window} does not exist"))?;
        let (config, inherited_cwd) = {
            let source = self.live_window(source_native_id, source_window)?;
            let inherited_cwd = source
                .tabs
                .as_ref()
                .and_then(|tabs| tabs.active_tab())
                .and_then(|tab| tab.working_directory());
            (source.config.clone(), inherited_cwd)
        };
        let selected_cwd = match working_directory {
            Some(directory) => std::path::PathBuf::from(directory),
            None => inherited_cwd
                .map_or_else(std::env::current_dir, |directory| {
                    Ok(std::path::PathBuf::from(directory))
                })
                .map_err(|error| {
                    format!("Could not determine the GUI process working directory: {error}")
                })?,
        };
        if !selected_cwd.is_absolute() || !selected_cwd.is_dir() {
            return Err(
                "Selected command-window working directory is not an available local directory; pass an existing absolute directory with --cwd"
                    .into(),
            );
        }
        if selected_cwd.to_str().is_none() {
            return Err(
                "Selected command-window working directory is not valid UTF-8; pass an existing UTF-8 absolute directory with --cwd"
                    .into(),
            );
        }
        let launch = WindowLaunch {
            force_new: true,
            tabs: vec![crate::cli::TabLaunch {
                working_directory: Some(selected_cwd),
                title: title.map(str::to_string),
                target: crate::cli::LaunchTarget::Command {
                    program: program.to_string(),
                    args: args.to_vec(),
                },
                environment: environment
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.value.clone()))
                    .collect(),
                environment_policy: environment_policy.clone(),
                splits: Vec::new(),
            }],
            ..WindowLaunch::default()
        };
        let result_index = self.pending.len();
        self.queue_window(config, launch)
            .map_err(|error| format!("Could not prepare a new command window: {error:#}"))?;
        self.pending[result_index].redact_startup_error_log = true;
        Ok(result_index)
    }

    pub(super) fn create_control_tab(
        &mut self,
        request_id: &str,
        window_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| (*control_id == window_id).then_some(*native_id))
            .ok_or_else(|| format!("GUI window {window_id} does not exist"))?;
        let target = self.live_window(native_id, window_id)?;
        let target_tabs = target
            .tabs
            .as_ref()
            .map_or(0, crate::tabs::TabManager::count);
        if target_tabs >= crate::control::protocol::MAX_TABS_PER_WINDOW {
            return Err(format!(
                "GUI window tab limit of {} reached",
                crate::control::protocol::MAX_TABS_PER_WINDOW
            ));
        }
        let total_tabs = process_tab_count(&self.windows);
        validate_process_tab_creation_capacity(total_tabs)?;
        let tab_id = self
            .windows
            .get_mut(&native_id)
            .expect("validated control window disappeared")
            .create_tab_for(Requester::Control)?;
        Ok(crate::control::protocol::Response::TabCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
        })
    }

    pub(super) fn launch_control_tab(
        &mut self,
        background: bool,
        request_id: &str,
        window_id: u64,
        program: &str,
        args: &[String],
        working_directory: Option<&str>,
        title: Option<&str>,
        environment: &[crate::control::protocol::EnvironmentVariable],
        environment_policy: &termeric_core::pty::EnvironmentPolicy,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        self.control_launches.prepare(request_id)?;
        let native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| (*control_id == window_id).then_some(*native_id))
            .ok_or_else(|| format!("GUI window {window_id} does not exist"))?;
        let target = self.live_window(native_id, window_id)?;
        let target_tabs = target
            .tabs
            .as_ref()
            .map_or(0, crate::tabs::TabManager::count);
        if target_tabs >= crate::control::protocol::MAX_TABS_PER_WINDOW {
            return Err(format!(
                "GUI window tab limit of {} reached",
                crate::control::protocol::MAX_TABS_PER_WINDOW
            ));
        }
        let total_tabs = process_tab_count(&self.windows);
        validate_process_tab_creation_capacity(total_tabs)?;
        let tab_id = self
            .windows
            .get_mut(&native_id)
            .expect("validated control window disappeared")
            .launch_control_tab(
                background,
                program,
                args,
                working_directory,
                title,
                environment,
                environment_policy,
            )?;
        self.record_control_launch(request_id, window_id, tab_id);
        Ok(crate::control::protocol::Response::TabCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
        })
    }

    pub(super) fn create_control_pane(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        direction: crate::panes::SplitDirection,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self
            .control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| (*control_id == window_id).then_some(*native_id))
            .ok_or_else(|| format!("GUI window {window_id} does not exist"))?;
        self.live_window(native_id, window_id)?;
        validate_process_pane_creation_capacity(process_pane_count(&self.windows))?;
        let created_pane = self
            .windows
            .get_mut(&native_id)
            .expect("validated control window disappeared")
            .create_control_pane(tab_id, pane_id, direction)?;
        Ok(crate::control::protocol::Response::PaneCreated {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id: created_pane,
        })
    }

    pub(super) fn focus_control_window(
        &mut self,
        request_id: &str,
        window_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let window = self.live_window(native_id, window_id)?;
        let native = window
            .window
            .as_ref()
            .ok_or_else(|| format!("GUI window {window_id} has no native window"))?;
        let already_focused = native.has_focus();
        if native_window_focus_disposition(window_id, already_focused, false, None, None)?
            == NativeWindowFocusDisposition::AlreadyFocused
        {
            return Ok(crate::control::protocol::Response::WindowFocusRequested {
                process_id: std::process::id(),
                request_id: request_id.to_string(),
                window_id,
                already_focused: true,
            });
        }
        let handle = native
            .window_handle()
            .map_err(|error| format!("Could not inspect GUI window {window_id}: {error}"))?;
        let disposition = native_window_focus_disposition(
            window_id,
            false,
            matches!(handle.as_raw(), RawWindowHandle::Wayland(_)),
            native.is_visible(),
            native.is_minimized(),
        )?;
        debug_assert_eq!(disposition, NativeWindowFocusDisposition::Request);
        native.focus_window();
        Ok(crate::control::protocol::Response::WindowFocusRequested {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            already_focused: false,
        })
    }

    pub(super) fn set_control_quick_access_visibility(
        &mut self,
        request_id: &str,
        window_id: u64,
        visible: bool,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let window = self.live_window_mut(native_id, window_id)?;
        if !window.launch.quick_access {
            return Err(format!(
                "GUI window {window_id} is not a quick-access terminal"
            ));
        }
        let native = window
            .window
            .clone()
            .ok_or_else(|| format!("GUI window {window_id} has no native window"))?;
        window.quick_access_visible = visible;
        native.set_visible(visible);
        window.handle_window_occlusion_change(!visible);
        if visible {
            native.set_minimized(false);
            native.focus_window();
            native.request_user_attention(Some(winit::window::UserAttentionType::Critical));
            window.request_redraw(RedrawReason::WindowEvent);
        }
        Ok(crate::control::protocol::Response::QuickAccessChanged {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            visible,
        })
    }

    pub(super) fn select_control_tab(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let changed = self
            .live_window_mut(native_id, window_id)?
            .select_tab_for(Requester::Control, tab_id)?;
        Ok(crate::control::protocol::Response::TabSelected {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            changed,
        })
    }

    pub(super) fn focus_control_pane(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (tab_changed, pane_changed) = self
            .live_window_mut(native_id, window_id)?
            .focus_pane_for(Requester::Control, tab_id, pane_id)?;
        Ok(crate::control::protocol::Response::PaneFocused {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id,
            tab_changed,
            pane_changed,
        })
    }

    pub(super) fn close_control_window(
        &mut self,
        request_id: &str,
        window_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let window = self
            .windows
            .get_mut(&native_id)
            .filter(|window| !window.should_exit)
            .ok_or_else(|| format!("GUI window {window_id} is already closing"))?;
        window.should_exit = true;
        Ok(crate::control::protocol::Response::WindowCloseRequested {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
        })
    }

    pub(super) fn close_control_tab(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        self.live_window_mut(native_id, window_id)?
            .close_tab_for(Requester::Control, tab_id)?;
        Ok(crate::control::protocol::Response::TabCloseRequested {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
        })
    }

    pub(super) fn close_control_pane(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (remaining_panes, focused_pane_id) = self
            .live_window_mut(native_id, window_id)?
            .close_pane_for(Requester::Control, tab_id, pane_id)?;
        Ok(crate::control::protocol::Response::PaneClosed {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id,
            remaining_panes,
            focused_pane_id,
        })
    }

    pub(super) fn move_control_tab(
        &mut self,
        request_id: &str,
        source_window_id: u64,
        tab_id: u64,
        destination_window_id: u64,
        destination_index: u16,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let source_native = self.control_native_window_id(source_window_id)?;
        let destination_native = self.control_native_window_id(destination_window_id)?;
        if source_native == destination_native {
            let changed = self
                .live_window_mut(source_native, source_window_id)?
                .move_control_tab_within_window(tab_id, destination_index)?;
            return Ok(crate::control::protocol::Response::TabMoved {
                process_id: std::process::id(),
                request_id: request_id.to_string(),
                source_window_id,
                source_tab_id: tab_id,
                destination_window_id,
                destination_tab_id: tab_id,
                destination_index,
                source_window_closing: false,
                changed,
            });
        }

        self.live_window(destination_native, destination_window_id)?;
        let mut destination = self
            .windows
            .remove(&destination_native)
            .expect("validated destination window remains present");
        let result = self
            .live_window_mut(source_native, source_window_id)
            .and_then(|source| {
                Self::transfer_control_tab(
                    source,
                    &mut destination,
                    tab_id,
                    usize::from(destination_index),
                )
            });
        self.windows.insert(destination_native, destination);
        let (destination_tab_id, source_window_closing) = result?;
        Ok(crate::control::protocol::Response::TabMoved {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            source_window_id,
            source_tab_id: tab_id,
            destination_window_id,
            destination_tab_id: u64::try_from(destination_tab_id)
                .map_err(|_| "Destination tab ID does not fit the control protocol".to_string())?,
            destination_index,
            source_window_closing,
            changed: true,
        })
    }

    pub(super) fn transfer_control_tab(
        source: &mut App,
        destination: &mut App,
        tab_id: u64,
        destination_index: usize,
    ) -> std::result::Result<(usize, bool), String> {
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let source_tabs = source
            .tabs
            .as_ref()
            .ok_or_else(|| "Source window has no initialized tab manager".to_string())?;
        let pinned = source_tabs
            .validate_window_tab_transfer_source_by_id(tab_id)
            .map_err(|error| format!("Could not move tab {tab_id}: {error:#}"))?;
        destination
            .tabs
            .as_ref()
            .ok_or_else(|| "Destination window has no initialized tab manager".to_string())?
            .validate_window_tab_transfer_destination(pinned, destination_index)
            .map_err(|error| format!("Could not move tab {tab_id}: {error:#}"))?;

        if let Some(refusal) = source.agent_view_tab_move_refusal(destination, tab_id) {
            return Err(format!("Could not move tab {tab_id}: {refusal}"));
        }
        if source
            .tab_rename
            .as_ref()
            .is_some_and(|rename| rename.tab_id == tab_id)
            && let Some(rename) = source.tab_rename.take()
            && let Some(tab) = source
                .tabs
                .as_mut()
                .and_then(|tabs| tabs.tab_mut_by_id(tab_id))
        {
            tab.set_user_title(rename.original);
        }
        let tab = source
            .tabs
            .as_mut()
            .expect("validated source tab manager remains present")
            .detach_tab_for_window_transfer(tab_id)
            .expect("prevalidated source tab remains detachable");
        let destination_tab_id = destination
            .tabs
            .as_mut()
            .expect("validated destination tab manager remains present")
            .attach_window_transferred_tab_at(tab, destination_index)
            .expect("prevalidated destination remains attachable");
        // The tab's agent view conversation moves with it.
        App::move_agent_view_tab(source, destination, tab_id, destination_tab_id);

        source.broadcast = crate::app::BroadcastState::Inactive;
        destination.broadcast = crate::app::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_capture = None;
        destination.terminal_mouse_capture = None;
        source.tab_dragging = None;
        destination.tab_dragging = None;
        source.refresh_focused_terminal_surfaces();
        destination.refresh_focused_terminal_surfaces();
        source.should_exit = source
            .tabs
            .as_ref()
            .is_none_or(crate::tabs::TabManager::is_empty);
        source.request_redraw(RedrawReason::Action);
        destination.request_redraw(RedrawReason::Action);
        Ok((destination_tab_id, source.should_exit))
    }

    pub(super) fn move_control_pane(
        &mut self,
        request_id: &str,
        source_window_id: u64,
        source_tab_id: u64,
        pane_id: u32,
        destination_window_id: u64,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: crate::panes::SplitDirection,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let source_native = self.control_native_window_id(source_window_id)?;
        let destination_native = self.control_native_window_id(destination_window_id)?;
        let destination_pane_id = if source_native == destination_native {
            self.live_window_mut(source_native, source_window_id)?
                .move_control_pane_within_window(
                    source_tab_id,
                    pane_id,
                    destination_tab_id,
                    target_pane_id,
                    direction,
                )?
        } else {
            self.live_window(destination_native, destination_window_id)?;
            let mut destination = self
                .windows
                .remove(&destination_native)
                .expect("validated destination window remains present");
            let result = self
                .live_window_mut(source_native, source_window_id)
                .and_then(|source| {
                    Self::transfer_control_pane(
                        source,
                        &mut destination,
                        source_tab_id,
                        pane_id,
                        destination_tab_id,
                        target_pane_id,
                        direction,
                    )
                });
            self.windows.insert(destination_native, destination);
            result?
        };
        Ok(crate::control::protocol::Response::PaneMoved {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            source_window_id,
            source_tab_id,
            source_pane_id: pane_id,
            destination_window_id,
            destination_tab_id,
            destination_pane_id,
            target_pane_id,
        })
    }

    pub(super) fn transfer_control_pane(
        source: &mut App,
        destination: &mut App,
        source_tab_id: u64,
        pane_id: u32,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: crate::panes::SplitDirection,
    ) -> std::result::Result<u32, String> {
        let source_tab_id = usize::try_from(source_tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let destination_tab_id = usize::try_from(destination_tab_id)
            .map_err(|_| "Destination tab ID does not fit this platform".to_string())?;
        let pane_id = crate::panes::PaneId(pane_id);
        let target_pane_id = crate::panes::PaneId(target_pane_id);
        source
            .tabs
            .as_ref()
            .ok_or_else(|| "Source window has no initialized tab manager".to_string())?
            .validate_pane_transfer_source_by_id(source_tab_id, pane_id)
            .map_err(|error| format!("Could not move pane {}: {error:#}", pane_id.0))?;
        destination
            .tabs
            .as_ref()
            .ok_or_else(|| "Destination window has no initialized tab manager".to_string())?
            .validate_pane_transfer_destination_by_id(destination_tab_id, target_pane_id)
            .map_err(|error| format!("Could not move pane {}: {error:#}", pane_id.0))?;
        destination
            .tabs
            .as_mut()
            .expect("validated destination tab manager remains present")
            .prepare_pane_transfer_destination_by_id(
                destination_tab_id,
                target_pane_id,
                destination.config.panes.cwd,
            )
            .map_err(|error| format!("Could not prepare destination pane: {error:#}"))?;
        let pane = source
            .tabs
            .as_mut()
            .expect("validated source tab manager remains present")
            .detach_pane_for_transfer_by_id(source_tab_id, pane_id)
            .expect("prevalidated source pane remains detachable");
        let destination_pane_id = destination
            .tabs
            .as_mut()
            .expect("prepared destination tab manager remains present")
            .attach_transferred_pane_by_id(destination_tab_id, target_pane_id, pane, direction)
            .expect("prevalidated destination remains attachable");

        source.broadcast = crate::app::BroadcastState::Inactive;
        destination.broadcast = crate::app::BroadcastState::Inactive;
        source.leave_copy_mode(false);
        destination.leave_copy_mode(false);
        source.terminal_mouse_capture = None;
        destination.terminal_mouse_capture = None;
        source.refresh_focused_terminal_surfaces();
        destination.refresh_focused_terminal_surfaces();
        source.request_redraw(RedrawReason::Action);
        destination.request_redraw(RedrawReason::Action);
        Ok(destination_pane_id.0)
    }

    pub(super) fn resize_control_window(
        &mut self,
        request_id: &str,
        window_id: u64,
        width: u32,
        height: u32,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let window = self.live_window(native_id, window_id)?;
        let native = window
            .window
            .as_ref()
            .ok_or_else(|| format!("GUI window {window_id} has no native window"))?;
        let immediate = native.request_inner_size(winit::dpi::PhysicalSize::new(width, height));
        Ok(crate::control::protocol::Response::WindowResizeRequested {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            requested_width: width,
            requested_height: height,
            immediate_width: immediate.map(|size| size.width),
            immediate_height: immediate.map(|size| size.height),
        })
    }

    pub(super) fn resize_control_divider(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (previous_ratio_millis, changed) = self
            .live_window_mut(native_id, window_id)?
            .resize_control_divider(tab_id, divider_id, ratio_millis)?;
        Ok(crate::control::protocol::Response::DividerResized {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            divider_id,
            previous_ratio_millis,
            ratio_millis,
            changed,
        })
    }

    pub(super) fn read_control_text(
        &self,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (screen, history_lines, text, truncated) = self
            .live_window(native_id, window_id)?
            .read_control_text(tab_id, pane_id, history_lines, max_bytes)?;
        Ok(crate::control::protocol::Response::Text {
            process_id: std::process::id(),
            window_id,
            tab_id,
            pane_id,
            screen,
            history_lines,
            text,
            truncated,
        })
    }

    pub(super) fn send_control_text(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        text: &str,
        paste: bool,
        enter: bool,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let (input_bytes, wire_bytes, disposition, confirmation_id) = self
            .live_window_mut(native_id, window_id)?
            .send_text_for(Requester::Control, tab_id, pane_id, text, paste, enter)?;
        Ok(crate::control::protocol::Response::InputAccepted {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id,
            input_bytes,
            wire_bytes,
            paste,
            enter,
            disposition,
            confirmation_id,
        })
    }

    pub(super) fn submit_control_text_to_agent(
        &mut self,
        request_id: &str,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        agent_id: &str,
        text: &str,
        supervisor: Option<&str>,
        peer_process: Option<u32>,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let pending_approval = self
            .live_window_mut(native_id, window_id)?
            .submit_control_text_to_agent(
                tab_id,
                pane_id,
                agent_id,
                text,
                supervisor,
                peer_process,
            )?;
        Ok(crate::control::protocol::Response::AgentInputAccepted {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            tab_id,
            pane_id,
            pending_approval,
        })
    }

    pub(super) fn answer_control_approval(
        &mut self,
        request_id: &str,
        window_id: u64,
        session_id: u64,
        approval_id: &str,
        allow: bool,
        reason: &str,
        supervisor: Option<&str>,
        peer_process: Option<u32>,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let answered = self
            .live_window_mut(native_id, window_id)?
            .answer_agent_view_approval(
                session_id,
                approval_id,
                allow,
                reason,
                supervisor,
                peer_process,
            )?;
        Ok(crate::control::protocol::Response::ApprovalAnswered {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            session_id,
            answered,
        })
    }

    pub(super) fn submit_control_text_to_session(
        &mut self,
        request_id: &str,
        window_id: u64,
        session_id: u64,
        text: &str,
        supervisor: Option<&str>,
        peer_process: Option<u32>,
    ) -> std::result::Result<crate::control::protocol::Response, String> {
        let native_id = self.control_native_window_id(window_id)?;
        let pending_approval = self
            .live_window_mut(native_id, window_id)?
            .submit_to_agent_view(session_id, text, supervisor, peer_process)?;
        Ok(crate::control::protocol::Response::SessionInputAccepted {
            process_id: std::process::id(),
            request_id: request_id.to_string(),
            window_id,
            session_id,
            pending_approval,
        })
    }

    pub(super) fn control_native_window_id(
        &self,
        window_id: u64,
    ) -> std::result::Result<WindowId, String> {
        self.control_window_ids
            .iter()
            .find_map(|(native_id, control_id)| (*control_id == window_id).then_some(*native_id))
            .ok_or_else(|| format!("GUI window {window_id} does not exist"))
    }
}
