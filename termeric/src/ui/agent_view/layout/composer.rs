//! The composer: the message field, its caret and selection, and the
//! controls around it.

use super::*;
use crate::ui::text::plural;

/// Fewest and most text lines the composer shows before scrolling.
pub(super) const COMPOSER_MIN_LINES: f32 = 2.0;

pub(super) const COMPOSER_MAX_LINES: f32 = 10.0;

impl Builder<'_, '_> {
    /// Lays the composer out at the bottom of the panel and returns its top.
    pub(super) fn composer(&mut self, region: &mut Region, panel: Bounds) -> f32 {
        let input = self.input;
        let state = input.state;
        let pad = self.m.px(14.0);
        let inner_x = panel.x + pad;
        let inner_width = panel.width - pad * 2.0;
        let conversation = match state.screen {
            Screen::Conversation => state.active.and_then(|id| state.conversation(id)),
            _ => None,
        };
        let session = match state.screen {
            Screen::Conversation => input.active(),
            _ => None,
        };
        let composer = match state.screen {
            Screen::Conversation => conversation.map(|view| &view.composer),
            _ => Some(&state.new_session.composer),
        };
        let text = composer.map_or("", |composer| composer.text()).to_owned();
        let working = session.is_some_and(|session| session.transcript().turn_active());

        // Chips: attachments first, then suggestions.
        struct ChipSpec {
            text: String,
            icon: Icon,
            attached: bool,
            target: Target,
            label: String,
        }
        let mut chips = Vec::new();
        if let Some(session) = session {
            for (index, queued) in session.queued().enumerate() {
                let first_line = queued.lines().next().unwrap_or_default();
                chips.push(ChipSpec {
                    text: format!("Queued · {first_line}"),
                    icon: Icon::History,
                    attached: true,
                    target: Target::CancelQueued(index),
                    label: format!("Cancel the queued message {first_line}"),
                });
            }
        }
        if let Some(view) = conversation {
            for (index, attachment) in view.attachments.iter().enumerate() {
                chips.push(ChipSpec {
                    text: attachment.label.clone(),
                    icon: Icon::Terminal,
                    attached: true,
                    target: Target::RemoveAttachment(index),
                    label: format!("Remove attachment {}", attachment.label),
                });
            }
        }
        if matches!(state.screen, Screen::Conversation | Screen::NewSession) {
            let attached = |prefix: &str| {
                conversation.is_some_and(|view| {
                    view.attachments
                        .iter()
                        .any(|attachment| attachment.label.starts_with(prefix))
                })
            };
            if let Some(lines) = self.input.suggestions.selection_lines
                && !attached("Terminal selection")
            {
                chips.push(ChipSpec {
                    text: format!("Terminal selection · {}", plural(lines, "line", "lines")),
                    icon: Icon::Plus,
                    attached: false,
                    target: Target::AttachSelection,
                    label: "Attach the terminal selection".into(),
                });
            }
            if let Some((command, lines, failed)) = &self.input.suggestions.last_command
                && !attached(command)
            {
                chips.push(ChipSpec {
                    text: format!(
                        "{command} · {}{}",
                        plural(*lines, "line", "lines"),
                        if *failed { ", failed" } else { "" }
                    ),
                    icon: Icon::Plus,
                    attached: false,
                    target: Target::AttachLastCommand,
                    label: format!("Attach the output of {command}"),
                });
            }
        }

        // Measure everything from the bottom up.
        let font = self.m.body;
        let text_width = inner_width - self.m.px(24.0);
        let text_layout = composer_layout(&text, font, text_width, &mut *self.measure);
        let lines = (text_layout.height / font.line_px)
            .round()
            .clamp(COMPOSER_MIN_LINES, COMPOSER_MAX_LINES);
        let text_height = lines * font.line_px;
        let toolbar_height = self.m.control + self.m.px(10.0);
        let box_height = self.m.px(10.0) + text_height + toolbar_height;
        let chip_row = if chips.is_empty() {
            0.0
        } else {
            self.m.chip + self.m.px(8.0)
        };
        let bottom = panel.bottom() - pad;
        let box_top = bottom - box_height;
        let top = box_top - chip_row - self.m.px(10.0);

        region.prims.push(Prim::Fill {
            bounds: Bounds::new(
                panel.x + self.m.stroke,
                top,
                panel.width - self.m.stroke,
                panel.bottom() - top,
            ),
            radius: 0.0,
            color: self.c.panel,
        });

        // Chips wrap onto one row; later ones are dropped if they do not fit.
        let mut chip_x = inner_x;
        for chip in chips {
            let remaining = inner_x + inner_width - chip_x;
            if remaining < self.m.px(80.0) {
                break;
            }
            let width = self.chip(
                region,
                chip_x,
                box_top - chip_row,
                &chip.text,
                Some(chip.icon),
                chip.attached.then_some(Icon::Close),
                chip.attached,
                !chip.attached,
                remaining,
                Some((chip.target, &chip.label)),
            );
            chip_x += width + self.m.px(6.0);
        }

        let box_bounds = Bounds::new(inner_x, box_top, inner_width, box_height);
        self.composer_box = Some(box_bounds);
        let focused = state.has_keyboard() && state.focus == Focus::Composer;
        region.prims.push(Prim::Fill {
            bounds: box_bounds,
            radius: self.m.px(10.0),
            color: self.c.surface,
        });
        region.prims.push(Prim::Outline {
            bounds: box_bounds,
            radius: self.m.px(10.0),
            stroke: self.m.stroke,
            color: if focused {
                scale_alpha(self.c.accent, 0.8)
            } else {
                self.c.border
            },
        });

        let text_origin = [inner_x + self.m.px(12.0), box_top + self.m.px(10.0)];
        let text_clip = Bounds::new(text_origin[0], text_origin[1], text_width, text_height);
        if text.is_empty() {
            let placeholder =
                self.placeholder(conversation.and_then(|view| view.placeholder), working);
            self.label(
                region,
                &placeholder,
                text_origin[0],
                text_origin[1],
                font.line_px,
                text_width,
                UiFace::SANS,
                font,
                self.c.text_muted,
            );
        }
        // Keep the caret's line visible when the text is taller than the box.
        let mut text_y = text_origin[1];
        let mut caret = None;
        if let Some(composer) = composer {
            let measure = &mut *self.measure;
            let mut width = |prefix: &str| measure.width(prefix, UiFace::SANS, font.font_px);
            let (line, x) = composer.caret_point(&text_layout, &mut width);
            let caret_top = line as f32 * font.line_px;
            if caret_top + font.line_px > text_height {
                text_y -= caret_top + font.line_px - text_height;
            }
            if let Some(selection) = composer.selection() {
                self.selection_rects(
                    region,
                    &text_layout,
                    &text,
                    selection,
                    [text_origin[0], text_y],
                    text_clip,
                    font,
                );
            }
            if !text.is_empty() {
                region.prims.push(Prim::Paragraph {
                    layout: Arc::clone(&text_layout),
                    x: text_origin[0],
                    y: text_y,
                    font_px: font.font_px,
                    paint: Arc::new([SpanPaint {
                        face: UiFace::SANS,
                        color: self.c.text_strong,
                    }]),
                });
            }
            let caret_bounds = Bounds::new(
                (text_origin[0] + x).round(),
                (text_y + caret_top + self.m.px(2.0)).round(),
                self.m.px(1.5).max(1.0),
                font.line_px - self.m.px(4.0),
            );
            caret = Some(Caret {
                bounds: caret_bounds,
            });
            if focused && caret_visible(state.caret_moved, self.input.now) {
                region.prims.push(Prim::Fill {
                    bounds: caret_bounds,
                    radius: 0.0,
                    color: self.c.text_strong,
                });
            }
        }
        self.caret = caret;
        self.composer_geometry = Some(ComposerGeometry {
            origin: [text_origin[0], text_y],
            wrap_width: text_width,
            size: font,
        });
        self.hit(
            region,
            Bounds::new(inner_x, box_top, inner_width, box_height - toolbar_height),
            Target::Composer,
            HitRole::TextInput,
            format!("Message {}", self.input.agent().short_name()),
            true,
        );

        // Toolbar.
        let toolbar_y = box_bounds.bottom() - toolbar_height + self.m.px(2.0);
        let mut x = inner_x + self.m.px(6.0);
        let model_label = model_name(&offered_models(self.input), &current_model(self.input));
        let (modes, current) = mode_state(self.input);
        let choice = modes.iter().find(|mode| mode.id == current);
        let approving = auto_approve(self.input);
        let mode_icon = match choice {
            _ if approving => Icon::Warning,
            Some(mode) if mode.plans => Icon::Plan,
            Some(mode) if mode.unrestricted => Icon::Warning,
            _ => Icon::Shield,
        };
        let mode_label = choice.map_or(current.as_str(), |mode| mode.label.as_str());
        let (mode_text, mode_name) = if approving {
            (
                format!("{mode_label} · Auto-approve"),
                format!("Permissions: {mode_label}, approving everything"),
            )
        } else {
            (mode_label.to_owned(), format!("Permissions: {mode_label}"))
        };
        let model_name = format!("Model: {model_label}");
        let efforts = offered_efforts(self.input);
        let effort_label = (!efforts.is_empty()).then(|| {
            let current = current_effort(self.input);
            efforts
                .iter()
                .find(|effort| Some(&effort.value) == current.as_ref())
                .map_or_else(
                    || current.clone().unwrap_or_else(|| "Effort".to_owned()),
                    |effort| effort.label.clone(),
                )
        });
        // The chips share the row left of the send button and the context
        // ring, whose label is at most "100%".
        let send_size = self.m.control;
        let ring = if session
            .and_then(|session| session.transcript().context())
            .is_some()
        {
            self.text_width("100%", UiFace::SANS, self.m.small) + self.m.px(28.0)
        } else {
            0.0
        };
        let chips_end = box_bounds.right() - self.m.px(14.0) - send_size - ring;
        let gap = self.m.px(2.0);
        let effort_width = effort_label.as_ref().map_or(0.0, |label| {
            self.chip_width(label, false, true)
                .min(inner_width / 5.0)
                .ceil()
        });
        // Approving everything is said in full; the model's name gives way.
        let mode_share = if efforts.is_empty() || approving {
            2.0
        } else {
            3.0
        };
        let width = self.chip(
            region,
            x,
            toolbar_y,
            &mode_text,
            Some(mode_icon),
            Some(Icon::ChevronDown),
            false,
            false,
            inner_width / mode_share,
            Some((Target::OpenMenu(MenuKind::Mode), &mode_name)),
        );
        x += width + gap;
        let effort_room = if effort_label.is_some() {
            effort_width + gap
        } else {
            0.0
        };
        let model_room = (chips_end - x - effort_room).max(self.m.px(60.0));
        let width = self.chip(
            region,
            x,
            toolbar_y,
            &model_label,
            None,
            Some(Icon::ChevronDown),
            false,
            false,
            model_room,
            Some((Target::OpenMenu(MenuKind::Model), &model_name)),
        );
        if let Some(effort_label) = effort_label {
            x += width + gap;
            let effort_name = format!("Effort: {effort_label}");
            self.chip(
                region,
                x,
                toolbar_y,
                &effort_label,
                None,
                Some(Icon::ChevronDown),
                false,
                false,
                effort_width,
                Some((Target::OpenMenu(MenuKind::Effort), &effort_name)),
            );
        }
        let send = Bounds::new(
            box_bounds.right() - self.m.px(6.0) - send_size,
            toolbar_y - self.m.px(2.0),
            send_size,
            send_size,
        );
        if let Some(usage) = session.and_then(|session| session.transcript().context()) {
            let percent = (usage.fraction() * 100.0).round() as u32;
            let label = format!("{percent}%");
            let label_width = self.text_width(&label, UiFace::SANS, self.m.small);
            let ring_radius = self.m.px(6.0);
            let label_x = send.x - self.m.px(10.0) - label_width;
            let ring_center = [
                label_x - self.m.px(6.0) - ring_radius,
                send.y + send.height / 2.0,
            ];
            region.prims.push(Prim::Arc {
                center: ring_center,
                radius: ring_radius,
                thickness: self.m.px(2.0),
                fraction: 1.0,
                color: self.c.border,
            });
            region.prims.push(Prim::Arc {
                center: ring_center,
                radius: ring_radius,
                thickness: self.m.px(2.0),
                fraction: usage.fraction(),
                color: if usage.fraction() > 0.85 {
                    self.c.attention
                } else {
                    self.c.text_secondary
                },
            });
            self.label(
                region,
                &label,
                label_x,
                send.y,
                send.height,
                label_width + 2.0,
                UiFace::SANS,
                self.m.small,
                self.c.text_muted,
            );
            region.nodes.push(RegionNode::Text(TextNode {
                bounds: Bounds::new(
                    ring_center[0] - ring_radius,
                    send.y,
                    send.x - ring_center[0] + ring_radius,
                    send.height,
                ),
                clip: None,
                text: format!("{percent} percent of the context used"),
                heading: false,
            }));
        }
        let (target, icon, fill, label) = if working && text.trim().is_empty() {
            (Target::Stop, Icon::Stop, self.c.text_strong, "Stop")
        } else {
            let label = if state.screen == Screen::NewSession {
                "Start"
            } else {
                "Send"
            };
            (
                if state.screen == Screen::NewSession {
                    Target::StartSession
                } else {
                    Target::Send
                },
                Icon::Send,
                self.c.accent,
                label,
            )
        };
        let enabled = target == Target::Stop
            || !text.trim().is_empty()
            || conversation.is_some_and(|view| !view.attachments.is_empty());
        let fill = if enabled {
            fill
        } else {
            scale_alpha(fill, 0.45)
        };
        region.prims.push(Prim::Fill {
            bounds: send,
            radius: send_size / 2.0,
            color: fill,
        });
        if self.hovered(&target) && enabled {
            region.prims.push(Prim::Fill {
                bounds: send,
                radius: send_size / 2.0,
                color: [1.0, 1.0, 1.0, 0.12],
            });
        }
        region.prims.push(Prim::Icon {
            icon,
            center: send.center(),
            size: self.m.px(15.0),
            color: self.c.on_accent,
        });
        self.hit(region, send, target, HitRole::Button, label, true);
        top
    }

    pub(super) fn placeholder(&self, hint: Option<&'static str>, working: bool) -> String {
        if let Some(hint) = hint {
            return hint.to_owned();
        }
        let agent = self.input.agent().short_name();
        match self.input.state.screen {
            Screen::NewSession => format!("What should {agent} work on?"),
            _ if working => format!("Queue a message for when {agent} finishes…"),
            _ => format!("Ask {agent} to change something…"),
        }
    }

    pub(super) fn selection_rects(
        &mut self,
        region: &mut Region,
        layout: &TextLayout,
        text: &str,
        selection: std::ops::Range<usize>,
        origin: [f32; 2],
        clip: Bounds,
        font: TextSize,
    ) {
        for line in &layout.lines {
            for run in &line.runs {
                let start = selection.start.max(run.start);
                let end = selection.end.min(run.start + run.text.len());
                if start >= end {
                    continue;
                }
                let before = self.text_width(&text[run.start..start], UiFace::SANS, font);
                let within = self.text_width(&text[start..end], UiFace::SANS, font);
                let bounds = Bounds::new(
                    origin[0] + run.x + before,
                    origin[1] + line.top,
                    within.max(self.m.px(3.0)),
                    line.height,
                );
                if let Some(bounds) = bounds.intersect(clip) {
                    region.prims.push(Prim::Fill {
                        bounds,
                        radius: 0.0,
                        color: self.c.selection,
                    });
                }
            }
        }
    }
}

/// The caret blinks on a one-second cycle, starting visible after it moves.
pub(super) fn caret_visible(moved: Instant, now: Instant) -> bool {
    let elapsed = now.saturating_duration_since(moved);
    elapsed < super::super::CARET_HOLD
        || (elapsed.as_millis() / super::super::CARET_PHASE.as_millis()).is_multiple_of(2)
}
