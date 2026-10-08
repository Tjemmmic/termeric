//! Plugin host operations: resolving a handler's target window, running its
//! host action, and routing the result back through its origin window.

use super::*;

impl Application {
    /// Keep every window's plugins at the host's one generation, so no
    /// window discards another's requests as stale. A window that reloaded
    /// its plugins (a plugin manager change, changed agents) has already
    /// advanced it; a registry change made elsewhere, such as by
    /// `termeric plugin`, is noticed here, polled once for the process.
    pub(super) fn sync_plugin_generations(&mut self, now: Instant) {
        if self.plugin_host.generation() != self.plugin_generation_seen {
            // That reload read the registry as it is now.
            self.last_plugin_registry_mtime = self
                .plugin_paths
                .as_ref()
                .and_then(crate::plugin::runtime::PluginPaths::registry_mtime);
        } else if now.saturating_duration_since(self.last_plugin_registry_check)
            >= App::CONFIG_POLL_INTERVAL
        {
            self.last_plugin_registry_check = now;
            let mtime = self
                .plugin_paths
                .as_ref()
                .and_then(crate::plugin::runtime::PluginPaths::registry_mtime);
            if mtime != self.last_plugin_registry_mtime {
                self.last_plugin_registry_mtime = mtime;
                self.plugin_host.advance_generation();
                log::info!("Third-party plugin registry hot-reloaded");
                for window in self.windows.values_mut() {
                    window.toasts.info("Third-party plugins reloaded");
                }
            }
        }
        let generation = self.plugin_host.generation();
        self.plugin_generation_seen = generation;
        for window in self.windows.values_mut() {
            if window.plugin_runtime_generation != generation {
                window.sync_plugins_from_config();
                window
                    .redraw_scheduler
                    .request(crate::app::RedrawReason::ConfigReload);
            }
        }
    }

    pub(super) fn plugin_process_inventory(&self) -> crate::control::protocol::ProcessInventory {
        let windows: Vec<_> = self
            .window_order
            .iter()
            .filter_map(|native_id| {
                let window = self.windows.get(native_id)?;
                let control_id = *self.control_window_ids.get(native_id)?;
                Some((control_id, window))
            })
            .collect();
        let mut inventory = build_plugin_process_inventory(&windows);
        // A live window without a control ID is missing from the list.
        inventory.truncated |= self.window_order.iter().any(|native_id| {
            self.windows
                .get(native_id)
                .is_some_and(|window| !window.should_exit)
                && !self.control_window_ids.contains_key(native_id)
        });
        inventory
    }

    /// Resolve the window a plugin host operation targets. An omitted window
    /// ID means the origin window; any other window needs `capability`
    /// granted with all-windows scope. Scope is checked before the window is
    /// looked up, so a window-scoped plugin learns nothing about other windows.
    pub(super) fn plugin_target_window(
        &self,
        origin: PluginOrigin,
        grants: &PluginPermissions,
        capability: Capability,
        requested: Option<u64>,
    ) -> std::result::Result<WindowId, String> {
        let window_id = requested.unwrap_or(origin.window_id);
        let native_id = if window_id == origin.window_id {
            origin.native_id
        } else {
            if grants.scope(capability) != Some(PermissionScope::AllWindows) {
                return Err(format!(
                    "window {window_id} is not the plugin's own window; targeting it requires {capability}:all-windows"
                ));
            }
            self.control_native_window_id(window_id)?
        };
        self.live_window(native_id, window_id)?;
        Ok(native_id)
    }

    /// Execute one handler query or host action for `origin` and encode the
    /// result event its handler receives next. A plugin window closure also
    /// returns the window to close once that result has been submitted.
    pub(super) fn plugin_host_operation_event(
        &mut self,
        plugin_id: &str,
        origin: PluginOrigin,
        grants: &PluginPermissions,
        operation: &StructuredResponseAction,
    ) -> std::result::Result<(String, Option<WindowId>), String> {
        match operation {
            StructuredResponseAction::QueryWorkspace { query_id } => {
                let inventory = match grants.scope(Capability::WorkspaceQuery) {
                    Some(PermissionScope::AllWindows) => self.plugin_process_inventory(),
                    _ => {
                        let origin_window = self
                            .windows
                            .get(&origin.native_id)
                            .ok_or_else(|| "originating window is gone".to_string())?;
                        build_plugin_process_inventory(&[(origin.window_id, origin_window)])
                    }
                };
                let event = crate::app::plugin_actions::plugin_workspace_query_event(
                    plugin_id,
                    origin.window_id,
                    query_id,
                    &inventory,
                )?;
                Ok((event, None))
            }
            StructuredResponseAction::QueryTerminal {
                window_id,
                tab_id,
                pane_id,
                history_lines,
                ..
            } => {
                let target =
                    self.plugin_target_window(origin, grants, Capability::TerminalRead, *window_id);
                let event = crate::app::plugin_actions::plugin_terminal_query_event(
                    plugin_id,
                    origin.window_id,
                    operation,
                    |max_bytes| {
                        let native_id = target.clone()?;
                        self.windows
                            .get(&native_id)
                            .ok_or_else(|| "target window is gone".to_string())?
                            .read_control_text(*tab_id, *pane_id, *history_lines, max_bytes)
                    },
                )?;
                Ok((event, None))
            }
            StructuredResponseAction::CloseWindow { window_id, .. } => {
                let outcome = self.validate_plugin_window_close(origin, grants, *window_id);
                let close_target = outcome.as_ref().ok().map(|(native_id, _)| *native_id);
                let event = crate::app::plugin_actions::plugin_action_result_event(
                    plugin_id,
                    origin.window_id,
                    operation,
                    outcome.map(|(_, outcome)| outcome),
                )?;
                Ok((event, close_target))
            }
            action => {
                let outcome = self.run_plugin_host_action(origin, grants, action);
                let event = crate::app::plugin_actions::plugin_action_result_event(
                    plugin_id,
                    origin.window_id,
                    action,
                    outcome,
                )?;
                Ok((event, None))
            }
        }
    }

    /// Run one host action and return its outcome fields.
    pub(super) fn run_plugin_host_action(
        &mut self,
        origin: PluginOrigin,
        grants: &PluginPermissions,
        action: &StructuredResponseAction,
    ) -> std::result::Result<serde_json::Value, String> {
        let capability = action.required_capability();
        match action {
            StructuredResponseAction::MoveTab {
                action_id,
                source_window_id,
                tab_id,
                destination_window_id,
                destination_index,
            } => {
                let source =
                    self.plugin_target_window(origin, grants, capability, *source_window_id)?;
                let destination =
                    self.plugin_target_window(origin, grants, capability, *destination_window_id)?;
                let source_id = source_window_id.unwrap_or(origin.window_id);
                let destination_id = destination_window_id.unwrap_or(origin.window_id);
                if source != destination {
                    self.validate_plugin_tab_move_source(source, *tab_id)?;
                }
                match self.move_control_tab(
                    action_id,
                    source_id,
                    *tab_id,
                    destination_id,
                    *destination_index,
                )? {
                    crate::control::protocol::Response::TabMoved {
                        destination_tab_id,
                        source_window_closing,
                        changed,
                        ..
                    } => Ok(serde_json::json!({
                        "destination_tab_id": destination_tab_id,
                        "source_window_closing": source_window_closing,
                        "changed": changed,
                    })),
                    _ => Err("tab movement returned an unexpected result".into()),
                }
            }
            StructuredResponseAction::MovePane {
                action_id,
                source_window_id,
                source_tab_id,
                pane_id,
                destination_window_id,
                destination_tab_id,
                target_pane_id,
                direction,
            } => {
                self.plugin_target_window(origin, grants, capability, *source_window_id)?;
                self.plugin_target_window(origin, grants, capability, *destination_window_id)?;
                match self.move_control_pane(
                    action_id,
                    source_window_id.unwrap_or(origin.window_id),
                    *source_tab_id,
                    *pane_id,
                    destination_window_id.unwrap_or(origin.window_id),
                    *destination_tab_id,
                    *target_pane_id,
                    *direction,
                )? {
                    crate::control::protocol::Response::PaneMoved {
                        destination_pane_id,
                        ..
                    } => Ok(serde_json::json!({ "destination_pane_id": destination_pane_id })),
                    _ => Err("pane movement returned an unexpected result".into()),
                }
            }
            StructuredResponseAction::FocusWindow {
                action_id,
                window_id,
            } => match self.focus_control_window(action_id, *window_id)? {
                crate::control::protocol::Response::WindowFocusRequested {
                    already_focused,
                    ..
                } => Ok(serde_json::json!({
                    "already_focused": already_focused,
                    "requested": !already_focused,
                })),
                _ => Err("native window focus returned an unexpected result".into()),
            },
            StructuredResponseAction::CreateTab { window_id, .. } => {
                let target = self.plugin_target_window(origin, grants, capability, *window_id)?;
                validate_process_tab_creation_capacity(process_tab_count(&self.windows))?;
                self.plugin_window_mut(target)
                    .run_plugin_window_action(action)
            }
            StructuredResponseAction::CreatePane { window_id, .. } => {
                let target = self.plugin_target_window(origin, grants, capability, *window_id)?;
                validate_process_pane_creation_capacity(process_pane_count(&self.windows))?;
                self.plugin_window_mut(target)
                    .run_plugin_window_action(action)
            }
            StructuredResponseAction::SelectTab { window_id, .. }
            | StructuredResponseAction::FocusPane { window_id, .. }
            | StructuredResponseAction::SendText { window_id, .. }
            | StructuredResponseAction::CloseTab { window_id, .. }
            | StructuredResponseAction::ClosePane { window_id, .. }
            | StructuredResponseAction::SwapPanes { window_id, .. }
            | StructuredResponseAction::BalancePanes { window_id, .. }
            | StructuredResponseAction::ApplyPaneLayout { window_id, .. }
            | StructuredResponseAction::ResizeDivider { window_id, .. } => {
                let target = self.plugin_target_window(origin, grants, capability, *window_id)?;
                self.plugin_window_mut(target)
                    .run_plugin_window_action(action)
            }
            StructuredResponseAction::ShowMessage { .. }
            | StructuredResponseAction::ShowNotice { .. }
            | StructuredResponseAction::QueryWorkspace { .. }
            | StructuredResponseAction::QueryTerminal { .. }
            | StructuredResponseAction::CloseWindow { .. } => {
                Err(format!("{} is not a host action", action.name()))
            }
        }
    }

    pub(super) fn plugin_window_mut(&mut self, native_id: WindowId) -> &mut App {
        self.windows
            .get_mut(&native_id)
            .expect("resolved plugin target window remains registered")
    }

    /// A cross-window tab move may not empty its source window; closing a
    /// window needs the separate window-close capability.
    pub(super) fn validate_plugin_tab_move_source(
        &self,
        source: WindowId,
        tab_id: u64,
    ) -> std::result::Result<(), String> {
        let tabs = self
            .windows
            .get(&source)
            .and_then(|window| window.tabs.as_ref())
            .ok_or_else(|| "Source window has no initialized tab manager".to_string())?;
        let tab_id = usize::try_from(tab_id)
            .map_err(|_| "Source tab ID does not fit this platform".to_string())?;
        let tab = tabs
            .tab_by_id(tab_id)
            .ok_or_else(|| format!("GUI tab {tab_id} does not exist in the source window"))?;
        if tab.closed || tab.is_closing() {
            return Err(format!("GUI tab {tab_id} is already closing"));
        }
        let non_closing_tabs = tabs
            .all_tabs()
            .iter()
            .filter(|tab| !tab.closed && !tab.is_closing())
            .count();
        if non_closing_tabs <= 1 {
            return Err(format!(
                "Cannot move the final non-closing tab {tab_id}; closing its window requires window-close"
            ));
        }
        Ok(())
    }

    /// Validate a plugin window closure and return the window to close with
    /// the result's outcome fields. Window scope closes only the origin window,
    /// from an invoked command, and may close the last window; all-windows
    /// scope closes any window except the last live one.
    pub(super) fn validate_plugin_window_close(
        &self,
        origin: PluginOrigin,
        grants: &PluginPermissions,
        window_id: Option<u64>,
    ) -> std::result::Result<(WindowId, serde_json::Value), String> {
        let target =
            self.plugin_target_window(origin, grants, Capability::WindowClose, window_id)?;
        let tab_count = self
            .windows
            .get(&target)
            .expect("resolved plugin target window remains registered")
            .validate_plugin_window_close()?;
        let live_windows = self
            .windows
            .values()
            .filter(|window| !window.should_exit && window.pending_plugin_window_close.is_none())
            .count();
        if grants.scope(Capability::WindowClose) == Some(PermissionScope::AllWindows)
            && live_windows <= 1
        {
            return Err(
                "Cannot close the final live window with window-close:all-windows; only a command-scoped window-close of the plugin's own window may"
                    .into(),
            );
        }
        let remaining_windows = u16::try_from(live_windows.saturating_sub(1))
            .map_err(|_| "live window count is unsupported".to_string())?;
        Ok((
            target,
            serde_json::json!({
                "scheduled": true,
                "tab_count": tab_count,
                "remaining_windows": remaining_windows,
            }),
        ))
    }

    /// Close a validated plugin target once its result has been submitted:
    /// the origin window waits for that exact result callback, while any other
    /// window closes immediately.
    pub(super) fn arm_plugin_window_close(
        &mut self,
        target_native_id: WindowId,
        origin_native_id: WindowId,
        plugin_id: &str,
        plugin_generation: u64,
        result_request_id: u64,
    ) {
        let target = self.plugin_window_mut(target_native_id);
        debug_assert!(!target.should_exit);
        debug_assert!(target.pending_plugin_window_close.is_none());
        if target_native_id == origin_native_id {
            target.pending_plugin_window_close = Some(PendingPluginWindowClose {
                plugin_id: plugin_id.to_string(),
                plugin_generation,
                result_request_id,
            });
        } else {
            target.should_exit = true;
        }
    }

    pub(super) fn handle_structured_plugin_completed(
        &mut self,
        completion: crate::plugin::structured::StructuredHandlerCompletion,
    ) {
        let origin_window_id = completion.window_id;
        let plugin_generation = completion.plugin_generation;
        // A control ID is never reused, so a result for a window that has
        // closed finds no window, rather than one that took its native ID.
        let Ok(origin_native_id) = self.control_native_window_id(origin_window_id) else {
            log::debug!(
                "Discarding structured plugin response {} from {:?}: window {origin_window_id} closed",
                completion.request_id,
                completion.plugin_id
            );
            return;
        };
        let Some(origin_window) = self.windows.get_mut(&origin_native_id) else {
            return;
        };
        let Some(host) = origin_window.accept_structured_plugin_completion(completion) else {
            return;
        };
        if origin_window.should_exit {
            log::debug!(
                "Discarding {} from plugin {:?}: originating window is closing",
                host.operation.name(),
                host.plugin_id
            );
            return;
        }
        let origin = PluginOrigin {
            native_id: origin_native_id,
            window_id: origin_window_id,
        };
        let crate::app::plugin_actions::StructuredHostOperation {
            plugin_id,
            operation,
            continuation,
        } = host;
        let result =
            self.plugin_host_operation_continuation(&plugin_id, origin, &operation, continuation);
        let Some(origin_window) = self.windows.get_mut(&origin_native_id) else {
            return;
        };
        let submitted = result.and_then(|(continuation, close_target)| {
            origin_window
                .submit_structured_handler(continuation)
                .map(|request_id| (request_id, close_target))
        });
        match submitted {
            Ok((request_id, Some(close_target))) => self.arm_plugin_window_close(
                close_target,
                origin_native_id,
                &plugin_id,
                plugin_generation,
                request_id,
            ),
            Ok((_, None)) => {}
            Err(error) => {
                let name = operation.name();
                log::error!("Structured plugin {plugin_id:?} {name} failed: {error}");
                origin_window
                    .toasts
                    .error(format!("Plugin {plugin_id} {name} failed: {error}"));
            }
        }
    }

    /// Execute a handler's host operation for `origin` and turn the handler's
    /// `continuation` into the invocation that delivers the result, under the
    /// authority the follow-up rule leaves it. A plugin window closure also
    /// returns the window to close once that result has been submitted.
    pub(super) fn plugin_host_operation_continuation(
        &mut self,
        plugin_id: &str,
        origin: PluginOrigin,
        operation: &StructuredResponseAction,
        mut continuation: StructuredHandlerInvocation,
    ) -> std::result::Result<(StructuredHandlerInvocation, Option<WindowId>), String> {
        // The operation may change the active tab of any window. What changed
        // before it is reported; what it changes answers only in its result.
        for window in self.windows.values_mut() {
            if !window.should_exit {
                window.report_active_tab_switch();
            }
        }
        let outcome = self.plugin_host_operation_event(
            plugin_id,
            origin,
            continuation.authority.grants(),
            operation,
        );
        for window in self.windows.values_mut() {
            window.absorb_active_tab_switch();
        }
        let (event_json, close_target) = outcome?;
        continuation.authority = continuation.authority.after(operation);
        continuation.event_json = event_json;
        Ok((continuation, close_target))
    }
}
