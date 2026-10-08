//! Harvest's Root Cellar and sprout menu: their nodes and actions.

use super::*;

impl App {
    pub(super) fn handle_harvest_accessibility_action(&mut self, request: ActionRequest) {
        if let Some((request_id, action)) = self
            .accessibility
            .harvest
            .targets
            .get(&request.target_node)
            .and_then(|target| match target {
                HarvestSemanticTarget::SproutAction { request_id, action } => {
                    Some((*request_id, *action))
                }
                HarvestSemanticTarget::CellarEntry(_) => None,
            })
        {
            let current = self
                .harvest
                .as_ref()
                .and_then(|harvest| harvest.sprout_menu.as_ref())
                .is_some_and(|menu| {
                    menu.request_id == request_id
                        && menu.actions.contains(&action)
                        && menu.action_enabled(action)
                });
            if !current {
                return;
            }
            match request.action {
                Action::Focus => {
                    if let Some(menu) = self
                        .harvest
                        .as_mut()
                        .and_then(|harvest| harvest.sprout_menu.as_mut())
                    {
                        menu.select(action);
                    }
                    self.accessibility.focus_override = Some(request.target_node);
                }
                Action::Click => {
                    self.execute_sprout_request(request_id, action);
                    self.accessibility.focus_override = None;
                }
                _ => {}
            }
            return;
        }

        if self
            .harvest
            .as_ref()
            .is_some_and(|harvest| harvest.sprout_menu.is_some())
        {
            // The context menu is modal. Background and retired actions fail
            // closed instead of falling through to application controls.
            return;
        }

        if request.target_node == CELLAR_CLOSE {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(CELLAR_CLOSE),
                Action::Click => {
                    self.apply_cellar_action(crate::harvest::CellarAction::Close);
                    self.accessibility.focus_override = None;
                }
                _ => {}
            }
            return;
        }
        if request.target_node == CELLAR_LIST {
            match request.action {
                Action::ScrollUp => {
                    if let Some(harvest) = &mut self.harvest {
                        harvest.cellar.select_page(-1);
                    }
                }
                Action::ScrollDown => {
                    if let Some(harvest) = &mut self.harvest {
                        harvest.cellar.select_page(1);
                    }
                }
                _ => {}
            }
            return;
        }
        let Some(id) = self
            .accessibility
            .harvest
            .targets
            .get(&request.target_node)
            .and_then(|target| match target {
                HarvestSemanticTarget::CellarEntry(id) => Some(*id),
                HarvestSemanticTarget::SproutAction { .. } => None,
            })
        else {
            return;
        };
        let visible = self
            .cellar_layout()
            .is_some_and(|layout| layout.rows.iter().any(|(entry_id, _)| *entry_id == id));
        if !visible {
            return;
        }
        match request.action {
            Action::Focus => {
                self.apply_cellar_action(crate::harvest::CellarAction::Select(id));
                self.accessibility.focus_override = Some(request.target_node);
            }
            Action::Click => {
                self.apply_cellar_action(crate::harvest::CellarAction::Activate(id));
                self.accessibility.focus_override = None;
            }
            _ => {}
        }
    }

    pub(super) fn cellar_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self
            .cellar_layout()
            .expect("the Cellar layer is open only while Harvest is on");
        let (entries, selected, total, evicted, scroll, healthy) = self
            .harvest
            .as_ref()
            .map(|harvest| {
                (
                    harvest
                        .cellar
                        .visible_entries()
                        .cloned()
                        .collect::<Vec<_>>(),
                    harvest.cellar.selected_id(),
                    harvest.cellar.len(),
                    harvest.cellar.evicted_commands(),
                    harvest.cellar.scroll,
                    (harvest.cellar.success_rate() * 100.0).round() as usize,
                )
            })
            .expect("the Cellar layer is open only while Harvest is on");
        let targets = entries
            .iter()
            .map(|entry| HarvestSemanticTarget::CellarEntry(entry.id))
            .collect::<HashSet<_>>();
        self.accessibility
            .harvest
            .retain(|target| targets.contains(target));
        let entry_nodes = entries
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    self.accessibility
                        .harvest
                        .node(&HarvestSemanticTarget::CellarEntry(entry.id)),
                )
            })
            .collect::<Vec<_>>();

        let mut nodes = Vec::with_capacity(entries.len() + 7);

        let mut title = Node::new(Role::Heading);
        title.set_label("Root Cellar");
        nodes.push((CELLAR_TITLE, title));

        let mut status = Node::new(Role::Status);
        status.set_label(if total == 0 {
            "No harvested commands".to_string()
        } else if evicted == 0 {
            format!("{total} retained harvested commands, {healthy}% healthy")
        } else {
            format!(
                "{total} retained harvested commands, {evicted} older commands evicted, {healthy}% healthy"
            )
        });
        status.set_live(Live::Polite);
        status.set_bounds(bounds_rect(layout.summary));
        nodes.push((CELLAR_STATUS, status));

        for (entry, (id, node_id)) in entries.iter().zip(&entry_nodes) {
            let mut node = Node::new(Role::ListBoxOption);
            node.set_label(entry.command.clone());
            let result = match entry.exit_code {
                Some(0) => "successful".to_string(),
                Some(code) => format!("failed with exit code {code}"),
                None => "still running".to_string(),
            };
            let owner = format!(
                "tab {}, pane {}",
                entry.source.tab_id,
                u64::from(entry.source.pane_id) + 1,
            );
            let retention = if entry.output_truncated {
                "; only the last 64 KiB of output is retained"
            } else {
                ""
            };
            node.set_description(format!(
                "{result}; {owner}; terminal line {}{retention}. Activate for command actions",
                entry.row
            ));
            node.set_selected(selected == Some(*id));
            node.add_action(Action::Focus);
            node.add_action(Action::Click);
            if let Some((_, row)) = layout.rows.iter().find(|(row_id, _)| row_id == id) {
                node.set_bounds(bounds_rect(*row));
            }
            nodes.push((*node_id, node));
        }

        let mut list = Node::new(Role::ListBox);
        list.set_label("Harvested command history");
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_children(
            entry_nodes
                .iter()
                .map(|(_, node)| *node)
                .collect::<Vec<_>>(),
        );
        list.set_bounds(bounds_rect(layout.list));
        if scroll > 0 {
            list.add_action(Action::ScrollUp);
        }
        if scroll.saturating_add(layout.max_visible) < total {
            list.add_action(Action::ScrollDown);
        }
        nodes.push((CELLAR_LIST, list));

        let mut close = Node::new(Role::Button);
        close.set_label("Close Root Cellar");
        close.set_bounds(bounds_rect(layout.close));
        close.add_action(Action::Focus);
        close.add_action(Action::Click);
        nodes.push((CELLAR_CLOSE, close));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Root Cellar command history");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children([CELLAR_TITLE, CELLAR_STATUS, CELLAR_LIST, CELLAR_CLOSE]);
        nodes.push((CELLAR_DIALOG, dialog));

        let default_focus = selected
            .and_then(|selected| {
                entry_nodes
                    .iter()
                    .find(|(id, _)| *id == selected)
                    .map(|(_, node)| *node)
            })
            .unwrap_or(CELLAR_CLOSE);
        let focus = self
            .accessibility
            .focus_override
            .filter(|focus| *focus == CELLAR_CLOSE || *focus == default_focus)
            .unwrap_or(default_focus);
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: CELLAR_DIALOG,
            nodes,
            focus,
        }
    }

    /// The sprout menu's nodes, placed where the menu is drawn.
    pub(super) fn sprout_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self
            .sprout_menu_layout()
            .expect("the sprout menu layer is open only while its menu is");
        let (request_id, actions, selected, command) = self
            .harvest
            .as_ref()
            .and_then(|harvest| harvest.sprout_menu.as_ref())
            .map(|menu| {
                (
                    menu.request_id,
                    menu.actions
                        .iter()
                        .map(|action| {
                            (
                                *action,
                                menu.action_label(*action),
                                menu.action_enabled(*action),
                                menu.action_description(*action),
                            )
                        })
                        .collect::<Vec<_>>(),
                    menu.selected,
                    menu.command
                        .as_ref()
                        .map(|entry| entry.command.clone())
                        .unwrap_or_else(|| "selected command".to_string()),
                )
            })
            .unwrap();
        let targets = actions
            .iter()
            .map(|(action, _, _, _)| HarvestSemanticTarget::SproutAction {
                request_id,
                action: *action,
            })
            .collect::<HashSet<_>>();
        self.accessibility
            .harvest
            .retain(|target| targets.contains(target));
        let action_nodes = actions
            .iter()
            .map(|(action, _, _, _)| {
                self.accessibility
                    .harvest
                    .node(&HarvestSemanticTarget::SproutAction {
                        request_id,
                        action: *action,
                    })
            })
            .collect::<Vec<_>>();

        let mut nodes = Vec::with_capacity(actions.len() + 4);

        for (index, (((_action, label, enabled, description), node_id), row)) in actions
            .iter()
            .zip(action_nodes.iter().copied())
            .zip(&layout.rows)
            .enumerate()
        {
            let mut node = Node::new(Role::MenuItem);
            node.set_label(label.as_str());
            if let Some(description) = description {
                node.set_description(description.as_str());
            }
            node.set_selected(index == selected);
            node.set_bounds(bounds_rect(*row));
            if *enabled {
                node.add_action(Action::Focus);
                node.add_action(Action::Click);
            } else {
                node.set_disabled();
            }
            nodes.push((node_id, node));
        }

        let mut menu = Node::new(Role::Menu);
        menu.set_label(format!("Actions for {command}"));
        menu.set_orientation(accesskit::Orientation::Vertical);
        menu.set_bounds(bounds_rect(layout.panel));
        menu.set_children(action_nodes.clone());
        nodes.push((SPROUT_MENU, menu));

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(format!("Harvest actions for {command}"));
        dialog.set_description("Press Escape to close without running an action");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children([SPROUT_MENU]);
        nodes.push((SPROUT_DIALOG, dialog));

        let default_focus = action_nodes
            .get(selected.min(action_nodes.len().saturating_sub(1)))
            .copied()
            .unwrap_or(SPROUT_MENU);
        let focus = self
            .accessibility
            .focus_override
            .filter(|focus| *focus == default_focus)
            .unwrap_or(default_focus);
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: SPROUT_DIALOG,
            nodes,
            focus,
        }
    }
}
