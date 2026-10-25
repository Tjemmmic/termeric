//! The panel's header, banners, scrollbar, jump-to-end control, and focus
//! ring.

use super::*;

impl Builder<'_, '_> {
    /// The part of the panel its content uses: all of it, or a centred
    /// column when the panel is much wider than lines read well.
    pub(super) fn column(&self, panel: Bounds) -> Bounds {
        let widest = self.m.px(COLUMN_WIDTH);
        if panel.width <= widest {
            return panel;
        }
        Bounds::new(
            (panel.x + (panel.width - widest) / 2.0).round(),
            panel.y,
            widest,
            panel.height,
        )
    }

    /// The button that switches between the panel beside the terminal and
    /// the panel filling the tab. Only a panel bound to a tab has one.
    pub(super) fn layout_toggle(&self) -> Option<(Icon, Target, &'static str)> {
        let state = self.input.state;
        state.tab?;
        Some(match state.layout {
            Layout::Split => (
                Icon::Expand,
                Target::SetLayout(Layout::Full),
                "Fill the tab",
            ),
            Layout::Full => (
                Icon::Terminal,
                Target::SetLayout(Layout::Split),
                "Show the terminal beside",
            ),
        })
    }

    pub(super) fn header(&mut self, region: &mut Region, panel: Bounds) -> Bounds {
        let input = self.input;
        let height = self.m.px(56.0);
        let header = Bounds::new(panel.x, panel.y, panel.width, height);
        let button = self.m.control;
        let button_y = header.y + ((height - button) / 2.0).round();
        let mut right = header.right() - self.m.px(8.0);
        let muted = self.c.text_secondary;
        match self.input.state.screen {
            Screen::Conversation => {
                let mut buttons =
                    vec![(Icon::More, Target::OpenMenu(MenuKind::More), "More actions")];
                buttons.extend(self.layout_toggle());
                buttons.extend([
                    (Icon::History, Target::ShowSessions, "All sessions"),
                    (Icon::Plus, Target::NewSession, "New session"),
                ]);
                for (icon, target, label) in buttons {
                    right -= button;
                    self.icon_button(
                        region,
                        Bounds::new(right, button_y, button, button),
                        icon,
                        target,
                        label,
                        muted,
                    );
                    right -= self.m.px(2.0);
                }
                let left = header.x + self.m.px(8.0);
                let title_area = Bounds::new(
                    left,
                    header.y + self.m.px(6.0),
                    right - left - self.m.px(4.0),
                    height - self.m.px(12.0),
                );
                let target = Target::SessionTitle;
                if self.hovered(&target) {
                    region.prims.push(Prim::Fill {
                        bounds: title_area,
                        radius: self.m.px(6.0),
                        color: self.c.surface_hover,
                    });
                }
                let Some(session) = input.active() else {
                    return header;
                };
                let title = session.title();
                let activity = session.activity();
                let text_x = title_area.x + self.m.px(24.0);
                let row = title_area.height / 2.0;
                self.status_dot(
                    region,
                    [
                        title_area.x + self.m.px(12.0),
                        title_area.y + row / 2.0 + self.m.px(2.0),
                    ],
                    self.activity_color(activity),
                );
                let chevron = self.m.px(14.0);
                let title_width = self.label(
                    region,
                    &title,
                    text_x,
                    title_area.y + self.m.px(2.0),
                    row,
                    title_area.right() - text_x - chevron - self.m.px(8.0),
                    UiFace::SANS_BOLD,
                    self.m.title,
                    self.c.text_strong,
                );
                region.prims.push(Prim::Icon {
                    icon: Icon::ChevronDown,
                    center: [
                        text_x + title_width + self.m.px(6.0) + chevron / 2.0,
                        title_area.y + self.m.px(2.0) + row / 2.0,
                    ],
                    size: chevron,
                    color: self.c.text_muted,
                });
                let meta = format!(
                    "{} · {} · {} · {}",
                    session.agent().name(),
                    self.short_path(session.cwd()),
                    session.model_label(),
                    activity.label()
                );
                self.label(
                    region,
                    &meta,
                    text_x,
                    title_area.y + row,
                    row,
                    title_area.right() - text_x - self.m.px(4.0),
                    UiFace::SANS,
                    self.m.small,
                    self.c.text_muted,
                );
                self.hit(
                    region,
                    title_area,
                    target,
                    HitRole::Button,
                    format!("{title}, {}. Show all sessions", activity.label()),
                    true,
                );
            }
            Screen::Sessions => {
                let back = Bounds::new(header.x + self.m.px(8.0), button_y, button, button);
                let can_go_back = self.input.active().is_some();
                if can_go_back {
                    self.icon_button(
                        region,
                        back,
                        Icon::Back,
                        Target::Back,
                        "Back to the conversation",
                        muted,
                    );
                }
                let title_x = if can_go_back {
                    back.right() + self.m.px(6.0)
                } else {
                    header.x + self.m.pad
                };
                let new_width = {
                    let width = self.text_width("New session", UiFace::SANS, self.m.body);
                    width + self.m.px(28.0) + self.m.icon + self.m.px(6.0)
                };
                let new_x = header.right() - self.m.px(10.0) - new_width;
                self.button(
                    region,
                    new_x,
                    button_y,
                    "New session",
                    Some(Icon::Plus),
                    ButtonKind::Secondary,
                    Target::NewSession,
                    f32::INFINITY,
                );
                self.label(
                    region,
                    "Agents",
                    title_x,
                    header.y,
                    height,
                    new_x - title_x,
                    UiFace::SANS_BOLD,
                    self.m.title,
                    self.c.text_strong,
                );
            }
            Screen::NewSession => {
                let mut buttons: Vec<(Icon, Target, &str)> =
                    self.layout_toggle().into_iter().collect();
                buttons.push((Icon::History, Target::ShowSessions, "All sessions"));
                for (index, (icon, target, label)) in buttons.into_iter().enumerate() {
                    if index > 0 {
                        right -= self.m.px(2.0);
                    }
                    right -= button;
                    self.icon_button(
                        region,
                        Bounds::new(right, button_y, button, button),
                        icon,
                        target,
                        label,
                        muted,
                    );
                }
                let title_x = if self.input.active().is_some() {
                    let back = Bounds::new(header.x + self.m.px(8.0), button_y, button, button);
                    self.icon_button(
                        region,
                        back,
                        Icon::Back,
                        Target::Back,
                        "Back to the conversation",
                        muted,
                    );
                    back.right() + self.m.px(6.0)
                } else {
                    header.x + self.m.pad
                };
                self.label(
                    region,
                    "New session",
                    title_x,
                    header.y,
                    height,
                    right - title_x,
                    UiFace::SANS_BOLD,
                    self.m.title,
                    self.c.text_strong,
                );
            }
        }
        region.prims.push(Prim::Fill {
            bounds: Bounds::new(
                panel.x,
                header.bottom() - self.m.stroke,
                panel.width,
                self.m.stroke,
            ),
            radius: 0.0,
            color: self.c.border,
        });
        header
    }

    pub(super) fn banner(
        &mut self,
        region: &mut Region,
        panel: Bounds,
        top: f32,
        banner: &Banner,
    ) -> f32 {
        let pad = self.m.pad;
        let (surface, accent, icon) = if banner.supervisor {
            (self.c.supervisor_surface, self.c.supervisor, Icon::Eye)
        } else {
            (self.c.attention_surface, self.c.attention, Icon::Warning)
        };
        let action_width = match &banner.action {
            Some((text, _)) => self.text_width(text, UiFace::SANS, self.m.body) + self.m.px(28.0),
            None => 0.0,
        };
        let text_x = panel.x + pad + self.m.icon + self.m.px(10.0);
        let text_width = panel.right() - pad - action_width - self.m.px(10.0) - text_x;
        let title_height = self.m.body.line_px;
        let detail = self.layout(
            &[TextSpan {
                text: &banner.detail,
                face: UiFace::SANS,
            }],
            self.m.small,
            Some(text_width.max(1.0)),
        );
        let height = (self.m.px(10.0) * 2.0 + title_height + detail.height)
            .max(self.m.control + self.m.px(16.0));
        let bounds = Bounds::new(
            panel.x + self.m.stroke,
            top,
            panel.width - self.m.stroke,
            height,
        );
        region.prims.push(Prim::Fill {
            bounds,
            radius: 0.0,
            color: surface,
        });
        region.prims.push(Prim::Icon {
            icon,
            center: [
                panel.x + pad + self.m.icon / 2.0,
                top + self.m.px(10.0) + title_height / 2.0,
            ],
            size: self.m.icon,
            color: accent,
        });
        let title = banner.title.clone();
        self.read_paragraph(
            region,
            &[(
                &title,
                SpanPaint {
                    face: UiFace::SANS_BOLD,
                    color: self.c.text_strong,
                },
            )],
            text_x,
            top + self.m.px(10.0),
            text_width,
            self.m.body,
            false,
        );
        let detail_text = banner.detail.clone();
        self.read_paragraph(
            region,
            &[(
                &detail_text,
                SpanPaint {
                    face: UiFace::SANS,
                    color: self.c.text_secondary,
                },
            )],
            text_x,
            top + self.m.px(10.0) + title_height,
            text_width,
            self.m.small,
            false,
        );
        if let Some((text, target)) = banner.action.clone() {
            let x = panel.right() - pad - action_width;
            self.button(
                region,
                x,
                top + ((height - self.m.control) / 2.0).round(),
                &text,
                None,
                ButtonKind::Secondary,
                target,
                f32::INFINITY,
            );
        }
        region.prims.push(Prim::Fill {
            bounds: Bounds::new(
                panel.x,
                bounds.bottom() - self.m.stroke,
                panel.width,
                self.m.stroke,
            ),
            radius: 0.0,
            color: self.c.border,
        });
        bounds.bottom()
    }

    /// Draw a scrollbar for `content` scrolled by `offset` in `viewport`,
    /// and return its geometry for dragging.
    pub(super) fn scrollbar(
        &self,
        region: &mut Region,
        viewport: Bounds,
        content: f32,
        offset: f32,
    ) -> ScrollbarGeometry {
        let width = self.m.px(4.0);
        let track = viewport.inset(0.0, self.m.px(4.0));
        let length = (track.height * viewport.height / content)
            .max(self.m.px(24.0))
            .min(track.height);
        let travel = track.height - length;
        let max = content - viewport.height;
        let y = track.y
            + if max > 0.0 {
                travel * offset / max
            } else {
                0.0
            };
        let thumb = Bounds::new(
            viewport.right() - width - self.m.px(3.0),
            y.round(),
            width,
            length.round(),
        );
        region.prims.push(Prim::Fill {
            bounds: thumb,
            radius: width / 2.0,
            color: scale_alpha(self.c.text_muted, 0.45),
        });
        // The thumb is thin; a press anywhere in the strip beside the edge
        // takes it.
        let grab_width = self.m.px(14.0);
        ScrollbarGeometry {
            track,
            thumb,
            grab: Bounds::new(
                viewport.right() - grab_width,
                track.y,
                grab_width,
                track.height,
            ),
        }
    }

    pub(super) fn jump_to_end(&mut self, region: &mut Region, viewport: Bounds) {
        let size = self.m.px(32.0);
        let bounds = Bounds::new(
            (viewport.x + (viewport.width - size) / 2.0).round(),
            viewport.bottom() - size - self.m.px(10.0),
            size,
            size,
        );
        self.card(
            region,
            bounds,
            size / 2.0,
            self.c.surface_hover,
            self.c.border,
        );
        region.prims.push(Prim::Icon {
            icon: Icon::ChevronDown,
            center: bounds.center(),
            size: self.m.icon,
            color: self.c.text_strong,
        });
        self.hit(
            region,
            bounds,
            Target::ScrollToEnd,
            HitRole::Button,
            "Jump to the latest message",
            false,
        );
    }

    /// Draws the keyboard focus ring around the focused target, wherever
    /// it lives.
    pub(super) fn focus_ring(
        &self,
        base: &mut Region,
        feed: &mut Region,
        overlay: &mut Region,
        viewport: Bounds,
    ) {
        if !self.input.state.has_keyboard() || !self.input.state.focus_visible {
            return;
        }
        let Focus::Target(target) = &self.input.state.focus else {
            return;
        };
        let ring = |region: &mut Region, bounds: Bounds, radius: f32| {
            let grow = self.m.px(2.0);
            region.prims.push(Prim::Outline {
                bounds: Bounds::new(
                    bounds.x - grow,
                    bounds.y - grow,
                    bounds.width + grow * 2.0,
                    bounds.height + grow * 2.0,
                ),
                radius: radius + grow,
                stroke: self.m.px(2.0),
                color: self.c.focus_ring,
            });
        };
        let radius = |bounds: Bounds| (bounds.height / 2.0).min(self.m.radius);
        if let Some(hit) = overlay.hits.iter().find(|hit| &hit.target == target) {
            let bounds = hit.bounds;
            ring(overlay, bounds, radius(bounds));
        } else if let Some(hit) = feed.hits.iter().find(|hit| &hit.target == target) {
            let bounds = hit.bounds;
            if bounds.intersect(viewport).is_some() {
                ring(feed, bounds, radius(bounds));
            }
        } else if let Some(hit) = base.hits.iter().find(|hit| &hit.target == target) {
            let bounds = hit.bounds;
            ring(base, bounds, radius(bounds));
        }
    }
}
