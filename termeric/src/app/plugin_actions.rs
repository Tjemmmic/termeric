//! Plugin command dispatch, hooks, actions, and structured-handler results.
//!
//! Handler host operations are split by ownership: this window accepts a
//! completed response and runs actions that target itself, while the
//! Application resolves target windows, enforces window scope, and routes
//! every result back to the handler through the origin window.

use super::*;
use crate::plugin::structured::{
    StructuredHandlerCompletion, StructuredHandlerInvocation, StructuredResponseAction,
};

const MAX_RESULT_ERROR_BYTES: usize = 1024;

/// A handler's one query or host action, taken from its response for the
/// Application to execute, with the handler that receives the result.
#[derive(Debug)]
pub(super) struct StructuredHostOperation {
    pub(super) plugin_id: String,
    pub(super) operation: StructuredResponseAction,
    pub(super) continuation: StructuredHandlerInvocation,
}

impl App {
    /// Accept one completed handler response for this origin window: finish a
    /// scheduled plugin closure of this window, discard stale generations,
    /// show UI actions, and hand back the response's host operation, if any.
    pub(super) fn accept_structured_plugin_completion(
        &mut self,
        completion: StructuredHandlerCompletion,
    ) -> Option<StructuredHostOperation> {
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
            return None;
        }
        if completion.plugin_generation != self.plugin_runtime_generation
            || !self.plugin_manager.contains(&completion.plugin_id)
        {
            log::debug!(
                "Discarding stale structured plugin response {} from {:?}",
                completion.request_id,
                completion.plugin_id
            );
            return None;
        }
        let actions = match completion.result {
            Ok(actions) => actions,
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
                return None;
            }
        };
        // Handler validation admits at most one host operation per response.
        let (mut operations, ui_actions): (Vec<_>, Vec<_>) = actions
            .into_iter()
            .partition(StructuredResponseAction::is_host_operation);
        for action in ui_actions {
            match action {
                StructuredResponseAction::ShowMessage { message } => {
                    self.execute_plugin_action(crate::plugin::PluginAction::ShowMessage { message })
                }
                StructuredResponseAction::ShowNotice { title, lines } => self
                    .execute_plugin_action(crate::plugin::PluginAction::ShowNotice {
                        title,
                        lines,
                    }),
                _ => {}
            }
        }
        let operation = operations.pop()?;
        Some(StructuredHostOperation {
            plugin_id: completion.plugin_id,
            operation,
            continuation: completion.continuation,
        })
    }

    pub(super) fn submit_structured_handler(
        &mut self,
        invocation: StructuredHandlerInvocation,
    ) -> Result<u64, String> {
        if self.window.is_none() {
            return Err("structured plugin handler requires a native window".into());
        }
        let proxy = super::EVENT_LOOP_PROXY
            .get()
            .cloned()
            .ok_or_else(|| "structured plugin event loop is unavailable".to_string())?;
        let deliver: crate::plugin::structured::CompletionSink = Arc::new(move |completion| {
            let _ = proxy.send_event(AppEvent::StructuredPluginCompleted(completion));
        });
        // Results come back by control ID, which no later window takes.
        self.plugin_host.submit(
            self.control_window_id,
            &self.plugin_requests,
            self.plugin_runtime_generation,
            invocation,
            deliver,
        )
    }

    /// Check that this window can be closed by a plugin and return its tab
    /// count. The closure itself waits for the plugin's result callback.
    pub(super) fn validate_plugin_window_close(&self) -> Result<u16, String> {
        if self.should_exit {
            return Err("window is already closing".into());
        }
        if self.pending_plugin_window_close.is_some() {
            return Err("window already has a pending plugin closure".into());
        }
        let tab_count = self
            .tabs
            .as_ref()
            .map(|tabs| tabs.all_tabs().len())
            .filter(|count| *count > 0)
            .ok_or_else(|| "window has no initialized tab owners".to_string())?;
        u16::try_from(tab_count).map_err(|_| "window tab count is unsupported".into())
    }

    /// Run one plugin host action that targets this window and return its
    /// outcome fields. The Application has already resolved this window from
    /// the plugin's grants and window scope, and checked process capacity.
    pub(super) fn run_plugin_window_action(
        &mut self,
        action: &StructuredResponseAction,
    ) -> Result<serde_json::Value, String> {
        match action {
            StructuredResponseAction::SelectTab { tab_id, .. } => {
                let changed = self.select_tab_for(Requester::Plugin, *tab_id)?;
                Ok(serde_json::json!({ "changed": changed }))
            }
            StructuredResponseAction::FocusPane {
                tab_id, pane_id, ..
            } => {
                let (tab_changed, pane_changed) =
                    self.focus_pane_for(Requester::Plugin, *tab_id, *pane_id)?;
                Ok(serde_json::json!({
                    "tab_changed": tab_changed,
                    "pane_changed": pane_changed,
                }))
            }
            StructuredResponseAction::SendText {
                tab_id,
                pane_id,
                text,
                paste,
                enter,
                ..
            } => {
                let (input_bytes, wire_bytes, disposition, confirmation_id) =
                    self.send_text_for(Requester::Plugin, *tab_id, *pane_id, text, *paste, *enter)?;
                Ok(serde_json::json!({
                    "input_bytes": input_bytes,
                    "wire_bytes": wire_bytes,
                    "disposition": disposition,
                    "confirmation_id": confirmation_id,
                }))
            }
            StructuredResponseAction::CreateTab { .. } => {
                let created_tab_id = self.create_tab_for(Requester::Plugin)?;
                Ok(serde_json::json!({ "created_tab_id": created_tab_id }))
            }
            StructuredResponseAction::CloseTab { tab_id, .. } => {
                let (remaining_tabs, active_tab_id, active_changed) =
                    self.close_tab_for(Requester::Plugin, *tab_id)?;
                let active_tab_id = active_tab_id
                    .expect("last-tab protection leaves one active ordinary or live-domain tab");
                Ok(serde_json::json!({
                    "scheduled": true,
                    "remaining_tabs": remaining_tabs,
                    "active_tab_id": active_tab_id,
                    "active_changed": active_changed,
                }))
            }
            StructuredResponseAction::CreatePane {
                tab_id,
                pane_id,
                direction,
                ..
            } => {
                let created_pane_id = self.create_control_pane(*tab_id, *pane_id, *direction)?;
                Ok(serde_json::json!({ "created_pane_id": created_pane_id }))
            }
            StructuredResponseAction::ClosePane {
                tab_id, pane_id, ..
            } => {
                let (remaining_panes, focused_pane_id) =
                    self.close_pane_for(Requester::Plugin, *tab_id, *pane_id)?;
                Ok(serde_json::json!({
                    "remaining_panes": remaining_panes,
                    "focused_pane_id": focused_pane_id,
                }))
            }
            StructuredResponseAction::SwapPanes {
                tab_id,
                first_pane_id,
                second_pane_id,
                ..
            } => {
                let (pane_order, focused_pane_id, zoomed_pane_id) =
                    self.swap_plugin_panes(*tab_id, *first_pane_id, *second_pane_id)?;
                Ok(serde_json::json!({
                    "pane_order": pane_order,
                    "focused_pane_id": focused_pane_id,
                    "zoomed_pane_id": zoomed_pane_id,
                }))
            }
            StructuredResponseAction::BalancePanes { tab_id, .. } => {
                let (changed, pane_order, focused_pane_id, zoomed_pane_id) =
                    self.balance_plugin_panes(*tab_id)?;
                Ok(serde_json::json!({
                    "changed": changed,
                    "pane_order": pane_order,
                    "focused_pane_id": focused_pane_id,
                    "zoomed_pane_id": zoomed_pane_id,
                }))
            }
            StructuredResponseAction::ApplyPaneLayout { tab_id, preset, .. } => {
                let (pane_order, focused_pane_id, zoomed_pane_id) =
                    self.apply_plugin_pane_layout(*tab_id, *preset)?;
                Ok(serde_json::json!({
                    "pane_order": pane_order,
                    "focused_pane_id": focused_pane_id,
                    "zoomed_pane_id": zoomed_pane_id,
                }))
            }
            StructuredResponseAction::ResizeDivider {
                tab_id,
                divider_id,
                ratio_millis,
                ..
            } => {
                let (previous_ratio_millis, changed) =
                    self.resize_control_divider(*tab_id, *divider_id, *ratio_millis)?;
                Ok(serde_json::json!({
                    "previous_ratio_millis": previous_ratio_millis,
                    "changed": changed,
                }))
            }
            _ => Err(format!("{} is not a single-window action", action.name())),
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
        let active_tab_id = self.tabs.as_ref().and_then(TabManager::active_tab_id);

        PluginContext {
            window_id: self.control_window_id,
            cwd,
            tab_count,
            active_tab,
            active_tab_id,
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
        for action in self.plugin_manager.dispatch(&hook, &ctx) {
            self.execute_plugin_action(action);
        }
    }

    /// Raise tab-switched when the active tab differs from the one plugins
    /// last heard about. Every way the active tab changes (keys, clicks,
    /// control requests, new, closed, and moved tabs, workspaces) is
    /// observed here rather than where it happens; a plugin's own changes are
    /// absorbed first, so they raise no event.
    pub(super) fn report_active_tab_switch(&mut self) {
        let Some(to) = self.tabs.as_ref().and_then(TabManager::active_tab_id) else {
            return;
        };
        if self.reported_active_tab == Some(to) {
            return;
        }
        let from = self.reported_active_tab.replace(to);
        self.dispatch_plugin_hook(crate::plugin::Hook::TabSwitched { from, to });
    }

    /// Take the current active tab as already reported: a plugin's own
    /// change answers only through its action result.
    pub(super) fn absorb_active_tab_switch(&mut self) {
        if let Some(active) = self.tabs.as_ref().and_then(TabManager::active_tab_id) {
            self.reported_active_tab = Some(active);
        }
    }

    /// Execute a plugin command by ID, routed to its exact owning plugin.
    pub(super) fn execute_plugin_command(&mut self, command_id: &str) {
        let ctx = self.build_plugin_context();
        if let Some(action) = self.plugin_manager.execute_command(command_id, &ctx) {
            self.execute_plugin_action(action);
        }
    }

    /// Execute one plugin action.
    pub(super) fn execute_plugin_action(&mut self, action: crate::plugin::PluginAction) {
        use crate::plugin::PluginAction;
        use crate::tabs::TabType;

        match action {
            PluginAction::InvokeStructuredHandler { invocation } => {
                let plugin_id = invocation.plugin_id.clone();
                if self.plugin_runtime_generation != self.plugin_host.generation() {
                    // Another window reloaded the plugins and this one has
                    // yet to follow; the event is dropped like the requests
                    // the reload cancelled.
                    log::debug!("Dropping a {plugin_id:?} event from before a plugin reload");
                    return;
                }
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
                            if let Some(plugin_id) = plugin_id
                                && let Some(tab) = tabs.tab_mut_by_id(id)
                            {
                                tab.set_plugin_profile_reference(
                                    tab.panes.focused_id(),
                                    &plugin_id,
                                    profile,
                                );
                            }
                            log::info!("Plugin opened tab {} running '{}'", id, program);
                            // The new tab is active; what describes the
                            // focused tab follows it.
                            self.refresh_focused_terminal_surfaces();
                            self.dispatch_plugin_hook(crate::plugin::Hook::TabCreated {
                                tab_id: id,
                            });
                        }
                        Err(e) => {
                            self.info_overlay.show(
                                format!("Failed to launch '{}'", program),
                                vec![format!("{}", e)],
                            );
                            self.request_redraw(RedrawReason::Action);
                            log::error!("Plugin failed to open tab for '{}': {}", program, e);
                        }
                    }
                }
            }
            PluginAction::ShowMessage { message } => {
                log::info!("Plugin message: {}", message);
                self.toasts.info(message);
            }
            PluginAction::ShowNotice { title, lines } => {
                log::info!("Plugin notice [{}]: {}", title, lines.join(" / "));
                self.info_overlay.show(title, lines);
                self.request_redraw(RedrawReason::Action);
            }
        }
    }
}

fn encode_result_envelope(
    plugin_id: &str,
    origin_window_id: u64,
    event: serde_json::Value,
) -> Result<String, String> {
    let kind = event["type"].as_str().unwrap_or("result").to_string();
    serde_json::to_string(&serde_json::json!({
        "schema_version": crate::plugin::structured::SCHEMA_VERSION,
        "plugin_id": plugin_id,
        "origin_window_id": origin_window_id,
        "event": event,
    }))
    .map_err(|error| format!("could not encode {kind}: {error}"))
}

/// Wrap one result event in the handler envelope, bounded so a persistent
/// handler can still receive it with its correlation ID.
fn plugin_result_envelope(
    plugin_id: &str,
    origin_window_id: u64,
    event: serde_json::Value,
) -> Result<String, String> {
    let kind = event["type"].as_str().unwrap_or("result").to_string();
    let encoded = encode_result_envelope(plugin_id, origin_window_id, event)?;
    let limit = crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES;
    if encoded.len() > limit {
        return Err(format!("{kind} exceeds {limit} bytes"));
    }
    Ok(encoded)
}

/// Add a bounded, control-safe error to a failed result event.
fn insert_result_error(event: &mut serde_json::Value, error: &str) {
    let mut budget = MAX_RESULT_ERROR_BYTES;
    let (error, error_truncated) = super::control_metadata(error, &mut budget);
    event["error"] = error.into();
    event["error_truncated"] = error_truncated.into();
}

/// The `workspace-query-result` for an inventory already limited to the
/// windows the plugin's workspace-query scope may see.
pub(super) fn plugin_workspace_query_event(
    plugin_id: &str,
    origin_window_id: u64,
    query_id: &str,
    inventory: &crate::control::protocol::ProcessInventory,
) -> Result<String, String> {
    plugin_result_envelope(
        plugin_id,
        origin_window_id,
        serde_json::json!({
            "type": "workspace-query-result",
            "query_id": query_id,
            "windows": inventory.windows,
            "truncated": inventory.truncated,
        }),
    )
}

/// The `action-result` for one host action: the request's own fields, with
/// omitted window IDs resolved to the origin, then either the outcome fields
/// or a bounded error.
pub(super) fn plugin_action_result_event(
    plugin_id: &str,
    origin_window_id: u64,
    action: &StructuredResponseAction,
    outcome: Result<serde_json::Value, String>,
) -> Result<String, String> {
    let mut event = serde_json::to_value(action)
        .map_err(|error| format!("could not encode {} result: {error}", action.name()))?;
    let fields = event
        .as_object_mut()
        .ok_or_else(|| format!("{} did not encode as an object", action.name()))?;
    fields.remove("type");
    // Input text is echoed only as byte counts.
    fields.remove("text");
    for key in ["window_id", "source_window_id", "destination_window_id"] {
        if fields.get(key).is_some_and(serde_json::Value::is_null) {
            fields.insert(key.into(), origin_window_id.into());
        }
    }
    event["type"] = "action-result".into();
    event["action"] = action.name().into();
    event["ok"] = outcome.is_ok().into();
    match outcome {
        Ok(serde_json::Value::Object(outcome)) => {
            for (key, value) in outcome {
                event[key.as_str()] = value;
            }
        }
        Ok(_) => return Err(format!("{} outcome is not an object", action.name())),
        Err(error) => insert_result_error(&mut event, &error),
    }
    plugin_result_envelope(plugin_id, origin_window_id, event)
}

/// The `terminal-query-result` for one `query-terminal` action. `read`
/// returns presented text within a byte budget; the budget shrinks until the
/// complete escaped event fits the persistent handler frame.
pub(super) fn plugin_terminal_query_event(
    plugin_id: &str,
    origin_window_id: u64,
    query: &StructuredResponseAction,
    read: impl Fn(u32) -> Result<(crate::control::protocol::TextScreen, u16, String, bool), String>,
) -> Result<String, String> {
    let StructuredResponseAction::QueryTerminal {
        query_id,
        window_id,
        tab_id,
        pane_id,
        history_lines,
        max_bytes,
    } = query
    else {
        return Err(format!("{} is not a terminal query", query.name()));
    };
    crate::plugin::structured::validate_terminal_query_bounds(*history_lines, *max_bytes)?;
    let limit = crate::plugin::structured::MAX_PERSISTENT_SOURCE_EVENT_BYTES;
    let mut attempt_bytes = *max_bytes;
    loop {
        let mut event = serde_json::json!({
            "type": "terminal-query-result",
            "query_id": query_id,
            "window_id": window_id.unwrap_or(origin_window_id),
            "tab_id": tab_id,
            "pane_id": pane_id,
            "requested_history_lines": history_lines,
            "requested_max_bytes": max_bytes,
        });
        match read(attempt_bytes) {
            Ok((screen, included_history, text, truncated)) => {
                event["ok"] = true.into();
                event["screen"] = serde_json::json!(screen);
                event["history_lines"] = included_history.into();
                event["text_bytes"] = text.len().into();
                event["text"] = text.into();
                event["truncated"] = (truncated || attempt_bytes < *max_bytes).into();
            }
            Err(error) => {
                event["ok"] = false.into();
                insert_result_error(&mut event, &error);
            }
        }
        let encoded = encode_result_envelope(plugin_id, origin_window_id, event)?;
        if encoded.len() <= limit {
            return Ok(encoded);
        }
        if attempt_bytes == 1 {
            return Err(format!("terminal-query-result exceeds {limit} bytes"));
        }
        let scaled = (usize::try_from(attempt_bytes).expect("u32 fits usize") * limit
            / encoded.len())
        .min(usize::try_from(attempt_bytes - 1).expect("u32 fits usize"))
        .max(1);
        attempt_bytes = u32::try_from(scaled).expect("scaled terminal query budget fits u32");
    }
}
