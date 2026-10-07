//! Surfaces added beside the ordinary tree: the docked agent view and toasts.

use super::*;

impl App {
    /// Adds the docked agent view beside the terminals. Modal surfaces
    /// replace the tree, so the panel joins only the ordinary one.
    pub(super) fn with_agent_view_accessibility(
        &mut self,
        mut snapshot: SemanticSnapshot,
    ) -> SemanticSnapshot {
        use crate::ui::agent_view::scene::{HitRole, Node as SceneNode, Target};

        let ordinary = snapshot
            .nodes
            .iter()
            .any(|(id, node)| *id == ROOT && node.children().contains(&TAB_LIST));
        let Some(scene) = self
            .agent_view
            .scene
            .clone()
            .filter(|_| ordinary && self.agent_view.state.is_open())
        else {
            return snapshot;
        };
        let shown: HashSet<&Target> = scene.hits.iter().map(|hit| &hit.target).collect();
        self.accessibility
            .agent_view
            .retain(|target| shown.contains(target));
        let composer = self
            .agent_view
            .state
            .composer()
            .map(|composer| composer.text().to_owned())
            .unwrap_or_default();
        let bounds = bounds_rect;
        let mut children = Vec::with_capacity(scene.nodes.len());
        let mut text_index = 0;
        for node in &scene.nodes {
            match node {
                SceneNode::Text(text) => {
                    let id = NodeId(AGENT_VIEW_TEXT_BASE + text_index);
                    text_index += 1;
                    let mut label = Node::new(if text.heading {
                        Role::Heading
                    } else {
                        Role::Label
                    });
                    label.set_label(text.text.clone());
                    let visible = match text.clip {
                        Some(clip) => text.bounds.intersect(clip),
                        None => Some(text.bounds),
                    };
                    if let Some(visible) = visible {
                        label.set_bounds(bounds(visible));
                    }
                    snapshot.nodes.push((id, label));
                    children.push(id);
                }
                SceneNode::Hit(index) => {
                    let hit = &scene.hits[*index];
                    if hit.target == Target::DismissMenu {
                        continue;
                    }
                    let id = self.accessibility.agent_view.node(&hit.target);
                    let mut control = match hit.role {
                        HitRole::Button => Node::new(Role::Button),
                        HitRole::Toggle { expanded } => {
                            let mut node = Node::new(Role::Button);
                            node.set_expanded(expanded);
                            node
                        }
                        HitRole::Option { selected } => {
                            let mut node = Node::new(Role::RadioButton);
                            node.set_toggled(accesskit::Toggled::from(selected));
                            node
                        }
                        HitRole::TextInput => {
                            let mut node = Node::new(Role::MultilineTextInput);
                            node.set_value(composer.clone());
                            node
                        }
                        HitRole::ListItem => Node::new(Role::ListItem),
                        HitRole::MenuItem { checked } => {
                            let mut node = Node::new(Role::MenuItemCheckBox);
                            node.set_toggled(accesskit::Toggled::from(checked));
                            node
                        }
                    };
                    control.set_label(hit.label.clone());
                    control.add_action(Action::Click);
                    control.add_action(Action::Focus);
                    if let Some(visible) = hit.visible_bounds() {
                        control.set_bounds(bounds(visible));
                    }
                    snapshot.nodes.push((id, control));
                    children.push(id);
                }
            }
        }
        let mut pane = Node::new(Role::Pane);
        pane.set_label("Agent view");
        pane.set_bounds(bounds(scene.panel));
        pane.set_children(children);
        snapshot.nodes.push((AGENT_VIEW_PANE, pane));
        if let Some((_, root)) = snapshot.nodes.iter_mut().find(|(id, _)| *id == ROOT) {
            let mut children = root.children().to_vec();
            children.push(AGENT_VIEW_PANE);
            root.set_children(children);
        }
        if self.agent_view.state.has_keyboard() {
            let focused = match &self.agent_view.state.focus {
                crate::ui::agent_view::Focus::Composer => Target::Composer,
                crate::ui::agent_view::Focus::Target(target) => target.clone(),
            };
            if let Some(node) = self.accessibility.agent_view.nodes.get(&focused) {
                snapshot.focus = *node;
            }
        }
        snapshot
    }

    /// Clicks and focus requests on the agent view. Returns whether the
    /// node was the panel's.
    pub(super) fn handle_agent_view_accessibility_action(
        &mut self,
        request: &ActionRequest,
    ) -> bool {
        use crate::ui::agent_view::scene::Target;
        let Some(target) = self
            .accessibility
            .agent_view
            .targets
            .get(&request.target_node)
            .cloned()
        else {
            return request.target_node == AGENT_VIEW_PANE;
        };
        let focus = if target == Target::Composer {
            crate::ui::agent_view::Focus::Composer
        } else {
            crate::ui::agent_view::Focus::Target(target.clone())
        };
        match request.action {
            Action::Click => {
                self.set_agent_view_keyboard(true);
                self.agent_view.state.focus = focus;
                self.activate_agent_view_target(target);
            }
            Action::Focus => {
                self.set_agent_view_keyboard(true);
                self.agent_view.state.focus = focus;
                self.agent_view.state.focus_visible = true;
                self.reveal_agent_view_target(&target);
            }
            _ => {}
        }
        true
    }

    pub(super) fn with_toast_accessibility(
        &self,
        mut snapshot: SemanticSnapshot,
    ) -> SemanticSnapshot {
        let toasts = self.toasts.build_accessibility_info();
        if toasts.is_empty() {
            return snapshot;
        }
        // Toasts sit in the terminal area's corner, clear of the dock.
        let UiMetrics {
            height: window_height,
            cell_width,
            cell_height,
            ..
        } = self.ui_metrics();
        let window_width = self.pane_placement().right;
        let placements = crate::ui::toast::toast_layout(
            &toasts,
            window_width,
            window_height,
            cell_width,
            cell_height,
        );
        let mut toast_ids = Vec::with_capacity(placements.len());
        for placement in placements {
            let toast = &toasts[placement.toast_index];
            let id = toast_node(toast.id);
            let severity = match toast.severity {
                crate::ui::toast::ToastSeverity::Info => "Information",
                crate::ui::toast::ToastSeverity::Success => "Success",
                crate::ui::toast::ToastSeverity::Warning => "Warning",
                crate::ui::toast::ToastSeverity::Error => "Error",
            };
            let mut node = Node::new(Role::Status);
            node.set_label(format!("{severity}: {}", toast.message));
            node.set_live(
                if toast.severity == crate::ui::toast::ToastSeverity::Error {
                    Live::Assertive
                } else {
                    Live::Polite
                },
            );
            node.add_action(Action::Click);
            node.add_action(Action::CustomAction);
            node.push_custom_action(CustomAction {
                id: TOAST_DISMISS_ACTION,
                description: "Dismiss".into(),
            });
            node.set_bounds(bounds_rect(placement.rect));
            snapshot.nodes.push((id, node));
            toast_ids.push(id);
        }
        if let Some((_, root)) = snapshot.nodes.iter_mut().find(|(id, _)| *id == ROOT) {
            let mut children = root.children().to_vec();
            children.extend(toast_ids);
            root.set_children(children);
        }
        snapshot
    }

    /// Clicking a toast or its Dismiss action dismisses it, as a mouse press
    /// on it does, so a persistent notice never needs the mouse.
    pub(super) fn handle_toast_accessibility_action(
        &mut self,
        toast: u64,
        request: &ActionRequest,
    ) {
        let dismiss = match request.action {
            Action::Click => true,
            Action::CustomAction => matches!(
                request.data,
                Some(ActionData::CustomAction(TOAST_DISMISS_ACTION))
            ),
            _ => false,
        };
        if dismiss && self.toasts.dismiss(toast) {
            self.request_redraw(RedrawReason::Accessibility);
        }
    }
}
