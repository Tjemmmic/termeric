//! Settings, the shortcuts panel, the Commands, session, and Agents panels,
//! and the Workspaces menu: each surface's nodes and its actions.

use super::*;

impl App {
    pub(super) fn handle_settings_accessibility_action(&mut self, request: ActionRequest) {
        if request.target_node == SETTINGS_INPUT {
            // A retained node must not edit or take focus from a live dropdown.
            if self.settings_panel.is_dropdown_open() {
                return;
            }
            match request.action {
                Action::Focus => {
                    self.accessibility.focus_override = Some(SETTINGS_INPUT);
                }
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.settings_panel.accessibility_set_filter(value) {
                        self.ime.clear_preedit();
                        self.accessibility.focus_override = Some(SETTINGS_INPUT);
                    }
                }
                _ => {}
            }
            return;
        }
        if matches!(
            request.target_node,
            SETTINGS_CLOSE | SETTINGS_SAVE | SETTINGS_BACK
        ) {
            match request.action {
                Action::Focus => self.accessibility.focus_override = Some(request.target_node),
                Action::Click if request.target_node == SETTINGS_CLOSE => {
                    self.settings_panel.close();
                    self.accessibility.focus_override = None;
                }
                Action::Click if request.target_node == SETTINGS_BACK => {
                    // Only the way back that is shown answers.
                    if self.settings_panel.shows_back(&self.config) {
                        self.settings_panel.leave_sub_page();
                        self.accessibility.focus_override = None;
                    }
                }
                Action::Click => {
                    self.apply_settings_action(crate::ui::settings::SettingsAction::SaveConfig);
                }
                _ => {}
            }
            return;
        }
        let Some(target) = self
            .accessibility
            .settings
            .targets
            .get(&request.target_node)
            .cloned()
        else {
            // Settings is modal. Stale requests for controls behind it must
            // never escape into the normal application action path.
            return;
        };

        use crate::ui::settings::SettingsAction;
        match target {
            SettingsSemanticTarget::Page(page) => match request.action {
                Action::Click => {
                    self.settings_panel.select_page(page);
                    self.accessibility.focus_override = Some(request.target_node);
                }
                Action::Focus => self.accessibility.focus_override = Some(request.target_node),
                _ => {}
            },
            SettingsSemanticTarget::Header(_) => {}
            SettingsSemanticTarget::Setting { section, label } => match request.action {
                Action::Focus => {
                    if self.settings_panel.accessibility_focus_setting(
                        section,
                        &label,
                        &self.config,
                        &self.shortcut_registry,
                    ) {
                        self.accessibility.focus_override = Some(request.target_node);
                    }
                }
                Action::Increment | Action::Decrement => {
                    let direction = if request.action == Action::Increment {
                        1
                    } else {
                        -1
                    };
                    let action = self.settings_panel.accessibility_adjust_setting(
                        section,
                        &label,
                        direction,
                        &self.config,
                        &self.shortcut_registry,
                    );
                    self.accessibility.focus_override = Some(request.target_node);
                    self.apply_settings_action(action);
                }
                Action::Click => {
                    let action = self.settings_panel.accessibility_activate_setting(
                        section,
                        &label,
                        &self.config,
                        &self.shortcut_registry,
                    );
                    if matches!(action, SettingsAction::None) {
                        if self.settings_panel.accessibility_set_dropdown_expanded(
                            section,
                            &label,
                            true,
                            &self.config,
                            &self.shortcut_registry,
                        ) {
                            self.accessibility.focus_override = Some(request.target_node);
                        }
                    } else {
                        self.apply_settings_action(action);
                    }
                }
                Action::Expand => {
                    if self.settings_panel.accessibility_set_dropdown_expanded(
                        section,
                        &label,
                        true,
                        &self.config,
                        &self.shortcut_registry,
                    ) {
                        self.accessibility.focus_override = Some(request.target_node);
                    }
                }
                Action::Collapse => {
                    self.settings_panel.accessibility_set_dropdown_expanded(
                        section,
                        &label,
                        false,
                        &self.config,
                        &self.shortcut_registry,
                    );
                    self.accessibility.focus_override = Some(request.target_node);
                }
                _ => {}
            },
            SettingsSemanticTarget::Switch { section, label } => match request.action {
                Action::Focus => {
                    if self.settings_panel.accessibility_focus_setting(
                        section,
                        &label,
                        &self.config,
                        &self.shortcut_registry,
                    ) {
                        self.accessibility.focus_override = Some(request.target_node);
                    }
                }
                Action::Click => {
                    let action = self.settings_panel.accessibility_flip_switch(
                        section,
                        &label,
                        &self.config,
                        &self.shortcut_registry,
                    );
                    self.accessibility.focus_override = Some(request.target_node);
                    self.apply_settings_action(action);
                }
                _ => {}
            },
            SettingsSemanticTarget::Option {
                section,
                label,
                value,
            } => {
                if request.action != Action::Click {
                    return;
                }
                let action = self.settings_panel.accessibility_choose_dropdown_option(
                    section,
                    &label,
                    &value,
                    &self.config,
                    &self.shortcut_registry,
                );
                if !matches!(action, SettingsAction::None) {
                    self.apply_settings_action(action);
                }
                if let Some(node) = self
                    .accessibility
                    .settings
                    .nodes
                    .get(&SettingsSemanticTarget::Setting { section, label })
                {
                    self.accessibility.focus_override = Some(*node);
                }
            }
        }
    }

    pub(super) fn settings_accessibility_snapshot(&mut self) -> ModalSnapshot {
        use crate::ui::settings::SettingKind;

        let layout = self.settings_layout();
        self.settings_panel.set_max_visible(layout.rows_that_fit);
        let mut info = self
            .settings_panel
            .build_info(&self.config, &self.shortcut_registry);
        info.unsaved = self.config_has_unsaved_changes();
        let end = (info.scroll_offset + layout.rows_that_fit).min(info.entries.len());
        let visible_entries = &info.entries[info.scroll_offset..end];

        let mut nodes = Vec::with_capacity(visible_entries.len() + 16);

        let mut input = Node::new(Role::SearchInput);
        input.set_label("Search settings");
        input.set_placeholder("Search settings");
        set_text_value(
            &mut input,
            SETTINGS_INPUT,
            &info.filter,
            Some(info.filter_cursor),
        );
        input.set_bounds(bounds_rect(layout.input));
        input.set_controls([SETTINGS_LIST]);
        if info.dropdown_open {
            input.set_read_only();
        } else {
            input.add_action(Action::Focus);
            input.add_action(Action::SetValue);
        }

        let selectables_before_offset = info.entries[..info.scroll_offset]
            .iter()
            .filter(|entry| !entry.is_header)
            .count();
        let mut selectable_index = selectables_before_offset;
        let mut list_children = Vec::with_capacity(visible_entries.len());
        let mut selected_setting = None;
        let mut open_dropdown = None;

        for (row, entry) in visible_entries.iter().enumerate() {
            let target = if entry.is_header {
                SettingsSemanticTarget::Header(entry.section)
            } else {
                SettingsSemanticTarget::Setting {
                    section: entry.section,
                    label: entry.label.to_string(),
                }
            };
            let node_id = self.accessibility.settings.node(&target);
            list_children.push(node_id);
            let row_rect = layout.row_rect(row);
            let bounds = bounds_rect(row_rect);

            if entry.is_header {
                let mut node = Node::new(Role::Heading);
                node.set_label(entry.label.as_ref());
                node.set_bounds(bounds);
                nodes.push((node_id, node));
                continue;
            }

            let is_selected = selectable_index == info.selected;
            if is_selected {
                selected_setting = Some(node_id);
            }
            let mut node = match &entry.kind {
                SettingKind::Numeric {
                    value,
                    min,
                    max,
                    step,
                    ..
                } => {
                    let mut node = Node::new(Role::SpinButton);
                    node.set_label(entry.label.as_ref());
                    node.set_description(format!("{} setting", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.set_numeric_value(f64::from(*value));
                    node.set_min_numeric_value(f64::from(*min));
                    node.set_max_numeric_value(f64::from(*max));
                    node.set_numeric_value_step(f64::from(*step));
                    node.add_action(Action::Increment);
                    node.add_action(Action::Decrement);
                    node.add_action(Action::Focus);
                    node
                }
                SettingKind::Dropdown { options, .. } => {
                    let mut node = Node::new(Role::ComboBox);
                    node.set_label(entry.label.as_ref());
                    node.set_description(format!("{} setting", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.set_expanded(is_selected && info.dropdown_open);
                    node.add_action(Action::Click);
                    node.add_action(Action::Expand);
                    node.add_action(Action::Collapse);
                    node.add_action(Action::Focus);
                    if is_selected && info.dropdown_open {
                        node.set_controls([SETTINGS_DROPDOWN]);
                        open_dropdown = Some((entry, options.clone()));
                    }
                    node
                }
                SettingKind::SteppedList { values, .. } => {
                    let mut node = Node::new(Role::ComboBox);
                    node.set_label(entry.label.as_ref());
                    node.set_description(format!("{} setting", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.set_expanded(is_selected && info.dropdown_open);
                    node.add_action(Action::Click);
                    node.add_action(Action::Expand);
                    node.add_action(Action::Collapse);
                    node.add_action(Action::Focus);
                    if is_selected && info.dropdown_open {
                        node.set_controls([SETTINGS_DROPDOWN]);
                        open_dropdown =
                            Some((entry, values.iter().map(ToString::to_string).collect()));
                    }
                    node
                }
                SettingKind::Action { .. } => {
                    let mut node = Node::new(Role::Button);
                    node.set_label(entry.label.as_ref());
                    node.set_description(format!("{} action", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                    node
                }
                SettingKind::Button { .. } => {
                    let mut node = Node::new(Role::Button);
                    node.set_label(entry.label.as_ref());
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                    node
                }
                SettingKind::SwitchedAction { on, .. } => {
                    let switch_id =
                        self.accessibility
                            .settings
                            .node(&SettingsSemanticTarget::Switch {
                                section: entry.section,
                                label: entry.label.to_string(),
                            });
                    let mut switch = Node::new(Role::Switch);
                    switch.set_label(format!("Turn on {}", entry.label));
                    switch.set_toggled(if *on {
                        accesskit::Toggled::True
                    } else {
                        accesskit::Toggled::False
                    });
                    switch.set_bounds(bounds_rect(layout.row_switch(row)));
                    switch.add_action(Action::Click);
                    switch.add_action(Action::Focus);
                    nodes.push((switch_id, switch));
                    let mut node = Node::new(Role::Button);
                    node.set_label(entry.label.as_ref());
                    node.set_description(format!("{} action", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                    node.set_children([switch_id]);
                    node
                }
                SettingKind::ReadOnly => {
                    let mut node = Node::new(Role::Label);
                    node.set_label(entry.label.as_ref());
                    node.set_description(format!("{} setting", entry.section.label()));
                    node.set_value(entry.value.clone());
                    node.set_read_only();
                    node.add_action(Action::Focus);
                    node
                }
            };
            // A button row is its button, as it is drawn and hit.
            node.set_bounds(match entry.kind {
                SettingKind::Button { .. } => bounds_rect(layout.row_button(row)),
                _ => bounds,
            });
            nodes.push((node_id, node));
            selectable_index += 1;
        }

        if let Some(selected) = selected_setting {
            input.set_active_descendant(selected);
        }
        nodes.push((SETTINGS_INPUT, input));

        let mut list = Node::new(Role::List);
        list.set_label(if !info.filter.is_empty() {
            "Search results".to_owned()
        } else if let Some(title) = &info.sub_page_title {
            title.clone()
        } else {
            info.page.label().to_owned()
        });
        list.set_orientation(accesskit::Orientation::Vertical);
        let first_row = layout.row_rect(0);
        list.set_bounds(rect(
            first_row.x,
            layout.list_y,
            first_row.width,
            visible_entries.len() as f32 * layout.row_height,
        ));
        list.set_children(list_children);
        nodes.push((SETTINGS_LIST, list));

        // Sidebar pages are tabs selecting what the list shows.
        let mut page_ids = Vec::new();
        for (index, page) in crate::ui::settings::SettingsPage::all().iter().enumerate() {
            let id = self
                .accessibility
                .settings
                .node(&SettingsSemanticTarget::Page(*page));
            let bounds = layout.page_rect(index);
            let mut tab = Node::new(Role::Tab);
            tab.set_label(page.label());
            tab.set_selected(info.filter.is_empty() && *page == info.page);
            tab.set_controls([SETTINGS_LIST]);
            tab.set_bounds(bounds_rect(bounds));
            tab.add_action(Action::Click);
            tab.add_action(Action::Focus);
            nodes.push((id, tab));
            page_ids.push(id);
        }
        let mut pages = Node::new(Role::TabList);
        pages.set_label("Settings pages");
        pages.set_orientation(accesskit::Orientation::Vertical);
        pages.set_bounds(bounds_rect(layout.sidebar));
        pages.set_children(page_ids);
        nodes.push((SETTINGS_PAGES, pages));

        for (id, label, description, bounds) in [
            (SETTINGS_CLOSE, "Close settings", None, layout.close_button),
            (
                SETTINGS_SAVE,
                "Save settings",
                Some(if info.unsaved {
                    "There are unsaved changes"
                } else {
                    "Settings match the saved file"
                }),
                layout.save_button,
            ),
        ] {
            let mut button = Node::new(Role::Button);
            button.set_label(label);
            if let Some(description) = description {
                button.set_description(description);
            }
            button.set_bounds(bounds_rect(bounds));
            button.add_action(Action::Click);
            button.add_action(Action::Focus);
            nodes.push((id, button));
        }

        let mut dialog_children = vec![SETTINGS_INPUT, SETTINGS_CLOSE, SETTINGS_PAGES];
        if info.sub_page_title.is_some() {
            let mut back = Node::new(Role::Button);
            back.set_label(format!("Back to {}", info.page.label()));
            back.set_keyboard_shortcut("Escape");
            back.set_bounds(bounds_rect(layout.back_button));
            back.add_action(Action::Click);
            back.add_action(Action::Focus);
            nodes.push((SETTINGS_BACK, back));
            dialog_children.push(SETTINGS_BACK);
        }
        dialog_children.extend([SETTINGS_LIST, SETTINGS_SAVE]);
        if info.entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No matching settings");
            empty.set_live(Live::Polite);
            nodes.push((SETTINGS_EMPTY, empty));
            dialog_children.push(SETTINGS_EMPTY);
        }

        if let Some((entry, options)) = open_dropdown {
            if let Some(dropdown_rect) = self
                .settings_panel
                .dropdown_layout(&layout, &info.entries)
                .map(|dropdown| dropdown.rect)
            {
                let mut option_nodes = Vec::new();
                for (visible_index, (index, value)) in options
                    .iter()
                    .enumerate()
                    .skip(dropdown_rect.first_option)
                    .take(dropdown_rect.visible_options)
                    .enumerate()
                {
                    let option_id =
                        self.accessibility
                            .settings
                            .node(&SettingsSemanticTarget::Option {
                                section: entry.section,
                                label: entry.label.to_string(),
                                value: value.clone(),
                            });
                    let mut option = Node::new(Role::ListBoxOption);
                    option.set_label(value.clone());
                    option.set_selected(index == info.dropdown_highlight);
                    option.add_action(Action::Click);
                    option.set_bounds(bounds_rect(dropdown_rect.option_bounds(visible_index)));
                    nodes.push((option_id, option));
                    option_nodes.push(option_id);
                }
                let mut dropdown = Node::new(Role::ListBox);
                dropdown.set_label(format!("{} options", entry.label));
                dropdown.set_orientation(accesskit::Orientation::Vertical);
                dropdown.set_bounds(bounds_rect(dropdown_rect.bounds));
                if let Some(highlighted) = option_nodes
                    .get(
                        info.dropdown_highlight
                            .saturating_sub(dropdown_rect.first_option),
                    )
                    .copied()
                {
                    dropdown.set_active_descendant(highlighted);
                }
                dropdown.set_children(option_nodes);
                nodes.push((SETTINGS_DROPDOWN, dropdown));
                dialog_children.push(SETTINGS_DROPDOWN);
            }
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Settings");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((SETTINGS_DIALOG, dialog));

        let default_focus = if info.dropdown_open {
            selected_setting.unwrap_or(SETTINGS_DIALOG)
        } else {
            SETTINGS_INPUT
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                (!info.dropdown_open || node != SETTINGS_INPUT)
                    && accessibility.is_settings_node(node)
            });
        ModalSnapshot {
            dialog: SETTINGS_DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_shortcuts_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::shortcuts_panel::ShortcutsAction;

        if request.target_node == SHORTCUTS_INPUT {
            if self.shortcuts_panel.is_recording() {
                return;
            }
            match request.action {
                Action::Focus => {
                    self.accessibility.focus_override = Some(SHORTCUTS_INPUT);
                }
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.shortcuts_panel.accessibility_set_filter(value) {
                        self.ime.clear_preedit();
                        self.accessibility.focus_override = Some(SHORTCUTS_INPUT);
                    }
                }
                _ => {}
            }
            return;
        }

        let Some(ShortcutsSemanticTarget::Action(action)) = self
            .accessibility
            .shortcuts
            .targets
            .get(&request.target_node)
            .copied()
        else {
            // The editor is modal. Stale requests for hidden application
            // controls and non-action headings stop here.
            return;
        };
        if !self
            .shortcuts_panel
            .is_visible_action(action, &self.shortcut_registry)
        {
            return;
        }
        match request.action {
            Action::Focus => {
                if self
                    .shortcuts_panel
                    .select_action(action, &self.shortcut_registry)
                {
                    self.accessibility.focus_override = Some(request.target_node);
                }
            }
            Action::Click => {
                self.apply_shortcuts_action(ShortcutsAction::StartRecording(action));
                self.accessibility.focus_override = Some(request.target_node);
            }
            Action::CustomAction
                if matches!(
                    request.data,
                    Some(ActionData::CustomAction(SHORTCUT_CLEAR_ACTION))
                ) =>
            {
                self.apply_shortcuts_action(ShortcutsAction::ClearShortcut(action));
                self.accessibility.focus_override = Some(request.target_node);
            }
            _ => {}
        }
    }

    pub(super) fn shortcuts_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let rows = self.list_dialog_rows(crate::ui::shortcuts_panel::SHORTCUTS_DIALOG);
        self.shortcuts_panel.set_max_visible(rows);
        let info = self.shortcuts_panel.build_info(&self.shortcut_registry);
        let layout = self.shortcuts_panel_layout(&info);

        let mut nodes = Vec::with_capacity(info.entries.len() + 8);

        let mut input = Node::new(Role::SearchInput);
        input.set_label("Filter keyboard shortcuts");
        input.set_placeholder("Filter shortcuts...");
        set_text_value(&mut input, SHORTCUTS_INPUT, &info.filter, Some(info.cursor));
        input.set_bounds(bounds_rect(layout.input));
        input.set_controls([SHORTCUTS_LIST]);
        if info.recording {
            input.set_read_only();
        } else {
            input.add_action(Action::Focus);
            input.add_action(Action::SetValue);
        }

        let mut list_children = Vec::with_capacity(info.entries.len());
        let mut selected_node = None;
        for (row, entry) in info.entries.iter().enumerate() {
            let target = if entry.is_header {
                ShortcutsSemanticTarget::Header(entry.action.meta().category)
            } else {
                ShortcutsSemanticTarget::Action(entry.action)
            };
            let node_id = self.accessibility.shortcuts.node(&target);
            list_children.push(node_id);
            let bounds = bounds_rect(layout.row(row));
            if entry.is_header {
                let mut heading = Node::new(Role::Heading);
                heading.set_label(entry.label);
                heading.set_bounds(bounds);
                nodes.push((node_id, heading));
                continue;
            }

            let mut node = Node::new(Role::Button);
            node.set_label(format!("Change {} shortcut", entry.label));
            node.set_description(format!(
                "{} shortcut; activate, then press a key; use the Clear binding action to unbind",
                entry.action.meta().category.label()
            ));
            node.set_value(entry.binding.clone());
            if entry.binding != "Unset" {
                node.set_keyboard_shortcut(entry.binding.clone());
            }
            node.set_selected(entry.selected);
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.add_action(Action::CustomAction);
            node.push_custom_action(CustomAction {
                id: SHORTCUT_CLEAR_ACTION,
                description: "Clear binding".into(),
            });
            node.set_bounds(bounds);
            if entry.selected {
                selected_node = Some(node_id);
            }
            nodes.push((node_id, node));
        }
        if let Some(selected) = selected_node {
            input.set_active_descendant(selected);
        }
        nodes.push((SHORTCUTS_INPUT, input));

        let mut list = Node::new(Role::List);
        list.set_label(format!("{} keyboard shortcuts", info.total_entries));
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(bounds_rect(layout.list));
        if let Some(selected) = selected_node {
            list.set_active_descendant(selected);
        }
        list.set_children(list_children);
        nodes.push((SHORTCUTS_LIST, list));

        let mut dialog_children = vec![SHORTCUTS_INPUT, SHORTCUTS_LIST];
        if info.entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No matching keyboard shortcuts");
            empty.set_live(Live::Polite);
            nodes.push((SHORTCUTS_EMPTY, empty));
            dialog_children.push(SHORTCUTS_EMPTY);
        }
        if info.recording || info.notice.is_some() {
            let mut status = Node::new(Role::Status);
            status.set_label(if info.recording {
                "Recording shortcut. Press a key to bind, Delete to clear, or Escape to cancel."
                    .to_string()
            } else {
                info.notice
                    .as_ref()
                    .map(crate::ui::shortcuts_panel::ShortcutNotice::message)
                    .unwrap_or_default()
            });
            status.set_live(Live::Polite);
            nodes.push((SHORTCUTS_STATUS, status));
            dialog_children.push(SHORTCUTS_STATUS);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Keyboard shortcuts");
        dialog.set_description("Search, change, or clear application keyboard shortcuts");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((SHORTCUTS_DIALOG, dialog));

        let default_focus = selected_node.unwrap_or(if info.recording {
            SHORTCUTS_DIALOG
        } else {
            SHORTCUTS_INPUT
        });
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                (!info.recording || node != SHORTCUTS_INPUT)
                    && accessibility.is_shortcuts_node(node)
            });
        ModalSnapshot {
            dialog: SHORTCUTS_DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_commands_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::commands::panel::CommandsPanelAction;

        match request.target_node {
            COMMANDS_LIST => match request.action {
                Action::ScrollUp => self.scroll_commands_panel(-1),
                Action::ScrollDown => self.scroll_commands_panel(1),
                _ => {}
            },
            COMMANDS_NEW => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::StartAdd);
                    self.accessibility.focus_override = Some(COMMANDS_NAME);
                }
                Action::Focus => {
                    let layout = self.commands_panel_layout();
                    self.commands_panel.focus_browse_control(
                        CommandControl::New,
                        &self.config.commands,
                        layout,
                    );
                    self.accessibility.focus_override = Some(COMMANDS_NEW);
                }
                _ => {}
            },
            COMMANDS_CLOSE => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::Close);
                    self.accessibility.focus_override = None;
                }
                Action::Focus => {
                    let layout = self.commands_panel_layout();
                    self.commands_panel.focus_browse_control(
                        CommandControl::Close,
                        &self.config.commands,
                        layout,
                    );
                    self.accessibility.focus_override = Some(COMMANDS_CLOSE);
                }
                _ => {}
            },
            COMMANDS_NAME | COMMANDS_COMMAND if self.commands_panel.is_editing() => {
                let is_name = request.target_node == COMMANDS_NAME;
                match request.action {
                    Action::Focus => {
                        self.apply_commands_panel_action(if is_name {
                            CommandsPanelAction::ClickName
                        } else {
                            CommandsPanelAction::ClickCommand
                        });
                        self.accessibility.focus_override = Some(request.target_node);
                    }
                    Action::SetValue => {
                        let Some(ActionData::Value(value)) = request.data.as_ref() else {
                            return;
                        };
                        let accepted = if is_name {
                            self.commands_panel.accessibility_set_name(value)
                        } else {
                            self.commands_panel.accessibility_set_command(value)
                        };
                        if accepted {
                            self.ime.clear_preedit();
                            self.accessibility.focus_override = Some(request.target_node);
                        }
                    }
                    _ => {}
                }
            }
            COMMANDS_SAVE if self.commands_panel.is_editing() => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::Submit);
                    self.accessibility.focus_override = Some(if self.commands_panel.is_editing() {
                        COMMANDS_SAVE
                    } else {
                        COMMANDS_NEW
                    });
                }
                Action::Focus => self.accessibility.focus_override = Some(COMMANDS_SAVE),
                _ => {}
            },
            COMMANDS_CANCEL if self.commands_panel.is_editing() => match request.action {
                Action::Click => {
                    self.apply_commands_panel_action(CommandsPanelAction::CancelEdit);
                    self.accessibility.focus_override = Some(COMMANDS_NEW);
                }
                Action::Focus => self.accessibility.focus_override = Some(COMMANDS_CANCEL),
                _ => {}
            },
            _ => {
                let Some(target) = self
                    .accessibility
                    .commands
                    .targets
                    .get(&request.target_node)
                    .cloned()
                else {
                    // The commands panel is modal; stale requests for hidden
                    // application controls must stop here.
                    return;
                };
                let key = match &target {
                    CommandsSemanticTarget::Group(key)
                    | CommandsSemanticTarget::Run(key)
                    | CommandsSemanticTarget::Delete(key) => key,
                };
                let Some(index) = custom_command_index(&self.config.commands, key) else {
                    return;
                };
                match (target, request.action) {
                    (CommandsSemanticTarget::Run(_), Action::Click) => {
                        self.apply_commands_panel_action(CommandsPanelAction::Run(index));
                        self.accessibility.focus_override = None;
                    }
                    (CommandsSemanticTarget::Delete(_), Action::Click) => {
                        self.apply_commands_panel_action(CommandsPanelAction::Delete(index));
                    }
                    (
                        target @ (CommandsSemanticTarget::Run(_)
                        | CommandsSemanticTarget::Delete(_)),
                        Action::Focus,
                    ) => {
                        let layout = self.commands_panel_layout();
                        let control = if matches!(target, CommandsSemanticTarget::Run(_)) {
                            CommandControl::Run(index)
                        } else {
                            CommandControl::Delete(index)
                        };
                        if self.commands_panel.focus_browse_control(
                            control,
                            &self.config.commands,
                            layout,
                        ) {
                            self.accessibility.focus_override = Some(request.target_node);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub(in crate::app) fn clear_commands_accessibility_focus(&mut self) {
        if self
            .accessibility
            .focus_override
            .is_some_and(|id| self.accessibility.is_commands_node(id))
        {
            self.accessibility.focus_override = None;
        }
    }

    /// A deletion changes duplicate occurrence identities. Retire all published
    /// row actions before the next frame; never reuse their node IDs for rows
    /// that shifted into a deleted occurrence. Fixed form/footer IDs stay live.
    pub(in crate::app) fn retire_commands_accessibility_rows(&mut self) {
        self.clear_commands_accessibility_focus();
        self.accessibility.commands.clear();
    }

    pub(in crate::app) fn retain_commands_accessibility_rows(
        &mut self,
        keys: &HashSet<CustomCommandSemanticKey>,
    ) {
        let retained = |target: &CommandsSemanticTarget| {
            let (CommandsSemanticTarget::Group(key)
            | CommandsSemanticTarget::Run(key)
            | CommandsSemanticTarget::Delete(key)) = target;
            keys.contains(key)
        };
        if self.accessibility.focus_override.is_some_and(|id| {
            self.accessibility
                .commands
                .targets
                .get(&id)
                .is_some_and(|target| !retained(target))
        }) {
            self.accessibility.focus_override = None;
        }
        self.accessibility.commands.retain(retained);
    }

    pub(super) fn commands_accessibility_snapshot(&mut self) -> ModalSnapshot {
        use crate::ui::commands::panel::EditMode;

        let info = self.commands_panel.build_info(&self.config.commands);
        let layout = self.commands_panel_layout();
        let keys = custom_command_keys(&info.commands);
        let retained = keys
            .iter()
            .flat_map(|key| {
                [
                    CommandsSemanticTarget::Group(key.clone()),
                    CommandsSemanticTarget::Run(key.clone()),
                    CommandsSemanticTarget::Delete(key.clone()),
                ]
            })
            .collect::<HashSet<_>>();
        self.accessibility
            .commands
            .retain(|target| retained.contains(target));

        let mut nodes = Vec::with_capacity(info.commands.len() * 3 + 12);

        let mut close = Node::new(Role::Button);
        close.set_label("Close custom commands");
        close.add_action(Action::Click);
        close.add_action(Action::Focus);
        close.set_bounds(bounds_rect(layout.close_button));
        nodes.push((COMMANDS_CLOSE, close));

        let mut new_command = Node::new(Role::Button);
        new_command.set_label("New command");
        new_command.add_action(Action::Click);
        new_command.add_action(Action::Focus);
        new_command.set_bounds(bounds_rect(layout.new_button));
        nodes.push((COMMANDS_NEW, new_command));

        let mut dialog_children = vec![COMMANDS_CLOSE, COMMANDS_NEW];
        if let Some(form) = layout.form {
            let name_active = info.edit_mode == EditMode::EditingName;
            let command_active = info.edit_mode == EditMode::EditingCommand;
            let mut name = command_form_input(
                COMMANDS_NAME,
                "Command name",
                crate::ui::commands::panel::NAME_PLACEHOLDER,
                &info.form_name,
                name_active.then_some(info.form_cursor),
                form.name_input,
            );
            name.add_action(Action::SetValue);
            nodes.push((COMMANDS_NAME, name));

            let mut command = command_form_input(
                COMMANDS_COMMAND,
                "Shell command",
                crate::ui::commands::panel::COMMAND_PLACEHOLDER,
                &info.form_command,
                command_active.then_some(info.form_cursor),
                form.command_input,
            );
            command.add_action(Action::SetValue);
            nodes.push((COMMANDS_COMMAND, command));

            let mut save = Node::new(Role::Button);
            save.set_label("Save command");
            save.set_description("Both name and shell command are required");
            save.add_action(Action::Click);
            save.add_action(Action::Focus);
            save.set_bounds(bounds_rect(form.save_button));
            nodes.push((COMMANDS_SAVE, save));

            let mut cancel = Node::new(Role::Button);
            cancel.set_label("Cancel new command");
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
            cancel.set_bounds(bounds_rect(form.cancel_button));
            nodes.push((COMMANDS_CANCEL, cancel));
            dialog_children.extend([
                COMMANDS_NAME,
                COMMANDS_COMMAND,
                COMMANDS_SAVE,
                COMMANDS_CANCEL,
            ]);
        }

        let mut command_groups = Vec::with_capacity(keys.len());
        for (index, (command, key)) in info.commands.iter().zip(keys.iter()).enumerate() {
            let group_id = self
                .accessibility
                .commands
                .node(&CommandsSemanticTarget::Group(key.clone()));
            let run_id = self
                .accessibility
                .commands
                .node(&CommandsSemanticTarget::Run(key.clone()));
            let delete_id = self
                .accessibility
                .commands
                .node(&CommandsSemanticTarget::Delete(key.clone()));
            let mut run = Node::new(Role::Button);
            run.set_label(format!("Run {}", command.name));
            let mut description = command.command.clone();
            if let Some(alias) = &command.alias {
                description.push_str(&format!(", alias {alias}"));
            }
            run.set_description(description);
            run.add_action(Action::Click);
            run.add_action(Action::Focus);
            if layout.visible_range().contains(&index) {
                run.set_bounds(bounds_rect(layout.command_run(index)));
            }
            nodes.push((run_id, run));

            let mut delete = Node::new(Role::Button);
            delete.set_label(format!("Delete {}", command.name));
            delete.set_description("Permanently removes this custom command");
            delete.add_action(Action::Click);
            delete.add_action(Action::Focus);
            if layout.visible_range().contains(&index) {
                delete.set_bounds(bounds_rect(layout.command_delete(index)));
            }
            nodes.push((delete_id, delete));

            let mut group = Node::new(Role::Group);
            group.set_label(command.name.clone());
            if layout.visible_range().contains(&index) {
                group.set_bounds(bounds_rect(layout.command_row(index)));
            }
            group.set_children([run_id, delete_id]);
            nodes.push((group_id, group));
            command_groups.push(group_id);
        }

        let mut list = Node::new(Role::List);
        list.set_label(format!("{} custom commands", info.commands.len()));
        if layout.scroll_offset > 0 {
            list.add_action(Action::ScrollUp);
        }
        if layout.visible_count > 0 && layout.visible_range().end < info.commands.len() {
            list.add_action(Action::ScrollDown);
        }
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel.x,
            layout.list_start_y,
            layout.panel.width,
            (layout.panel.height - layout.list_start_y).max(0.0),
        ));
        list.set_children(command_groups);
        nodes.push((COMMANDS_LIST, list));
        dialog_children.push(COMMANDS_LIST);

        if info.commands.is_empty() && info.edit_mode == EditMode::Browse {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No saved commands");
            empty.set_live(Live::Polite);
            nodes.push((COMMANDS_EMPTY, empty));
            dialog_children.push(COMMANDS_EMPTY);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Custom commands");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((COMMANDS_DIALOG, dialog));

        let default_focus = match info.edit_mode {
            EditMode::EditingName => COMMANDS_NAME,
            EditMode::EditingCommand => COMMANDS_COMMAND,
            EditMode::Browse => match self.commands_panel.browse_focus(&info.commands) {
                Some(CommandControl::Close) => COMMANDS_CLOSE,
                Some(CommandControl::Run(index)) => {
                    self.accessibility.commands.nodes
                        [&CommandsSemanticTarget::Run(keys[index].clone())]
                }
                Some(CommandControl::Delete(index)) => {
                    self.accessibility.commands.nodes
                        [&CommandsSemanticTarget::Delete(keys[index].clone())]
                }
                _ => COMMANDS_NEW,
            },
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                accessibility.is_commands_node(node)
            });
        ModalSnapshot {
            dialog: COMMANDS_DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_workspace_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::workspace_panel::WorkspacePanelAction;

        match request.target_node {
            WORKSPACE_BACK | WORKSPACE_SNAPSHOTS => match request.action {
                Action::Click => {
                    self.apply_workspace_panel_action(if request.target_node == WORKSPACE_BACK {
                        WorkspacePanelAction::ShowWorkspaces
                    } else {
                        WorkspacePanelAction::ShowSnapshots
                    });
                    self.accessibility.focus_override = None;
                }
                Action::Focus => self.accessibility.focus_override = Some(request.target_node),
                _ => {}
            },
            WORKSPACE_CLOSE => match request.action {
                Action::Click => {
                    self.apply_workspace_panel_action(WorkspacePanelAction::Close);
                    self.accessibility.focus_override = None;
                }
                Action::Focus => self.accessibility.focus_override = Some(WORKSPACE_CLOSE),
                _ => {}
            },
            WORKSPACE_RENAME_INPUT if self.workspace_panel.is_renaming() => match request.action {
                Action::Focus => {
                    self.accessibility.focus_override = Some(WORKSPACE_RENAME_INPUT);
                }
                Action::SetValue => {
                    let Some(ActionData::Value(value)) = request.data.as_ref() else {
                        return;
                    };
                    if self.workspace_panel.accessibility_set_rename(value) {
                        self.ime.clear_preedit();
                        self.accessibility.focus_override = Some(WORKSPACE_RENAME_INPUT);
                    }
                }
                _ => {}
            },
            WORKSPACE_RENAME_SAVE if self.workspace_panel.is_renaming() => match request.action {
                Action::Click => {
                    self.apply_workspace_panel_action(WorkspacePanelAction::SubmitRename);
                    self.accessibility.focus_override = None;
                }
                Action::Focus => self.accessibility.focus_override = Some(WORKSPACE_RENAME_SAVE),
                _ => {}
            },
            WORKSPACE_RENAME_CANCEL if self.workspace_panel.is_renaming() => match request.action {
                Action::Click => {
                    self.apply_workspace_panel_action(WorkspacePanelAction::CancelRename);
                    self.accessibility.focus_override = None;
                }
                Action::Focus => self.accessibility.focus_override = Some(WORKSPACE_RENAME_CANCEL),
                _ => {}
            },
            _ => {
                if self.workspace_panel.is_renaming() {
                    return;
                }
                let Some(target) = self
                    .accessibility
                    .workspace
                    .targets
                    .get(&request.target_node)
                    .cloned()
                else {
                    return;
                };
                let key = match &target {
                    WorkspaceSemanticTarget::Group(key)
                    | WorkspaceSemanticTarget::Load(key)
                    | WorkspaceSemanticTarget::Rename(key)
                    | WorkspaceSemanticTarget::Delete(key) => key,
                };
                if key.workspaces != self.workspace_panel.is_workspace_mode()
                    || !self.workspace_panel.is_visible_id(&key.id)
                {
                    return;
                }
                match (target, request.action) {
                    (
                        WorkspaceSemanticTarget::Load(key)
                        | WorkspaceSemanticTarget::Rename(key)
                        | WorkspaceSemanticTarget::Delete(key),
                        Action::Focus,
                    ) => {
                        if self.workspace_panel.select_id(&key.id) {
                            self.accessibility.focus_override = Some(request.target_node);
                        }
                    }
                    (WorkspaceSemanticTarget::Load(key), Action::Click) => {
                        self.apply_workspace_panel_action(WorkspacePanelAction::Load(key.id));
                        self.accessibility.focus_override = None;
                    }
                    (WorkspaceSemanticTarget::Rename(key), Action::Click) => {
                        self.apply_workspace_panel_action(WorkspacePanelAction::Rename(key.id));
                        if self.workspace_panel.is_renaming() {
                            self.accessibility.focus_override = Some(WORKSPACE_RENAME_INPUT);
                        }
                    }
                    (WorkspaceSemanticTarget::Delete(key), Action::Click) => {
                        self.apply_workspace_panel_action(WorkspacePanelAction::Delete(key.id));
                        self.accessibility.focus_override = None;
                    }
                    _ => {}
                }
            }
        }
    }

    pub(super) fn workspace_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self.workspace_panel_layout();
        self.workspace_panel.set_max_visible(layout.rows_that_fit);
        let info = self.workspace_panel.build_info();

        let keys = info
            .entries
            .iter()
            .map(|entry| WorkspaceSemanticKey {
                id: entry.id.clone(),
                workspaces: info.workspaces,
            })
            .collect::<Vec<_>>();
        let retained = keys
            .iter()
            .flat_map(|key| {
                let mut targets = vec![
                    WorkspaceSemanticTarget::Group(key.clone()),
                    WorkspaceSemanticTarget::Load(key.clone()),
                    WorkspaceSemanticTarget::Delete(key.clone()),
                ];
                if info.workspaces {
                    targets.push(WorkspaceSemanticTarget::Rename(key.clone()));
                }
                targets
            })
            .collect::<HashSet<_>>();
        self.accessibility
            .workspace
            .retain(|target| retained.contains(target));

        let mut nodes = Vec::with_capacity(layout.visible_count * 4 + 10);

        let mut close = Node::new(Role::Button);
        close.set_label(if info.workspaces {
            "Close workspaces"
        } else {
            "Close tab snapshots"
        });
        close.add_action(Action::Click);
        close.add_action(Action::Focus);
        close.set_bounds(bounds_rect(layout.close_button));
        nodes.push((WORKSPACE_CLOSE, close));

        let mut dialog_children = vec![WORKSPACE_CLOSE];
        if let Some(back) = layout.back_button {
            let mut node = Node::new(Role::Button);
            node.set_label("Back to workspaces");
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.set_bounds(bounds_rect(back));
            nodes.push((WORKSPACE_BACK, node));
            dialog_children.push(WORKSPACE_BACK);
        }
        if info.snapshot_count > 0
            && let Some(link) = layout.snapshots_link
        {
            let mut node = Node::new(Role::Button);
            node.set_label(format!("Tab snapshots ({})", info.snapshot_count));
            node.set_description("Show snapshots saved when tabs closed");
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            node.set_bounds(bounds_rect(link));
            nodes.push((WORKSPACE_SNAPSHOTS, node));
            dialog_children.push(WORKSPACE_SNAPSHOTS);
        }
        if let (Some(value), Some(rename)) = (&info.rename_text, layout.rename) {
            let mut input = Node::new(Role::TextInput);
            input.set_label("Workspace name");
            set_text_value(
                &mut input,
                WORKSPACE_RENAME_INPUT,
                value,
                Some(info.rename_cursor),
            );
            input.add_action(Action::Focus);
            input.add_action(Action::SetValue);
            input.set_bounds(bounds_rect(rename.input));
            nodes.push((WORKSPACE_RENAME_INPUT, input));

            let mut save = Node::new(Role::Button);
            save.set_label("Save workspace name");
            save.add_action(Action::Click);
            save.add_action(Action::Focus);
            save.set_bounds(bounds_rect(rename.save_button));
            nodes.push((WORKSPACE_RENAME_SAVE, save));

            let mut cancel = Node::new(Role::Button);
            cancel.set_label("Cancel workspace rename");
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
            cancel.set_bounds(bounds_rect(rename.cancel_button));
            nodes.push((WORKSPACE_RENAME_CANCEL, cancel));
            dialog_children.extend([
                WORKSPACE_RENAME_INPUT,
                WORKSPACE_RENAME_SAVE,
                WORKSPACE_RENAME_CANCEL,
            ]);
        }

        let end = (info.scroll_offset + layout.visible_count).min(info.entries.len());
        let rename_active = info.rename_text.is_some();
        let mut group_nodes = Vec::with_capacity(layout.visible_count);
        let mut selected_group = None;
        let mut selected_load = None;
        for (visible_index, absolute_index) in (info.scroll_offset..end).enumerate() {
            let entry = &info.entries[absolute_index];
            let key = keys[absolute_index].clone();
            let geometry = layout.entry(visible_index);
            let group_id = self
                .accessibility
                .workspace
                .node(&WorkspaceSemanticTarget::Group(key.clone()));
            let load_id = self
                .accessibility
                .workspace
                .node(&WorkspaceSemanticTarget::Load(key.clone()));
            let delete_id = self
                .accessibility
                .workspace
                .node(&WorkspaceSemanticTarget::Delete(key.clone()));
            let mut load = Node::new(Role::Button);
            load.set_label(format!("Open {}", entry.display_name));
            load.set_description(if info.workspaces {
                "Relaunch this workspace's tabs and panes"
            } else {
                "Open this tab snapshot in a new shell"
            });
            if rename_active {
                load.set_disabled();
            } else {
                load.add_action(Action::Click);
                load.add_action(Action::Focus);
            }
            load.set_bounds(bounds_rect(geometry.load_button));
            nodes.push((load_id, load));

            let mut children = vec![load_id];
            if info.workspaces {
                let rename_id = self
                    .accessibility
                    .workspace
                    .node(&WorkspaceSemanticTarget::Rename(key.clone()));
                let mut rename = Node::new(Role::Button);
                rename.set_label(format!("Rename {}", entry.display_name));
                if rename_active {
                    rename.set_disabled();
                } else {
                    rename.add_action(Action::Click);
                    rename.add_action(Action::Focus);
                }
                if let Some(bounds) = geometry.rename_button {
                    rename.set_bounds(bounds_rect(bounds));
                }
                nodes.push((rename_id, rename));
                children.push(rename_id);
            }

            let mut delete = Node::new(Role::Button);
            delete.set_label(format!("Delete {}", entry.display_name));
            delete.set_description(if info.workspaces {
                "Permanently deletes this saved workspace"
            } else {
                "Permanently deletes this tab snapshot"
            });
            if rename_active {
                delete.set_disabled();
            } else {
                delete.add_action(Action::Click);
                delete.add_action(Action::Focus);
            }
            delete.set_bounds(bounds_rect(geometry.delete_button));
            nodes.push((delete_id, delete));
            children.push(delete_id);

            let mut group = Node::new(Role::Group);
            group.set_label(entry.display_name.clone());
            let description = if entry.subtitle.is_empty() {
                entry.last_modified.clone()
            } else {
                format!("{}; {}", entry.subtitle, entry.last_modified)
            };
            if !description.is_empty() {
                group.set_description(description);
            }
            group.set_bounds(bounds_rect(geometry.row));
            group.set_children(children);
            nodes.push((group_id, group));
            group_nodes.push(group_id);
            if absolute_index == info.selected_index {
                selected_group = Some(group_id);
                selected_load = Some(load_id);
            }
        }

        let mut list = Node::new(Role::List);
        list.set_label(if info.workspaces {
            format!("{} saved workspaces", info.entries.len())
        } else {
            format!("{} tab snapshots", info.entries.len())
        });
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel.x,
            layout.list_start_y,
            layout.panel.width,
            (layout.panel.height - layout.list_start_y).max(0.0),
        ));
        if let Some(selected) = selected_group {
            list.set_active_descendant(selected);
        }
        list.set_children(group_nodes);
        nodes.push((WORKSPACE_LIST, list));
        dialog_children.push(WORKSPACE_LIST);

        if info.entries.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label(if info.workspaces {
                "No saved workspaces"
            } else {
                "No tab snapshots"
            });
            empty.set_live(Live::Polite);
            nodes.push((WORKSPACE_EMPTY, empty));
            dialog_children.push(WORKSPACE_EMPTY);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label(if info.workspaces {
            "Workspaces"
        } else {
            "Tab snapshots"
        });
        dialog.set_description(if info.workspaces {
            "Opening relaunches the saved tabs and panes"
        } else {
            "Saved when tabs close. Opening starts a new shell"
        });
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((WORKSPACE_DIALOG, dialog));

        let default_focus = if info.rename_text.is_some() {
            WORKSPACE_RENAME_INPUT
        } else {
            selected_load.unwrap_or(WORKSPACE_CLOSE)
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                accessibility.is_workspace_node(node)
            });
        ModalSnapshot {
            dialog: WORKSPACE_DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_agents_accessibility_action(&mut self, request: ActionRequest) {
        use crate::ui::agents_panel::{AgentsControl, AgentsPanelAction};

        let control = match request.target_node {
            AGENTS_LIST => {
                let delta = match request.action {
                    Action::ScrollUp => -1,
                    Action::ScrollDown => 1,
                    _ => return,
                };
                let layout = self.agents_panel_layout();
                self.agents_panel.scroll_rows(delta, layout);
                return;
            }
            AGENTS_CLOSE => AgentsControl::Close,
            AGENTS_SUPERVISOR_TOGGLE if self.supervisor_running() => AgentsControl::Supervisor,
            node => {
                // The panel is modal: a request for anything else, including
                // a row whose agent has gone, stops here.
                let Some(index) = self
                    .accessibility
                    .agents
                    .targets
                    .get(&node)
                    .and_then(|identity| self.agents_panel.shown_index(*identity))
                else {
                    return;
                };
                AgentsControl::Agent(index)
            }
        };
        match request.action {
            Action::Click => {
                let action = match control {
                    AgentsControl::Close => AgentsPanelAction::Close,
                    AgentsControl::Supervisor => AgentsPanelAction::ToggleSupervisor,
                    AgentsControl::Agent(index) => AgentsPanelAction::Show(index),
                };
                self.apply_agents_panel_action(action);
                self.accessibility.focus_override =
                    (control == AgentsControl::Supervisor).then_some(AGENTS_SUPERVISOR_TOGGLE);
            }
            Action::Focus => {
                let layout = self.agents_panel_layout();
                if self.agents_panel.focus_control_at(control, layout) {
                    self.accessibility.focus_override = Some(request.target_node);
                }
            }
            _ => {}
        }
    }

    pub(in crate::app) fn clear_agents_accessibility_focus(&mut self) {
        if self
            .accessibility
            .focus_override
            .is_some_and(|id| self.accessibility.is_agents_node(id))
        {
            self.accessibility.focus_override = None;
        }
    }

    pub(super) fn agents_accessibility_snapshot(&mut self) -> ModalSnapshot {
        use crate::ui::agents_panel::AgentsControl;

        let info = self.agents_panel_info();
        let layout = self.agents_panel_layout();
        let shown: HashSet<_> = info.rows.iter().map(|row| row.identity).collect();
        self.accessibility
            .agents
            .retain(|target| shown.contains(target));

        let mut nodes = Vec::with_capacity(info.rows.len() + 12);

        let mut close = Node::new(Role::Button);
        close.set_label("Close agents");
        close.add_action(Action::Click);
        close.add_action(Action::Focus);
        close.set_bounds(bounds_rect(layout.close_button));
        nodes.push((AGENTS_CLOSE, close));
        let mut dialog_children = vec![AGENTS_CLOSE];

        let mut row_ids = Vec::with_capacity(info.rows.len());
        for (index, row) in info.rows.iter().enumerate() {
            let id = self.accessibility.agents.node(&row.identity);
            let mut button = Node::new(Role::Button);
            button.set_label(format!("{}, {}", row.name, row.place));
            button.set_description(row.summary.clone());
            button.add_action(Action::Click);
            button.add_action(Action::Focus);
            if layout.visible_range().contains(&index) {
                button.set_bounds(bounds_rect(layout.row(index)));
            }
            nodes.push((id, button));
            row_ids.push(id);
        }
        let mut list = Node::new(Role::List);
        list.set_label(format!("{} agents", info.rows.len()));
        if layout.scroll_offset > 0 {
            list.add_action(Action::ScrollUp);
        }
        if layout.visible_count > 0 && layout.visible_range().end < info.rows.len() {
            list.add_action(Action::ScrollDown);
        }
        list.set_orientation(accesskit::Orientation::Vertical);
        list.set_bounds(rect(
            layout.panel.x,
            layout.list_start_y,
            layout.panel.width,
            layout.visible_count as f32 * layout.row_height,
        ));
        list.set_children(row_ids.clone());
        nodes.push((AGENTS_LIST, list));
        dialog_children.push(AGENTS_LIST);

        if info.rows.is_empty() {
            let mut empty = Node::new(Role::Status);
            empty.set_label("No agents in this window");
            empty.set_live(Live::Polite);
            nodes.push((AGENTS_EMPTY, empty));
            dialog_children.push(AGENTS_EMPTY);
        }

        if let (Some(supervisor), Some(section)) = (&info.supervisor, layout.supervisor) {
            let mut toggle = Node::new(Role::Button);
            toggle.set_label(if supervisor.held {
                "Hand back to supervisor"
            } else {
                "Take over from supervisor"
            });
            toggle.add_action(Action::Click);
            toggle.add_action(Action::Focus);
            toggle.set_bounds(bounds_rect(section.button));
            nodes.push((AGENTS_SUPERVISOR_TOGGLE, toggle));

            let mut entry_ids = Vec::with_capacity(section.entry_count);
            for (index, entry) in supervisor
                .entries
                .iter()
                .take(section.entry_count)
                .enumerate()
            {
                let id = NodeId(AGENTS_ACTIVITY_BASE + index as u64);
                let mut item = Node::new(Role::ListItem);
                item.set_label(entry.clone());
                item.set_bounds(bounds_rect(section.entry(index)));
                nodes.push((id, item));
                entry_ids.push(id);
            }
            let mut activity = Node::new(Role::List);
            activity.set_label("Supervisor activity");
            activity.set_children(entry_ids);
            nodes.push((AGENTS_ACTIVITY, activity));

            let mut group = Node::new(Role::Group);
            group.set_label(format!("Supervisor, {}", supervisor.mode));
            group.set_bounds(bounds_rect(section.section));
            group.set_children([AGENTS_SUPERVISOR_TOGGLE, AGENTS_ACTIVITY]);
            nodes.push((AGENTS_SUPERVISOR, group));
            dialog_children.push(AGENTS_SUPERVISOR);
        }

        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Agents");
        dialog.set_modal();
        dialog.set_bounds(bounds_rect(layout.panel));
        dialog.set_children(dialog_children);
        nodes.push((AGENTS_DIALOG, dialog));

        let default_focus = match info.focus {
            Some(AgentsControl::Agent(index)) => {
                row_ids.get(index).copied().unwrap_or(AGENTS_CLOSE)
            }
            Some(AgentsControl::Supervisor) if info.supervisor.is_some() => {
                AGENTS_SUPERVISOR_TOGGLE
            }
            _ => AGENTS_CLOSE,
        };
        let focus = self
            .accessibility
            .modal_focus(&nodes, default_focus, |accessibility, node| {
                accessibility.is_agents_node(node)
            });
        ModalSnapshot {
            dialog: AGENTS_DIALOG,
            nodes,
            focus,
        }
    }

    pub(super) fn handle_workspace_menu_accessibility_action(&mut self, request: ActionRequest) {
        let Some(row) = workspace_menu_row(request.target_node) else {
            return;
        };
        match request.action {
            Action::Focus => {
                if let Some(menu) = self.workspace_menu.as_mut() {
                    menu.set_highlight(Some(row));
                }
                self.accessibility.focus_override = Some(request.target_node);
            }
            Action::Click => {
                let action = self
                    .workspace_menu
                    .as_ref()
                    .and_then(|menu| menu.action_at(row));
                if let Some(action) = action {
                    self.activate_workspace_menu(action);
                }
                self.accessibility.focus_override = None;
            }
            _ => {}
        }
    }

    /// The open Workspaces menu as a menu of items sharing the drawn geometry.
    pub(super) fn workspace_menu_accessibility_snapshot(&mut self) -> ModalSnapshot {
        let layout = self.workspace_menu_layout();
        let menu = self
            .workspace_menu
            .as_ref()
            .expect("the Workspaces menu layer is open only while its menu is");
        let mut nodes = Vec::new();
        let mut items = Vec::new();
        for (row, entry) in menu.rows().iter().enumerate() {
            let crate::ui::popup_menu::MenuRow::Item(item) = entry else {
                continue;
            };
            let id = workspace_menu_node(row);
            let mut node = Node::new(Role::MenuItem);
            node.set_label(item.label.as_str());
            let description = match (&item.detail, item.checked) {
                (Some(detail), true) => Some(format!("{detail}, current workspace")),
                (Some(detail), false) => Some(detail.clone()),
                (None, true) => Some("Current workspace".into()),
                (None, false) => None,
            };
            if let Some(description) = description {
                node.set_description(description);
            }
            node.set_selected(menu.highlighted() == Some(row));
            if item.enabled {
                node.add_action(Action::Focus);
                node.add_action(Action::Click);
            } else {
                node.set_disabled();
            }
            if let Some(&bounds) = layout.as_ref().and_then(|layout| layout.rows.get(row)) {
                node.set_bounds(bounds_rect(bounds));
            }
            items.push(id);
            nodes.push((id, node));
        }
        let mut menu_node = Node::new(Role::Menu);
        menu_node.set_label("Workspaces");
        menu_node.set_orientation(accesskit::Orientation::Vertical);
        if let Some(layout) = &layout {
            menu_node.set_bounds(bounds_rect(layout.panel));
        }
        menu_node.set_children(items);
        nodes.push((WORKSPACE_MENU, menu_node));

        let focus = menu
            .highlighted()
            .map_or(WORKSPACE_MENU, workspace_menu_node);
        self.accessibility.focus_override = Some(focus);
        ModalSnapshot {
            dialog: WORKSPACE_MENU,
            nodes,
            focus,
        }
    }
}

fn command_form_input(
    id: NodeId,
    label: &str,
    placeholder: &str,
    value: &str,
    cursor: Option<usize>,
    bounds: Bounds,
) -> Node {
    let mut input = Node::new(Role::TextInput);
    input.set_label(label);
    input.set_placeholder(placeholder);
    set_text_value(&mut input, id, value, cursor);
    input.add_action(Action::Focus);
    input.set_bounds(bounds_rect(bounds));
    input
}

fn workspace_menu_node(row: usize) -> NodeId {
    NodeId(WORKSPACE_MENU_ENTRY_BASE + row as u64)
}

/// Menu rows are bounded, so the range never reaches the next base.
pub(super) fn workspace_menu_row(node: NodeId) -> Option<usize> {
    node.0
        .checked_sub(WORKSPACE_MENU_ENTRY_BASE)
        .filter(|row| *row < 64)
        .map(|row| row as usize)
}
