//! The agent view's menus: their items, the command popover, search, and
//! rows.

use super::*;

/// An open menu's list, laid out where it scrolls.
pub(super) struct MenuList {
    pub(super) region: Region,
    pub(super) viewport: Bounds,
    /// Height of the whole list.
    pub(super) height: f32,
    pub(super) scroll: f32,
}

/// Menus longer than this can be searched.
pub(super) const MENU_SEARCH_AFTER: usize = 8;

/// The tallest a menu's list grows before it scrolls.
pub(super) const MENU_LIST_MAX: f32 = 440.0;

/// Whether a menu is long enough to search.
pub fn menu_searchable(input: &Input<'_>, kind: MenuKind) -> bool {
    menu_items(input, kind).len() > MENU_SEARCH_AFTER
}

/// A menu's rows that match `filter`: each of its words appears in the
/// row's text, heading, or detail, in any case.
pub fn filtered_menu_items(input: &Input<'_>, kind: MenuKind, filter: &str) -> Vec<MenuItem> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    menu_items(input, kind)
        .into_iter()
        .filter(|item| {
            let haystack = format!(
                "{} {} {}",
                item.group.as_deref().unwrap_or_default(),
                item.text,
                item.detail
            )
            .to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .collect()
}

/// What a menu's search field offers to find.
pub(super) fn search_placeholder(kind: MenuKind) -> &'static str {
    match kind {
        MenuKind::Model => "Search models",
        MenuKind::Folder => "Search folders",
        _ => "Search",
    }
}

/// One menu row.
pub struct MenuItem {
    pub text: String,
    pub detail: String,
    pub action: MenuAction,
    pub checked: bool,
    /// The heading it is listed under, as a model's provider.
    pub group: Option<String>,
}

/// The rows of a menu, for layout and keyboard navigation alike.
pub fn menu_items(input: &Input<'_>, kind: MenuKind) -> Vec<MenuItem> {
    let session = input.active();
    match kind {
        MenuKind::Mode => {
            let (modes, current) = mode_state(input);
            let mut items: Vec<MenuItem> = selectable_modes(&modes, input.allow_bypass)
                .into_iter()
                .map(|mode| MenuItem {
                    text: mode.label.clone(),
                    detail: mode.description.clone(),
                    action: MenuAction::SetMode(mode.id.clone()),
                    checked: mode.id == current,
                    group: None,
                })
                .collect();
            // Beside the modes, as the agent's own interface has it, and
            // like a mode that skips every check, only after the opt-in.
            let agent = input.agent();
            if agent.offers_auto_approve() && input.allow_bypass {
                let approving = auto_approve(input);
                items.push(MenuItem {
                    text: "Auto-approve".into(),
                    detail: format!(
                        "Allow everything {} asks about without asking, as its own \
                         auto-approve does",
                        agent.name()
                    ),
                    action: MenuAction::SetAutoApprove(!approving),
                    checked: approving,
                    group: None,
                });
            }
            items
        }
        MenuKind::Model => {
            let current = current_model(input);
            offered_models(input)
                .into_iter()
                .map(|model| MenuItem {
                    checked: model.value == current,
                    text: model.display_name,
                    detail: model.description,
                    action: MenuAction::SetModel(model.value),
                    group: model.group,
                })
                .collect()
        }
        MenuKind::Effort => {
            let current = current_effort(input);
            offered_efforts(input)
                .into_iter()
                .map(|effort| MenuItem {
                    checked: Some(&effort.value) == current.as_ref(),
                    text: effort.label,
                    detail: effort.description,
                    action: MenuAction::SetEffort(effort.value),
                    group: None,
                })
                .collect()
        }
        MenuKind::More => {
            let agent = input.agent();
            let mut items = vec![
                MenuItem {
                    text: "Open in a terminal".into(),
                    detail: format!(
                        "Continue this conversation in {}'s own interface",
                        agent.name()
                    ),
                    action: MenuAction::OpenInTerminal,
                    checked: false,
                    group: None,
                },
                MenuItem {
                    text: "Copy conversation".into(),
                    detail: String::new(),
                    action: MenuAction::CopyConversation,
                    checked: false,
                    group: None,
                },
            ];
            if session.is_some() {
                items.push(MenuItem {
                    text: "End session".into(),
                    detail: format!(
                        "Stop {} and remove this session; continue it later from New session",
                        agent.name()
                    ),
                    action: MenuAction::EndSession,
                    checked: false,
                    group: None,
                });
            }
            items.push(MenuItem {
                text: "Hide panel".into(),
                detail: String::new(),
                action: MenuAction::HidePanel,
                checked: false,
                group: None,
            });
            items
        }
        MenuKind::Folder => {
            let current = &input.new_session.cwd;
            let mut choices = vec![current.clone()];
            for choice in &input.new_session.cwd_choices {
                if !choices.contains(choice) {
                    choices.push(choice.clone());
                }
            }
            choices
                .into_iter()
                .map(|path| MenuItem {
                    checked: path == *current,
                    text: path.display().to_string(),
                    detail: String::new(),
                    action: MenuAction::SetFolder(path),
                    group: None,
                })
                .collect()
        }
    }
}

/// The modes the shown agent offers and the id of the one it runs in: the
/// conversation's, or on the new-session page the one chosen for the agent
/// about to start.
pub(super) fn mode_state(input: &Input) -> (Vec<ModeChoice>, String) {
    match (input.state.screen, input.active()) {
        (Screen::Conversation, Some(session)) => {
            (session.modes(), session.transcript().mode().to_owned())
        }
        _ => {
            let form = &input.state.new_session;
            let modes = input
                .tab_session()
                .filter(|session| session.agent() == form.agent)
                .map_or_else(|| form.agent.modes(), |session| session.modes());
            let current = form
                .mode
                .clone()
                .unwrap_or_else(|| form.agent.default_mode().to_owned());
            (modes, current)
        }
    }
}

/// What the shown agent reported: the shown session's handshake, else any
/// running session's of the same agent (they share an account).
pub(super) fn reported_handshake<'a>(input: &'a Input) -> Option<&'a Handshake> {
    let agent = input.agent();
    let shown = match input.state.screen {
        Screen::Sessions => None,
        _ => input.tab_session(),
    };
    shown
        .into_iter()
        .chain(input.sessions.iter().copied())
        .filter(|session| session.agent() == agent)
        .find_map(|session| session.transcript().handshake())
}

/// The models the agent offers, else a fallback list until it lists them.
pub(super) fn offered_models(input: &Input) -> Vec<ModelChoice> {
    reported_handshake(input)
        .map(|handshake| handshake.models.clone())
        .filter(|models| !models.is_empty())
        .unwrap_or_else(|| input.agent().fallback_models())
}

/// The model shown and checked: the conversation's or the form's choice,
/// else the one the agent says it runs.
pub(super) fn current_model(input: &Input) -> String {
    let chosen = match (input.state.screen, input.active()) {
        (Screen::Conversation, Some(session)) => session.model(),
        _ => input.state.new_session.model.as_deref(),
    };
    chosen
        .or_else(|| {
            reported_handshake(input).and_then(|handshake| handshake.current_model.as_deref())
        })
        .unwrap_or("default")
        .to_owned()
}

/// The levels of effort the current model offers; empty when it offers no
/// choice.
pub(super) fn offered_efforts(input: &Input) -> Vec<EffortChoice> {
    reported_handshake(input)
        .map(|handshake| handshake.efforts.clone())
        .unwrap_or_default()
}

/// The level of effort shown and checked, like [`current_model`].
pub(super) fn current_effort(input: &Input) -> Option<String> {
    let chosen = match (input.state.screen, input.active()) {
        (Screen::Conversation, Some(session)) => return session.effort().map(str::to_owned),
        _ => input.state.new_session.effort.clone(),
    };
    chosen.or_else(|| {
        reported_handshake(input).and_then(|handshake| handshake.current_effort.clone())
    })
}

/// Whether everything the shown agent asks about is approved.
pub(super) fn auto_approve(input: &Input) -> bool {
    match (input.state.screen, input.active()) {
        (Screen::Conversation, Some(session)) => session.auto_approve(),
        _ => input.state.new_session.auto_approve && input.allow_bypass,
    }
}

impl Builder<'_, '_> {
    /// The raised card menus and the command list draw on.
    pub(super) fn popover_frame(&self, region: &mut Region, bounds: Bounds) {
        for spread in [8.0, 4.0] {
            let grow = self.m.px(spread);
            region.prims.push(Prim::Fill {
                bounds: Bounds::new(
                    bounds.x - grow / 2.0,
                    bounds.y + grow / 2.0,
                    bounds.width + grow,
                    bounds.height + grow / 2.0,
                ),
                radius: self.m.radius + grow,
                color: [0.0, 0.0, 0.0, 0.12],
            });
        }
        self.card(region, bounds, self.m.radius, self.c.surface, self.c.border);
    }

    /// The agent's commands matching what follows a leading `/`, above the
    /// message box. Typing continues in the box while it shows.
    pub(super) fn command_menu(&mut self, region: &mut Region, anchor: Bounds) {
        let input = self.input;
        let offered = super::super::commands::offered(input.state, input.sessions);
        let shown = super::super::commands::shown(input.state, offered);
        if shown.is_empty() {
            return;
        }
        let highlighted = input.state.commands.highlighted.min(shown.len() - 1);
        let row_pad = self.m.px(10.0);
        let row_height = self.m.body.line_px + self.m.small.line_px + self.m.px(8.0);
        let height = row_height * shown.len() as f32 + self.m.px(6.0) * 2.0;
        let bounds = Bounds::new(
            anchor.x,
            (anchor.y - height - self.m.px(6.0)).round(),
            anchor.width,
            height.round(),
        );
        self.popover_frame(region, bounds);
        let text_width = bounds.width - self.m.px(8.0) - row_pad * 2.0;
        for (index, command) in shown.into_iter().enumerate() {
            let row = Bounds::new(
                bounds.x + self.m.px(4.0),
                bounds.y + self.m.px(6.0) + row_height * index as f32,
                bounds.width - self.m.px(8.0),
                row_height,
            );
            let target = Target::Command(command.name.clone());
            let selected = index == highlighted;
            if selected || self.hovered(&target) {
                region.prims.push(Prim::Fill {
                    bounds: row,
                    radius: self.m.px(6.0),
                    color: self.c.surface_hover,
                });
            }
            let name = format!("/{}", command.name);
            let text_x = row.x + row_pad;
            let line_top = row.y + self.m.px(4.0);
            let name_width = self.label(
                region,
                &name,
                text_x,
                line_top,
                self.m.body.line_px,
                text_width,
                UiFace::SANS_BOLD,
                self.m.body,
                self.c.text_strong,
            );
            if !command.argument_hint.is_empty() {
                let gap = self.m.px(6.0);
                self.label(
                    region,
                    &command.argument_hint,
                    text_x + name_width + gap,
                    line_top,
                    self.m.body.line_px,
                    (text_width - name_width - gap).max(0.0),
                    UiFace::SANS,
                    self.m.body,
                    self.c.text_muted,
                );
            }
            self.label(
                region,
                &command.description,
                text_x,
                line_top + self.m.body.line_px,
                self.m.small.line_px,
                text_width,
                UiFace::SANS,
                self.m.small,
                self.c.text_secondary,
            );
            let mut label = name;
            if !command.argument_hint.is_empty() {
                label.push(' ');
                label.push_str(&command.argument_hint);
            }
            if !command.description.is_empty() {
                label.push_str(". ");
                label.push_str(&command.description);
            }
            self.hit(
                region,
                row,
                target,
                HitRole::Option { selected },
                label,
                false,
            );
        }
    }

    /// An open menu: its frame and search field on `overlay`, and its list,
    /// which scrolls once it is taller than the room it has.
    pub(super) fn menu(
        &mut self,
        overlay: &mut Region,
        panel: Bounds,
        menu: &OpenMenu,
    ) -> Option<MenuList> {
        let searchable = menu_searchable(self.input, menu.kind);
        let items = filtered_menu_items(
            self.input,
            menu.kind,
            if searchable { menu.filter.text() } else { "" },
        );
        if items.is_empty() && !searchable {
            return None;
        }
        // A click anywhere else in the panel closes the menu.
        self.hit(
            overlay,
            panel,
            Target::DismissMenu,
            HitRole::Button,
            "Close menu",
            false,
        );
        let width =
            (panel.width - self.m.pad * 2.0).min(self.m.px(if searchable { 360.0 } else { 320.0 }));
        let row_pad = self.m.px(12.0);
        let text_width = width - row_pad * 2.0 - self.m.icon - self.m.px(8.0);
        let edge = self.m.px(6.0);

        // Rows from the list's top: a heading wherever the group changes.
        enum Row {
            Heading(String),
            Item(usize, Option<Arc<TextLayout>>),
        }
        let grouped = items.iter().any(|item| item.group.is_some());
        let heading_height = self.m.small.line_px + self.m.px(10.0);
        let mut rows: Vec<(Row, f32)> = Vec::new();
        let mut group: Option<&str> = None;
        for (index, item) in items.iter().enumerate() {
            if grouped && (index == 0 || item.group.as_deref() != group) {
                group = item.group.as_deref();
                let heading = group.unwrap_or("Other").to_owned();
                rows.push((Row::Heading(heading), heading_height));
            }
            let detail = (!item.detail.is_empty()).then(|| {
                self.layout(
                    &[TextSpan {
                        text: &item.detail,
                        face: UiFace::SANS,
                    }],
                    self.m.small,
                    Some(text_width),
                )
            });
            let height = self.m.body.line_px
                + detail.as_ref().map_or(0.0, |detail| detail.height)
                + self.m.px(12.0);
            rows.push((Row::Item(index, detail), height));
        }
        let empty_height = self.m.body.line_px + self.m.px(16.0);
        let content_height = if items.is_empty() {
            empty_height
        } else {
            rows.iter().map(|(_, height)| height).sum()
        };

        // Open upward from controls in the lower half, as the composer's do,
        // and only as tall as the room on that side.
        let upward = menu.anchor.y > panel.y + panel.height / 2.0;
        let room = if upward {
            menu.anchor.y - self.m.px(6.0) - (panel.y + self.m.px(4.0))
        } else {
            panel.bottom() - self.m.px(4.0) - (menu.anchor.bottom() + self.m.px(6.0))
        };
        let search_height = if searchable {
            self.m.control + self.m.px(8.0)
        } else {
            0.0
        };
        let list_room = (room - edge * 2.0 - search_height)
            .min(self.m.px(MENU_LIST_MAX))
            .max(self.m.px(80.0));
        let list_height = content_height.min(list_room).round();
        let height = edge * 2.0 + search_height + list_height;
        let x = menu
            .anchor
            .x
            .min(panel.right() - self.m.pad - width)
            .max(panel.x + self.m.pad);
        let y = if upward {
            menu.anchor.y - height - self.m.px(6.0)
        } else {
            menu.anchor.bottom() + self.m.px(6.0)
        }
        .max(panel.y + self.m.px(4.0));
        let bounds = Bounds::new(x.round(), y.round(), width, height.round());
        self.popover_frame(overlay, bounds);

        if searchable {
            self.menu_search(overlay, bounds, menu);
        }

        let viewport = Bounds::new(
            bounds.x,
            bounds.y + edge + search_height,
            bounds.width,
            list_height,
        );
        let max_scroll = (content_height - viewport.height).max(0.0);
        let checked = items.iter().position(|item| item.checked);
        let highlighted = if menu.fresh {
            checked.unwrap_or(0)
        } else {
            menu.highlighted
        }
        .min(items.len().saturating_sub(1));
        // Opened, a menu shows its checked item in the middle of its list.
        let scroll = if menu.fresh {
            let mut top = 0.0;
            let mut middle = None;
            for (row, row_height) in &rows {
                if let Row::Item(index, _) = row
                    && Some(*index) == checked
                {
                    middle = Some(top + row_height / 2.0);
                }
                top += row_height;
            }
            middle.map_or(0.0, |middle| {
                (middle - viewport.height / 2.0).clamp(0.0, max_scroll)
            })
        } else {
            menu.scroll.offset(max_scroll, self.input.now)
        };

        let mut list = Region::default();
        if items.is_empty() {
            self.label(
                &mut list,
                "Nothing matches",
                viewport.x + row_pad,
                viewport.y + self.m.px(4.0),
                self.m.body.line_px + self.m.px(8.0),
                viewport.width - row_pad * 2.0,
                UiFace::SANS,
                self.m.body,
                self.c.text_muted,
            );
        }
        let mut row_y = viewport.y;
        for (row, row_height) in rows {
            match row {
                Row::Heading(heading) => {
                    let text_x = viewport.x + row_pad;
                    self.label(
                        &mut list,
                        &heading,
                        text_x,
                        row_y + self.m.px(4.0),
                        self.m.small.line_px + self.m.px(4.0),
                        viewport.width - row_pad * 2.0,
                        UiFace::SANS_BOLD,
                        self.m.small,
                        self.c.text_muted,
                    );
                    list.nodes.push(RegionNode::Text(TextNode {
                        bounds: Bounds::new(text_x, row_y, viewport.width, row_height),
                        clip: None,
                        text: heading,
                        heading: true,
                    }));
                }
                Row::Item(index, detail) => {
                    let item = &items[index];
                    let row = Bounds::new(
                        viewport.x + self.m.px(4.0),
                        row_y,
                        viewport.width - self.m.px(8.0),
                        row_height,
                    );
                    self.menu_row(
                        &mut list,
                        row,
                        item,
                        detail,
                        text_width,
                        index == highlighted,
                    );
                }
            }
            row_y += row_height;
        }
        let scroll = scroll.round();
        translate(&mut list, -scroll, viewport);
        if max_scroll > 0.0 {
            self.scrollbar(&mut list, viewport, content_height, scroll);
        }
        Some(MenuList {
            region: list,
            viewport,
            height: content_height,
            scroll,
        })
    }

    /// The field a long menu's search is typed into.
    pub(super) fn menu_search(&mut self, overlay: &mut Region, bounds: Bounds, menu: &OpenMenu) {
        let field = Bounds::new(
            bounds.x + self.m.px(8.0),
            bounds.y + self.m.px(6.0),
            bounds.width - self.m.px(16.0),
            self.m.control,
        );
        overlay.prims.push(Prim::Fill {
            bounds: field,
            radius: self.m.px(6.0),
            color: self.c.surface_hover,
        });
        let icon_size = self.m.px(14.0);
        overlay.prims.push(Prim::Icon {
            icon: Icon::Search,
            center: [
                field.x + self.m.px(10.0) + icon_size / 2.0,
                field.y + field.height / 2.0,
            ],
            size: icon_size,
            color: self.c.text_muted,
        });
        let text_x = field.x + self.m.px(18.0) + icon_size;
        let text_width = field.right() - self.m.px(10.0) - text_x;
        let placeholder = search_placeholder(menu.kind);
        let typed = !menu.filter.text().is_empty();
        let drawn = self.label(
            overlay,
            if typed {
                menu.filter.text()
            } else {
                placeholder
            },
            text_x,
            field.y,
            field.height,
            text_width,
            UiFace::SANS,
            self.m.body,
            if typed {
                self.c.text_strong
            } else {
                self.c.text_muted
            },
        );
        // Typing always goes to the search, so its caret always shows.
        let caret_x = if typed { text_x + drawn } else { text_x };
        overlay.prims.push(Prim::Fill {
            bounds: Bounds::new(
                (caret_x + self.m.px(1.0)).round(),
                field.y + self.m.px(7.0),
                self.m.px(1.5).max(1.0),
                field.height - self.m.px(14.0),
            ),
            radius: 0.0,
            color: self.c.text_strong,
        });
        let label = if typed {
            format!("{placeholder}: {}", menu.filter.text())
        } else {
            placeholder.to_owned()
        };
        self.hit(
            overlay,
            field,
            Target::MenuSearch,
            HitRole::TextInput,
            label,
            false,
        );
    }

    /// One choice in a menu.
    pub(super) fn menu_row(
        &mut self,
        region: &mut Region,
        row: Bounds,
        item: &MenuItem,
        detail: Option<Arc<TextLayout>>,
        text_width: f32,
        highlighted: bool,
    ) {
        let row_pad = self.m.px(12.0);
        let target = Target::Menu(item.action.clone());
        if self.hovered(&target) || highlighted {
            region.prims.push(Prim::Fill {
                bounds: row,
                radius: self.m.px(6.0),
                color: self.c.surface_hover,
            });
        }
        if item.checked {
            region.prims.push(Prim::Icon {
                icon: Icon::Check,
                center: [
                    row.x + row_pad / 2.0 + self.m.icon / 2.0,
                    row.y + self.m.px(6.0) + self.m.body.line_px / 2.0,
                ],
                size: self.m.px(14.0),
                color: self.c.accent,
            });
        }
        let text_x = row.x + row_pad / 2.0 + self.m.icon + self.m.px(8.0);
        self.label(
            region,
            &item.text,
            text_x,
            row.y + self.m.px(6.0),
            self.m.body.line_px,
            text_width,
            UiFace::SANS,
            self.m.body,
            self.c.text_strong,
        );
        if let Some(detail) = detail {
            region.prims.push(Prim::Paragraph {
                layout: detail,
                x: text_x,
                y: row.y + self.m.px(6.0) + self.m.body.line_px,
                font_px: self.m.small.font_px,
                paint: Arc::new([SpanPaint {
                    face: UiFace::SANS,
                    color: self.c.text_muted,
                }]),
            });
        }
        let mut label = item.text.clone();
        if let Some(group) = &item.group {
            label.push_str(", ");
            label.push_str(group);
        }
        if !item.detail.is_empty() {
            label.push_str(". ");
            label.push_str(&item.detail);
        }
        self.hit(
            region,
            row,
            target,
            HitRole::MenuItem {
                checked: item.checked,
            },
            label,
            true,
        );
    }
}
