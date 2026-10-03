//! Plugin command dispatch, context, transcript, export, and diff actions.
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
/// Application to execute, with the persistent handler that receives the
/// result.
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
        let Some(continuation) = completion.continuation else {
            let error = "host operations require a live persistent handler";
            log::error!(
                "Structured plugin {:?} {} failed: {error}",
                completion.plugin_id,
                operation.name()
            );
            self.toasts.error(format!(
                "Plugin {} {} failed: {error}",
                completion.plugin_id,
                operation.name()
            ));
            return None;
        };
        Some(StructuredHostOperation {
            plugin_id: completion.plugin_id,
            operation,
            continuation,
        })
    }

    pub(super) fn submit_structured_handler(
        &mut self,
        invocation: StructuredHandlerInvocation,
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
                let changed = self.select_plugin_tab(*tab_id)?;
                Ok(serde_json::json!({ "changed": changed }))
            }
            StructuredResponseAction::FocusPane {
                tab_id, pane_id, ..
            } => {
                let (tab_changed, pane_changed) = self.focus_plugin_pane(*tab_id, *pane_id)?;
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
                    self.send_plugin_text(*tab_id, *pane_id, text, *paste, *enter)?;
                Ok(serde_json::json!({
                    "input_bytes": input_bytes,
                    "wire_bytes": wire_bytes,
                    "disposition": disposition,
                    "confirmation_id": confirmation_id,
                }))
            }
            StructuredResponseAction::CreateTab { .. } => {
                let created_tab_id = self.create_plugin_tab()?;
                Ok(serde_json::json!({ "created_tab_id": created_tab_id }))
            }
            StructuredResponseAction::CloseTab { tab_id, .. } => {
                let (remaining_tabs, active_tab_id, active_changed) =
                    self.close_plugin_tab(*tab_id)?;
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
                    self.close_plugin_pane(*tab_id, *pane_id)?;
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

    pub(super) fn apply_diff_review_action(
        &mut self,
        action: crate::ui::diff_review::DiffReviewAction,
    ) {
        use crate::ui::diff_review::DiffReviewAction;

        match action {
            DiffReviewAction::Copy { request_id }
                if self.diff_review.change_text(request_id).is_some() =>
            {
                let text = self.diff_review.change_text(request_id).unwrap_or_default();
                self.copy_to_clipboard(text.to_owned(), "Copied diff to clipboard");
            }
            DiffReviewAction::Stage { request_id } => self.stage_reviewed_change(request_id),
            DiffReviewAction::Revert { request_id } => {
                self.confirm_revert_reviewed_change(request_id)
            }
            DiffReviewAction::Copy { request_id } => {
                let raw = self.diff_review.target(request_id).and_then(|target| {
                    let tabs = self.tabs.as_ref()?;
                    tabs.origin_for_pane(target.tab_id, target.pane_id)?;
                    tabs.tab_by_id(target.tab_id)?
                        .pane_session(target.pane_id)?
                        .diff_detector
                        .detected_by_id(target.diff_id)
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
                    && tabs
                        .origin_for_pane(target.tab_id, target.pane_id)
                        .is_some()
                    && let Some(tab) = tabs.tab_mut_by_id(target.tab_id)
                    && let Some(state) = tab.pane_session_mut(target.pane_id)
                {
                    state.diff_detector.mark_reviewed(target.diff_id);
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
                    self.overlay_flow = None;
                    self.cancel_pending_hook_change(request_id);
                    self.cancel_pending_revert(request_id);
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
                if self.handle_agent_hook_choice(request_id, &choice_id) {
                    return;
                }
                if self.handle_revert_choice(request_id, &choice_id) {
                    return;
                }
                if self
                    .pending_file_upload()
                    .is_some_and(|pending| pending.request_id == request_id)
                    && matches!(
                        choice_id.as_str(),
                        "file-upload-confirm" | "file-upload-cancel"
                    )
                {
                    self.info_overlay.dismiss(request_id);
                    let upload = self.take_pending_file_upload();
                    if choice_id == "file-upload-confirm" {
                        if let Some(upload) = upload {
                            self.start_file_upload(upload);
                        }
                    }
                }
            }
            InfoOverlayAction::SubmitInput { request_id } => {
                if !self.submit_font_feature_manager_input(request_id)
                    && !self.submit_font_range_manager_input(request_id)
                    && !self.submit_runtime_plugin_manager_input(request_id)
                    && self.info_overlay.dismiss(request_id)
                {
                    self.dismiss_pending_kitty_download(request_id);
                    self.overlay_flow = None;
                    self.cancel_pending_hook_change(request_id);
                    self.cancel_pending_revert(request_id);
                }
            }
            InfoOverlayAction::CancelInput { request_id } => {
                if !self.cancel_font_feature_manager_input(request_id)
                    && !self.cancel_font_range_manager_input(request_id)
                    && !self.cancel_runtime_plugin_manager_input(request_id)
                    && self.info_overlay.dismiss(request_id)
                {
                    self.dismiss_pending_kitty_download(request_id);
                    self.overlay_flow = None;
                    self.cancel_pending_hook_change(request_id);
                    self.cancel_pending_revert(request_id);
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
            window_id: self.control_window_id,
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
        for action in self.plugin_manager.dispatch(&hook, &ctx) {
            self.execute_plugin_action(action);
        }
    }

    /// Execute a plugin command by ID, routed to its exact owning plugin.
    pub(super) fn execute_plugin_command(&mut self, command_id: &str) {
        let ctx = self.build_plugin_context();
        if let Some(action) = self.plugin_manager.execute_command(command_id, &ctx) {
            self.execute_plugin_action(action);
        }
    }

    pub(super) fn selected_diff(
        &self,
        identity: crate::tabs::TerminalIdentity,
    ) -> Option<crate::terminal::diff_detector::DetectedDiff> {
        let tabs = self.tabs.as_ref()?;
        tabs.origin_for_pane(identity.tab_id, identity.pane_id)?;
        Some(
            tabs.tab_by_id(identity.tab_id)?
                .pane_session(identity.pane_id)?
                .diff_detector
                .latest_unreviewed()?
                .clone(),
        )
    }

    pub(super) fn open_agent_diff_review(&mut self, identity: crate::tabs::TerminalIdentity) {
        if let Some(diff) = self.selected_diff(identity) {
            self.diff_review
                .show(&diff, identity.tab_id, identity.pane_id);
        } else {
            self.toasts.info("No unreviewed diffs");
        }
    }

    pub(super) fn copy_agent_diff(&mut self, identity: crate::tabs::TerminalIdentity) {
        if let Some(diff) = self.selected_diff(identity) {
            self.copy_to_clipboard(diff.raw, "Copied diff");
        } else {
            self.toasts.info("No unreviewed diffs to copy");
        }
    }

    /// Open the context picker for the agent in `terminal`. Terminal output
    /// comes from another pane of the same tab when there is one, else from
    /// the first shell tab.
    pub(super) fn open_context_picker_for(
        &mut self,
        terminal: crate::tabs::TerminalIdentity,
        agent: &crate::agents::AgentDefinition,
    ) {
        let Some(tab) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.tab_by_id(terminal.tab_id))
        else {
            return;
        };
        let cwd = tab.pane_working_directory(terminal.pane_id);
        self.context_picker.open(
            cwd,
            crate::ui::context_picker::ContextTarget {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                terminal,
            },
        );
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
        let terminal_output = self
            .context_picker
            .target()
            .and_then(|target| self.context_source_output(target.terminal));
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
        self.request_redraw(RedrawReason::WindowEvent);
    }

    /// The last 50 lines of the terminal to draw context from: a non-agent
    /// pane of the agent's tab, else the first open shell tab whose focused
    /// terminal is not running an agent.
    pub(super) fn context_source_output(
        &self,
        agent: crate::tabs::TerminalIdentity,
    ) -> Option<String> {
        let tabs = self.tabs.as_ref()?;
        let tail = |terminal: &termeric_core::Terminal| {
            let lines = terminal.grid().plain_lines();
            let start = lines.len().saturating_sub(50);
            lines[start..].join("\n")
        };
        // A shell tab can host an agent started by hand; its screen is the
        // agent's conversation, not the user's work.
        let not_an_agent = |tab: &crate::tabs::Tab, id| {
            tab.pane_session(id)
                .is_some_and(|state| state.agent.is_none())
        };
        let tab = tabs.tab_by_id(agent.tab_id)?;
        let sibling = tab
            .owned_pane_ids()
            .into_iter()
            .filter(|id| *id != agent.pane_id)
            .find(|id| not_an_agent(tab, *id))
            .and_then(|id| tab.pane_terminal(id));
        if let Some(terminal) = sibling {
            return Some(tail(terminal));
        }
        tabs.iter()
            .find(|(_, t)| {
                matches!(
                    t.tab_type,
                    crate::tabs::TabType::Shell | crate::tabs::TabType::Profile { .. }
                ) && !t.closed
                    && t.id != agent.tab_id
                    && not_an_agent(t, t.focused_terminal_identity().pane_id)
            })
            .and_then(|(_, tab)| tab.focused_terminal().map(tail))
    }

    /// Paste gathered context into the agent that asked for it.
    fn inject_context_to_target(
        &mut self,
        target: &crate::ui::context_picker::ContextTarget,
        text: &str,
    ) -> bool {
        self.send_to_agent(
            super::agents::AgentTarget {
                identity: target.terminal,
                agent_id: target.agent_id.clone(),
                agent_name: target.agent_name.clone(),
            },
            text,
        )
    }

    /// Execute one plugin action.
    pub(super) fn execute_plugin_action(&mut self, action: crate::plugin::PluginAction) {
        use crate::plugin::PluginAction;
        use crate::tabs::TabType;

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
                            if let Some(plugin_id) = plugin_id
                                && let Some(tab) = tabs.tab_mut_by_id(id)
                            {
                                tab.set_plugin_profile_reference(
                                    tab.single_pane_id(),
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

    /// Toggle transcript recording for the active plugin session
    pub(super) fn toggle_transcript(&mut self) {
        if let Some(identity) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.active_tab())
            .map(|tab| tab.focused_terminal_identity())
        {
            self.toggle_terminal_transcript(identity);
        } else {
            self.toasts.warning("No active terminal");
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
