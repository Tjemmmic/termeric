//! The guardrail confirmation, dialog, diff review, and context
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

    pub(super) fn handle_dialog_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::dialog::{DialogAction, DialogFocus, DialogScroll};

        let request_id = self.dialog.request_id();
        let target = if request.target_node == DIALOG_BODY {
            None
        } else {
            match self
                .accessibility
                .dialog
                .targets
                .get(&request.target_node)
                .cloned()
            {
                Some(target) => Some(target),
                // The dialog is modal. Background and retired request nodes
                // are consumed instead of falling through to the chrome.
                None => return,
            }
        };
        let target_request = match &target {
            None => request_id,
            Some(
                DialogSemanticTarget::Field(request_id)
                | DialogSemanticTarget::Row { request_id, .. }
                | DialogSemanticTarget::Button { request_id, .. },
            ) => *request_id,
        };
        if !self.dialog.is_awaiting(target_request) {
            return;
        }
        let row_index =
            |app: &Self, id: &str| app.dialog.rows().iter().position(|row| row.id == id);
        let action = match (request.action, target) {
            (Action::Focus, target) => {
                let focus = match target {
                    None => None,
                    Some(DialogSemanticTarget::Field(_)) => Some(DialogFocus::Field),
                    Some(DialogSemanticTarget::Row { id, .. }) => {
                        row_index(self, &id).map(DialogFocus::Row)
                    }
                    Some(DialogSemanticTarget::Button { index, .. }) => {
                        Some(DialogFocus::Button(index))
                    }
                };
                self.dialog.set_focus(request_id, focus);
                self.ime.clear_preedit();
                self.accessibility.focus_override = Some(request.target_node);
                DialogAction::None
            }
            (Action::Click, Some(DialogSemanticTarget::Button { index, .. })) => {
                self.dialog.activate_button(request_id, index)
            }
            (Action::Click, Some(DialogSemanticTarget::Row { id, .. })) => row_index(self, &id)
                .map_or(DialogAction::None, |row| {
                    self.dialog.activate_row(request_id, row)
                }),
            (Action::SetValue, Some(DialogSemanticTarget::Field(_))) => {
                let Some(ActionData::Value(value)) = request.data.as_ref() else {
                    return;
                };
                if self.dialog.replace_field_text(request_id, value) {
                    self.dialog.set_focus(request_id, Some(DialogFocus::Field));
                    self.accessibility.focus_override = Some(request.target_node);
                }
                DialogAction::None
            }
            (Action::ScrollUp, None) => DialogAction::Scroll {
                request_id,
                by: DialogScroll::Pages(-1),
            },
            (Action::ScrollDown, None) => DialogAction::Scroll {
                request_id,
                by: DialogScroll::Pages(1),
            },
            _ => DialogAction::None,
        };
        self.apply_dialog_action(action);
    }

    /// The dialog's nodes: an alert dialog for confirmations and errors and
    /// a dialog otherwise, labelled by its heading and described by its
    /// body, with its field, list rows, and buttons, all bounded by the
    /// layout it is drawn with.
    pub(super) fn dialog_accessibility_snapshot(&mut self) -> ModalSnapshot {
        use crate::ui::dialog::{ButtonRole, DialogFocus};

        let layout = self.dialog_layout();
        let request_id = self.dialog.request_id();
        let awaiting = self.dialog.is_awaiting_current();
        let offset = self.dialog.scroll_offset();
        let field = self.dialog.field(request_id);
        let rows = self.dialog.rows().to_vec();
        let buttons = self.dialog.buttons().to_vec();

        let field_target = DialogSemanticTarget::Field(request_id);
        let row_target = |id: &str| DialogSemanticTarget::Row {
            request_id,
            id: id.to_owned(),
        };
        let button_target = |index| DialogSemanticTarget::Button { request_id, index };
        let mut targets: HashSet<DialogSemanticTarget> =
            (0..buttons.len()).map(button_target).collect();
        if field.is_some() {
            targets.insert(field_target.clone());
        }
        targets.extend(rows.iter().map(|row| row_target(&row.id)));
        self.accessibility
            .dialog
            .retain(|target| targets.contains(target));
        let field_id = field
            .is_some()
            .then(|| self.accessibility.dialog.node(&field_target));
        let row_ids: Vec<NodeId> = rows
            .iter()
            .map(|row| self.accessibility.dialog.node(&row_target(&row.id)))
            .collect();
        let button_ids: Vec<NodeId> = (0..buttons.len())
            .map(|index| self.accessibility.dialog.node(&button_target(index)))
            .collect();

        let bounds = |bounds: Option<crate::ui::geometry::Bounds>| bounds.map(bounds_rect);
        let mut nodes = Vec::with_capacity(4 + rows.len() + buttons.len());
        let title = self.dialog.title().to_owned();
        let description = self
            .dialog
            .body()
            .iter()
            .map(crate::ui::dialog::DialogBlock::content)
            .collect::<Vec<_>>()
            .join("\n\n");

        let mut heading = Node::new(Role::Heading);
        heading.set_label(title.clone());
        if let Some(rect) = bounds(layout.as_ref().map(|layout| layout.heading_bounds)) {
            heading.set_bounds(rect);
        }
        nodes.push((DIALOG_HEADING, heading));

        let mut body_children = Vec::new();
        if let (Some(field), Some(field_id)) = (field.as_ref(), field_id) {
            let mut node = Node::new(Role::TextInput);
            node.set_label(field.label.as_str());
            match &field.error {
                Some(error) => node.set_description(error.as_str()),
                None => node.set_description(format!("At most {}", field.limit)),
            }
            set_text_value(&mut node, field_id, &field.value, Some(field.cursor_byte));
            if let Some(rect) = bounds(layout.as_ref().and_then(|layout| layout.field_box(offset)))
            {
                node.set_bounds(rect);
            }
            if awaiting {
                node.add_action(Action::Focus);
                node.add_action(Action::SetValue);
            } else {
                node.set_disabled();
            }
            nodes.push((field_id, node));
            body_children.push(field_id);
        }
        if !rows.is_empty() {
            let focused_row = match self.dialog.focus() {
                Some(DialogFocus::Row(row)) => Some(row),
                _ => None,
            };
            for (index, (row, node_id)) in rows.iter().zip(&row_ids).enumerate() {
                let mut node = Node::new(Role::ListItem);
                node.set_label(row.label.as_str());
                if let Some(detail) = &row.detail {
                    node.set_description(detail.as_str());
                }
                node.set_selected(focused_row == Some(index));
                if let Some(rect) =
                    bounds(layout.as_ref().and_then(|layout| layout.row(index, offset)))
                {
                    node.set_bounds(rect);
                }
                if awaiting {
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                } else {
                    node.set_disabled();
                }
                nodes.push((*node_id, node));
            }
            let mut list = Node::new(Role::ListBox);
            list.set_label(title.clone());
            if let Some(layout) = &layout {
                let first = layout.rows_top - offset;
                list.set_bounds(bounds_rect(crate::ui::geometry::Bounds::new(
                    layout.body.x,
                    layout.body.y + first,
                    layout.content_width,
                    layout.row_height * rows.len() as f32,
                )));
            }
            list.set_children(row_ids.clone());
            nodes.push((DIALOG_LIST, list));
            body_children.push(DIALOG_LIST);
        }

        let mut body = Node::new(Role::Document);
        body.set_label(title.clone());
        body.set_value(description.clone());
        if let Some(rect) = bounds(layout.as_ref().map(|layout| layout.body)) {
            body.set_bounds(rect);
        }
        body.set_children(body_children);
        let extent = self.dialog.extent();
        if awaiting {
            body.add_action(Action::Focus);
            if offset > 0.0 {
                body.add_action(Action::ScrollUp);
            }
            if offset + extent.viewport < extent.content {
                body.add_action(Action::ScrollDown);
            }
        } else {
            body.set_disabled();
        }
        nodes.push((DIALOG_BODY, body));

        for (index, (button, node_id)) in buttons.iter().zip(&button_ids).enumerate() {
            let mut node = Node::new(Role::Button);
            node.set_label(button.label.as_str());
            if button.role == ButtonRole::Cancel {
                node.set_keyboard_shortcut("Escape");
            }
            if let Some(rect) = bounds(
                layout
                    .as_ref()
                    .and_then(|layout| layout.buttons.get(index).copied()),
            ) {
                node.set_bounds(rect);
            }
            if awaiting {
                node.add_action(Action::Click);
                node.add_action(Action::Focus);
            } else {
                node.set_disabled();
            }
            nodes.push((*node_id, node));
        }

        let mut dialog = Node::new(if self.dialog.kind().is_alert() {
            Role::AlertDialog
        } else {
            Role::Dialog
        });
        dialog.set_label(title);
        if !description.is_empty() {
            dialog.set_description(description);
        }
        dialog.set_modal();
        dialog.add_action(Action::Focus);
        if let Some(rect) = bounds(layout.as_ref().map(|layout| layout.card)) {
            dialog.set_bounds(rect);
        }
        let mut children = vec![DIALOG_HEADING, DIALOG_BODY];
        children.extend(&button_ids);
        dialog.set_children(children);
        nodes.push((DIALOG, dialog));

        // Assistive focus follows the dialog's own focus.
        let model_focus = self.dialog.focus().and_then(|focus| match focus {
            DialogFocus::Field => field_id,
            DialogFocus::Row(row) => row_ids.get(row).copied(),
            DialogFocus::Button(button) => button_ids.get(button).copied(),
        });
        let focus = model_focus.unwrap_or(if awaiting { DIALOG_BODY } else { DIALOG });
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_diff_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::diff_review::{DiffReviewAction, DiffReviewFocus};

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
            DiffSemanticTarget::Content(request_id) | DiffSemanticTarget::Button(request_id, _) => {
                request_id
            }
        };
        if !self.diff_review.is_awaiting(request_id) {
            return;
        }
        let action = match (request.action, target) {
            (Action::Focus, target) => {
                let focus = match target {
                    DiffSemanticTarget::Content(_) => DiffReviewFocus::Content,
                    DiffSemanticTarget::Button(_, button) => DiffReviewFocus::Button(button),
                };
                if self.diff_review.set_focus(request_id, focus) {
                    self.accessibility.focus_override = Some(request.target_node);
                }
                DiffReviewAction::None
            }
            (Action::Click, DiffSemanticTarget::Button(request_id, button)) => {
                button.action(request_id)
            }
            (Action::ScrollUp, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: DiffReviewScroll::Rows(-(self.diff_review.viewport().rows() as isize)),
                }
            }
            (Action::ScrollDown, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: DiffReviewScroll::Rows(self.diff_review.viewport().rows() as isize),
                }
            }
            (Action::ScrollLeft, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: DiffReviewScroll::Columns(
                        -(self.diff_review.viewport().columns() as isize),
                    ),
                }
            }
            (Action::ScrollRight, DiffSemanticTarget::Content(request_id)) => {
                DiffReviewAction::Scroll {
                    request_id,
                    by: DiffReviewScroll::Columns(self.diff_review.viewport().columns() as isize),
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
        let buttons = self.diff_review.buttons();
        let mut targets = HashSet::from([content_target]);
        targets.extend(
            buttons
                .iter()
                .map(|button| DiffSemanticTarget::Button(request_id, *button)),
        );
        self.accessibility
            .diff
            .retain(|target| targets.contains(target));
        let content_id = self.accessibility.diff.node(&content_target);

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

        let title_text = format!("Changes in {}", self.diff_review.filename());
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

        let mut button_ids = Vec::with_capacity(buttons.len());
        let mut focused_id = None;
        for &button in buttons {
            use crate::ui::diff_review::DiffReviewButton;
            let Some(bounds) = layout.button(button) else {
                continue;
            };
            let id = self
                .accessibility
                .diff
                .node(&DiffSemanticTarget::Button(request_id, button));
            let mut node = Node::new(if button == DiffReviewButton::Done {
                Role::DefaultButton
            } else {
                Role::Button
            });
            node.set_label(match button {
                DiffReviewButton::Stage => "Stage this file",
                DiffReviewButton::Revert => "Revert this file to how it was when the agent started",
                DiffReviewButton::Copy => "Copy complete raw diff",
                DiffReviewButton::Done => "Mark this diff reviewed and close",
            });
            if button == DiffReviewButton::Done {
                node.set_keyboard_shortcut("Escape");
            }
            node.set_bounds(bounds_rect(bounds));
            if awaiting {
                node.add_action(Action::Click);
                node.add_action(Action::Focus);
            } else {
                node.set_disabled();
            }
            if self.diff_review.focused_button() == Some(button) {
                focused_id = Some(id);
            }
            nodes.push((id, node));
            button_ids.push(id);
        }

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
        children.extend(button_ids);
        dialog.set_children(children);
        nodes.push((DIFF_DIALOG, dialog));

        // Assistive focus follows the review's own focus.
        let focus = if awaiting {
            focused_id.unwrap_or(content_id)
        } else {
            DIFF_DIALOG
        };
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: DIFF_DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_context_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::context_picker::{ContextPickerAction, ContextPickerFocus};

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
            | ContextSemanticTarget::Button(request_id, _)
            | ContextSemanticTarget::Source { request_id, .. } => request_id,
        };
        if !self.context_picker.is_awaiting(request_id) {
            return;
        }
        let action = match (request.action, target) {
            // Assistive focus moves the picker's own focus.
            (Action::Focus, target) => {
                let focus = match target {
                    ContextSemanticTarget::Source { source, .. } => {
                        self.context_picker.select_source(request_id, source);
                        ContextPickerFocus::Sources
                    }
                    ContextSemanticTarget::List(_) => ContextPickerFocus::Sources,
                    ContextSemanticTarget::FileInput(_) => ContextPickerFocus::FileInput,
                    ContextSemanticTarget::Button(_, button) => ContextPickerFocus::Button(button),
                };
                if self.context_picker.set_focus(request_id, focus) {
                    self.accessibility.focus_override = Some(request.target_node);
                }
                ContextPickerAction::None
            }
            (Action::Click, ContextSemanticTarget::Source { source, .. }) => {
                ContextPickerAction::Toggle { request_id, source }
            }
            (Action::Click, ContextSemanticTarget::Button(_, button)) => {
                self.context_picker.button_action(request_id, button)
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
        use crate::ui::context_picker::{
            ContextPickerButton, ContextPickerFocus, ContextPickerPhase, ContextSourceId,
        };

        let layout = self.context_picker_layout();
        self.context_picker.set_viewport(layout.visible_count);
        let snapshot = self.context_picker.snapshot();
        let request_id = snapshot.request_id;
        let list_target = ContextSemanticTarget::List(request_id);
        let input_target = ContextSemanticTarget::FileInput(request_id);
        let visible_sources: Vec<_> = snapshot
            .sources
            .iter()
            .skip(snapshot.scroll_offset)
            .take(layout.visible_count)
            .cloned()
            .collect();

        let mut targets: HashSet<_> = snapshot
            .buttons()
            .iter()
            .map(|&button| ContextSemanticTarget::Button(request_id, button))
            .collect();
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
                node.set_description(if source.available {
                    "Toggle this source in the context bundle"
                } else if source.id == ContextSourceId::SpecificFiles {
                    "Choose files first"
                } else {
                    "Unavailable for the current working directory"
                });
                node.set_toggled(if source.selected {
                    Toggled::True
                } else {
                    Toggled::False
                });
                node.set_selected(source_index == snapshot.selected_index);
                node.set_bounds(bounds_rect(layout.row(visible_index)));
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
                ContextPickerPhase::Gathering => "Gathering context in the background".into(),
            });
        let mut status = Node::new(Role::Status);
        status.set_label(status_text);
        status.set_live(Live::Polite);
        nodes.push((CONTEXT_STATUS, status));

        let mut button_ids = Vec::with_capacity(snapshot.buttons().len());
        for &button in snapshot.buttons() {
            let Some(bounds) = layout.button(button) else {
                continue;
            };
            let id = self
                .accessibility
                .context
                .node(&ContextSemanticTarget::Button(request_id, button));
            let mut node = Node::new(if button == ContextPickerButton::Primary {
                Role::DefaultButton
            } else {
                Role::Button
            });
            node.set_label(button.label(snapshot.phase));
            match button {
                ContextPickerButton::ChooseFiles => {
                    node.set_description(if snapshot.is_enabled(button) {
                        "Opens a comma-separated file path editor bounded to the working directory"
                    } else {
                        "Needs a working directory"
                    })
                }
                ContextPickerButton::Cancel => node.set_keyboard_shortcut("Escape"),
                ContextPickerButton::Primary => {}
            }
            node.set_bounds(bounds_rect(bounds));
            if snapshot.awaiting && snapshot.is_enabled(button) {
                node.add_action(Action::Click);
                node.add_action(Action::Focus);
            } else {
                node.set_disabled();
            }
            nodes.push((id, node));
            button_ids.push((button, id));
        }

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
        children.push(CONTEXT_STATUS);
        children.extend(button_ids.iter().map(|(_, id)| *id));
        dialog.set_children(children);
        nodes.push((CONTEXT_DIALOG, dialog));

        // Assistive focus follows the picker's own focus: from the list, to
        // the selected source while it is shown and can be toggled.
        let focus = if snapshot.awaiting {
            match snapshot.focus {
                ContextPickerFocus::Sources => source_nodes
                    .iter()
                    .enumerate()
                    .find(|(visible_index, (source, _))| {
                        source.available
                            && snapshot.scroll_offset + visible_index == snapshot.selected_index
                    })
                    .map(|(_, (_, id))| *id)
                    .or(list_id),
                ContextPickerFocus::FileInput => input_id,
                ContextPickerFocus::Button(focused) => button_ids
                    .iter()
                    .find(|(button, _)| *button == focused)
                    .map(|(_, id)| *id),
            }
            .unwrap_or(CONTEXT_DIALOG)
        } else {
            CONTEXT_DIALOG
        };
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: CONTEXT_DIALOG,
            nodes,
            focus,
        }
    }
}
