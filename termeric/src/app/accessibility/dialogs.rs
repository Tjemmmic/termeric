//! The guardrail confirmation, info overlay, diff review, and context
//! picker: each dialog's nodes and the actions assistive technology takes on
//! them.

use super::*;

impl App {
    pub(super) fn handle_guardrail_accessibility_action(&mut self, request: ActionRequest) {
        let Some(target) = self
            .accessibility
            .guardrail
            .targets
            .get(&request.target_node)
            .copied()
        else {
            // Confirmation is modal and safety-critical. Any stale or
            // background request is consumed without side effects.
            return;
        };
        let (request_id, action) = match target {
            GuardrailSemanticTarget::Confirm(request_id) => (
                request_id,
                crate::terminal::guardrails::GuardrailAction::Confirm { request_id },
            ),
            GuardrailSemanticTarget::Cancel(request_id) => (
                request_id,
                crate::terminal::guardrails::GuardrailAction::Cancel { request_id },
            ),
        };
        if !self.guardrail.is_awaiting(request_id) {
            return;
        }
        match request.action {
            Action::Focus => {
                self.accessibility.focus_override = Some(request.target_node);
            }
            Action::Click => {
                self.apply_guardrail_action(action);
                self.accessibility.focus_override = Some(GUARDRAIL_DIALOG);
            }
            _ => {}
        }
    }

    pub(super) fn guardrail_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self.guardrail_layout();
        let request_id = self.guardrail.request_id();
        let awaiting = self.guardrail.is_awaiting_current();
        let matched = self.guardrail.matched.clone();
        let confirm_target = GuardrailSemanticTarget::Confirm(request_id);
        let cancel_target = GuardrailSemanticTarget::Cancel(request_id);
        self.accessibility
            .guardrail
            .retain(|target| *target == confirm_target || *target == cancel_target);
        let confirm_id = self.accessibility.guardrail.node(&confirm_target);
        let cancel_id = self.accessibility.guardrail.node(&cancel_target);

        let mut nodes = Vec::with_capacity(7);

        let (command_value, reason_value, context_value, critical) = matched.as_ref().map_or_else(
            || {
                (
                    "Guarded command unavailable".to_string(),
                    "The confirmation request is closing".to_string(),
                    "No command can be released".to_string(),
                    true,
                )
            },
            |matched| {
                (
                    crate::safe_text::truncate_with_ellipsis(&matched.command, 1024),
                    crate::safe_text::truncate_with_ellipsis(&matched.reason, 512),
                    format!(
                        "Where: {}",
                        crate::safe_text::truncate_with_ellipsis(&self.guardrail.target, 256)
                    ),
                    matched.severity == crate::terminal::guardrails::Severity::Critical,
                )
            },
        );

        let title_y = layout.card.y + layout.padding;
        let command_y = title_y + layout.title_height;
        let reason_y = command_y + layout.command_height;
        let context_y = reason_y + layout.reason_height;
        let content_x = layout.card.x + layout.padding;
        let content_width = layout.card.width - layout.padding * 2.0;

        let mut command = Node::new(Role::Code);
        command.set_label("Guarded command");
        command.set_value(command_value);
        command.set_bounds(rect(
            content_x,
            command_y,
            content_width,
            layout.command_height,
        ));
        nodes.push((GUARDRAIL_COMMAND, command));

        let mut reason = Node::new(Role::Alert);
        reason.set_label(reason_value.clone());
        reason.set_description(if critical {
            "Critical destructive-command warning"
        } else {
            "Potentially destructive-command warning"
        });
        reason.set_live(if critical {
            Live::Assertive
        } else {
            Live::Polite
        });
        reason.set_bounds(rect(
            content_x,
            reason_y,
            content_width,
            layout.reason_height,
        ));
        nodes.push((GUARDRAIL_REASON, reason));

        let mut context = Node::new(Role::Label);
        context.set_label(context_value.clone());
        context.set_bounds(rect(
            content_x,
            context_y,
            content_width,
            layout.context_height,
        ));
        nodes.push((GUARDRAIL_CONTEXT, context));

        let mut cancel = Node::new(Role::Button);
        cancel.set_label("Cancel guarded command");
        cancel.set_description("Discard all buffered guarded input; recommended safe action");
        cancel.set_keyboard_shortcut("Enter or Escape");
        cancel.set_bounds(bounds_rect(layout.cancel_button));
        if awaiting {
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
        } else {
            cancel.set_disabled();
        }
        nodes.push((cancel_id, cancel));

        let mut confirm = Node::new(Role::Button);
        confirm.set_label(if critical {
            "Run dangerous command anyway"
        } else {
            "Confirm guarded command"
        });
        confirm.set_description(format!(
            "Release all buffered input to the exact captured terminal destination or destination set; {reason_value}; {context_value}"
        ));
        confirm.set_keyboard_shortcut("Ctrl+Enter");
        confirm.set_bounds(bounds_rect(layout.confirm_button));
        if awaiting {
            confirm.add_action(Action::Click);
            confirm.add_action(Action::Focus);
        } else {
            confirm.set_disabled();
        }
        nodes.push((confirm_id, confirm));

        let mut dialog = Node::new(Role::AlertDialog);
        dialog.set_label(if critical {
            "Dangerous command blocked"
        } else {
            "Command requires confirmation"
        });
        dialog.set_description("Review the exact command, target, and user before choosing");
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        dialog.set_bounds(bounds_rect(layout.card));
        dialog.set_children([
            GUARDRAIL_COMMAND,
            GUARDRAIL_REASON,
            GUARDRAIL_CONTEXT,
            cancel_id,
            confirm_id,
        ]);
        nodes.push((GUARDRAIL_DIALOG, dialog));

        let default_focus = if awaiting {
            cancel_id
        } else {
            GUARDRAIL_DIALOG
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                accessibility.is_guardrail_node(node) && (awaiting || node == GUARDRAIL_DIALOG)
            });
        ModalSnapshot {
            dialog: GUARDRAIL_DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_info_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::info_overlay::InfoOverlayAction;

        let Some(target) = self
            .accessibility
            .info
            .targets
            .get(&request.target_node)
            .cloned()
        else {
            // The notice is modal. Background and retired request nodes are
            // consumed instead of falling through to terminal chrome.
            return;
        };
        let request_id = match &target {
            InfoSemanticTarget::Content(request_id)
            | InfoSemanticTarget::Input(request_id)
            | InfoSemanticTarget::Copy(request_id)
            | InfoSemanticTarget::Dismiss(request_id) => *request_id,
            InfoSemanticTarget::Choice { request_id, .. } => *request_id,
        };
        if !self.info_overlay.is_awaiting(request_id) {
            return;
        }
        let action = match (request.action, target) {
            (Action::Focus, target) => {
                use crate::ui::info_overlay::InfoOverlayFocus;
                let focus = match target {
                    InfoSemanticTarget::Content(_) => None,
                    InfoSemanticTarget::Input(_) => Some(InfoOverlayFocus::Input),
                    InfoSemanticTarget::Choice { id, .. } => Some(InfoOverlayFocus::Choice(id)),
                    InfoSemanticTarget::Copy(_) => Some(InfoOverlayFocus::Copy),
                    InfoSemanticTarget::Dismiss(_) => Some(InfoOverlayFocus::Continue),
                };
                self.info_overlay.set_focus(request_id, focus);
                self.ime.clear_preedit();
                self.accessibility.focus_override = Some(request.target_node);
                InfoOverlayAction::None
            }
            (Action::Click, InfoSemanticTarget::Copy(request_id)) => {
                InfoOverlayAction::Copy { request_id }
            }
            (Action::Click, InfoSemanticTarget::Dismiss(request_id)) => {
                if self.info_overlay.has_input() {
                    InfoOverlayAction::SubmitInput { request_id }
                } else {
                    InfoOverlayAction::Dismiss { request_id }
                }
            }
            (Action::SetValue, InfoSemanticTarget::Input(request_id)) => {
                let Some(ActionData::Value(value)) = request.data.as_ref() else {
                    return;
                };
                if self.info_overlay.replace_input_text(request_id, value) {
                    self.info_overlay.set_focus(
                        request_id,
                        Some(crate::ui::info_overlay::InfoOverlayFocus::Input),
                    );
                    self.accessibility.focus_override = Some(request.target_node);
                }
                InfoOverlayAction::None
            }
            (
                Action::Click,
                InfoSemanticTarget::Choice {
                    request_id,
                    id: choice_id,
                },
            ) => InfoOverlayAction::Choose {
                request_id,
                choice_id,
            },
            (Action::ScrollUp, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::Scroll {
                    request_id,
                    by: OverlayScroll::Rows(-(self.info_overlay.viewport().rows() as isize)),
                }
            }
            (Action::ScrollDown, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::Scroll {
                    request_id,
                    by: OverlayScroll::Rows(self.info_overlay.viewport().rows() as isize),
                }
            }
            (Action::ScrollLeft, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::Scroll {
                    request_id,
                    by: OverlayScroll::Columns(-(self.info_overlay.viewport().columns() as isize)),
                }
            }
            (Action::ScrollRight, InfoSemanticTarget::Content(request_id)) => {
                InfoOverlayAction::Scroll {
                    request_id,
                    by: OverlayScroll::Columns(self.info_overlay.viewport().columns() as isize),
                }
            }
            _ => InfoOverlayAction::None,
        };
        self.apply_info_overlay_action(action);
    }

    pub(super) fn info_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self.info_overlay_layout();
        self.info_overlay
            .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
        let request_id = self.info_overlay.request_id();
        let awaiting = self.info_overlay.is_awaiting_current();
        let scroll_offset = self.info_overlay.viewport().row();
        let horizontal_offset = self.info_overlay.viewport().column();
        let total_lines = self.info_overlay.lines().len();
        let input_info = self.info_overlay.input_info(request_id);
        let input_visible = input_info.as_ref().is_some_and(|input| {
            layout
                .source_line_rect(input.source_line, scroll_offset, total_lines)
                .is_some()
        });

        let content_target = InfoSemanticTarget::Content(request_id);
        let copy_target = InfoSemanticTarget::Copy(request_id);
        let dismiss_target = InfoSemanticTarget::Dismiss(request_id);
        let visible_choices: Vec<_> = self
            .info_overlay
            .choices()
            .iter()
            .filter(|choice| {
                choice.source_line >= scroll_offset
                    && choice.source_line < scroll_offset.saturating_add(layout.rows_that_fit)
            })
            .cloned()
            .collect();
        let mut targets = HashSet::from([
            content_target.clone(),
            copy_target.clone(),
            dismiss_target.clone(),
        ]);
        if input_visible {
            targets.insert(InfoSemanticTarget::Input(request_id));
        }
        targets.extend(
            visible_choices
                .iter()
                .map(|choice| InfoSemanticTarget::Choice {
                    request_id,
                    id: choice.id.clone(),
                }),
        );
        self.accessibility
            .info
            .retain(|target| targets.contains(target));
        let content_id = self.accessibility.info.node(&content_target);
        let copy_id = self.accessibility.info.node(&copy_target);
        let dismiss_id = self.accessibility.info.node(&dismiss_target);
        let input_id = input_visible.then(|| {
            self.accessibility
                .info
                .node(&InfoSemanticTarget::Input(request_id))
        });
        let choice_nodes: Vec<_> = visible_choices
            .iter()
            .map(|choice| {
                let target = InfoSemanticTarget::Choice {
                    request_id,
                    id: choice.id.clone(),
                };
                (choice.clone(), self.accessibility.info.node(&target))
            })
            .collect();

        let visible_lines: Vec<_> = self
            .info_overlay
            .visible_lines()
            .map(|(_, line)| {
                line.chars()
                    .skip(horizontal_offset)
                    .take(layout.columns_that_fit)
                    .collect::<String>()
            })
            .collect();
        let first_line = scroll_offset.saturating_add(1).min(total_lines.max(1));
        let last_line = scroll_offset
            .saturating_add(layout.rows_that_fit)
            .min(total_lines);
        let status_text = format!(
            "Showing lines {first_line} through {last_line} of {total_lines}, starting at column {}",
            horizontal_offset + 1
        );

        let mut nodes = Vec::with_capacity(7 + choice_nodes.len());

        let mut title = Node::new(Role::Heading);
        title.set_label(self.info_overlay.title());
        title.set_bounds(rect(
            layout.card.x + layout.padding,
            layout.card.y + layout.padding,
            layout.card.width - layout.padding * 2.0,
            layout.title_height,
        ));
        nodes.push((INFO_TITLE, title));

        let mut content = Node::new(Role::Document);
        content.set_label("Notice content");
        content.set_value(visible_lines.join("\n"));
        content.set_description(status_text.clone());
        content.set_bounds(bounds_rect(layout.content));
        let mut content_children = input_id.into_iter().collect::<Vec<_>>();
        content_children.extend(choice_nodes.iter().map(|(_, node)| *node));
        content.set_children(content_children);
        if awaiting {
            content.add_action(Action::Focus);
            if self.info_overlay.viewport().row() > 0 {
                content.add_action(Action::ScrollUp);
            }
            if self.info_overlay.viewport().row() < self.info_overlay.viewport().max_row() {
                content.add_action(Action::ScrollDown);
            }
            if self.info_overlay.viewport().column() > 0 {
                content.add_action(Action::ScrollLeft);
            }
            if self.info_overlay.viewport().column() < self.info_overlay.viewport().max_column() {
                content.add_action(Action::ScrollRight);
            }
        } else {
            content.set_disabled();
        }
        nodes.push((content_id, content));

        if let (Some(input), Some(input_id)) = (input_info.as_ref(), input_id)
            && let Some(input_rect) =
                layout.source_line_rect(input.source_line, scroll_offset, total_lines)
        {
            let mut node = Node::new(Role::TextInput);
            node.set_label(input.label.trim_end_matches([' ', ':']));
            node.set_description(format!("Single-line value, at most {}", input.limit));
            set_text_value(&mut node, input_id, &input.value, Some(input.cursor_byte));
            node.set_bounds(bounds_rect(input_rect));
            if awaiting {
                node.add_action(Action::Focus);
                node.add_action(Action::SetValue);
            } else {
                node.set_disabled();
            }
            nodes.push((input_id, node));
        }

        for (choice, node_id) in &choice_nodes {
            let visible_row = choice.source_line.saturating_sub(scroll_offset);
            let mut node = Node::new(Role::Button);
            node.set_label(choice.label.as_str());
            node.set_description("Select this notice option");
            if let Some(shortcut) = choice.shortcut {
                node.set_keyboard_shortcut(shortcut.to_string());
            }
            node.set_bounds(rect(
                layout.content.x,
                layout.content.y + visible_row as f32 * layout.line_height,
                layout.content.width,
                layout.line_height,
            ));
            if awaiting {
                node.add_action(Action::Click);
                node.add_action(Action::Focus);
            } else {
                node.set_disabled();
            }
            nodes.push((*node_id, node));
        }

        let mut status = Node::new(Role::Status);
        status.set_label(status_text);
        status.set_live(Live::Polite);
        nodes.push((INFO_STATUS, status));

        let mut copy = Node::new(Role::Button);
        copy.set_label("Copy notice content");
        if self.info_overlay.copy_key_free() {
            copy.set_keyboard_shortcut("C");
        }
        copy.set_bounds(bounds_rect(layout.copy_button));
        if awaiting {
            copy.add_action(Action::Click);
            copy.add_action(Action::Focus);
        } else {
            copy.set_disabled();
        }
        nodes.push((copy_id, copy));

        let mut dismiss = Node::new(Role::DefaultButton);
        if input_info.is_some() {
            dismiss.set_label("Submit input");
            dismiss.set_keyboard_shortcut("Enter");
        } else {
            dismiss.set_label("Continue and close notice");
            dismiss.set_keyboard_shortcut("Enter or Escape");
        }
        dismiss.set_bounds(bounds_rect(layout.continue_button));
        if awaiting {
            dismiss.add_action(Action::Click);
            dismiss.add_action(Action::Focus);
        } else {
            dismiss.set_disabled();
        }
        nodes.push((dismiss_id, dismiss));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(self.info_overlay.title());
        dialog.set_description("Application notice with bounded scrollable content");
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        dialog.set_bounds(bounds_rect(layout.card));
        let mut children = vec![INFO_TITLE, content_id];
        children.extend([INFO_STATUS, copy_id, dismiss_id]);
        dialog.set_children(children);
        nodes.push((INFO_DIALOG, dialog));

        let model_focus = self.info_overlay.focus().and_then(|focus| {
            use crate::ui::info_overlay::InfoOverlayFocus;
            match focus {
                InfoOverlayFocus::Input => input_id,
                InfoOverlayFocus::Choice(id) => choice_nodes
                    .iter()
                    .find(|(choice, _)| &choice.id == id)
                    .map(|(_, node)| *node),
                InfoOverlayFocus::Copy => Some(copy_id),
                InfoOverlayFocus::Continue => Some(dismiss_id),
            }
        });
        let focus = model_focus.unwrap_or(if awaiting { content_id } else { INFO_DIALOG });
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: INFO_DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_diff_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::diff_review::DiffReviewAction;

        let Some(target) = self
            .accessibility
            .diff
            .targets
            .get(&request.target_node)
            .copied()
        else {
            // Diff review is modal. Stale nodes and background application
            // controls are consumed without falling through.
            return;
        };
        let request_id = match target {
            DiffSemanticTarget::Content(request_id)
            | DiffSemanticTarget::Copy(request_id)
            | DiffSemanticTarget::Dismiss(request_id)
            | DiffSemanticTarget::Stage(request_id)
            | DiffSemanticTarget::Revert(request_id) => request_id,
        };
        if !self.diff_review.is_awaiting(request_id) {
            return;
        }
        let action = match (request.action, target) {
            (Action::Focus, _) => {
                self.accessibility.focus_override = Some(request.target_node);
                DiffReviewAction::None
            }
            (Action::Click, DiffSemanticTarget::Copy(request_id)) => {
                DiffReviewAction::Copy { request_id }
            }
            (Action::Click, DiffSemanticTarget::Dismiss(request_id)) => {
                DiffReviewAction::Dismiss { request_id }
            }
            (Action::Click, DiffSemanticTarget::Stage(request_id)) => {
                DiffReviewAction::Stage { request_id }
            }
            (Action::Click, DiffSemanticTarget::Revert(request_id)) => {
                DiffReviewAction::Revert { request_id }
            }
            (Action::ScrollUp, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: OverlayScroll::Rows(-(self.diff_review.viewport().rows() as isize)),
                }
            }
            (Action::ScrollDown, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: OverlayScroll::Rows(self.diff_review.viewport().rows() as isize),
                }
            }
            (Action::ScrollLeft, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: OverlayScroll::Columns(-(self.diff_review.viewport().columns() as isize)),
                }
            }
            (Action::ScrollRight, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: OverlayScroll::Columns(self.diff_review.viewport().columns() as isize),
                }
            }
            _ => DiffReviewAction::None,
        };
        self.apply_diff_review_action(action);
    }

    pub(super) fn diff_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self.diff_review_layout();
        self.diff_review
            .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
        let request_id = self.diff_review.request_id();
        let awaiting = self.diff_review.is_awaiting_current();
        let scroll_offset = self.diff_review.viewport().row();
        let horizontal_offset = self.diff_review.viewport().column();
        let total_rows = self.diff_review.rows().len();

        let content_target = DiffSemanticTarget::Content(request_id);
        let copy_target = DiffSemanticTarget::Copy(request_id);
        let dismiss_target = DiffSemanticTarget::Dismiss(request_id);
        let change_actions = [
            (DiffSemanticTarget::Stage(request_id), layout.stage_button),
            (DiffSemanticTarget::Revert(request_id), layout.revert_button),
        ];
        let mut targets = HashSet::from([content_target, copy_target, dismiss_target]);
        targets.extend(
            change_actions
                .iter()
                .filter(|(_, button)| button.is_some())
                .map(|(target, _)| *target),
        );
        self.accessibility
            .diff
            .retain(|target| targets.contains(target));
        let content_id = self.accessibility.diff.node(&content_target);
        let copy_id = self.accessibility.diff.node(&copy_target);
        let dismiss_id = self.accessibility.diff.node(&dismiss_target);

        let visible_rows: Vec<_> = self
            .diff_review
            .visible_rows()
            .map(|(_, row)| {
                row.display_text()
                    .chars()
                    .skip(horizontal_offset)
                    .take(layout.columns_that_fit)
                    .collect::<String>()
            })
            .collect();
        let first_row = scroll_offset.saturating_add(1).min(total_rows.max(1));
        let last_row = scroll_offset
            .saturating_add(layout.rows_that_fit)
            .min(total_rows);
        let status_text = format!(
            "Showing diff rows {first_row} through {last_row} of {total_rows}, starting at column {}",
            horizontal_offset + 1
        );

        let mut nodes = Vec::with_capacity(7);

        let title_text = format!("Diff: {}", self.diff_review.filename());
        let mut title = Node::new(Role::Heading);
        title.set_label(title_text.as_str());
        title.set_bounds(rect(
            layout.card.x + layout.padding,
            layout.card.y + layout.padding,
            layout.card.width - layout.padding * 2.0,
            layout.title_height,
        ));
        nodes.push((DIFF_TITLE, title));

        let mut content = Node::new(Role::Document);
        content.set_label(if self.diff_review.has_change_actions() {
            "Changes to this file since the agent started"
        } else {
            "Detected unified diff"
        });
        content.set_value(visible_rows.join("\n"));
        content.set_description(status_text.clone());
        content.set_bounds(bounds_rect(layout.content));
        if awaiting {
            content.add_action(Action::Focus);
            if self.diff_review.viewport().row() > 0 {
                content.add_action(Action::ScrollUp);
            }
            if self.diff_review.viewport().row() < self.diff_review.viewport().max_row() {
                content.add_action(Action::ScrollDown);
            }
            if self.diff_review.viewport().column() > 0 {
                content.add_action(Action::ScrollLeft);
            }
            if self.diff_review.viewport().column() < self.diff_review.viewport().max_column() {
                content.add_action(Action::ScrollRight);
            }
        } else {
            content.set_disabled();
        }
        nodes.push((content_id, content));

        let mut status = Node::new(Role::Status);
        status.set_label(status_text);
        status.set_live(Live::Polite);
        nodes.push((DIFF_STATUS, status));

        let mut change_ids = Vec::new();
        for (target, button) in change_actions {
            let Some(button) = button else {
                continue;
            };
            let (label, shortcut) = match target {
                DiffSemanticTarget::Stage(_) => ("Stage this file", "S"),
                _ => ("Revert this file to how it was when the agent started", "R"),
            };
            let id = self.accessibility.diff.node(&target);
            let mut node = Node::new(Role::Button);
            node.set_label(label);
            node.set_keyboard_shortcut(shortcut);
            node.set_bounds(bounds_rect(button));
            if awaiting {
                node.add_action(Action::Click);
                node.add_action(Action::Focus);
            } else {
                node.set_disabled();
            }
            nodes.push((id, node));
            change_ids.push(id);
        }

        let mut copy = Node::new(Role::Button);
        copy.set_label("Copy complete raw diff");
        copy.set_keyboard_shortcut("C");
        copy.set_bounds(bounds_rect(layout.copy_button));
        if awaiting {
            copy.add_action(Action::Click);
            copy.add_action(Action::Focus);
        } else {
            copy.set_disabled();
        }
        nodes.push((copy_id, copy));

        let mut dismiss = Node::new(Role::DefaultButton);
        dismiss.set_label("Mark this diff reviewed and close");
        dismiss.set_keyboard_shortcut("Enter or Escape");
        dismiss.set_bounds(bounds_rect(layout.done_button));
        if awaiting {
            dismiss.add_action(Action::Click);
            dismiss.add_action(Action::Focus);
        } else {
            dismiss.set_disabled();
        }
        nodes.push((dismiss_id, dismiss));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(title_text);
        dialog.set_description(if self.diff_review.has_change_actions() {
            "Scrollable review of one file an agent changed, which can be staged or reverted"
        } else {
            "Scrollable review of one detected unified diff"
        });
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        dialog.set_bounds(bounds_rect(layout.card));
        let mut children = vec![DIFF_TITLE, content_id, DIFF_STATUS];
        children.extend(change_ids);
        children.extend([copy_id, dismiss_id]);
        dialog.set_children(children);
        nodes.push((DIFF_DIALOG, dialog));

        let focus = self.accessibility.modal_focus(
            &nodes,
            if awaiting { dismiss_id } else { DIFF_DIALOG },
            |accessibility, node| {
                accessibility.is_diff_node(node) && (awaiting || node == DIFF_DIALOG)
            },
        );
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: DIFF_DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_context_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::context_picker::ContextPickerAction;

        let Some(target) = self
            .accessibility
            .context
            .targets
            .get(&request.target_node)
            .copied()
        else {
            return;
        };
        let request_id = match target {
            ContextSemanticTarget::List(request_id)
            | ContextSemanticTarget::FileInput(request_id)
            | ContextSemanticTarget::Cancel(request_id)
            | ContextSemanticTarget::Primary(request_id) => request_id,
            ContextSemanticTarget::Source { request_id, .. } => request_id,
        };
        if !self.context_picker.is_awaiting(request_id) {
            return;
        }
        let action = match (request.action, target) {
            (Action::Focus, ContextSemanticTarget::Source { source, .. }) => {
                self.context_picker.select_source(request_id, source);
                self.accessibility.focus_override = Some(request.target_node);
                ContextPickerAction::None
            }
            (Action::Focus, _) => {
                self.accessibility.focus_override = Some(request.target_node);
                ContextPickerAction::None
            }
            (Action::Click, ContextSemanticTarget::Source { source, .. }) => {
                ContextPickerAction::Toggle { request_id, source }
            }
            (Action::Click, ContextSemanticTarget::Cancel(_)) => {
                ContextPickerAction::Cancel { request_id }
            }
            (Action::Click, ContextSemanticTarget::Primary(_)) => {
                ContextPickerAction::Primary { request_id }
            }
            (Action::SetValue, ContextSemanticTarget::FileInput(_)) => {
                let Some(ActionData::Value(value)) = request.data else {
                    return;
                };
                ContextPickerAction::SetFileInput {
                    request_id,
                    value: value.into(),
                }
            }
            (Action::ScrollUp, ContextSemanticTarget::List(_)) => ContextPickerAction::Scroll {
                request_id,
                delta: -(self.context_picker.viewport_rows() as isize),
            },
            (Action::ScrollDown, ContextSemanticTarget::List(_)) => ContextPickerAction::Scroll {
                request_id,
                delta: self.context_picker.viewport_rows() as isize,
            },
            _ => ContextPickerAction::None,
        };
        self.apply_context_picker_action(action);
    }

    pub(super) fn context_accessibility_snapshot(&mut self) -> ModalSnapshot {
        use crate::ui::context_picker::{ContextPickerPhase, ContextSourceId};

        let layout = self.context_picker_layout();
        self.context_picker.set_viewport(layout.visible_count);
        let snapshot = self.context_picker.snapshot();
        let request_id = snapshot.request_id;
        let list_target = ContextSemanticTarget::List(request_id);
        let input_target = ContextSemanticTarget::FileInput(request_id);
        let cancel_target = ContextSemanticTarget::Cancel(request_id);
        let primary_target = ContextSemanticTarget::Primary(request_id);
        let visible_sources: Vec<_> = snapshot
            .sources
            .iter()
            .skip(snapshot.scroll_offset)
            .take(layout.visible_count)
            .cloned()
            .collect();

        let mut targets = HashSet::from([cancel_target, primary_target]);
        match snapshot.phase {
            ContextPickerPhase::Sources => {
                targets.insert(list_target);
                targets.extend(visible_sources.iter().map(|source| {
                    ContextSemanticTarget::Source {
                        request_id,
                        source: source.id,
                    }
                }));
            }
            ContextPickerPhase::FilePaths => {
                targets.insert(input_target);
            }
            ContextPickerPhase::Gathering => {}
        }
        self.accessibility
            .context
            .retain(|target| targets.contains(target));
        let cancel_id = self.accessibility.context.node(&cancel_target);
        let primary_id = self.accessibility.context.node(&primary_target);
        let list_id = (snapshot.phase == ContextPickerPhase::Sources)
            .then(|| self.accessibility.context.node(&list_target));
        let input_id = (snapshot.phase == ContextPickerPhase::FilePaths)
            .then(|| self.accessibility.context.node(&input_target));
        let source_nodes: Vec<_> = if snapshot.phase == ContextPickerPhase::Sources {
            visible_sources
                .iter()
                .map(|source| {
                    let id = self
                        .accessibility
                        .context
                        .node(&ContextSemanticTarget::Source {
                            request_id,
                            source: source.id,
                        });
                    (source.clone(), id)
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut nodes = Vec::with_capacity(8 + source_nodes.len());

        let title_text = match snapshot.phase {
            ContextPickerPhase::Sources => "Insert Context",
            ContextPickerPhase::FilePaths => "Choose Workspace Files",
            ContextPickerPhase::Gathering => "Gathering Context",
        };
        let mut title = Node::new(Role::Heading);
        title.set_label(title_text);
        title.set_bounds(rect(
            layout.card.x + layout.padding,
            layout.card.y + layout.padding,
            layout.card.width - layout.padding * 2.0,
            layout.title_height,
        ));
        nodes.push((CONTEXT_TITLE, title));

        if let Some(list_id) = list_id {
            let mut list = Node::new(Role::List);
            list.set_label("Available context sources");
            list.set_bounds(bounds_rect(layout.content));
            list.set_children(source_nodes.iter().map(|(_, id)| *id).collect::<Vec<_>>());
            if snapshot.awaiting {
                list.add_action(Action::Focus);
                if snapshot.scroll_offset > 0 {
                    list.add_action(Action::ScrollUp);
                }
                if snapshot.scroll_offset
                    < snapshot.sources.len().saturating_sub(layout.visible_count)
                {
                    list.add_action(Action::ScrollDown);
                }
            } else {
                list.set_disabled();
            }
            nodes.push((list_id, list));

            for (visible_index, (source, node_id)) in source_nodes.iter().enumerate() {
                let source_index = snapshot.scroll_offset + visible_index;
                let mut node = Node::new(Role::CheckBox);
                node.set_label(source.label.as_str());
                node.set_description(if source.id == ContextSourceId::SpecificFiles {
                    "Opens a workspace-bounded comma-separated file path editor"
                } else if source.available {
                    "Toggle this source in the context bundle"
                } else {
                    "Unavailable for the current working directory"
                });
                node.set_toggled(if source.selected {
                    Toggled::True
                } else {
                    Toggled::False
                });
                node.set_selected(source_index == snapshot.selected_index);
                node.set_bounds(rect(
                    layout.content.x,
                    layout.content.y + visible_index as f32 * layout.row_height,
                    layout.content.width,
                    layout.row_height,
                ));
                if snapshot.awaiting && source.available {
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                } else {
                    node.set_disabled();
                }
                nodes.push((*node_id, node));
            }
        }

        if let Some(input_id) = input_id {
            let mut input = Node::new(Role::TextInput);
            input.set_label("Workspace file paths");
            input.set_description(
                "Comma-separated paths; files must remain inside the context working directory",
            );
            input.set_value(snapshot.file_input.as_str());
            input.set_bounds(bounds_rect(layout.file_input));
            if snapshot.awaiting {
                input.add_action(Action::Focus);
                input.add_action(Action::SetValue);
            } else {
                input.set_disabled();
            }
            nodes.push((input_id, input));
        }

        let status_text = snapshot
            .status
            .clone()
            .unwrap_or_else(|| match snapshot.phase {
                ContextPickerPhase::Sources => format!(
                    "{} of {} sources selected",
                    snapshot
                        .sources
                        .iter()
                        .filter(|source| source.selected)
                        .count(),
                    snapshot.sources.len()
                ),
                ContextPickerPhase::FilePaths => {
                    "Enter comma-separated workspace-relative file paths".into()
                }
                ContextPickerPhase::Gathering => {
                    "Gathering bounded context in the background".into()
                }
            });
        let mut status = Node::new(Role::Status);
        status.set_label(status_text);
        status.set_live(Live::Polite);
        nodes.push((CONTEXT_STATUS, status));

        let mut cancel = Node::new(Role::Button);
        cancel.set_label(snapshot.cancel_label());
        cancel.set_keyboard_shortcut("Escape");
        cancel.set_bounds(bounds_rect(layout.cancel_button));
        if snapshot.awaiting {
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
        } else {
            cancel.set_disabled();
        }
        nodes.push((cancel_id, cancel));

        let primary_enabled = snapshot.awaiting && snapshot.phase != ContextPickerPhase::Gathering;
        let mut primary = Node::new(Role::DefaultButton);
        primary.set_label(snapshot.primary_label());
        primary.set_keyboard_shortcut("Enter");
        primary.set_bounds(bounds_rect(layout.primary_button));
        if primary_enabled {
            primary.add_action(Action::Click);
            primary.add_action(Action::Focus);
        } else {
            primary.set_disabled();
        }
        nodes.push((primary_id, primary));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(title_text);
        dialog.set_description(
            "Choose bounded local context and insert it into one exact terminal destination",
        );
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        dialog.set_bounds(bounds_rect(layout.card));
        let mut children = vec![CONTEXT_TITLE];
        if let Some(main) = list_id.or(input_id) {
            children.push(main);
        }
        children.extend([CONTEXT_STATUS, cancel_id, primary_id]);
        dialog.set_children(children);
        nodes.push((CONTEXT_DIALOG, dialog));

        let default_focus = match snapshot.phase {
            ContextPickerPhase::Sources => source_nodes
                .iter()
                .enumerate()
                .find(|(visible_index, (source, _))| {
                    source.available
                        && snapshot.scroll_offset + visible_index == snapshot.selected_index
                })
                .map(|(_, (_, id))| *id)
                .or_else(|| {
                    source_nodes
                        .iter()
                        .find(|(source, _)| source.available)
                        .map(|(_, id)| *id)
                })
                .unwrap_or(primary_id),
            ContextPickerPhase::FilePaths => input_id.unwrap_or(cancel_id),
            ContextPickerPhase::Gathering => cancel_id,
        };
        let focus = self.accessibility.modal_focus(
            &nodes,
            if snapshot.awaiting {
                default_focus
            } else {
                CONTEXT_DIALOG
            },
            |accessibility, node| {
                accessibility.is_context_node(node) && (snapshot.awaiting || node == CONTEXT_DIALOG)
            },
        );
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: CONTEXT_DIALOG,
            nodes,
            focus,
        }
    }
}
