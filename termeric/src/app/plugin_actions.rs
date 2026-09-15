//! Plugin command dispatch, context, transcript, export, and diff actions.

use super::*;

impl App {
    pub(super) fn handle_structured_plugin_completed(
        &mut self,
        completion: crate::plugin::structured::StructuredHandlerCompletion,
    ) {
        if self
            .pending_plugin_window_close
            .as_ref()
            .is_some_and(|pending| pending.matches(&completion))
        {
            self.pending_plugin_window_close = None;
            if let Err(error) = &completion.result {
                log::warn!(
                    "Plugin {:?} window-close result callback {} failed before teardown: {}",
                    completion.plugin_id,
                    completion.request_id,
                    error
                );
            }
            self.should_exit = true;
            return;
        }
        if completion.plugin_generation != self.plugin_runtime_generation
            || !self.plugin_manager.contains(&completion.plugin_id)
        {
            log::debug!(
                "Discarding stale structured plugin response {} from {:?}",
                completion.request_id,
                completion.plugin_id
            );
            return;
        }
        let mut continuation = completion.continuation;
        let mut workspace_operation = None;
        let actions = match completion.result {
            Ok(actions) => actions
                .into_iter()
                .filter_map(|action| {
                    match action {
                    crate::plugin::structured::StructuredResponseAction::ShowMessage {
                        message,
                    } => Some(crate::plugin::PluginAction::ShowMessage { message }),
                    crate::plugin::structured::StructuredResponseAction::ShowNotice {
                        title,
                        lines,
                    } => Some(crate::plugin::PluginAction::ShowNotice { title, lines }),
                    crate::plugin::structured::StructuredResponseAction::QueryWorkspace {
                        query_id,
                    } => {
                        workspace_operation = Some(
                            crate::plugin::structured::StructuredResponseAction::QueryWorkspace {
                                query_id,
                            },
                        );
                        None
                    }
                    crate::plugin::structured::StructuredResponseAction::QueryProcess {
                        query_id,
                    } => {
                        workspace_operation = Some(
                            crate::plugin::structured::StructuredResponseAction::QueryProcess {
                                query_id,
                            },
                        );
                        None
                    }
                    action @ (crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::FocusProcessPane {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::QueryProcessTerminal {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::SendProcessText {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::CreateProcessPane {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::MoveProcessPane {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::MoveProcessTab {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::CloseProcessPane {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::CloseProcessTab {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::SwapProcessPanes {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::BalanceProcessPanes {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::ApplyProcessPaneLayout {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::CloseProcessWindow {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::FocusProcessWindow {
                        ..
                    }) => {
                        workspace_operation = Some(action);
                        None
                    }
                    crate::plugin::structured::StructuredResponseAction::QueryTerminal {
                        query_id,
                        tab_id,
                        pane_id,
                        history_lines,
                        max_bytes,
                    } => {
                        workspace_operation = Some(
                            crate::plugin::structured::StructuredResponseAction::QueryTerminal {
                                query_id,
                                tab_id,
                                pane_id,
                                history_lines,
                                max_bytes,
                            },
                        );
                        None
                    }
                    action @ (crate::plugin::structured::StructuredResponseAction::SelectTab {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::FocusPane {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::MoveTab { .. }
                    | crate::plugin::structured::StructuredResponseAction::ResizeDivider {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::CreatePane {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::CreateTab {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::ClosePane {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::CloseTab {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::MovePane {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::SendText {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::SwapPanes {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::BalancePanes {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::ApplyPaneLayout {
                        ..
                    }
                    | crate::plugin::structured::StructuredResponseAction::CloseWindow {
                        ..
                    }) => {
                        workspace_operation = Some(action);
                        None
                    }
                }
                })
                .collect(),
            Err(error) => {
                log::error!(
                    "Structured plugin {:?} request {} failed: {}",
                    completion.plugin_id,
                    completion.request_id,
                    error
                );
                self.toasts.error(format!(
                    "Plugin {} event handler failed: {}",
                    completion.plugin_id, error
                ));
                Vec::new()
            }
        };
        if let Some(operation) = workspace_operation {
            let operation_name = match &operation {
                crate::plugin::structured::StructuredResponseAction::QueryWorkspace { .. } => {
                    "query"
                }
                crate::plugin::structured::StructuredResponseAction::QueryProcess { .. } => {
                    "process-query"
                }
                crate::plugin::structured::StructuredResponseAction::SelectProcessTab {
                    ..
                } => "select-process-tab",
                crate::plugin::structured::StructuredResponseAction::FocusProcessPane {
                    ..
                } => "focus-process-pane",
                crate::plugin::structured::StructuredResponseAction::QueryProcessTerminal {
                    ..
                } => "query-process-terminal",
                crate::plugin::structured::StructuredResponseAction::SendProcessText { .. } => {
                    "send-process-text"
                }
                crate::plugin::structured::StructuredResponseAction::CreateProcessTab {
                    ..
                } => "create-process-tab",
                crate::plugin::structured::StructuredResponseAction::CreateProcessPane {
                    ..
                } => "create-process-pane",
                crate::plugin::structured::StructuredResponseAction::MoveProcessPane { .. } => {
                    "move-process-pane"
                }
                crate::plugin::structured::StructuredResponseAction::MoveProcessTab { .. } => {
                    "move-process-tab"
                }
                crate::plugin::structured::StructuredResponseAction::CloseProcessPane {
                    ..
                } => "close-process-pane",
                crate::plugin::structured::StructuredResponseAction::CloseProcessTab { .. } => {
                    "close-process-tab"
                }
                crate::plugin::structured::StructuredResponseAction::SwapProcessPanes {
                    ..
                } => "swap-process-panes",
                crate::plugin::structured::StructuredResponseAction::BalanceProcessPanes {
                    ..
                } => "balance-process-panes",
                crate::plugin::structured::StructuredResponseAction::ApplyProcessPaneLayout {
                    ..
                } => "apply-process-pane-layout",
                crate::plugin::structured::StructuredResponseAction::CloseProcessWindow {
                    ..
                } => "close-process-window",
                crate::plugin::structured::StructuredResponseAction::FocusProcessWindow {
                    ..
                } => "focus-process-window",
                crate::plugin::structured::StructuredResponseAction::QueryTerminal { .. } => {
                    "terminal-query"
                }
                crate::plugin::structured::StructuredResponseAction::SelectTab { .. } => {
                    "select-tab"
                }
                crate::plugin::structured::StructuredResponseAction::FocusPane { .. } => {
                    "focus-pane"
                }
                crate::plugin::structured::StructuredResponseAction::MoveTab { .. } => "move-tab",
                crate::plugin::structured::StructuredResponseAction::ResizeDivider { .. } => {
                    "resize-divider"
                }
                crate::plugin::structured::StructuredResponseAction::CreatePane { .. } => {
                    "create-pane"
                }
                crate::plugin::structured::StructuredResponseAction::CreateTab { .. } => {
                    "create-tab"
                }
                crate::plugin::structured::StructuredResponseAction::ClosePane { .. } => {
                    "close-pane"
                }
                crate::plugin::structured::StructuredResponseAction::CloseTab { .. } => "close-tab",
                crate::plugin::structured::StructuredResponseAction::MovePane { .. } => "move-pane",
                crate::plugin::structured::StructuredResponseAction::SendText { .. } => "send-text",
                crate::plugin::structured::StructuredResponseAction::SwapPanes { .. } => {
                    "swap-panes"
                }
                crate::plugin::structured::StructuredResponseAction::BalancePanes { .. } => {
                    "balance-panes"
                }
                crate::plugin::structured::StructuredResponseAction::ApplyPaneLayout { .. } => {
                    "apply-pane-layout"
                }
                crate::plugin::structured::StructuredResponseAction::CloseWindow { .. } => {
                    "close-window"
                }
                _ => unreachable!("only workspace operations are retained"),
            };
            let result = self.continue_structured_workspace_operation(
                &completion.plugin_id,
                operation,
                continuation.take(),
            );
            if let Err(error) = result {
                log::error!(
                    "Structured plugin {:?} workspace {} failed: {}",
                    completion.plugin_id,
                    operation_name,
                    error
                );
                self.toasts.error(format!(
                    "Plugin {} workspace {} failed: {}",
                    completion.plugin_id, operation_name, error
                ));
            }
        }
        self.execute_plugin_actions(actions);
    }

    pub(super) fn continue_structured_workspace_operation(
        &mut self,
        plugin_id: &str,
        operation: crate::plugin::structured::StructuredResponseAction,
        continuation: Option<crate::plugin::structured::StructuredHandlerInvocation>,
    ) -> Result<(), String> {
        let mut invocation = continuation.ok_or_else(|| {
            "workspace operations require a live persistent handler continuation".to_string()
        })?;
        let schedules_window_close = matches!(
            operation,
            crate::plugin::structured::StructuredResponseAction::CloseWindow { .. }
        ) && self.validate_plugin_window_close().is_ok();
        match operation {
            crate::plugin::structured::StructuredResponseAction::QueryWorkspace { query_id } => {
                invocation.event_json =
                    self.structured_workspace_query_event(plugin_id, &query_id)?;
                // A query result cannot query again, but may perform one
                // separately granted workspace mutation.
                invocation.allow_workspace_query = false;
            }
            crate::plugin::structured::StructuredResponseAction::QueryProcess { .. } => {
                return Err(
                    "process queries require the process-owned Application window registry".into(),
                );
            }
            crate::plugin::structured::StructuredResponseAction::SelectProcessTab { .. }
            | crate::plugin::structured::StructuredResponseAction::FocusProcessPane { .. }
            | crate::plugin::structured::StructuredResponseAction::CreateProcessTab { .. }
            | crate::plugin::structured::StructuredResponseAction::CreateProcessPane { .. }
            | crate::plugin::structured::StructuredResponseAction::MoveProcessPane { .. } => {
                return Err(
                    "process actions require the process-owned Application window registry".into(),
                );
            }
            crate::plugin::structured::StructuredResponseAction::MoveProcessTab { .. } => {
                return Err(
                    "process actions require the process-owned Application window registry".into(),
                );
            }
            crate::plugin::structured::StructuredResponseAction::CloseProcessPane { .. } => {
                return Err(
                    "process actions require the process-owned Application window registry".into(),
                );
            }
            crate::plugin::structured::StructuredResponseAction::CloseProcessTab { .. } => {
                return Err(
                    "process actions require the process-owned Application window registry".into(),
                );
            }
            crate::plugin::structured::StructuredResponseAction::SwapProcessPanes { .. }
            | crate::plugin::structured::StructuredResponseAction::BalanceProcessPanes { .. }
            | crate::plugin::structured::StructuredResponseAction::ApplyProcessPaneLayout {
                ..
            }
            | crate::plugin::structured::StructuredResponseAction::CloseProcessWindow { .. }
            | crate::plugin::structured::StructuredResponseAction::FocusProcessWindow { .. } => {
                return Err(
                    "process actions require the process-owned Application window registry".into(),
                );
            }
            crate::plugin::structured::StructuredResponseAction::QueryTerminal {
                query_id,
                tab_id,
                pane_id,
                history_lines,
                max_bytes,
            } => {
                invocation.event_json = self.structured_terminal_query_event(
                    plugin_id,
                    &query_id,
                    tab_id,
                    pane_id,
                    history_lines,
                    max_bytes,
                )?;
                // Presented terminal content is the broadest disclosure in
                // the public API. Its result cannot initiate another query or
                // workspace mutation; a later external event may start a new
                // independently bounded request.
                clear_structured_host_authority(&mut invocation);
            }
            action @ (crate::plugin::structured::StructuredResponseAction::SelectTab { .. }
            | crate::plugin::structured::StructuredResponseAction::FocusPane { .. }
            | crate::plugin::structured::StructuredResponseAction::MoveTab { .. }
            | crate::plugin::structured::StructuredResponseAction::ResizeDivider {
                ..
            }
            | crate::plugin::structured::StructuredResponseAction::CreatePane {
                ..
            }
            | crate::plugin::structured::StructuredResponseAction::CreateTab { .. }
            | crate::plugin::structured::StructuredResponseAction::ClosePane { .. }
            | crate::plugin::structured::StructuredResponseAction::CloseTab { .. }
            | crate::plugin::structured::StructuredResponseAction::MovePane { .. }
            | crate::plugin::structured::StructuredResponseAction::SendText { .. }
            | crate::plugin::structured::StructuredResponseAction::SwapPanes { .. }
            | crate::plugin::structured::StructuredResponseAction::BalancePanes {
                ..
            }
            | crate::plugin::structured::StructuredResponseAction::ApplyPaneLayout {
                ..
            }
            | crate::plugin::structured::StructuredResponseAction::CloseWindow {
                ..
            }) => {
                invocation.event_json =
                    self.structured_workspace_action_event(plugin_id, action)?;
                // A mutation result is terminal for host operations.
                clear_structured_host_authority(&mut invocation);
            }
            _ => return Err("structured response did not contain a workspace operation".into()),
        }
        let plugin_generation = self.plugin_runtime_generation;
        let result_request_id = self.submit_structured_handler(invocation)?;
        if schedules_window_close {
            self.pending_plugin_window_close = Some(PendingPluginWindowClose {
                plugin_id: plugin_id.to_string(),
                plugin_generation,
                result_request_id,
            });
        }
        Ok(())
    }

    pub(super) fn continue_structured_process_query(
        &mut self,
        event_json: String,
        continuation: Option<crate::plugin::structured::StructuredHandlerInvocation>,
    ) -> Result<u64, String> {
        let mut invocation = continuation.ok_or_else(|| {
            "process queries require a live persistent handler continuation".to_string()
        })?;
        invocation.event_json = event_json;
        reduce_structured_process_query_authority(&mut invocation);
        self.submit_structured_handler(invocation)
    }

    pub(super) fn continue_structured_process_action(
        &mut self,
        event_json: String,
        continuation: Option<crate::plugin::structured::StructuredHandlerInvocation>,
    ) -> Result<u64, String> {
        let mut invocation = continuation.ok_or_else(|| {
            "process actions require a live persistent handler continuation".to_string()
        })?;
        invocation.event_json = event_json;
        clear_structured_host_authority(&mut invocation);
        self.submit_structured_handler(invocation)
    }

    pub(super) fn continue_structured_process_terminal_query(
        &mut self,
        event_json: String,
        continuation: Option<crate::plugin::structured::StructuredHandlerInvocation>,
    ) -> Result<u64, String> {
        let mut invocation = continuation.ok_or_else(|| {
            "process terminal queries require a live persistent handler continuation".to_string()
        })?;
        invocation.event_json = event_json;
        clear_structured_host_authority(&mut invocation);
        self.submit_structured_handler(invocation)
    }

    pub(super) fn continue_structured_process_terminal_input(
        &mut self,
        event_json: String,
        continuation: Option<crate::plugin::structured::StructuredHandlerInvocation>,
    ) -> Result<u64, String> {
        let mut invocation = continuation.ok_or_else(|| {
            "process terminal input requires a live persistent handler continuation".to_string()
        })?;
        invocation.event_json = event_json;
        clear_structured_host_authority(&mut invocation);
        self.submit_structured_handler(invocation)
    }

    fn submit_structured_handler(
        &mut self,
        invocation: crate::plugin::structured::StructuredHandlerInvocation,
    ) -> Result<u64, String> {
        let window = self
            .window
            .as_ref()
            .ok_or_else(|| "structured plugin handler requires a native window".to_string())?;
        let proxy = super::EVENT_LOOP_PROXY
            .get()
            .cloned()
            .ok_or_else(|| "structured plugin event loop is unavailable".to_string())?;
        self.structured_plugin_worker.submit(
            window.id(),
            self.plugin_runtime_generation,
            invocation,
            proxy,
        )
    }

    pub(super) fn structured_workspace_query_event(
        &self,
        plugin_id: &str,
        query_id: &str,
    ) -> Result<String, String> {
        const MAX_QUERY_TABS: usize = 16;
        const MAX_QUERY_PANES: usize = 32;
        const MAX_QUERY_DIVIDERS: usize = MAX_QUERY_PANES - 1;
        const MAX_QUERY_METADATA_BYTES: usize = 2 * 1024;
        const MAX_QUERY_WORKSPACE_ID_BYTES: usize = 512;
        const MAX_QUERY_CWD_BYTES: usize = 1024;

        let ctx = self.build_plugin_context();
        let mut cwd_budget = MAX_QUERY_CWD_BYTES;
        let (cwd, cwd_truncated) = ctx.cwd.as_deref().map_or((None, false), |cwd| {
            let (cwd, truncated) = super::control_metadata(cwd, &mut cwd_budget);
            (Some(cwd), truncated)
        });
        let mut workspace_budget = MAX_QUERY_WORKSPACE_ID_BYTES;
        let (workspace_id, workspace_truncated) =
            self.current_workspace_id
                .as_deref()
                .map_or((None, false), |workspace| {
                    let (workspace, truncated) =
                        super::control_metadata(workspace, &mut workspace_budget);
                    (Some(workspace), truncated)
                });
        let mut budget = super::ControlInventoryBudget {
            tabs: MAX_QUERY_TABS,
            panes: MAX_QUERY_PANES,
            dividers: MAX_QUERY_DIVIDERS,
            metadata_bytes: MAX_QUERY_METADATA_BYTES - MAX_QUERY_WORKSPACE_ID_BYTES,
        };
        // The plugin result is scoped to this exact App/window, so no synthetic
        // or process-global window identifier enters the public contract.
        let window = self.control_inventory(1, &mut budget);
        let envelope = serde_json::json!({
            "schema_version": 1,
            "plugin_id": plugin_id,
            "event": {
                "type": "workspace-query-result",
                "query_id": query_id,
                "snapshot_schema_version": 1,
                "active_working_directory": cwd,
                "active_working_directory_truncated": cwd_truncated,
                "tab_count": ctx.tab_count,
                "active_tab_index": ctx.active_tab,
                "terminal": {
                    "columns": ctx.terminal_size.0,
                    "rows": ctx.terminal_size.1,
                },
                "window": {
                    "focused": window.focused,
                    "visible": window.visible,
                    "quick_access": window.quick_access,
                    "workspace_id": workspace_id,
                    "pixel_width": window.pixel_width,
                    "pixel_height": window.pixel_height,
                    "tabs": window.tabs,
                    "truncated": window.truncated || workspace_truncated,
                },
            },
        });
        let encoded = serde_json::to_string(&envelope)
            .map_err(|error| format!("could not encode workspace query result: {error}"))?;
        if encoded.len() > crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES {
            return Err(format!(
                "workspace query result exceeds {} bytes",
                crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
            ));
        }
        Ok(encoded)
    }

    pub(super) fn structured_process_query_event(
        plugin_id: &str,
        query_id: &str,
        origin_window_id: u64,
        inventory: &crate::control::protocol::ProcessInventory,
    ) -> Result<String, String> {
        let envelope = serde_json::json!({
            "schema_version": 1,
            "plugin_id": plugin_id,
            "event": {
                "type": "process-query-result",
                "query_id": query_id,
                "snapshot_schema_version": 1,
                "process_id": inventory.process_id,
                "origin_window_id": origin_window_id,
                "windows": inventory.windows,
                "truncated": inventory.truncated,
            },
        });
        let encoded = serde_json::to_string(&envelope)
            .map_err(|error| format!("could not encode process query result: {error}"))?;
        if encoded.len() > crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES {
            return Err(format!(
                "process query result exceeds {} bytes",
                crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
            ));
        }
        Ok(encoded)
    }

    pub(super) fn structured_workspace_action_event(
        &mut self,
        plugin_id: &str,
        action: crate::plugin::structured::StructuredResponseAction,
    ) -> Result<String, String> {
        const MAX_ACTION_ERROR_BYTES: usize = 1024;

        let event = match action {
            crate::plugin::structured::StructuredResponseAction::SelectTab {
                action_id,
                tab_id,
            } => match self.select_plugin_tab(tab_id) {
                Ok(changed) => serde_json::json!({
                    "type": "workspace-action-result",
                    "action_id": action_id,
                    "action": "select-tab",
                    "ok": true,
                    "tab_id": tab_id,
                    "changed": changed,
                    "error": null,
                }),
                Err(error) => {
                    let mut budget = MAX_ACTION_ERROR_BYTES;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "select-tab",
                        "ok": false,
                        "tab_id": tab_id,
                        "changed": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            },
            crate::plugin::structured::StructuredResponseAction::FocusPane {
                action_id,
                tab_id,
                pane_id,
            } => match self.focus_plugin_pane(tab_id, pane_id) {
                Ok((tab_changed, pane_changed)) => serde_json::json!({
                    "type": "workspace-action-result",
                    "action_id": action_id,
                    "action": "focus-pane",
                    "ok": true,
                    "tab_id": tab_id,
                    "pane_id": pane_id,
                    "tab_changed": tab_changed,
                    "pane_changed": pane_changed,
                    "error": null,
                }),
                Err(error) => {
                    let mut budget = MAX_ACTION_ERROR_BYTES;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "focus-pane",
                        "ok": false,
                        "tab_id": tab_id,
                        "pane_id": pane_id,
                        "tab_changed": null,
                        "pane_changed": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            },
            crate::plugin::structured::StructuredResponseAction::MoveTab {
                action_id,
                tab_id,
                destination_index,
            } => match self.move_control_tab_within_window(tab_id, destination_index) {
                Ok(changed) => serde_json::json!({
                    "type": "workspace-action-result",
                    "action_id": action_id,
                    "action": "move-tab",
                    "ok": true,
                    "tab_id": tab_id,
                    "destination_index": destination_index,
                    "changed": changed,
                    "error": null,
                }),
                Err(error) => {
                    let mut budget = MAX_ACTION_ERROR_BYTES;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "move-tab",
                        "ok": false,
                        "tab_id": tab_id,
                        "destination_index": destination_index,
                        "changed": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            },
            crate::plugin::structured::StructuredResponseAction::ResizeDivider {
                action_id,
                tab_id,
                divider_id,
                ratio_millis,
            } => match self.resize_control_divider(tab_id, divider_id, ratio_millis) {
                Ok((previous_ratio_millis, changed)) => serde_json::json!({
                    "type": "workspace-action-result",
                    "action_id": action_id,
                    "action": "resize-divider",
                    "ok": true,
                    "tab_id": tab_id,
                    "divider_id": divider_id,
                    "previous_ratio_millis": previous_ratio_millis,
                    "ratio_millis": ratio_millis,
                    "changed": changed,
                    "error": null,
                }),
                Err(error) => {
                    let mut budget = MAX_ACTION_ERROR_BYTES;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "resize-divider",
                        "ok": false,
                        "tab_id": tab_id,
                        "divider_id": divider_id,
                        "previous_ratio_millis": null,
                        "ratio_millis": ratio_millis,
                        "changed": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            },
            crate::plugin::structured::StructuredResponseAction::CreatePane {
                action_id,
                tab_id,
                pane_id,
                direction,
            } => {
                let pane_direction = match direction {
                    crate::control::protocol::SplitDirection::Horizontal => {
                        crate::panes::SplitDirection::Horizontal
                    }
                    crate::control::protocol::SplitDirection::Vertical => {
                        crate::panes::SplitDirection::Vertical
                    }
                };
                match self.create_control_pane(tab_id, pane_id, pane_direction) {
                    Ok(created_pane_id) => serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "create-pane",
                        "ok": true,
                        "tab_id": tab_id,
                        "pane_id": pane_id,
                        "direction": direction,
                        "created_pane_id": created_pane_id,
                        "error": null,
                    }),
                    Err(error) => {
                        let mut budget = MAX_ACTION_ERROR_BYTES;
                        let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                        serde_json::json!({
                            "type": "workspace-action-result",
                            "action_id": action_id,
                            "action": "create-pane",
                            "ok": false,
                            "tab_id": tab_id,
                            "pane_id": pane_id,
                            "direction": direction,
                            "created_pane_id": null,
                            "error": error,
                            "error_truncated": error_truncated,
                        })
                    }
                }
            }
            crate::plugin::structured::StructuredResponseAction::CreateTab { action_id } => {
                match self.create_plugin_tab() {
                    Ok(created_tab_id) => serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "create-tab",
                        "ok": true,
                        "created_tab_id": created_tab_id,
                        "error": null,
                    }),
                    Err(error) => {
                        let mut budget = MAX_ACTION_ERROR_BYTES;
                        let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                        serde_json::json!({
                            "type": "workspace-action-result",
                            "action_id": action_id,
                            "action": "create-tab",
                            "ok": false,
                            "created_tab_id": null,
                            "error": error,
                            "error_truncated": error_truncated,
                        })
                    }
                }
            }
            crate::plugin::structured::StructuredResponseAction::ClosePane {
                action_id,
                tab_id,
                pane_id,
            } => match self.close_plugin_pane(tab_id, pane_id) {
                Ok((remaining_panes, focused_pane_id)) => serde_json::json!({
                    "type": "workspace-action-result",
                    "action_id": action_id,
                    "action": "close-pane",
                    "ok": true,
                    "tab_id": tab_id,
                    "pane_id": pane_id,
                    "remaining_panes": remaining_panes,
                    "focused_pane_id": focused_pane_id,
                    "error": null,
                }),
                Err(error) => {
                    let mut budget = MAX_ACTION_ERROR_BYTES;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "close-pane",
                        "ok": false,
                        "tab_id": tab_id,
                        "pane_id": pane_id,
                        "remaining_panes": null,
                        "focused_pane_id": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            },
            crate::plugin::structured::StructuredResponseAction::CloseTab { action_id, tab_id } => {
                match self.close_plugin_tab(tab_id) {
                    Ok((remaining_tabs, active_tab_id, active_changed)) => serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "close-tab",
                        "ok": true,
                        "tab_id": tab_id,
                        "scheduled": true,
                        "remaining_tabs": remaining_tabs,
                        "active_tab_id": active_tab_id,
                        "active_changed": active_changed,
                        "error": null,
                    }),
                    Err(error) => {
                        let mut budget = MAX_ACTION_ERROR_BYTES;
                        let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                        serde_json::json!({
                            "type": "workspace-action-result",
                            "action_id": action_id,
                            "action": "close-tab",
                            "ok": false,
                            "tab_id": tab_id,
                            "scheduled": false,
                            "remaining_tabs": null,
                            "active_tab_id": null,
                            "active_changed": null,
                            "error": error,
                            "error_truncated": error_truncated,
                        })
                    }
                }
            }
            crate::plugin::structured::StructuredResponseAction::MovePane {
                action_id,
                source_tab_id,
                pane_id,
                destination_tab_id,
                target_pane_id,
                direction,
            } => {
                let pane_direction = match direction {
                    crate::control::protocol::SplitDirection::Horizontal => {
                        crate::panes::SplitDirection::Horizontal
                    }
                    crate::control::protocol::SplitDirection::Vertical => {
                        crate::panes::SplitDirection::Vertical
                    }
                };
                match self.move_plugin_pane_within_window(
                    source_tab_id,
                    pane_id,
                    destination_tab_id,
                    target_pane_id,
                    pane_direction,
                ) {
                    Ok((destination_pane_id, source_remaining_panes, active_changed)) => {
                        serde_json::json!({
                            "type": "workspace-action-result",
                            "action_id": action_id,
                            "action": "move-pane",
                            "ok": true,
                            "source_tab_id": source_tab_id,
                            "pane_id": pane_id,
                            "destination_tab_id": destination_tab_id,
                            "target_pane_id": target_pane_id,
                            "direction": direction,
                            "destination_pane_id": destination_pane_id,
                            "source_remaining_panes": source_remaining_panes,
                            "active_tab_id": destination_tab_id,
                            "focused_pane_id": destination_pane_id,
                            "active_changed": active_changed,
                            "error": null,
                        })
                    }
                    Err(error) => {
                        let mut budget = MAX_ACTION_ERROR_BYTES;
                        let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                        serde_json::json!({
                            "type": "workspace-action-result",
                            "action_id": action_id,
                            "action": "move-pane",
                            "ok": false,
                            "source_tab_id": source_tab_id,
                            "pane_id": pane_id,
                            "destination_tab_id": destination_tab_id,
                            "target_pane_id": target_pane_id,
                            "direction": direction,
                            "destination_pane_id": null,
                            "source_remaining_panes": null,
                            "active_tab_id": null,
                            "focused_pane_id": null,
                            "active_changed": null,
                            "error": error,
                            "error_truncated": error_truncated,
                        })
                    }
                }
            }
            crate::plugin::structured::StructuredResponseAction::SendText {
                action_id,
                tab_id,
                pane_id,
                text,
                paste,
                enter,
            } => match self.send_plugin_text(tab_id, pane_id, &text, paste, enter) {
                Ok((input_bytes, wire_bytes, disposition, confirmation_id)) => {
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "send-text",
                        "ok": true,
                        "tab_id": tab_id,
                        "pane_id": pane_id,
                        "input_bytes": input_bytes,
                        "wire_bytes": wire_bytes,
                        "paste": paste,
                        "enter": enter,
                        "disposition": disposition,
                        "confirmation_id": confirmation_id,
                        "error": null,
                    })
                }
                Err(error) => {
                    let mut budget = MAX_ACTION_ERROR_BYTES;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "send-text",
                        "ok": false,
                        "tab_id": tab_id,
                        "pane_id": pane_id,
                        "input_bytes": null,
                        "wire_bytes": null,
                        "paste": paste,
                        "enter": enter,
                        "disposition": null,
                        "confirmation_id": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            },
            crate::plugin::structured::StructuredResponseAction::SwapPanes {
                action_id,
                tab_id,
                first_pane_id,
                second_pane_id,
            } => match self.swap_plugin_panes(tab_id, first_pane_id, second_pane_id) {
                Ok((pane_order, focused_pane_id, zoomed_pane_id)) => serde_json::json!({
                    "type": "workspace-action-result",
                    "action_id": action_id,
                    "action": "swap-panes",
                    "ok": true,
                    "tab_id": tab_id,
                    "first_pane_id": first_pane_id,
                    "second_pane_id": second_pane_id,
                    "pane_order": pane_order,
                    "focused_pane_id": focused_pane_id,
                    "zoomed_pane_id": zoomed_pane_id,
                    "error": null,
                }),
                Err(error) => {
                    let mut budget = MAX_ACTION_ERROR_BYTES;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "swap-panes",
                        "ok": false,
                        "tab_id": tab_id,
                        "first_pane_id": first_pane_id,
                        "second_pane_id": second_pane_id,
                        "pane_order": null,
                        "focused_pane_id": null,
                        "zoomed_pane_id": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            },
            crate::plugin::structured::StructuredResponseAction::BalancePanes {
                action_id,
                tab_id,
            } => match self.balance_plugin_panes(tab_id) {
                Ok((changed, pane_order, focused_pane_id, zoomed_pane_id)) => {
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "balance-panes",
                        "ok": true,
                        "tab_id": tab_id,
                        "changed": changed,
                        "pane_order": pane_order,
                        "focused_pane_id": focused_pane_id,
                        "zoomed_pane_id": zoomed_pane_id,
                        "error": null,
                    })
                }
                Err(error) => {
                    let mut budget = MAX_ACTION_ERROR_BYTES;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "balance-panes",
                        "ok": false,
                        "tab_id": tab_id,
                        "changed": null,
                        "pane_order": null,
                        "focused_pane_id": null,
                        "zoomed_pane_id": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            },
            crate::plugin::structured::StructuredResponseAction::ApplyPaneLayout {
                action_id,
                tab_id,
                preset,
            } => {
                let pane_preset = match preset {
                    crate::plugin::structured::PluginPaneLayoutPreset::Columns => {
                        crate::panes::PaneLayoutPreset::Columns
                    }
                    crate::plugin::structured::PluginPaneLayoutPreset::Rows => {
                        crate::panes::PaneLayoutPreset::Rows
                    }
                    crate::plugin::structured::PluginPaneLayoutPreset::Grid => {
                        crate::panes::PaneLayoutPreset::Grid
                    }
                    crate::plugin::structured::PluginPaneLayoutPreset::Tall => {
                        crate::panes::PaneLayoutPreset::Tall
                    }
                    crate::plugin::structured::PluginPaneLayoutPreset::Fat => {
                        crate::panes::PaneLayoutPreset::Fat
                    }
                };
                match self.apply_plugin_pane_layout(tab_id, pane_preset) {
                    Ok((pane_order, focused_pane_id, zoomed_pane_id)) => serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "apply-pane-layout",
                        "ok": true,
                        "tab_id": tab_id,
                        "preset": preset,
                        "pane_order": pane_order,
                        "focused_pane_id": focused_pane_id,
                        "zoomed_pane_id": zoomed_pane_id,
                        "error": null,
                    }),
                    Err(error) => {
                        let mut budget = MAX_ACTION_ERROR_BYTES;
                        let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                        serde_json::json!({
                            "type": "workspace-action-result",
                            "action_id": action_id,
                            "action": "apply-pane-layout",
                            "ok": false,
                            "tab_id": tab_id,
                            "preset": preset,
                            "pane_order": null,
                            "focused_pane_id": null,
                            "zoomed_pane_id": null,
                            "error": error,
                            "error_truncated": error_truncated,
                        })
                    }
                }
            }
            crate::plugin::structured::StructuredResponseAction::CloseWindow { action_id } => {
                match self.validate_plugin_window_close() {
                    Ok(tab_count) => serde_json::json!({
                        "type": "workspace-action-result",
                        "action_id": action_id,
                        "action": "close-window",
                        "ok": true,
                        "scheduled": true,
                        "tab_count": tab_count,
                        "error": null,
                    }),
                    Err(error) => {
                        let mut budget = MAX_ACTION_ERROR_BYTES;
                        let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                        serde_json::json!({
                            "type": "workspace-action-result",
                            "action_id": action_id,
                            "action": "close-window",
                            "ok": false,
                            "scheduled": false,
                            "tab_count": null,
                            "error": error,
                            "error_truncated": error_truncated,
                        })
                    }
                }
            }
            _ => return Err("structured response did not contain a workspace action".into()),
        };
        let envelope = serde_json::json!({
            "schema_version": 1,
            "plugin_id": plugin_id,
            "event": event,
        });
        let encoded = serde_json::to_string(&envelope)
            .map_err(|error| format!("could not encode workspace action result: {error}"))?;
        if encoded.len() > crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES {
            return Err(format!(
                "workspace action result exceeds {} bytes",
                crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES
            ));
        }
        Ok(encoded)
    }

    pub(super) fn validate_plugin_window_close(&self) -> Result<u16, String> {
        if self.should_exit {
            return Err("current window is already closing".into());
        }
        if self.pending_plugin_window_close.is_some() {
            return Err("current window already has a pending plugin closure".into());
        }
        let tab_count = self
            .tabs
            .as_ref()
            .map(|tabs| tabs.all_tabs().len())
            .filter(|count| *count > 0)
            .ok_or_else(|| "current window has no initialized tab owners".to_string())?;
        u16::try_from(tab_count).map_err(|_| "current window tab count is unsupported".into())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn structured_terminal_query_event(
        &self,
        plugin_id: &str,
        query_id: &str,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    ) -> Result<String, String> {
        if history_lines > crate::control::protocol::MAX_TEXT_HISTORY_LINES {
            return Err(format!(
                "terminal query history is limited to {} lines",
                crate::control::protocol::MAX_TEXT_HISTORY_LINES
            ));
        }
        if !(1..=crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES).contains(&max_bytes) {
            return Err(format!(
                "terminal query bytes must be between 1 and {}",
                crate::plugin::structured::MAX_TERMINAL_QUERY_BYTES
            ));
        }

        let mut attempt_bytes = max_bytes;
        loop {
            let event = match self.read_control_text(tab_id, pane_id, history_lines, attempt_bytes)
            {
                Ok((screen, included_history, text, truncated)) => serde_json::json!({
                    "type": "terminal-query-result",
                    "query_id": query_id,
                    "text_schema_version": 1,
                    "ok": true,
                    "tab_id": tab_id,
                    "pane_id": pane_id,
                    "screen": screen,
                    "requested_history_lines": history_lines,
                    "requested_max_bytes": max_bytes,
                    "history_lines": included_history,
                    "text_bytes": text.len(),
                    "text": text,
                    "truncated": truncated || attempt_bytes < max_bytes,
                    "error": null,
                }),
                Err(error) => {
                    let mut budget = 1024;
                    let (error, error_truncated) = super::control_metadata(&error, &mut budget);
                    serde_json::json!({
                        "type": "terminal-query-result",
                        "query_id": query_id,
                        "text_schema_version": 1,
                        "ok": false,
                        "tab_id": tab_id,
                        "pane_id": pane_id,
                        "screen": null,
                        "requested_history_lines": history_lines,
                        "requested_max_bytes": max_bytes,
                        "history_lines": null,
                        "text_bytes": 0,
                        "text": null,
                        "truncated": null,
                        "error": error,
                        "error_truncated": error_truncated,
                    })
                }
            };
            let envelope = serde_json::json!({
                "schema_version": 1,
                "plugin_id": plugin_id,
                "event": event,
            });
            let encoded = serde_json::to_string(&envelope)
                .map_err(|error| format!("could not encode terminal query result: {error}"))?;
            let limit = crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES;
            if encoded.len() <= limit {
                return Ok(encoded);
            }
            if attempt_bytes == 1 {
                return Err(format!("terminal query result exceeds {limit} bytes"));
            }
            let scaled = (usize::try_from(attempt_bytes).expect("u32 fits usize") * limit
                / encoded.len())
            .min(usize::try_from(attempt_bytes - 1).expect("u32 fits usize"))
            .max(1);
            attempt_bytes = u32::try_from(scaled).expect("scaled terminal query budget fits u32");
        }
    }

    pub(super) fn apply_diff_review_action(
        &mut self,
        action: crate::ui::diff_review::DiffReviewAction,
    ) {
        use crate::ui::diff_review::DiffReviewAction;

        match action {
            DiffReviewAction::Copy { request_id } => {
                let raw = self.diff_review.target(request_id).and_then(|target| {
                    self.tabs
                        .as_ref()?
                        .iter()
                        .find(|(_, tab)| tab.id == target.tab_id)
                        .and_then(|(_, tab)| tab.diff_detector.detected_by_id(target.diff_id))
                        .map(|diff| diff.raw.clone())
                });
                if let Some(raw) = raw {
                    self.copy_to_clipboard(raw, "Copied diff to clipboard");
                } else if self.diff_review.is_awaiting(request_id) {
                    self.toasts
                        .warning("The reviewed diff is no longer available");
                }
            }
            DiffReviewAction::Dismiss { request_id } => {
                let target = self.diff_review.target(request_id);
                if self.diff_review.dismiss(request_id)
                    && let (Some(target), Some(tabs)) = (target, &mut self.tabs)
                    && let Some(tab) = tabs.tab_mut_by_id(target.tab_id)
                {
                    tab.diff_detector.mark_reviewed(target.diff_id);
                }
            }
            DiffReviewAction::ScrollRows { request_id, delta } => {
                self.diff_review.scroll_rows(request_id, delta);
            }
            DiffReviewAction::ScrollColumns { request_id, delta } => {
                self.diff_review.scroll_columns(request_id, delta);
            }
            DiffReviewAction::ScrollToTop { request_id } => {
                self.diff_review.scroll_to_top(request_id);
            }
            DiffReviewAction::ScrollToBottom { request_id } => {
                self.diff_review.scroll_to_bottom(request_id);
            }
            DiffReviewAction::ScrollToLeft { request_id } => {
                self.diff_review.scroll_to_left(request_id);
            }
            DiffReviewAction::ScrollToRight { request_id } => {
                self.diff_review.scroll_to_right(request_id);
            }
            DiffReviewAction::None => {}
        }
    }

    pub(super) fn apply_info_overlay_action(
        &mut self,
        action: crate::ui::info_overlay::InfoOverlayAction,
    ) {
        use crate::ui::info_overlay::InfoOverlayAction;
        match action {
            InfoOverlayAction::Copy { request_id } => {
                if let Some(text) = self.info_overlay.copy_text(request_id) {
                    self.copy_to_clipboard(text, "Copied notice to clipboard");
                }
            }
            InfoOverlayAction::Dismiss { request_id } => {
                if self.info_overlay.dismiss(request_id) {
                    self.dismiss_pending_kitty_download(request_id);
                    // A replacement notice may have superseded a profile
                    // picker. Any older hidden action context is obsolete too.
                    self.pending_profile_selection = None;
                    self.pending_runtime_plugin_management = None;
                    self.pending_font_range_management = None;
                    self.pending_font_feature_management = None;
                    self.pending_file_upload = None;
                }
            }
            InfoOverlayAction::Choose {
                request_id,
                choice_id,
            } => {
                let choice_is_current = self.info_overlay.choice(request_id, &choice_id).is_some();
                if !choice_is_current {
                    return;
                }
                if self.handle_kitty_download_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_kitty_upload_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_font_feature_manager_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_font_range_manager_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_runtime_plugin_manager_choice(request_id, &choice_id) {
                    return;
                }
                if self
                    .pending_file_upload
                    .as_ref()
                    .is_some_and(|pending| pending.request_id == request_id)
                    && matches!(
                        choice_id.as_str(),
                        "file-upload-confirm" | "file-upload-cancel"
                    )
                {
                    self.info_overlay.dismiss(request_id);
                    let upload = self.pending_file_upload.take();
                    if choice_id == "file-upload-confirm" {
                        if let Some(upload) = upload {
                            self.start_file_upload(upload);
                        }
                    }
                    return;
                }
                let selection = self
                    .pending_profile_selection
                    .as_ref()
                    .filter(|pending| pending.request_id == request_id)
                    .filter(|pending| pending.profiles.iter().any(|name| name == &choice_id))
                    .map(|pending| (pending.terminal, pending.plugin_id.clone(), choice_id));
                let Some((terminal, plugin_id, profile_name)) = selection else {
                    return;
                };
                self.info_overlay.dismiss(request_id);
                self.pending_profile_selection = None;
                self.apply_plugin_profile(terminal, &plugin_id, profile_name);
            }
            InfoOverlayAction::SubmitInput { request_id } => {
                if !self.submit_font_feature_manager_input(request_id)
                    && !self.submit_font_range_manager_input(request_id)
                    && !self.submit_runtime_plugin_manager_input(request_id)
                    && self.info_overlay.dismiss(request_id)
                {
                    self.dismiss_pending_kitty_download(request_id);
                    self.pending_profile_selection = None;
                    self.pending_runtime_plugin_management = None;
                    self.pending_font_range_management = None;
                    self.pending_font_feature_management = None;
                    self.pending_file_upload = None;
                }
            }
            InfoOverlayAction::CancelInput { request_id } => {
                if !self.cancel_font_feature_manager_input(request_id)
                    && !self.cancel_font_range_manager_input(request_id)
                    && !self.cancel_runtime_plugin_manager_input(request_id)
                    && self.info_overlay.dismiss(request_id)
                {
                    self.dismiss_pending_kitty_download(request_id);
                    self.pending_profile_selection = None;
                    self.pending_runtime_plugin_management = None;
                    self.pending_font_range_management = None;
                    self.pending_font_feature_management = None;
                    self.pending_file_upload = None;
                }
            }
            InfoOverlayAction::ScrollLines { request_id, delta } => {
                self.info_overlay.scroll_lines(request_id, delta);
            }
            InfoOverlayAction::ScrollColumns { request_id, delta } => {
                self.info_overlay.scroll_columns(request_id, delta);
            }
            InfoOverlayAction::ScrollToTop { request_id } => {
                self.info_overlay.scroll_to_top(request_id);
            }
            InfoOverlayAction::ScrollToBottom { request_id } => {
                self.info_overlay.scroll_to_bottom(request_id);
            }
            InfoOverlayAction::ScrollToLeft { request_id } => {
                self.info_overlay.scroll_to_left(request_id);
            }
            InfoOverlayAction::ScrollToRight { request_id } => {
                self.info_overlay.scroll_to_right(request_id);
            }
            InfoOverlayAction::None => {}
        }
    }

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
            "Plugin hook: {} (tabs={}, active={}, size={}x{})",
            hook.name(),
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
    /// are matched only for the built-in AI plugin namespaces. Third-party
    /// commands are routed to their exact owning plugin.
    pub(super) fn execute_plugin_command(&mut self, command_id: &str) {
        if let Some((plugin_id, suffix)) = command_id.split_once('.')
            && matches!(plugin_id, "claude-code" | "codex")
        {
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
            for (_, tab) in tabs.iter() {
                if tab.diff_detector.unreviewed_count() > 0 {
                    if let Some(diff) = tab.diff_detector.latest_unreviewed() {
                        self.diff_review.show(diff, tab.id);
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
        let Some((cwd, target)) = self.tabs.as_ref().and_then(|tabs| {
            let (tab, pane_id) = tabs
                .iter()
                .find_map(|(_, tab)| tab.plugin_pane_id(plugin_id).map(|pane_id| (tab, pane_id)))?;
            let terminal = crate::tabs::TerminalIdentity {
                tab_id: tab.id,
                pane_id,
            };
            let cwd = tab.pane_working_directory(pane_id).or_else(|| {
                tabs.iter()
                    .find(|(_, candidate)| !candidate.closed)
                    .and_then(|(_, candidate)| candidate.working_directory())
            });
            Some((
                cwd,
                crate::ui::context_picker::ContextTarget {
                    plugin_id: plugin_id.to_string(),
                    terminal,
                },
            ))
        }) else {
            self.toasts
                .warning(format!("Open {plugin_id} before inserting context"));
            return;
        };
        self.context_picker.open(cwd, target);
    }

    pub(super) fn apply_context_picker_action(
        &mut self,
        action: crate::ui::context_picker::ContextPickerAction,
    ) {
        let start = matches!(
            action,
            crate::ui::context_picker::ContextPickerAction::Primary { request_id }
                if self.context_picker.is_awaiting(request_id)
                    && self.context_picker.phase()
                        == crate::ui::context_picker::ContextPickerPhase::Sources
        );
        let request_id = self.context_picker.request_id();
        self.context_picker.apply(action);
        if start {
            self.start_context_gather(request_id);
        }
    }

    fn start_context_gather(&mut self, request_id: u64) {
        let terminal_output = if let Some(tabs) = &self.tabs {
            tabs.iter()
                .find(|(_, t)| {
                    matches!(
                        t.tab_type,
                        crate::tabs::TabType::Shell | crate::tabs::TabType::Profile { .. }
                    ) && !t.closed
                })
                .and_then(|(_, tab)| {
                    let lines = tab.focused_terminal()?.grid().export_scrollback_text();
                    let start = lines.len().saturating_sub(50);
                    Some(lines[start..].join("\n"))
                })
        } else {
            None
        };
        let Some(request) = self
            .context_picker
            .begin_gather(request_id, terminal_output)
        else {
            return;
        };
        let Some(window_id) = self.window.as_ref().map(|window| window.id()) else {
            self.handle_context_gathered(
                request_id,
                Err("Context gathering requires a native window".into()),
            );
            return;
        };
        let Some(proxy) = super::EVENT_LOOP_PROXY.get().cloned() else {
            self.handle_context_gathered(
                request_id,
                Err("Context gathering event channel is unavailable".into()),
            );
            return;
        };
        let spawn = std::thread::Builder::new()
            .name("termeric-context-gather".into())
            .spawn(move || {
                let result = crate::ui::context_picker::gather_context(request);
                let _ = proxy.send_event(super::AppEvent::ContextGathered {
                    window_id,
                    request_id,
                    result,
                });
            });
        if let Err(error) = spawn {
            self.handle_context_gathered(
                request_id,
                Err(format!("Could not start context gathering: {error}")),
            );
        }
    }

    pub(super) fn handle_context_gathered(
        &mut self,
        request_id: u64,
        result: Result<String, String>,
    ) {
        use crate::ui::context_picker::ContextPickerCompletion;
        match self.context_picker.complete(request_id, result) {
            ContextPickerCompletion::Ignored => return,
            ContextPickerCompletion::Failed => {}
            ContextPickerCompletion::Ready { target, text } => {
                self.inject_context_to_target(&target, &text);
            }
        }
        self.redraw_scheduler
            .request(super::redraw_scheduler::RedrawReason::WindowEvent);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn inject_context_to_target(
        &mut self,
        target: &crate::ui::context_picker::ContextTarget,
        text: &str,
    ) -> bool {
        let origin = self.tabs.as_ref().and_then(|tabs| {
            let tab = tabs.tab_by_id(target.terminal.tab_id)?;
            if tab.pane_plugin_id(target.terminal.pane_id) != Some(target.plugin_id.as_str()) {
                return None;
            }
            tabs.origin_for_pane(target.terminal.tab_id, target.terminal.pane_id)
        });
        let injected = format!("\n--- Context ---\n{text}\n--- End Context ---\n");
        let written = origin.is_some_and(|origin| {
            self.tabs.as_mut().is_some_and(|tabs| {
                tabs.write_user_to_origin(origin, injected.as_bytes())
                    .is_ok()
            })
        });
        if written {
            self.toasts
                .success(format!("Context sent to {}", target.plugin_id));
        } else {
            self.toasts.warning(format!(
                "The original {} context destination is no longer writable",
                target.plugin_id
            ));
        }
        written
    }

    /// Scroll the selected plugin owner to its last error-like line.
    ///
    /// Focus that exact live owner, refresh its surfaces, then scan only its
    /// scrollback for error/failed/panic/exception patterns.
    pub(super) fn jump_to_last_error(&mut self, plugin_id: &str) {
        let Some(identity) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.plugin_terminal_identity(plugin_id))
        else {
            log::info!("No '{}' tab found for jump-to-error", plugin_id);
            self.toasts.warning(format!("No {plugin_id} tab is open"));
            return;
        };
        if self
            .focus_plugin_pane(identity.tab_id as u64, identity.pane_id.0)
            .is_err()
        {
            self.toasts
                .warning(format!("The {plugin_id} terminal is no longer available"));
            return;
        }
        let terminal = self.tabs.as_mut().and_then(|tabs| {
            let origin = tabs.origin_for_pane(identity.tab_id, identity.pane_id)?;
            tabs.terminal_for_origin_mut(origin)
        });
        if let Some(terminal) = terminal {
            let lines = terminal.grid().export_scrollback_text();
            let error_pos = lines.iter().rposition(|line| {
                let lower = line.to_lowercase();
                lower.contains("error")
                    || lower.contains("failed")
                    || lower.contains("panic")
                    || lower.contains("exception")
            });
            if let Some(pos) = error_pos {
                let offset = lines.len().saturating_sub(pos);
                terminal.grid_mut().set_scroll_offset(offset);
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
                PluginAction::InvokeStructuredHandler { invocation } => {
                    let plugin_id = invocation.plugin_id.clone();
                    if let Err(error) = self.submit_structured_handler(invocation) {
                        log::debug!(
                            "Skipping structured plugin handler {:?}: {}",
                            plugin_id,
                            error
                        );
                        self.toasts.error(error);
                    }
                }
                PluginAction::OpenTab {
                    program,
                    args,
                    cwd,
                    title,
                    plugin_id,
                    profile,
                    env,
                } => {
                    let tab_type = plugin_id
                        .clone()
                        .map(|id| TabType::Plugin { plugin_id: id });
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
                                if let Some(profile) = profile
                                    && let Some(plugin_id) = plugin_id
                                    && let Some(tab) = tabs.active_tab_mut()
                                {
                                    tab.set_plugin_profile(
                                        tab.single_pane_id(),
                                        &plugin_id,
                                        profile,
                                    );
                                }
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
                        tabs.write_to_plugin_terminal(&plugin_id, b"\x03clear\r")
                    } else {
                        false
                    };
                    if cleared {
                        self.toasts.success(format!("Cleared {plugin_id}"));
                    } else {
                        self.toasts
                            .warning(format!("No writable {plugin_id} terminal found"));
                    }
                }
                PluginAction::CopyFromPluginTab { plugin_id, lines } => {
                    let text = self
                        .tabs
                        .as_ref()
                        .and_then(|tabs| tabs.copy_from_plugin_terminal(&plugin_id, lines));
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
                    self.restart_plugin(&plugin_id);
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
                                    if let Some(mut recorder) = tab.stop_transcript() {
                                        let path = recorder.path().display().to_string();
                                        match recorder.finish(std::time::Duration::from_millis(500))
                                        {
                                            Ok(()) => {
                                                log::info!(
                                                    "Transcript recording stopped: {}",
                                                    path
                                                );
                                                self.info_overlay.show(
                                                    "Transcript Stopped".to_string(),
                                                    vec![
                                                        "Transcript saved to:".into(),
                                                        format!("  {}", path),
                                                    ],
                                                );
                                            }
                                            Err(error) => {
                                                log::error!(
                                                    "Transcript shutdown incomplete: {error}"
                                                );
                                                self.info_overlay.show(
                                                    "Transcript Incomplete".to_string(),
                                                    vec![
                                                        format!("Could not durably flush: {error}"),
                                                        format!("Partial path: {path}"),
                                                    ],
                                                );
                                            }
                                        }
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
                        let Some(terminal) = self
                            .tabs
                            .as_ref()
                            .and_then(|tabs| tabs.plugin_terminal_identity(&plugin_id))
                        else {
                            self.toasts
                                .warning(format!("No active {plugin_id} terminal found"));
                            continue;
                        };
                        let mut lines = vec!["Press a number key to select:".into(), String::new()];
                        let mut choices = Vec::new();
                        for (i, name) in profiles.iter().enumerate() {
                            if i < 9 {
                                let source_line = lines.len();
                                lines.push(format!("  {}  {}", i + 1, name));
                                choices.push(crate::ui::info_overlay::InfoOverlayChoice::new(
                                    name,
                                    name,
                                    source_line,
                                    char::from_digit((i + 1) as u32, 10),
                                ));
                            }
                        }
                        let profile = self
                            .tabs
                            .as_ref()
                            .and_then(|tabs| tabs.tab_by_id(terminal.tab_id))
                            .and_then(|tab| tab.pane_plugin_launch(terminal.pane_id))
                            .and_then(|(_, profile)| profile)
                            .unwrap_or("default");
                        lines.push(String::new());
                        lines.push(format!("Next restart: {profile}"));
                        let request_id = self.info_overlay.show_choices(
                            format!("Select Profile — {}", plugin_id),
                            lines,
                            choices,
                        );
                        self.pending_profile_selection = Some(PendingProfileSelection {
                            request_id,
                            terminal,
                            plugin_id,
                            profiles,
                        });
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                PluginAction::ApplyProfile {
                    plugin_id,
                    profile_name,
                } => {
                    if let Some(terminal) = self
                        .tabs
                        .as_ref()
                        .and_then(|tabs| tabs.plugin_terminal_identity(&plugin_id))
                    {
                        self.apply_plugin_profile(terminal, &plugin_id, profile_name);
                    } else {
                        self.toasts
                            .warning(format!("No active {plugin_id} terminal found"));
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

    /// Inject context text into the selected live plugin terminal owner.
    pub(super) fn inject_context_to_plugin(&mut self, plugin_id: &str, text: &str) -> bool {
        let injected = format!("\n--- Context ---\n{}\n--- End Context ---\n", text);
        let written = self
            .tabs
            .as_mut()
            .is_some_and(|tabs| tabs.write_to_plugin_terminal(plugin_id, injected.as_bytes()));
        if written {
            log::info!(
                "Injected {} chars of context into {}",
                injected.len(),
                plugin_id
            );
            self.toasts.success(format!("Context sent to {plugin_id}"));
        } else {
            self.toasts
                .warning(format!("No writable {plugin_id} terminal found"));
        }
        written
    }
}

pub(super) fn clear_structured_host_authority(
    invocation: &mut crate::plugin::structured::StructuredHandlerInvocation,
) {
    invocation.allow_workspace_query = false;
    invocation.allow_workspace_actions = false;
    invocation.allow_workspace_topology = false;
    invocation.allow_terminal_query = false;
    invocation.allow_pane_create = false;
    invocation.allow_tab_create = false;
    invocation.allow_pane_close = false;
    invocation.allow_tab_close = false;
    invocation.allow_pane_move = false;
    invocation.allow_terminal_input = false;
    invocation.allow_pane_layout = false;
    invocation.allow_window_close = false;
    invocation.allow_process_query = false;
    invocation.allow_process_actions = false;
    invocation.allow_process_terminal_text = false;
    invocation.allow_process_terminal_input = false;
    invocation.allow_process_tab_create = false;
    invocation.allow_process_pane_create = false;
    invocation.allow_process_pane_move = false;
    invocation.allow_process_tab_move = false;
    invocation.allow_process_pane_close = false;
    invocation.allow_process_tab_close = false;
    invocation.allow_process_pane_layout = false;
    invocation.allow_process_window_close = false;
    invocation.allow_process_window_focus = false;
}

pub(super) fn reduce_structured_process_query_authority(
    invocation: &mut crate::plugin::structured::StructuredHandlerInvocation,
) {
    let retain_process_actions = invocation.allow_process_actions;
    let retain_process_terminal_text = invocation.allow_process_terminal_text;
    let retain_process_terminal_input = invocation.allow_process_terminal_input;
    let retain_process_tab_create = invocation.allow_process_tab_create;
    let retain_process_pane_create = invocation.allow_process_pane_create;
    let retain_process_pane_move = invocation.allow_process_pane_move;
    let retain_process_tab_move = invocation.allow_process_tab_move;
    let retain_process_pane_close = invocation.allow_process_pane_close;
    let retain_process_tab_close = invocation.allow_process_tab_close;
    let retain_process_pane_layout = invocation.allow_process_pane_layout;
    let retain_process_window_close = invocation.allow_process_window_close;
    let retain_process_window_focus = invocation.allow_process_window_focus;
    clear_structured_host_authority(invocation);
    invocation.allow_process_actions = retain_process_actions;
    invocation.allow_process_terminal_text = retain_process_terminal_text;
    invocation.allow_process_terminal_input = retain_process_terminal_input;
    invocation.allow_process_tab_create = retain_process_tab_create;
    invocation.allow_process_pane_create = retain_process_pane_create;
    invocation.allow_process_pane_move = retain_process_pane_move;
    invocation.allow_process_tab_move = retain_process_tab_move;
    invocation.allow_process_pane_close = retain_process_pane_close;
    invocation.allow_process_tab_close = retain_process_tab_close;
    invocation.allow_process_pane_layout = retain_process_pane_layout;
    invocation.allow_process_window_close = retain_process_window_close;
    invocation.allow_process_window_focus = retain_process_window_focus;
}
