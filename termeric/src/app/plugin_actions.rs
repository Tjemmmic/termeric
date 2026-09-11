//! Plugin command dispatch, context, transcript, export, and diff actions.

use super::*;

impl App {
    pub(super) fn handle_structured_plugin_completed(
        &mut self,
        completion: crate::plugin::structured::StructuredHandlerCompletion,
    ) {
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

    fn continue_structured_workspace_operation(
        &mut self,
        plugin_id: &str,
        operation: crate::plugin::structured::StructuredResponseAction,
        continuation: Option<crate::plugin::structured::StructuredHandlerInvocation>,
    ) -> Result<(), String> {
        let mut invocation = continuation.ok_or_else(|| {
            "workspace operations require a live persistent handler continuation".to_string()
        })?;
        match operation {
            crate::plugin::structured::StructuredResponseAction::QueryWorkspace { query_id } => {
                invocation.event_json =
                    self.structured_workspace_query_event(plugin_id, &query_id)?;
                // A query result cannot query again, but may perform one
                // separately granted workspace mutation.
                invocation.allow_workspace_query = false;
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
            }) => {
                invocation.event_json =
                    self.structured_workspace_action_event(plugin_id, action)?;
                // A mutation result is terminal for host operations.
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
            }
            _ => return Err("structured response did not contain a workspace operation".into()),
        }
        self.submit_structured_handler(invocation).map(|_| ())
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
                    // A replacement notice may have superseded a profile
                    // picker. Any older hidden action context is obsolete too.
                    self.pending_profile_selection = None;
                    self.pending_runtime_plugin_management = None;
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
                    .map(|pending| (pending.plugin_id.clone(), choice_id));
                let Some((plugin_id, profile_name)) = selection else {
                    return;
                };
                self.info_overlay.dismiss(request_id);
                self.pending_profile_selection = None;
                self.execute_plugin_actions(vec![crate::plugin::PluginAction::ApplyProfile {
                    plugin_id,
                    profile_name,
                }]);
            }
            InfoOverlayAction::SubmitInput { request_id } => {
                if !self.submit_runtime_plugin_setting_input(request_id)
                    && self.info_overlay.dismiss(request_id)
                {
                    self.pending_profile_selection = None;
                    self.pending_runtime_plugin_management = None;
                    self.pending_file_upload = None;
                }
            }
            InfoOverlayAction::CancelInput { request_id } => {
                if !self.cancel_runtime_plugin_setting_input(request_id)
                    && self.info_overlay.dismiss(request_id)
                {
                    self.pending_profile_selection = None;
                    self.pending_runtime_plugin_management = None;
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
            let (_, tab) = tabs.iter().find(|(_, tab)| {
                !tab.closed
                    && !tab.is_closing()
                    && matches!(
                        &tab.tab_type,
                        crate::tabs::TabType::Plugin { plugin_id: id } if id == plugin_id
                    )
            })?;
            let origin = if let Some(panes) = &tab.pane_manager {
                crate::tabs::TerminalEventOrigin::Pane {
                    tab_id: tab.id,
                    pane_id: panes.focused_id(),
                }
            } else {
                crate::tabs::TerminalEventOrigin::Tab { tab_id: tab.id }
            };
            let cwd = tab.working_directory().or_else(|| {
                tabs.iter()
                    .find(|(_, candidate)| !candidate.closed)
                    .and_then(|(_, candidate)| candidate.working_directory())
            });
            Some((
                cwd,
                crate::ui::context_picker::ContextTarget {
                    plugin_id: plugin_id.to_string(),
                    origin,
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
        let destination_matches = self.tabs.as_ref().is_some_and(|tabs| {
            let tab_id = match target.origin {
                crate::tabs::TerminalEventOrigin::Tab { tab_id }
                | crate::tabs::TerminalEventOrigin::Pane { tab_id, .. } => tab_id,
            };
            tabs.iter().any(|(_, tab)| {
                tab.id == tab_id
                    && !tab.closed
                    && !tab.is_closing()
                    && matches!(
                        &tab.tab_type,
                        crate::tabs::TabType::Plugin { plugin_id }
                            if plugin_id == &target.plugin_id
                    )
            })
        });
        let injected = format!("\n--- Context ---\n{text}\n--- End Context ---\n");
        let written = destination_matches
            && self.tabs.as_mut().is_some_and(|tabs| {
                tabs.write_user_to_origin(target.origin, injected.as_bytes())
                    .is_ok()
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
        let content = tabs
            .iter()
            .find(|(i, _)| *i == idx)
            .and_then(|(_, tab)| tab.focused_terminal())
            .map(|terminal| terminal.grid().export_scrollback_text().join("\n"));

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
            let Some(terminal) = tab.focused_terminal_mut() else {
                return;
            };
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
                                if let Some(profile) = profile
                                    && let Some(tab) = tabs.active_tab_mut()
                                {
                                    tab.set_plugin_profile(profile);
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
                        let request_id = self.info_overlay.show_choices(
                            format!("Select Profile — {}", plugin_id),
                            lines,
                            choices,
                        );
                        self.pending_profile_selection = Some(PendingProfileSelection {
                            request_id,
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
                    let applied = self
                        .tabs
                        .as_mut()
                        .and_then(|tabs| tabs.plugin_tab_mut(&plugin_id))
                        .and_then(|tab| {
                            tab.ai_session.as_ref()?;
                            tab.set_plugin_profile(profile_name.clone());
                            Some(())
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
