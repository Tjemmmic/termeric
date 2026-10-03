//! The sessions page and the new-session page.

use crate::agent_view::transcript::{Activity, Item};
use crate::agent_view::{present, session::AgentSession};
use crate::ui::color::with_alpha;
use crate::ui::text_layout::UiFace;

use super::super::scene::{Bounds, HitRole, Icon, MenuKind, Prim, SpanPaint, Target, TextNode};
use super::feed::duration;
use super::{AgentAvailability, Builder, ButtonKind, Region, RegionNode};

/// Past conversations offered on the new-session page.
const RECENT_PAST: usize = 4;

impl Builder<'_, '_> {
    pub(super) fn sessions_page(&mut self, viewport: Bounds) -> (Region, f32) {
        let input = self.input;
        let mut region = Region::default();
        let x = viewport.x + self.m.px(10.0);
        let width = viewport.width - self.m.px(20.0);
        let mut y = self.m.px(12.0);

        // Sessions started ahead of a first message hold no conversation.
        let conversations: Vec<&AgentSession> = input
            .sessions
            .iter()
            .copied()
            .filter(|session| !session.is_blank())
            .collect();
        let groups: [(&str, Vec<&AgentSession>); 3] = [
            (
                "Needs you",
                conversations
                    .iter()
                    .copied()
                    .filter(|session| session.activity() == Activity::NeedsYou)
                    .collect(),
            ),
            (
                "Working",
                conversations
                    .iter()
                    .copied()
                    .filter(|session| {
                        matches!(session.activity(), Activity::Working | Activity::Starting)
                    })
                    .collect(),
            ),
            (
                "Recent",
                conversations
                    .iter()
                    .copied()
                    .filter(|session| {
                        !matches!(
                            session.activity(),
                            Activity::NeedsYou | Activity::Working | Activity::Starting
                        )
                    })
                    .collect(),
            ),
        ];
        let mut any = false;
        for (title, sessions) in groups {
            if sessions.is_empty() {
                continue;
            }
            any = true;
            y = self.section_title(&mut region, title, x, y, width, title == "Needs you");
            for session in sessions {
                y += self.session_row(&mut region, session, x, y, width);
            }
            y += self.m.px(8.0);
        }
        if !input.terminal_agents.is_empty() {
            any = true;
            y = self.section_title(&mut region, "In terminals", x, y, width, false);
            for agent in input.terminal_agents {
                let dot = if agent.needs_you {
                    self.c.attention
                } else {
                    self.c.text_muted
                };
                let target = Target::OpenTerminalAgent(agent.identity);
                y += self.list_row(
                    &mut region,
                    target,
                    x,
                    y,
                    width,
                    dot,
                    &agent.name,
                    "",
                    &agent.summary,
                    &agent.place,
                );
            }
            y += self.m.px(8.0);
        }
        if let Some(first) = input.past.first() {
            any = true;
            let heading = format!("Earlier {} conversations", first.agent.name());
            y = self.section_title(&mut region, &heading, x, y, width, false);
            for past in input.past {
                let target = Target::ResumePast {
                    agent: past.agent,
                    id: past.id.clone(),
                };
                y += self.list_row(
                    &mut region,
                    target,
                    x,
                    y,
                    width,
                    [0.0; 4],
                    &past.title,
                    &past.when,
                    "Continue this conversation",
                    "",
                );
            }
            y += self.m.px(8.0);
        }
        if !any {
            let text =
                "No agent sessions yet. Start one to have an agent work beside your terminals.";
            y += self.m.px(16.0);
            y += self.read_paragraph(
                &mut region,
                &[(
                    text,
                    SpanPaint {
                        face: UiFace::SANS,
                        color: self.c.text_muted,
                    },
                )],
                x + self.m.px(6.0),
                y,
                width - self.m.px(12.0),
                self.m.body,
                false,
            );
            y += self.m.px(12.0);
            self.button(
                &mut region,
                x + self.m.px(6.0),
                y,
                "New session",
                Some(Icon::Plus),
                ButtonKind::Primary,
                Target::NewSession,
                f32::INFINITY,
            );
            y += self.m.control + self.m.px(12.0);
        }
        if let Some(supervisor) = input.supervisor.clone() {
            y += self.m.px(4.0);
            y += self.supervisor_card(&mut region, &supervisor, x, y, width);
        }
        (region, y + self.m.px(12.0))
    }

    fn section_title(
        &mut self,
        region: &mut Region,
        title: &str,
        x: f32,
        y: f32,
        width: f32,
        attention: bool,
    ) -> f32 {
        let height = self.m.px(24.0);
        let color = if attention {
            self.c.attention
        } else {
            self.c.text_muted
        };
        self.label(
            region,
            title,
            x + self.m.px(10.0),
            y,
            height,
            width,
            UiFace::SANS_BOLD,
            self.m.small,
            color,
        );
        region.nodes.push(RegionNode::Text(TextNode {
            bounds: Bounds::new(x, y, width, height),
            clip: None,
            text: title.to_owned(),
            heading: true,
        }));
        y + height
    }

    fn session_row(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let activity = session.activity();
        let summary = session_summary(session);
        let place = format!(
            "{} · {}",
            self.short_path(session.cwd()),
            session.model_label()
        );
        let when = crate::ui::age::compact_age(session.last_change().elapsed());
        let target = Target::OpenSession(session.id());
        let current = self.input.state.active == Some(session.id());
        let height = self.list_row(
            region,
            target,
            x,
            y,
            width,
            self.activity_color(activity),
            &session.title(),
            &when,
            &summary,
            &place,
        );
        if current {
            region.prims.insert(
                0,
                Prim::Fill {
                    bounds: Bounds::new(x, y, width, height),
                    radius: self.m.px(8.0),
                    color: self.c.surface,
                },
            );
        }
        height
    }

    /// A two- or three-line list row. Returns its height.
    fn list_row(
        &mut self,
        region: &mut Region,
        target: Target,
        x: f32,
        y: f32,
        width: f32,
        dot: [f32; 4],
        title: &str,
        when: &str,
        summary: &str,
        place: &str,
    ) -> f32 {
        let pad = self.m.px(10.0);
        let line = self.m.body.line_px;
        let small = self.m.small.line_px;
        let height = pad * 2.0
            + line
            + if summary.is_empty() { 0.0 } else { small }
            + if place.is_empty() { 0.0 } else { small };
        let bounds = Bounds::new(x, y, width, height);
        if self.hovered(&target) {
            region.prims.push(Prim::Fill {
                bounds,
                radius: self.m.px(8.0),
                color: self.c.surface_hover,
            });
        }
        if dot[3] > 0.0 {
            self.status_dot(
                region,
                [x + pad + self.m.px(4.0), y + pad + line / 2.0],
                dot,
            );
        }
        let text_x = x + pad + self.m.px(18.0);
        let when_width = if when.is_empty() {
            0.0
        } else {
            self.text_width(when, UiFace::SANS, self.m.small)
        };
        let text_right = x + width - pad - when_width - self.m.px(8.0);
        self.label(
            region,
            title,
            text_x,
            y + pad,
            line,
            text_right - text_x,
            UiFace::SANS_BOLD,
            self.m.body,
            self.c.text_strong,
        );
        if !when.is_empty() {
            self.label(
                region,
                when,
                x + width - pad - when_width,
                y + pad,
                line,
                when_width + 2.0,
                UiFace::SANS,
                self.m.small,
                self.c.text_muted,
            );
        }
        let mut line_y = y + pad + line;
        if !summary.is_empty() {
            self.label(
                region,
                summary,
                text_x,
                line_y,
                small,
                x + width - pad - text_x,
                UiFace::SANS,
                self.m.small,
                self.c.text_secondary,
            );
            line_y += small;
        }
        if !place.is_empty() {
            self.label(
                region,
                place,
                text_x,
                line_y,
                small,
                x + width - pad - text_x,
                UiFace::SANS,
                self.m.small,
                self.c.text_muted,
            );
        }
        let spoken = [title, summary, place, when]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        self.hit(region, bounds, target, HitRole::ListItem, spoken, true);
        height
    }

    fn supervisor_card(
        &mut self,
        region: &mut Region,
        supervisor: &super::SupervisorCard,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let pad = self.m.px(12.0);
        let inner = width - pad * 2.0;
        let mut body = Region::default();
        let mut cursor = y + pad;
        body.prims.push(Prim::Icon {
            icon: Icon::Eye,
            center: [x + pad + self.m.px(8.0), cursor + self.m.body.line_px / 2.0],
            size: self.m.icon,
            color: self.c.supervisor,
        });
        let title_width = self.label(
            &mut body,
            "Supervisor",
            x + pad + self.m.px(24.0),
            cursor,
            self.m.body.line_px,
            inner / 2.0,
            UiFace::SANS_BOLD,
            self.m.body,
            self.c.text_strong,
        );
        self.label(
            &mut body,
            &supervisor.status,
            x + pad + self.m.px(34.0) + title_width,
            cursor,
            self.m.body.line_px,
            inner - title_width - self.m.px(34.0),
            UiFace::SANS,
            self.m.small,
            self.c.supervisor,
        );
        body.nodes.push(RegionNode::Text(TextNode {
            bounds: Bounds::new(x, cursor, width, self.m.body.line_px),
            clip: None,
            text: format!("Supervisor, {}", supervisor.status),
            heading: true,
        }));
        cursor += self.m.body.line_px + self.m.px(6.0);
        if let Some(latest) = &supervisor.latest {
            cursor += self.read_paragraph(
                &mut body,
                &[(
                    latest,
                    SpanPaint {
                        face: UiFace::SANS,
                        color: self.c.text_secondary,
                    },
                )],
                x + pad,
                cursor,
                inner,
                self.m.small,
                false,
            );
            cursor += self.m.px(8.0);
        }
        let text = if supervisor.held {
            "Hand back"
        } else {
            "Take over"
        };
        self.button(
            &mut body,
            x + pad,
            cursor,
            text,
            None,
            ButtonKind::Secondary,
            Target::ToggleSupervisor,
            f32::INFINITY,
        );
        cursor += self.m.control + pad;
        let bounds = Bounds::new(x, y, width, cursor - y);
        region.prims.push(Prim::Fill {
            bounds,
            radius: self.m.radius,
            color: self.c.surface,
        });
        region.prims.push(Prim::Outline {
            bounds,
            radius: self.m.radius,
            stroke: self.m.stroke,
            color: self.c.border,
        });
        super::merge(region, body);
        bounds.height
    }

    pub(super) fn new_session_page(&mut self, viewport: Bounds) -> (Region, f32) {
        let input = self.input;
        let info = &input.new_session;
        let mut region = Region::default();
        let x = viewport.x + self.m.pad;
        let width = viewport.width - self.m.pad * 2.0;
        let mut y = self.m.pad;

        y = self.field_label(&mut region, "Agent", x, y, width);
        for (index, (agent, availability)) in info.agents.iter().enumerate() {
            if index > 0 {
                y += self.m.px(8.0);
            }
            let name = agent.name();
            let status = match availability {
                AgentAvailability::Ready => "Ready".to_owned(),
                AgentAvailability::Looking => format!("Looking for {name}…"),
                AgentAvailability::NotInstalled => {
                    format!("Not installed. Install {name}, then try again.")
                }
                AgentAvailability::Disabled => {
                    "Turned off in the agent settings (Settings, Agents)".to_owned()
                }
            };
            let unavailable = matches!(
                availability,
                AgentAvailability::NotInstalled | AgentAvailability::Disabled
            );
            let action = if unavailable {
                CardAction::None
            } else {
                CardAction::Choose(Target::ChooseAgent(*agent))
            };
            y += self.option_card(
                &mut region,
                x,
                y,
                width,
                name,
                &status,
                *agent == info.agent,
                unavailable,
                action,
            );
        }
        y += self.m.px(18.0);

        y = self.field_label(&mut region, "Folder", x, y, width);
        let folder = self.short_path(&info.cwd);
        y += self.option_card(
            &mut region,
            x,
            y,
            width,
            &folder,
            &info.cwd_source,
            false,
            false,
            CardAction::Menu(Target::OpenMenu(MenuKind::Folder)),
        );
        y += self.m.px(18.0);

        let recent: Vec<_> = input.past.iter().take(RECENT_PAST).collect();
        if !recent.is_empty() {
            y = self.field_label(&mut region, "Continue a conversation", x, y, width);
            for past in recent {
                let target = Target::ResumePast {
                    agent: past.agent,
                    id: past.id.clone(),
                };
                y += self.list_row(
                    &mut region,
                    target,
                    x - self.m.px(10.0),
                    y,
                    width + self.m.px(20.0),
                    [0.0; 4],
                    &past.title,
                    &past.when,
                    "",
                    "",
                );
            }
            y += self.m.px(12.0);
        }

        let link = format!("Open {} in a terminal instead", info.agent.short_name());
        let link = link.as_str();
        let link_width = self.text_width(link, UiFace::SANS, self.m.small);
        let link_bounds = Bounds::new(
            (x + (width - link_width) / 2.0).round() - self.m.px(6.0),
            y,
            link_width + self.m.px(12.0),
            self.m.control,
        );
        let hovered = self.hovered(&Target::OpenInTerminal);
        self.label(
            &mut region,
            link,
            link_bounds.x + self.m.px(6.0),
            y,
            self.m.control,
            link_width + 2.0,
            UiFace::SANS,
            self.m.small,
            if hovered {
                self.c.text_strong
            } else {
                self.c.text_secondary
            },
        );
        self.hit(
            &mut region,
            link_bounds,
            Target::OpenInTerminal,
            HitRole::Button,
            link,
            true,
        );
        y += self.m.control;
        (region, y + self.m.pad)
    }

    fn field_label(&mut self, region: &mut Region, text: &str, x: f32, y: f32, width: f32) -> f32 {
        let height = self.m.px(22.0);
        self.label(
            region,
            text,
            x,
            y,
            height,
            width,
            UiFace::SANS_BOLD,
            self.m.small,
            self.c.text_muted,
        );
        region.nodes.push(RegionNode::Text(TextNode {
            bounds: Bounds::new(x, y, width, height),
            clip: None,
            text: text.to_owned(),
            heading: true,
        }));
        y + height + self.m.px(4.0)
    }

    /// A choice card: a radio mark, a title, and a description.
    fn option_card(
        &mut self,
        region: &mut Region,
        x: f32,
        y: f32,
        width: f32,
        title: &str,
        detail: &str,
        selected: bool,
        disabled: bool,
        action: CardAction,
    ) -> f32 {
        let pad = self.m.px(12.0);
        let marker = self.m.px(16.0);
        let menu = matches!(action, CardAction::Menu(_));
        let text_x = x
            + pad
            + if menu {
                self.m.px(24.0)
            } else {
                marker + self.m.px(12.0)
            };
        let text_width = x + width - pad - text_x - if menu { self.m.px(20.0) } else { 0.0 };
        let mut body = Region::default();
        let title_face = if menu {
            UiFace::MONO
        } else {
            UiFace::SANS_BOLD
        };
        let title_size = if menu { self.m.mono } else { self.m.body };
        let title_color = if disabled {
            self.c.text_muted
        } else {
            self.c.text_strong
        };
        let title_height = self
            .paragraph(
                &mut body,
                &[(
                    title,
                    SpanPaint {
                        face: title_face,
                        color: title_color,
                    },
                )],
                text_x,
                y + pad,
                text_width,
                title_size,
            )
            .height;
        let detail_height = self
            .paragraph(
                &mut body,
                &[(
                    detail,
                    SpanPaint {
                        face: UiFace::SANS,
                        color: self.c.text_muted,
                    },
                )],
                text_x,
                y + pad + title_height,
                text_width,
                self.m.small,
            )
            .height;
        let bounds = Bounds::new(
            x,
            y,
            width,
            (pad * 2.0 + title_height + detail_height).round(),
        );
        let hovered = match &action {
            CardAction::Menu(target) | CardAction::Choose(target) => self.hovered(target),
            CardAction::None => false,
        };
        if selected {
            region.prims.push(Prim::Fill {
                bounds,
                radius: self.m.radius,
                color: with_alpha(self.c.accent, 0.08),
            });
        } else if hovered {
            region.prims.push(Prim::Fill {
                bounds,
                radius: self.m.radius,
                color: self.c.surface_hover,
            });
        }
        region.prims.push(Prim::Outline {
            bounds,
            radius: self.m.radius,
            stroke: self.m.stroke,
            color: if selected {
                self.c.accent
            } else {
                self.c.border
            },
        });
        let marker_center = [x + pad + marker / 2.0, y + pad + title_size.line_px / 2.0];
        if menu {
            region.prims.push(Prim::Icon {
                icon: Icon::Folder,
                center: marker_center,
                size: self.m.px(15.0),
                color: self.c.text_secondary,
            });
            region.prims.push(Prim::Icon {
                icon: Icon::ChevronDown,
                center: [
                    x + width - pad - self.m.px(7.0),
                    bounds.y + bounds.height / 2.0,
                ],
                size: self.m.px(14.0),
                color: self.c.text_muted,
            });
        } else {
            let marker_bounds = Bounds::new(
                marker_center[0] - marker / 2.0,
                marker_center[1] - marker / 2.0,
                marker,
                marker,
            );
            let color = if selected {
                self.c.accent
            } else {
                self.c.border
            };
            region.prims.push(Prim::Outline {
                bounds: marker_bounds,
                radius: marker / 2.0,
                stroke: self.m.px(1.5),
                color,
            });
            if selected {
                region.prims.push(Prim::Fill {
                    bounds: marker_bounds.inset(self.m.px(4.0), self.m.px(4.0)),
                    radius: marker,
                    color,
                });
            }
        }
        super::merge(region, body);
        let spoken = format!("{title}. {detail}");
        match action {
            CardAction::Menu(target) => {
                self.hit(region, bounds, target, HitRole::Button, spoken, true)
            }
            CardAction::Choose(target) => self.hit(
                region,
                bounds,
                target,
                HitRole::Option { selected },
                spoken,
                true,
            ),
            CardAction::None => region.nodes.push(RegionNode::Text(TextNode {
                bounds,
                clip: None,
                text: if selected {
                    format!("{spoken} Selected.")
                } else {
                    spoken
                },
                heading: false,
            })),
        }
        bounds.height
    }
}

/// What a card does when pressed.
enum CardAction {
    /// Shown only, e.g. an agent that cannot start.
    None,
    /// Opens a menu of choices.
    Menu(Target),
    /// Chooses the card among its siblings.
    Choose(Target),
}

/// What a session is doing, in a few words.
pub fn session_summary(session: &AgentSession) -> String {
    let transcript = session.transcript();
    if let Some(approval) = transcript.approvals().first() {
        let subject = approval
            .input
            .get("command")
            .or_else(|| approval.input.get("file_path"))
            .and_then(|value| value.as_str())
            .map(|subject| present::relative(subject, session.cwd()));
        if approval.tool_name == present::PATCH_TOOL {
            return match present::patch_paths(&approval.input).as_slice() {
                [path] => format!("Wants to change {}", present::relative(path, session.cwd())),
                paths => format!("Wants to change {} files", paths.len()),
            };
        }
        return match (approval.tool_name.as_str(), subject) {
            ("Bash", Some(command)) => format!("Wants to run {command}"),
            ("AskUserQuestion", _) => "Asked you a question".to_owned(),
            ("ExitPlanMode", _) => "Has a plan for you to review".to_owned(),
            (_, Some(subject)) => format!("Wants to change {subject}"),
            (tool, None) => format!("Wants to use {}", present::summarize_name(tool)),
        };
    }
    match session.activity() {
        Activity::Starting => "Starting".to_owned(),
        Activity::Working => {
            let latest = transcript.items().iter().rev().find_map(|item| match item {
                Item::Tool(tool) => {
                    let summary = present::summarize(tool, session.cwd());
                    Some(
                        format!("{} {}", summary.verb, summary.subject)
                            .trim()
                            .to_owned(),
                    )
                }
                Item::Text { .. } => Some("Writing".to_owned()),
                _ => None,
            });
            match (latest, transcript.turn_elapsed()) {
                (Some(latest), Some(elapsed)) => format!("{latest} · {}", duration(elapsed)),
                (Some(latest), None) => latest,
                (None, _) => "Working".to_owned(),
            }
        }
        Activity::Finished { success: false } => transcript
            .items()
            .iter()
            .rev()
            .find_map(|item| match item {
                Item::TurnEnd(end) => end.outcome.error.clone(),
                _ => None,
            })
            .unwrap_or_else(|| "Stopped with an error".to_owned()),
        Activity::Exited => "Closed".to_owned(),
        Activity::Stopped => "Stopped".to_owned(),
        Activity::Idle | Activity::Finished { success: true } | Activity::NeedsYou => transcript
            .last_answer()
            .and_then(|answer| answer.lines().map(str::trim).find(|line| !line.is_empty()))
            .map_or_else(|| "Ready".to_owned(), str::to_owned),
    }
}
