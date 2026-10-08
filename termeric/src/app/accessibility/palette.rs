//! The palette-style modals (command palette, domain picker, Unicode
//! picker): their nodes, wording, and actions.

use super::*;

impl App {
    /// The query takes text and focus; a visible entry takes focus or a
    /// click. The modal hides everything behind it, so a stale request for
    /// anything else is dropped.
    pub(super) fn handle_palette_modal_accessibility_action(
        &mut self,
        modal: PaletteModal,
        request: ActionRequest,
    ) {
        let ids = palette_modal_node_ids(modal);
        if request.target_node == ids.input {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(ids.input),
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.palette_owner_mut(modal).set_query(value) {
                        self.ime.clear_preedit();
                        self.accessibility.focus_override = Some(ids.input);
                    }
                }
                _ => {}
            }
            return;
        }
        if !matches!(request.action, Action::Click | Action::Focus) {
            return;
        }
        let Some(command) = self
            .accessibility
            .palette_modal(modal)
            .targets
            .get(&request.target_node)
            .cloned()
        else {
            return;
        };
        let info = self.palette_owner(modal).build_info();
        let layout = self.palette_layout(&info);
        if !info
            .entries
            .iter()
            .take(layout.visible_count)
            .any(|entry| entry.id == command)
            || !self.palette_entry_actionable(modal, &command)
        {
            return;
        }
        if request.action == Action::Focus {
            if self
                .palette_owner_mut(modal)
                .palette_mut()
                .select_command(&command)
            {
                self.accessibility.focus_override = Some(ids.input);
            }
        } else if let Some(choice) = self.execute_palette_modal_command(modal, &command) {
            // A staged picker moves on with its query still focused; the
            // others close.
            self.accessibility.focus_override =
                (modal == PaletteModal::Domains).then_some(ids.input);
            self.finish_palette_input(Some(choice));
        }
    }

    pub(super) fn palette_modal_accessibility_snapshot(
        &mut self,
        modal: PaletteModal,
    ) -> ModalSnapshot {
        let ids = palette_modal_node_ids(modal);
        let rows = self.list_dialog_rows(crate::ui::command_palette::PALETTE_DIALOG);
        self.palette_owner_mut(modal)
            .palette_mut()
            .set_max_visible(rows);
        let info = self.palette_owner(modal).build_info();
        let layout = self.palette_layout(&info);
        let words = self.palette_modal_wording(modal, &info);
        let editable = self.palette_owner(modal).query_editable();

        let registered = self
            .palette_owner(modal)
            .palette()
            .command_ids()
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        self.accessibility
            .palette_modal_mut(modal)
            .retain(|id| registered.contains(id));
        let visible_entries = info
            .entries
            .iter()
            .take(layout.visible_count)
            .cloned()
            .collect::<Vec<_>>();
        let entry_nodes = visible_entries
            .iter()
            .map(|entry| self.accessibility.palette_modal_mut(modal).node(&entry.id))
            .collect::<Vec<_>>();
        let actionable = visible_entries
            .iter()
            .map(|entry| self.palette_entry_actionable(modal, &entry.id))
            .collect::<Vec<_>>();
        let selected_action = entry_nodes
            .get(info.selected)
            .filter(|_| actionable[info.selected])
            .copied();

        let mut nodes = Vec::with_capacity(entry_nodes.len() + 5);

        let mut input = Node::new(Role::SearchInput);
        input.set_label(words.input);
        input.set_placeholder(info.placeholder.clone());
        set_text_value(&mut input, ids.input, &info.input, Some(info.cursor));
        input.set_bounds(bounds_rect(layout.input));
        input.set_controls([ids.list]);
        input.add_action(Action::Focus);
        if editable {
            input.add_action(Action::SetValue);
        } else {
            input.set_read_only();
        }
        if let Some(selected) = selected_action {
            input.set_active_descendant(selected);
        }
        nodes.push((ids.input, input));

        for (row, ((entry, node_id), actionable)) in visible_entries
            .iter()
            .zip(entry_nodes.iter().copied())
            .zip(actionable.iter().copied())
            .enumerate()
        {
            let mut node = Node::new(if actionable {
                Role::ListBoxOption
            } else {
                Role::Status
            });
            node.set_label(entry.title.clone());
            node.set_description(if modal == PaletteModal::Commands {
                let mut description = format!("{} {}", entry.category, info.item_noun);
                if entry.destructive {
                    description.push_str(", destructive");
                }
                description
            } else {
                entry.category.clone()
            });
            if let Some(shortcut) = &entry.shortcut {
                node.set_keyboard_shortcut(shortcut.clone());
            }
            node.set_bounds(bounds_rect(layout.row(row)));
            if actionable {
                node.set_selected(row == info.selected);
                node.add_action(Action::Click);
                node.add_action(Action::Focus);
            } else {
                node.set_live(Live::Polite);
            }
            nodes.push((node_id, node));
        }

        let mut list = Node::new(if actionable.contains(&true) {
            Role::ListBox
        } else {
            Role::List
        });
        list.set_label(words.list);
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(bounds_rect(layout.list));
        if let Some(selected) = selected_action {
            list.set_active_descendant(selected);
        }
        list.set_children(entry_nodes);
        nodes.push((ids.list, list));

        let mut dialog_children = vec![ids.input, ids.list];
        if visible_entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label(words.empty);
            empty.set_live(Live::Polite);
            nodes.push((ids.empty, empty));
            dialog_children.push(ids.empty);
        }
        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(words.dialog);
        if let Some(description) = words.description {
            dialog.set_description(description);
        }
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((ids.dialog, dialog));

        self.accessibility.focus_override = Some(ids.input);
        ModalSnapshot {
            dialog: ids.dialog,
            nodes,
            focus: ids.input,
        }
    }

    /// Whether choosing an entry does anything. Only a live-domain picker
    /// lists status lines among its choices.
    fn palette_entry_actionable(&self, modal: PaletteModal, command: &str) -> bool {
        modal != PaletteModal::Domains || self.domain_picker.is_actionable(command)
    }

    fn palette_modal_wording(
        &self,
        modal: PaletteModal,
        info: &crate::ui::command_palette::PaletteInfo,
    ) -> PaletteWording {
        match modal {
            PaletteModal::Commands => PaletteWording {
                dialog: info.dialog_label.to_owned(),
                description: None,
                input: "Search commands",
                list: format!("{} results", info.item_noun),
                empty: format!("No matching {}s", info.item_noun),
            },
            PaletteModal::Domains => PaletteWording {
                dialog: "Attach live domain".into(),
                description: Some(if self.domain_picker.is_busy() {
                    "Waiting for authoritative domain state; Escape returns to hosts"
                } else if !self.domain_picker.can_filter() {
                    "Domain discovery status; Escape returns to hosts"
                } else {
                    "Choose a local or saved SSH host, then choose a live domain"
                }),
                input: "Search live-domain hosts or domains",
                list: format!("Live-domain {} results", info.item_noun),
                empty: "No matching live-domain results".into(),
            },
            PaletteModal::Unicode => PaletteWording {
                dialog: "Unicode character picker".into(),
                description: Some(
                    "Search common characters or enter U+ followed by a non-control Unicode scalar",
                ),
                input: "Search Unicode characters or enter a code point",
                list: "Unicode character results".into(),
                empty: info
                    .status
                    .unwrap_or("No matching Unicode character")
                    .into(),
            },
        }
    }
}

/// The fixed nodes of a palette-style modal's dialog.
struct PaletteNodeIds {
    dialog: NodeId,
    input: NodeId,
    list: NodeId,
    empty: NodeId,
}

fn palette_modal_node_ids(modal: PaletteModal) -> PaletteNodeIds {
    match modal {
        PaletteModal::Commands => PaletteNodeIds {
            dialog: PALETTE_DIALOG,
            input: PALETTE_INPUT,
            list: PALETTE_LIST,
            empty: PALETTE_EMPTY,
        },
        PaletteModal::Domains => PaletteNodeIds {
            dialog: DOMAIN_DIALOG,
            input: DOMAIN_INPUT,
            list: DOMAIN_LIST,
            empty: DOMAIN_EMPTY,
        },
        PaletteModal::Unicode => PaletteNodeIds {
            dialog: UNICODE_DIALOG,
            input: UNICODE_INPUT,
            list: UNICODE_LIST,
            empty: UNICODE_EMPTY,
        },
    }
}

/// What assistive technology hears a palette-style modal called.
struct PaletteWording {
    dialog: String,
    description: Option<&'static str>,
    input: &'static str,
    list: String,
    empty: String,
}
