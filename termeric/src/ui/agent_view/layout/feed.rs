//! The conversation: messages, tool calls, decisions, and turn endings.

use std::path::Path;
use std::sync::Arc;

use crate::agent_view::event::{ApprovalRequest, ModeChoice};
use crate::agent_view::markdown::{self, Block, Span, SpanStyle};
use crate::agent_view::present::{self, DiffLineKind, Entry, PATCH_TOOL, TodoStatus, ToolKind};
use crate::agent_view::session::AgentSession;
use crate::agent_view::transcript::{
    Activity, Attachment, DecidedBy, Item, Notice, NoticeKind, ToolCall, ToolState, TurnEnd,
};
use crate::ui::geometry::Bounds;
use crate::ui::icon::Icon;
use crate::ui::text_layout::{TextSize, TextSpan, UiFace};

use super::super::ConversationView;
use super::super::scene::{ExpandKey, HitRole, Prim, SpanPaint, Target, TextNode};
use super::{Builder, ButtonKind, Region, RegionNode, merge};
use crate::ui::text::plural;

/// Diff lines shown before the rest is summarized.
const MAX_DIFF_LINES: usize = 200;
/// Command output lines shown in a card.
const OUTPUT_LINES: usize = 12;
/// Diffs this long start collapsed.
const COLLAPSE_DIFF_LINES: usize = 24;
/// An expanded block taller than this many lines also ends in a control
/// that collapses it.
const COLLAPSE_CONTROL_LINES: f32 = 8.0;

impl Builder<'_, '_> {
    /// The conversation in content coordinates, and its height.
    pub(super) fn conversation(
        &mut self,
        session: &AgentSession,
        viewport: Bounds,
    ) -> (Region, f32) {
        let mut region = Region::default();
        let transcript = session.transcript();
        let fallback = ConversationView::default();
        let view = self
            .input
            .state
            .conversation(session.id())
            .unwrap_or(&fallback);
        let x = viewport.x + self.m.pad;
        let width = viewport.width - self.m.pad * 2.0;
        let mut y = self.m.pad;
        let items = transcript.items();
        let latest_todo = items.iter().rposition(
            |item| matches!(item, Item::Tool(tool) if present::kind(&tool.name) == ToolKind::Todo),
        );

        if items.is_empty() && session.activity() != Activity::Starting {
            let hint = format!(
                "Ask {} to change, explain, or run something in {}.",
                session.agent().short_name(),
                self.short_path(session.cwd())
            );
            let height = self.read_paragraph(
                &mut region,
                &[(
                    &hint,
                    SpanPaint {
                        face: UiFace::SANS,
                        color: self.c.text_muted,
                    },
                )],
                x,
                y + self.m.px(24.0),
                width,
                self.m.body,
                false,
            );
            y += self.m.px(24.0) + height;
        }

        let entries = present::entries(items, |id| transcript.approval_for_tool(id).is_some());
        for entry in entries {
            let height = match entry {
                Entry::Item(index) => {
                    self.item(&mut region, session, view, index, latest_todo, x, y, width)
                }
                Entry::Explored(indices) => {
                    self.explored(&mut region, session, view, &indices, x, y, width)
                }
            };
            if height > 0.0 {
                y += height + self.m.gap;
            }
        }

        // Requests with no tool call in the conversation, such as a
        // sub-agent's, still need an answer.
        for approval in transcript.approvals() {
            let shown = approval.tool_use_id.as_deref().is_some_and(|id| {
                items
                    .iter()
                    .any(|item| matches!(item, Item::Tool(tool) if tool.id == id))
            });
            if !shown {
                let height = self.approval_card(&mut region, session, approval, x, y, width);
                y += height + self.m.gap;
            }
        }

        if let Some(request) = self
            .input
            .supervisor_request
            .as_ref()
            .filter(|request| request.session == session.id())
        {
            let height = self.supervisor_request_card(&mut region, request, x, y, width);
            y += height + self.m.gap;
        }

        y = self.status_line(&mut region, session, x, y, width);
        (region, y + self.m.pad)
    }

    fn item(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        view: &ConversationView,
        index: usize,
        latest_todo: Option<usize>,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        match &session.transcript().items()[index] {
            Item::User { text, attachments } => {
                let height = self.user_message(region, text, attachments, x, y, width);
                match unlisted_command(session, index, text) {
                    Some(name) => height + self.run_in_terminal(region, name, x, y + height, width),
                    None => height,
                }
            }
            Item::Text { text, .. } => {
                if text.trim().is_empty() {
                    0.0
                } else {
                    self.markdown(region, text, x, y, width)
                }
            }
            Item::Thinking {
                text, streaming, ..
            } => self.thinking(region, view, index, text, *streaming, x, y, width),
            Item::Tool(tool) => {
                self.tool(region, session, view, tool, index, latest_todo, x, y, width)
            }
            Item::TurnEnd(end) => self.turn_end(region, end, index, x, y, width),
            Item::Notice(notice) => self.notice(region, session, notice, x, y, width),
        }
    }

    fn user_message(
        &mut self,
        region: &mut Region,
        text: &str,
        attachments: &[Attachment],
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let pad_x = self.m.px(12.0);
        let pad_y = self.m.px(10.0);
        let inner = width - pad_x * 2.0;
        // Measure first to size the card, then draw into it.
        let mut body = Region::default();
        let mut cursor = y + pad_y;
        let mut chip_x = x + pad_x;
        for attachment in attachments {
            let chip_width = self.chip(
                &mut body,
                chip_x,
                cursor,
                &attachment.label,
                Some(Icon::Terminal),
                None,
                false,
                false,
                inner,
                None,
            );
            chip_x += chip_width + self.m.px(6.0);
            if chip_x > x + pad_x + inner - self.m.px(80.0) {
                chip_x = x + pad_x;
                cursor += self.m.chip + self.m.px(6.0);
            }
        }
        if !attachments.is_empty() && chip_x > x + pad_x {
            cursor += self.m.chip + self.m.px(8.0);
        }
        if !text.is_empty() {
            let height = self.read_paragraph(
                &mut body,
                &[(
                    text,
                    SpanPaint {
                        face: UiFace::SANS,
                        color: self.c.text_strong,
                    },
                )],
                x + pad_x,
                cursor,
                inner,
                self.m.body,
                false,
            );
            cursor += height;
        }
        let bounds = Bounds::new(x, y, width, (cursor + pad_y - y).round());
        self.card(
            region,
            bounds,
            self.m.px(10.0),
            self.c.surface,
            self.c.border,
        );
        region.nodes.push(RegionNode::Text(TextNode {
            bounds,
            clip: None,
            text: "You said".into(),
            heading: true,
        }));
        merge(region, body);
        bounds.height
    }

    // ── Markdown ─────────────────────────────────────────────────────────

    pub(super) fn markdown(
        &mut self,
        region: &mut Region,
        text: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let mut cursor = y;
        let blocks = markdown::parse(text);
        let count = blocks.len();
        for (position, block) in blocks.into_iter().enumerate() {
            let height = match block {
                Block::Paragraph(spans) => self.spans(
                    region,
                    &spans,
                    x,
                    cursor,
                    width,
                    self.m.body,
                    SpanTone::Body,
                    false,
                ),
                Block::Heading { level, spans } => {
                    let size = if level <= 2 {
                        self.m.title
                    } else {
                        self.m.body
                    };
                    self.spans(
                        region,
                        &spans,
                        x,
                        cursor,
                        width,
                        size,
                        SpanTone::Heading,
                        true,
                    )
                }
                Block::ListItem {
                    number,
                    depth,
                    checked,
                    spans,
                } => {
                    let indent = self.m.px(18.0) * f32::from(depth);
                    let marker_width = self.m.px(18.0);
                    let marker_x = x + indent;
                    let line_center = cursor + self.m.body.line_px / 2.0;
                    match (checked, number) {
                        (Some(done), _) => {
                            let size = self.m.px(12.0);
                            let bounds =
                                Bounds::new(marker_x, line_center - size / 2.0, size, size);
                            region.prims.push(Prim::Outline {
                                bounds,
                                radius: self.m.px(3.0),
                                stroke: self.m.stroke,
                                color: self.c.text_muted,
                            });
                            if done {
                                region.prims.push(Prim::Icon {
                                    icon: Icon::Check,
                                    center: bounds.center(),
                                    size: self.m.px(11.0),
                                    color: self.c.success,
                                });
                            }
                        }
                        (None, Some(number)) => {
                            let label = format!("{number}.");
                            self.label(
                                region,
                                &label,
                                marker_x,
                                cursor,
                                self.m.body.line_px,
                                marker_width + self.m.px(6.0),
                                UiFace::SANS,
                                self.m.body,
                                self.c.text_secondary,
                            );
                        }
                        (None, None) => {
                            let size = self.m.px(5.0);
                            region.prims.push(Prim::Fill {
                                bounds: Bounds::new(
                                    marker_x + self.m.px(4.0),
                                    (line_center - size / 2.0).round(),
                                    size,
                                    size,
                                ),
                                radius: size / 2.0,
                                color: self.c.text_secondary,
                            });
                        }
                    }
                    let text_x = marker_x
                        + marker_width
                        + if number.is_some_and(|n| n >= 10) {
                            self.m.px(6.0)
                        } else {
                            0.0
                        };
                    self.spans(
                        region,
                        &spans,
                        text_x,
                        cursor,
                        x + width - text_x,
                        self.m.body,
                        SpanTone::Body,
                        false,
                    )
                }
                Block::Quote(spans) => {
                    let inset = self.m.px(12.0);
                    let height = self.spans(
                        region,
                        &spans,
                        x + inset,
                        cursor,
                        width - inset,
                        self.m.body,
                        SpanTone::Quiet,
                        false,
                    );
                    region.prims.push(Prim::Fill {
                        bounds: Bounds::new(x, cursor, self.m.px(3.0), height),
                        radius: self.m.px(1.5),
                        color: self.c.border,
                    });
                    height
                }
                Block::Code { text, .. } => self.code_block(region, &text, x, cursor, width),
                Block::Table(rows) => self.code_block(region, &rows.join("\n"), x, cursor, width),
                Block::Rule => {
                    region.prims.push(Prim::Fill {
                        bounds: Bounds::new(x, cursor + self.m.px(8.0), width, self.m.stroke),
                        radius: 0.0,
                        color: self.c.border,
                    });
                    self.m.px(16.0)
                }
            };
            cursor += height;
            if position + 1 < count {
                cursor += self.m.px(8.0);
            }
        }
        cursor - y
    }

    fn spans(
        &mut self,
        region: &mut Region,
        spans: &[Span],
        x: f32,
        y: f32,
        width: f32,
        size: TextSize,
        tone: SpanTone,
        heading: bool,
    ) -> f32 {
        let painted: Vec<(&str, SpanPaint)> = spans
            .iter()
            .map(|span| (span.text.as_str(), self.span_paint(span.style, tone)))
            .collect();
        let layout = self.paragraph(region, &painted, x, y, width, size);
        // Inline code sits on a soft background.
        for line in &layout.lines {
            for run in &line.runs {
                if spans.get(run.span).is_some_and(|span| span.style.code) {
                    let inset = self.m.px(2.0);
                    region.prims.push(Prim::Fill {
                        bounds: Bounds::new(
                            (x + run.x - inset).round(),
                            (y + line.top + inset).round(),
                            (run.width + inset * 2.0).round(),
                            (line.height - inset * 2.0).round(),
                        ),
                        radius: self.m.px(4.0),
                        color: self.c.code_surface,
                    });
                }
            }
        }
        region.nodes.push(RegionNode::Text(TextNode {
            bounds: Bounds::new(x, y, width, layout.height),
            clip: None,
            text: markdown::plain_text(spans),
            heading,
        }));
        layout.height
    }

    fn span_paint(&self, style: SpanStyle, tone: SpanTone) -> SpanPaint {
        let base = match tone {
            SpanTone::Body => self.c.text,
            SpanTone::Heading => self.c.text_strong,
            SpanTone::Quiet => self.c.text_secondary,
        };
        if style.code {
            return SpanPaint {
                face: UiFace::MONO,
                color: self.c.code_text,
            };
        }
        let face = UiFace {
            mono: false,
            bold: style.bold || tone == SpanTone::Heading,
            italic: style.italic,
        };
        let color = if style.link {
            self.c.link
        } else if style.bold {
            self.c.text_strong
        } else {
            base
        };
        SpanPaint { face, color }
    }

    fn code_block(&mut self, region: &mut Region, text: &str, x: f32, y: f32, width: f32) -> f32 {
        let pad = self.m.px(10.0);
        let text = if text.is_empty() { " " } else { text };
        let mut body = Region::default();
        let layout = self.paragraph(
            &mut body,
            &[(
                text,
                SpanPaint {
                    face: UiFace::MONO,
                    color: self.c.text,
                },
            )],
            x + pad,
            y + pad,
            width - pad * 2.0,
            self.m.mono,
        );
        let bounds = Bounds::new(x, y, width, (layout.height + pad * 2.0).round());
        region.prims.push(Prim::Fill {
            bounds,
            radius: self.m.px(6.0),
            color: self.c.code_surface,
        });
        region.nodes.push(RegionNode::Text(TextNode {
            bounds,
            clip: None,
            text: text.to_owned(),
            heading: false,
        }));
        merge(region, body);
        bounds.height
    }

    // ── Rows and cards ───────────────────────────────────────────────────

    fn thinking(
        &mut self,
        region: &mut Region,
        view: &ConversationView,
        index: usize,
        text: &str,
        streaming: bool,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let key = ExpandKey::Thinking(index);
        let expanded = view.expanded(&key, false);
        let label = if streaming { "Thinking…" } else { "Thought" };
        let row = self.row(
            region,
            x,
            y,
            width,
            Some(expanded),
            if streaming { None } else { Some(Icon::Agent) },
            label,
            "",
            false,
            None,
            RowStatus::None,
        );
        if streaming {
            region.prims.push(Prim::Spinner {
                center: [x + self.m.px(8.0) + self.m.icon, y + row / 2.0],
                radius: self.m.px(5.0),
                thickness: self.m.px(1.6),
                phase: spinner_phase(self.input.now, self.input.state.reduced_motion()),
                color: self.c.accent,
            });
        }
        self.hit(
            region,
            Bounds::new(x, y, width, row),
            Target::Toggle(key),
            HitRole::Toggle { expanded },
            label,
            true,
        );
        if !expanded || text.trim().is_empty() {
            return row;
        }
        let indent = self.m.px(22.0);
        let height = self.read_paragraph(
            region,
            &[(
                text.trim(),
                SpanPaint {
                    face: UiFace {
                        italic: true,
                        ..UiFace::SANS
                    },
                    color: self.c.text_muted,
                },
            )],
            x + indent,
            y + row + self.m.px(2.0),
            width - indent,
            self.m.small,
            false,
        );
        row + height + self.m.px(4.0)
    }

    /// A one-line row: optional chevron, icon, verb, subject, and status.
    /// Returns its height.
    fn row(
        &mut self,
        region: &mut Region,
        x: f32,
        y: f32,
        width: f32,
        chevron: Option<bool>,
        icon: Option<Icon>,
        verb: &str,
        subject: &str,
        subject_is_code: bool,
        detail: Option<&str>,
        status: RowStatus,
    ) -> f32 {
        let height = self.m.control;
        let center = y + height / 2.0;
        let mut cursor = x + self.m.px(4.0);
        if let Some(expanded) = chevron {
            region.prims.push(Prim::Icon {
                icon: if expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                center: [cursor + self.m.px(7.0), center],
                size: self.m.px(13.0),
                color: self.c.text_muted,
            });
        }
        cursor += self.m.px(18.0);
        if let Some(icon) = icon {
            region.prims.push(Prim::Icon {
                icon,
                center: [cursor + self.m.px(7.0), center],
                size: self.m.px(14.0),
                color: self.c.text_secondary,
            });
        }
        cursor += self.m.px(22.0);
        let status_width = self.status_width(status);
        let right = x + width - status_width - self.m.px(6.0);
        let verb_width = self.label(
            region,
            verb,
            cursor,
            y,
            height,
            right - cursor,
            UiFace::SANS,
            self.m.body,
            self.c.text,
        );
        cursor += verb_width + self.m.px(8.0);
        if !subject.is_empty() && cursor < right {
            let face = if subject_is_code {
                UiFace::MONO
            } else {
                UiFace::SANS
            };
            let size = if subject_is_code {
                self.m.mono
            } else {
                self.m.body
            };
            let subject_width = self.label(
                region,
                subject,
                cursor,
                y,
                height,
                right - cursor,
                face,
                size,
                self.c.text_secondary,
            );
            cursor += subject_width + self.m.px(8.0);
        }
        if let Some(detail) = detail
            && cursor < right
        {
            self.label(
                region,
                detail,
                cursor,
                y,
                height,
                right - cursor,
                UiFace::SANS,
                self.m.small,
                self.c.text_muted,
            );
        }
        self.draw_status(region, status, x + width - status_width, y, height);
        height
    }

    fn status_width(&mut self, status: RowStatus) -> f32 {
        let text = status.text();
        if text.is_empty() {
            return match status {
                RowStatus::None => 0.0,
                _ => self.m.px(18.0),
            };
        }
        self.text_width(text, UiFace::SANS, self.m.small) + self.m.px(20.0)
    }

    fn draw_status(&mut self, region: &mut Region, status: RowStatus, x: f32, y: f32, height: f32) {
        let center = y + height / 2.0;
        let (icon, color) = match status {
            RowStatus::None => return,
            RowStatus::Running | RowStatus::Preparing => {
                region.prims.push(Prim::Spinner {
                    center: [x + self.m.px(7.0), center],
                    radius: self.m.px(5.0),
                    thickness: self.m.px(1.6),
                    phase: spinner_phase(self.input.now, self.input.state.reduced_motion()),
                    color: self.c.accent,
                });
                if status == RowStatus::Preparing {
                    let text = status.text();
                    let width = self.text_width(text, UiFace::SANS, self.m.small);
                    self.label(
                        region,
                        text,
                        x + self.m.px(18.0),
                        y,
                        height,
                        width + 2.0,
                        UiFace::SANS,
                        self.m.small,
                        self.c.accent,
                    );
                }
                return;
            }
            RowStatus::Succeeded => (Icon::Check, self.c.success),
            RowStatus::Failed => (Icon::Cross, self.c.danger),
            RowStatus::Denied => (Icon::Cross, self.c.danger),
            RowStatus::Stopped => (Icon::Stop, self.c.text_muted),
            RowStatus::NeedsApproval => (Icon::Shield, self.c.attention),
        };
        region.prims.push(Prim::Icon {
            icon,
            center: [x + self.m.px(7.0), center],
            size: self.m.px(13.0),
            color,
        });
        let text = status.text();
        if !text.is_empty() {
            let width = self.text_width(text, UiFace::SANS, self.m.small);
            self.label(
                region,
                text,
                x + self.m.px(18.0),
                y,
                height,
                width + 2.0,
                UiFace::SANS,
                self.m.small,
                color,
            );
        }
    }

    fn explored(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        view: &ConversationView,
        indices: &[usize],
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let items = session.transcript().items();
        let tools: Vec<&ToolCall> = indices
            .iter()
            .filter_map(|index| match &items[*index] {
                Item::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect();
        let Some(first) = tools.first() else {
            return 0.0;
        };
        let key = ExpandKey::Explored(first.id.clone());
        let expanded = view.expanded(&key, false);
        let running = tools
            .iter()
            .any(|tool| tool.result.is_none() && !tool.abandoned && !tool.withdrawn);
        let summary = present::explored_summary(tools.iter().copied());
        let status = if running {
            RowStatus::Running
        } else {
            RowStatus::None
        };
        let row = self.row(
            region,
            x,
            y,
            width,
            Some(expanded),
            Some(Icon::Search),
            "Explored",
            &summary,
            false,
            None,
            status,
        );
        self.hit(
            region,
            Bounds::new(x, y, width, row),
            Target::Toggle(key),
            HitRole::Toggle { expanded },
            format!("Explored {summary}"),
            true,
        );
        if !expanded {
            return row;
        }
        let mut cursor = y + row;
        let indent = self.m.px(18.0);
        for tool in tools {
            let summary = present::summarize(tool, session.cwd());
            let icon = if present::kind(&tool.name) == ToolKind::Read {
                Icon::File
            } else {
                Icon::Search
            };
            let status = row_status(tool, false);
            let height = self.row(
                region,
                x + indent,
                cursor,
                width - indent,
                None,
                Some(icon),
                &summary.verb,
                &summary.subject,
                summary.subject_is_code,
                summary.detail.as_deref(),
                status,
            );
            region.nodes.push(RegionNode::Text(TextNode {
                bounds: Bounds::new(x + indent, cursor, width - indent, height),
                clip: None,
                text: format!("{} {}", summary.verb, summary.subject),
                heading: false,
            }));
            cursor += height;
        }
        cursor - y
    }

    fn tool(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        view: &ConversationView,
        tool: &ToolCall,
        index: usize,
        latest_todo: Option<usize>,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let transcript = session.transcript();
        let approval = transcript.approval_for_tool(&tool.id);
        let kind = present::kind(&tool.name);
        let mut height = match kind {
            ToolKind::Edit => {
                self.edit_card(region, session, view, tool, approval.is_some(), x, y, width)
            }
            ToolKind::Command => {
                self.command_card(region, view, tool, approval.is_some(), x, y, width)
            }
            ToolKind::Todo => {
                if Some(index) == latest_todo {
                    self.todo_card(region, tool, x, y, width)
                } else {
                    let row = self.row(
                        region,
                        x,
                        y,
                        width,
                        None,
                        Some(Icon::Check),
                        "Updated to-dos",
                        "",
                        false,
                        None,
                        RowStatus::None,
                    );
                    region.nodes.push(RegionNode::Text(TextNode {
                        bounds: Bounds::new(x, y, width, row),
                        clip: None,
                        text: "Updated to-dos".into(),
                        heading: false,
                    }));
                    row
                }
            }
            ToolKind::Question => match approval {
                Some(approval) => self.question_card(region, view, approval, x, y, width),
                None => self.answered_question(region, tool, x, y, width),
            },
            ToolKind::Plan => {
                let modes = session.modes();
                self.plan_card(region, tool, approval, &modes, x, y, width)
            }
            ToolKind::SubAgent => self.sub_agent(region, session, view, tool, x, y, width),
            ToolKind::Read | ToolKind::Search | ToolKind::Web | ToolKind::Other => {
                self.generic_tool(region, session, view, tool, approval.is_some(), x, y, width)
            }
        };
        if let Some(approval) = approval
            && !matches!(kind, ToolKind::Question | ToolKind::Plan)
        {
            height += self.m.px(8.0);
            height += self.approval_card(region, session, approval, x, y + height, width);
        }
        if let Some(note) = decision_note(tool) {
            let (color, icon) = match (tool.decision.as_ref().map(|decision| decision.by), note.1) {
                (Some(DecidedBy::Supervisor), _) => (self.c.supervisor, Icon::Eye),
                (_, true) => (self.c.text_muted, Icon::Check),
                _ => (self.c.text_muted, Icon::Cross),
            };
            let line_y = y + height + self.m.px(4.0);
            region.prims.push(Prim::Icon {
                icon,
                center: [x + self.m.px(10.0), line_y + self.m.small.line_px / 2.0],
                size: self.m.px(12.0),
                color,
            });
            let text_height = self.read_paragraph(
                region,
                &[(
                    &note.0,
                    SpanPaint {
                        face: UiFace::SANS,
                        color,
                    },
                )],
                x + self.m.px(22.0),
                line_y,
                width - self.m.px(22.0),
                self.m.small,
                false,
            );
            height += self.m.px(4.0) + text_height;
        }
        height
    }

    fn generic_tool(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        view: &ConversationView,
        tool: &ToolCall,
        awaiting: bool,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let summary = present::summarize(tool, session.cwd());
        let icon = match present::kind(&tool.name) {
            ToolKind::Read => Icon::File,
            ToolKind::Search => Icon::Search,
            ToolKind::Web => Icon::Globe,
            _ => Icon::Agent,
        };
        let result = tool
            .result
            .as_ref()
            .filter(|result| !result.content.trim().is_empty());
        let key = ExpandKey::Tool(tool.id.clone());
        let expandable = result.is_some() && present::kind(&tool.name) == ToolKind::Other;
        let failed = tool.result.as_ref().is_some_and(|result| result.is_error);
        let expanded = expandable && view.expanded(&key, failed);
        let status = row_status(tool, awaiting);
        let row = self.row(
            region,
            x,
            y,
            width,
            expandable.then_some(expanded),
            Some(icon),
            &summary.verb,
            &summary.subject,
            summary.subject_is_code,
            summary.detail.as_deref(),
            status,
        );
        let label = format!("{} {}", summary.verb, summary.subject);
        if expandable {
            self.hit(
                region,
                Bounds::new(x, y, width, row),
                Target::Toggle(key),
                HitRole::Toggle { expanded },
                label,
                true,
            );
        } else {
            region.nodes.push(RegionNode::Text(TextNode {
                bounds: Bounds::new(x, y, width, row),
                clip: None,
                text: label,
                heading: false,
            }));
        }
        let mut height = row;
        if failed && !expandable {
            height += self.error_line(
                region,
                tool,
                x + self.m.px(40.0),
                y + height,
                width - self.m.px(40.0),
            );
        }
        if expanded && let Some(result) = result {
            let (lines, skipped) = present::output_tail(&result.content, 40);
            let mut text = lines.join("\n");
            if skipped > 0 {
                text = format!("… {} earlier\n{text}", plural(skipped, "line", "lines"));
            }
            let indent = self.m.px(22.0);
            height += self.m.px(4.0);
            let block = self.code_block(region, &text, x + indent, y + height, width - indent);
            height += block;
            height += self.collapse_control(
                region,
                &ExpandKey::Tool(tool.id.clone()),
                block,
                x + indent,
                y + height,
                width - indent,
            );
        }
        height
    }

    fn error_line(
        &mut self,
        region: &mut Region,
        tool: &ToolCall,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let Some(result) = &tool.result else {
            return 0.0;
        };
        let first = crate::safe_text::first_line(&result.content).unwrap_or("It failed");
        let text = strip_tool_error(first);
        self.read_paragraph(
            region,
            &[(
                &text,
                SpanPaint {
                    face: UiFace::SANS,
                    color: self.c.danger,
                },
            )],
            x,
            y,
            width,
            self.m.small,
            false,
        )
    }

    /// One card per file the call changes or proposes to change.
    fn edit_card(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        view: &ConversationView,
        tool: &ToolCall,
        awaiting: bool,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let diffs = present::file_diffs(tool, session.cwd());
        if diffs.is_empty() {
            return self.generic_tool(region, session, view, tool, awaiting, x, y, width);
        }
        let mut height = 0.0;
        for (index, diff) in diffs.iter().enumerate() {
            if index > 0 {
                height += self.m.px(6.0);
            }
            let key = match index {
                0 => ExpandKey::Tool(tool.id.clone()),
                _ => ExpandKey::Tool(format!("{}#{index}", tool.id)),
            };
            height += self.file_card(
                region,
                view,
                tool,
                diff,
                key,
                index == 0,
                awaiting,
                x,
                y + height,
                width,
            );
        }
        height
    }

    /// A file's diff under a header; `first` carries the call's error.
    fn file_card(
        &mut self,
        region: &mut Region,
        view: &ConversationView,
        tool: &ToolCall,
        diff: &present::FileDiff,
        key: ExpandKey,
        first: bool,
        awaiting: bool,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let header_height = self.m.px(38.0);
        let expanded = view.expanded(&key, diff.lines.len() <= COLLAPSE_DIFF_LINES || awaiting);
        let status = row_status(tool, awaiting);
        let mut body = Region::default();
        let mut cursor = y + header_height;
        if first && matches!(status, RowStatus::Failed | RowStatus::Denied) && tool.result.is_some()
        {
            cursor += self.m.px(6.0);
            cursor += self.error_line(
                &mut body,
                tool,
                x + self.m.px(12.0),
                cursor,
                width - self.m.px(24.0),
            );
            cursor += self.m.px(6.0);
        }
        if expanded {
            cursor += self.m.px(6.0);
            cursor = self.diff_lines(&mut body, &diff.lines, x, cursor, width);
            cursor += self.m.px(6.0);
        }
        let bounds = Bounds::new(x, y, width, (cursor - y).round());
        self.header_card(
            region,
            bounds,
            header_height,
            expanded || cursor > y + header_height,
            self.c.panel,
            self.c.border,
        );

        // Header: icon, file name, folder, counts, status, chevron.
        let center = y + header_height / 2.0;
        let mut header_x = x + self.m.px(12.0);
        region.prims.push(Prim::Icon {
            icon: Icon::Pencil,
            center: [header_x + self.m.px(7.0), center],
            size: self.m.px(14.0),
            color: self.c.text_secondary,
        });
        header_x += self.m.px(22.0);
        let path = Path::new(&diff.path);
        let file_name = path.file_name().map_or_else(
            || diff.path.clone(),
            |name| name.to_string_lossy().into_owned(),
        );
        let folder = path
            .parent()
            .map(|parent| parent.display().to_string())
            .filter(|parent| !parent.is_empty());
        let chevron_x = x + width - self.m.px(30.0);
        let status_width = self.status_width(status);
        let counts = if diff.created {
            format!("new · +{}", diff.added)
        } else if diff.deleted {
            format!("deleted · −{}", diff.removed)
        } else {
            format!("+{} −{}", diff.added, diff.removed)
        };
        let counts_width = self.text_width(&counts, UiFace::MONO, self.m.mono);
        let right = chevron_x - status_width - counts_width - self.m.px(16.0);
        let name_width = self.label(
            region,
            &file_name,
            header_x,
            y,
            header_height,
            right - header_x,
            UiFace::SANS_BOLD,
            self.m.body,
            self.c.text_strong,
        );
        header_x += name_width + self.m.px(8.0);
        if let Some(folder) = &folder
            && header_x < right
        {
            let folder_width = self.label(
                region,
                folder,
                header_x,
                y,
                header_height,
                right - header_x,
                UiFace::SANS,
                self.m.small,
                self.c.text_muted,
            );
            header_x += folder_width + self.m.px(8.0);
        }
        let counts_x = header_x.min(right + self.m.px(8.0));
        let added = format!("+{}", diff.added);
        if diff.created || diff.deleted {
            self.label(
                region,
                &counts,
                counts_x,
                y,
                header_height,
                counts_width + 2.0,
                UiFace::MONO,
                self.m.mono,
                if diff.created {
                    self.c.added_text
                } else {
                    self.c.removed_text
                },
            );
        } else {
            let added_width = self.label(
                region,
                &added,
                counts_x,
                y,
                header_height,
                counts_width,
                UiFace::MONO,
                self.m.mono,
                self.c.added_text,
            );
            let removed = format!("−{}", diff.removed);
            self.label(
                region,
                &removed,
                counts_x + added_width + self.m.px(6.0),
                y,
                header_height,
                counts_width,
                UiFace::MONO,
                self.m.mono,
                self.c.removed_text,
            );
        }
        self.draw_status(
            region,
            status,
            chevron_x - status_width - self.m.px(4.0),
            y,
            header_height,
        );
        region.prims.push(Prim::Icon {
            icon: if expanded {
                Icon::ChevronUp
            } else {
                Icon::ChevronDown
            },
            center: [chevron_x + self.m.px(15.0), center],
            size: self.m.px(14.0),
            color: self.c.text_muted,
        });
        let verb = match (diff.created, diff.deleted) {
            (true, _) => "Created",
            (_, true) => "Deleted",
            _ => "Edited",
        };
        self.hit(
            region,
            Bounds::new(x, y, width, header_height),
            Target::Toggle(key),
            HitRole::Toggle { expanded },
            format!(
                "{verb} {}, {} lines added, {} removed",
                diff.path, diff.added, diff.removed
            ),
            true,
        );
        merge(region, body);
        bounds.height
    }

    fn diff_lines(
        &mut self,
        region: &mut Region,
        lines: &[present::DiffLine],
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let gutter = self.m.px(40.0);
        let text_x = x + gutter + self.m.px(6.0);
        let text_width = width - gutter - self.m.px(12.0);
        let mut cursor = y;
        for line in lines.iter().take(MAX_DIFF_LINES) {
            if line.kind == DiffLineKind::Gap {
                let text = if line.text.is_empty() {
                    "⋯"
                } else {
                    &line.text
                };
                self.label(
                    region,
                    text,
                    text_x,
                    cursor,
                    self.m.mono.line_px,
                    text_width,
                    UiFace::SANS,
                    self.m.small,
                    self.c.text_muted,
                );
                cursor += self.m.mono.line_px;
                continue;
            }
            let (surface, color) = match line.kind {
                DiffLineKind::Added => (Some(self.c.added_surface), self.c.added_text),
                DiffLineKind::Removed => (Some(self.c.removed_surface), self.c.removed_text),
                _ => (None, self.c.text_secondary),
            };
            let content = if line.text.is_empty() {
                " "
            } else {
                &line.text
            };
            let layout = self.paragraph(
                region,
                &[(
                    content,
                    SpanPaint {
                        face: UiFace::MONO,
                        color,
                    },
                )],
                text_x,
                cursor,
                text_width,
                self.m.mono,
            );
            let height = layout.height;
            if let Some(surface) = surface {
                region.prims.push(Prim::Fill {
                    bounds: Bounds::new(
                        x + self.m.stroke,
                        cursor,
                        width - self.m.stroke * 2.0,
                        height,
                    ),
                    radius: 0.0,
                    color: surface,
                });
            }
            let sign = match line.kind {
                DiffLineKind::Added => "+",
                DiffLineKind::Removed => "−",
                _ => "",
            };
            let number = line
                .number
                .map_or_else(|| sign.to_owned(), |number| format!("{sign}{number}"));
            if !number.is_empty() {
                let number_width = self.text_width(&number, UiFace::MONO, self.m.mono);
                let gutter_color = if surface.is_some() {
                    color
                } else {
                    self.c.gutter_text
                };
                self.label(
                    region,
                    &number,
                    x + gutter - number_width,
                    cursor,
                    self.m.mono.line_px,
                    gutter,
                    UiFace::MONO,
                    self.m.mono,
                    gutter_color,
                );
            }
            cursor += height;
        }
        if lines.len() > MAX_DIFF_LINES {
            let more = format!(
                "{} more",
                plural(lines.len() - MAX_DIFF_LINES, "line", "lines")
            );
            self.label(
                region,
                &more,
                text_x,
                cursor,
                self.m.mono.line_px,
                text_width,
                UiFace::SANS,
                self.m.small,
                self.c.text_muted,
            );
            cursor += self.m.mono.line_px;
        }
        cursor
    }

    fn command_card(
        &mut self,
        region: &mut Region,
        view: &ConversationView,
        tool: &ToolCall,
        awaiting: bool,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let command = tool
            .input
            .get("command")
            .and_then(|value| value.as_str())
            .unwrap_or(&tool.name);
        let description = tool
            .input
            .get("description")
            .and_then(|value| value.as_str());
        let header_height = self.m.px(38.0);
        let status = row_status(tool, awaiting);
        let failed = matches!(status, RowStatus::Failed);
        let output = tool
            .result
            .as_ref()
            .map(|result| result.content.as_str())
            .filter(|text| !text.trim().is_empty());
        let key = ExpandKey::Tool(tool.id.clone());
        let expandable = output.is_some();
        let expanded = expandable && view.expanded(&key, failed);
        let mut body = Region::default();
        let mut cursor = y + header_height;
        if expanded && let Some(output) = output {
            let (lines, skipped) = present::output_tail(output, OUTPUT_LINES);
            let pad = self.m.px(12.0);
            cursor += self.m.px(8.0);
            if skipped > 0 {
                let note = format!("{} earlier", plural(skipped, "line", "lines"));
                self.label(
                    &mut body,
                    &note,
                    x + pad,
                    cursor,
                    self.m.small.line_px,
                    width - pad * 2.0,
                    UiFace::SANS,
                    self.m.small,
                    self.c.text_muted,
                );
                cursor += self.m.small.line_px;
            }
            let text = lines.join("\n");
            let color = if failed {
                self.c.removed_text
            } else {
                self.c.text_secondary
            };
            let layout = self.paragraph(
                &mut body,
                &[(
                    &text,
                    SpanPaint {
                        face: UiFace::MONO,
                        color,
                    },
                )],
                x + pad,
                cursor,
                width - pad * 2.0,
                self.m.mono,
            );
            body.nodes.push(RegionNode::Text(TextNode {
                bounds: Bounds::new(x + pad, cursor, width - pad * 2.0, layout.height),
                clip: None,
                text,
                heading: false,
            }));
            cursor += layout.height + self.m.px(8.0);
            cursor += self.collapse_control(
                &mut body,
                &key,
                layout.height,
                x + pad,
                cursor,
                width - pad * 2.0,
            );
        }
        let bounds = Bounds::new(x, y, width, (cursor - y).round());
        self.header_card(
            region,
            bounds,
            header_height,
            expanded,
            self.c.well,
            self.c.border,
        );
        let center = y + header_height / 2.0;
        let mut header_x = x + self.m.px(12.0);
        region.prims.push(Prim::Icon {
            icon: Icon::Terminal,
            center: [header_x + self.m.px(7.0), center],
            size: self.m.px(14.0),
            color: self.c.text_secondary,
        });
        header_x += self.m.px(22.0);
        let chevron_space = if expandable {
            self.m.px(30.0)
        } else {
            self.m.px(8.0)
        };
        let status_width = self.status_width(status);
        let right = x + width - chevron_space - status_width - self.m.px(8.0);
        let first_line = command.lines().next().unwrap_or_default();
        self.label(
            region,
            first_line,
            header_x,
            y,
            header_height,
            right - header_x,
            UiFace::MONO,
            self.m.mono,
            self.c.text_strong,
        );
        self.draw_status(region, status, right + self.m.px(4.0), y, header_height);
        if expandable {
            region.prims.push(Prim::Icon {
                icon: if expanded {
                    Icon::ChevronUp
                } else {
                    Icon::ChevronDown
                },
                center: [x + width - self.m.px(15.0), center],
                size: self.m.px(14.0),
                color: self.c.text_muted,
            });
        }
        let label = match description {
            Some(description) => format!("Command {command}: {description}, {}", status.text()),
            None => format!("Command {command}, {}", status.text()),
        };
        if expandable {
            self.hit(
                region,
                Bounds::new(x, y, width, header_height),
                Target::Toggle(key),
                HitRole::Toggle { expanded },
                label,
                true,
            );
        } else {
            region.nodes.push(RegionNode::Text(TextNode {
                bounds: Bounds::new(x, y, width, header_height),
                clip: None,
                text: label,
                heading: false,
            }));
        }
        merge(region, body);
        bounds.height
    }

    fn todo_card(
        &mut self,
        region: &mut Region,
        tool: &ToolCall,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let todos = present::todos(tool).unwrap_or_default();
        let done = todos
            .iter()
            .filter(|todo| todo.status == TodoStatus::Completed)
            .count();
        let pad = self.m.px(12.0);
        let header = self.m.px(34.0);
        let mut body = Region::default();
        let mut cursor = y + header;
        for todo in &todos {
            let (color, text_color) = match todo.status {
                TodoStatus::Completed => (self.c.success, self.c.text_muted),
                TodoStatus::InProgress => (self.c.accent, self.c.text_strong),
                TodoStatus::Pending => (self.c.text_muted, self.c.text),
            };
            let size = self.m.px(12.0);
            let marker = Bounds::new(
                x + pad,
                cursor + (self.m.body.line_px - size) / 2.0,
                size,
                size,
            );
            match todo.status {
                TodoStatus::Completed => body.prims.push(Prim::Icon {
                    icon: Icon::Check,
                    center: marker.center(),
                    size: self.m.px(13.0),
                    color,
                }),
                TodoStatus::InProgress => body.prims.push(Prim::Fill {
                    bounds: marker.inset(self.m.px(2.0), self.m.px(2.0)),
                    radius: size,
                    color,
                }),
                TodoStatus::Pending => body.prims.push(Prim::Outline {
                    bounds: marker,
                    radius: size / 2.0,
                    stroke: self.m.stroke,
                    color,
                }),
            }
            let face = if todo.status == TodoStatus::InProgress {
                UiFace::SANS_BOLD
            } else {
                UiFace::SANS
            };
            let height = self.read_paragraph(
                &mut body,
                &[(
                    &todo.text,
                    SpanPaint {
                        face,
                        color: text_color,
                    },
                )],
                x + pad + size + self.m.px(10.0),
                cursor,
                width - pad * 2.0 - size - self.m.px(10.0),
                self.m.body,
                false,
            );
            cursor += height + self.m.px(4.0);
        }
        cursor += self.m.px(6.0);
        let bounds = Bounds::new(x, y, width, (cursor - y).round());
        self.card(region, bounds, self.m.radius, self.c.surface, self.c.border);
        let title_width = self.label(
            region,
            "To-dos",
            x + pad,
            y,
            header,
            width / 2.0,
            UiFace::SANS_BOLD,
            self.m.body,
            self.c.text_strong,
        );
        let progress = format!("{done} of {}", todos.len());
        self.label(
            region,
            &progress,
            x + pad + title_width + self.m.px(8.0),
            y,
            header,
            width / 2.0,
            UiFace::SANS,
            self.m.small,
            self.c.text_muted,
        );
        region.nodes.push(RegionNode::Text(TextNode {
            bounds: Bounds::new(x, y, width, header),
            clip: None,
            text: format!("To-dos, {progress} done"),
            heading: true,
        }));
        merge(region, body);
        bounds.height
    }

    fn sub_agent(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        view: &ConversationView,
        tool: &ToolCall,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let summary = present::summarize(tool, session.cwd());
        let progress = match (&tool.sub_agent.latest, tool.sub_agent.tool_calls) {
            (_, 0) => None,
            (Some(latest), count) => Some(format!(
                "{} · {latest}",
                plural(count as usize, "step", "steps")
            )),
            (None, count) => Some(plural(count as usize, "step", "steps")),
        };
        let report = tool
            .result
            .as_ref()
            .filter(|result| !result.content.trim().is_empty());
        let key = ExpandKey::Tool(tool.id.clone());
        let expanded = report.is_some() && view.expanded(&key, false);
        let status = row_status(tool, false);
        let row = self.row(
            region,
            x,
            y,
            width,
            report.is_some().then_some(expanded),
            Some(Icon::Agent),
            "Sub-agent",
            &summary.subject,
            false,
            progress.as_deref(),
            status,
        );
        let label = format!(
            "Sub-agent {}: {}",
            summary.subject,
            progress.clone().unwrap_or_default()
        );
        if report.is_some() {
            self.hit(
                region,
                Bounds::new(x, y, width, row),
                Target::Toggle(key),
                HitRole::Toggle { expanded },
                label,
                true,
            );
        } else {
            region.nodes.push(RegionNode::Text(TextNode {
                bounds: Bounds::new(x, y, width, row),
                clip: None,
                text: label,
                heading: false,
            }));
        }
        let mut height = row;
        if expanded && let Some(report) = report {
            let indent = self.m.px(22.0);
            height += self.m.px(4.0);
            let block = self.markdown(
                region,
                &report.content,
                x + indent,
                y + height,
                width - indent,
            );
            height += block;
            height += self.collapse_control(
                region,
                &ExpandKey::Tool(tool.id.clone()),
                block,
                x + indent,
                y + height,
                width - indent,
            );
        }
        height
    }

    /// Below an expanded block `block` tall, a control that collapses it
    /// when it is long enough to have pushed its header away. Returns the
    /// height it takes.
    fn collapse_control(
        &mut self,
        region: &mut Region,
        key: &ExpandKey,
        block: f32,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        if block <= self.m.mono.line_px * COLLAPSE_CONTROL_LINES {
            return 0.0;
        }
        let gap = self.m.px(4.0);
        self.button(
            region,
            x,
            y + gap,
            "Collapse",
            Some(Icon::ChevronUp),
            ButtonKind::Ghost,
            Target::Collapse(key.clone()),
            width,
        );
        gap + self.m.control + gap
    }

    // ── Decisions ────────────────────────────────────────────────────────

    /// A card asking for a decision on a tool call. Returns its height.
    pub(super) fn approval_card(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        approval: &ApprovalRequest,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let pad = self.m.px(12.0);
        let inner = width - pad * 2.0;
        let mut body = Region::default();
        let mut cursor = y + pad;
        let title = approval_title(approval, session.cwd());
        body.prims.push(Prim::Icon {
            icon: Icon::Shield,
            center: [x + pad + self.m.px(8.0), cursor + self.m.body.line_px / 2.0],
            size: self.m.icon,
            color: self.c.attention,
        });
        cursor += self.read_paragraph(
            &mut body,
            &[(
                &title,
                SpanPaint {
                    face: UiFace::SANS_BOLD,
                    color: self.c.text_strong,
                },
            )],
            x + pad + self.m.px(24.0),
            cursor,
            inner - self.m.px(24.0),
            self.m.body,
            true,
        );
        cursor += self.m.px(8.0);
        if let Some(subject) = approval_subject(approval) {
            let box_pad = self.m.px(9.0);
            let mut code = Region::default();
            let layout = self.paragraph(
                &mut code,
                &[(
                    &subject,
                    SpanPaint {
                        face: UiFace::MONO,
                        color: self.c.code_text,
                    },
                )],
                x + pad + box_pad,
                cursor + box_pad,
                inner - box_pad * 2.0,
                self.m.mono,
            );
            let bounds = Bounds::new(x + pad, cursor, inner, layout.height + box_pad * 2.0);
            body.prims.push(Prim::Fill {
                bounds,
                radius: self.m.px(6.0),
                color: self.c.well,
            });
            body.nodes.push(RegionNode::Text(TextNode {
                bounds,
                clip: None,
                text: subject.clone(),
                heading: false,
            }));
            merge(&mut body, code);
            cursor += bounds.height + self.m.px(8.0);
        }
        if let Some(suggestion) = session.transcript().suggestion(&approval.request_id) {
            let verb = if suggestion.allow {
                "allowing"
            } else {
                "denying"
            };
            let text = if suggestion.reason.is_empty() {
                format!("The supervisor suggests {verb}.")
            } else {
                format!("The supervisor suggests {verb}: {}", suggestion.reason)
            };
            body.prims.push(Prim::Icon {
                icon: Icon::Eye,
                center: [
                    x + pad + self.m.px(7.0),
                    cursor + self.m.small.line_px / 2.0,
                ],
                size: self.m.px(13.0),
                color: self.c.supervisor,
            });
            cursor += self.read_paragraph(
                &mut body,
                &[(
                    &text,
                    SpanPaint {
                        face: UiFace::SANS,
                        color: self.c.supervisor,
                    },
                )],
                x + pad + self.m.px(20.0),
                cursor,
                inner - self.m.px(20.0),
                self.m.small,
                false,
            );
            cursor += self.m.px(10.0);
        }
        let explanation = [approval.description.as_deref(), approval.reason.as_deref()]
            .into_iter()
            .flatten()
            .filter(|text| Some(*text) != approval_subject(approval).as_deref())
            .collect::<Vec<_>>()
            .join(". ");
        if !explanation.is_empty() {
            cursor += self.read_paragraph(
                &mut body,
                &[(
                    &explanation,
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
            cursor += self.m.px(10.0);
        }
        // Buttons wrap when the panel is narrow.
        let request_id = approval.request_id.clone();
        let mut buttons: Vec<(String, ButtonKind, Target)> = vec![(
            "Allow".into(),
            ButtonKind::Primary,
            Target::Allow {
                request_id: request_id.clone(),
            },
        )];
        if let Some(remember) = &approval.remember {
            buttons.push((
                remember.label.clone(),
                ButtonKind::Secondary,
                Target::AllowAlways {
                    request_id: request_id.clone(),
                },
            ));
        }
        buttons.push((
            "Deny".into(),
            ButtonKind::Ghost,
            Target::Deny { request_id },
        ));
        cursor = self.button_rows(&mut body, &buttons, x + pad, cursor, inner);
        cursor += pad;
        let bounds = Bounds::new(x, y, width, (cursor - y).round());
        self.card(
            region,
            bounds,
            self.m.radius,
            self.c.attention_surface,
            self.c.attention_border,
        );
        merge(region, body);
        bounds.height
    }

    /// A message the supervisor wants to send, for the person to decide.
    fn supervisor_request_card(
        &mut self,
        region: &mut Region,
        request: &super::SupervisorRequest,
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
        let title = format!("Send this to {}?", request.recipient);
        cursor += self.read_paragraph(
            &mut body,
            &[(
                &title,
                SpanPaint {
                    face: UiFace::SANS_BOLD,
                    color: self.c.text_strong,
                },
            )],
            x + pad + self.m.px(24.0),
            cursor,
            inner - self.m.px(24.0),
            self.m.body,
            true,
        );
        cursor += self.m.px(8.0);
        // The whole message, as it would be sent.
        let box_pad = self.m.px(9.0);
        let mut message = Region::default();
        let layout = self.paragraph(
            &mut message,
            &[(
                &request.text,
                SpanPaint {
                    face: UiFace::SANS,
                    color: self.c.text,
                },
            )],
            x + pad + box_pad,
            cursor + box_pad,
            inner - box_pad * 2.0,
            self.m.body,
        );
        let bounds = Bounds::new(x + pad, cursor, inner, layout.height + box_pad * 2.0);
        body.prims.push(Prim::Fill {
            bounds,
            radius: self.m.px(6.0),
            color: self.c.well,
        });
        body.nodes.push(RegionNode::Text(TextNode {
            bounds,
            clip: None,
            text: request.text.clone(),
            heading: false,
        }));
        merge(&mut body, message);
        cursor += bounds.height + self.m.px(10.0);
        let buttons = [
            (
                "Send".to_owned(),
                ButtonKind::Primary,
                Target::SupervisorSend,
            ),
            (
                "Don't send".to_owned(),
                ButtonKind::Ghost,
                Target::SupervisorDecline,
            ),
        ];
        cursor = self.button_rows(&mut body, &buttons, x + pad, cursor, inner);
        cursor += pad;
        let card = Bounds::new(x, y, width, (cursor - y).round());
        self.card(
            region,
            card,
            self.m.radius,
            self.c.supervisor_surface,
            self.c.supervisor,
        );
        merge(region, body);
        card.height
    }

    /// Offers the agent's terminal for a command it does not run here.
    fn run_in_terminal(
        &mut self,
        region: &mut Region,
        name: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let top = y + self.m.px(6.0);
        let note = format!(
            "{} runs /{name} only in its own terminal.",
            self.input.agent().name()
        );
        let text_height = self
            .paragraph(
                region,
                &[(
                    &note,
                    SpanPaint {
                        face: UiFace::SANS,
                        color: self.c.text_secondary,
                    },
                )],
                x,
                top,
                width,
                self.m.small,
            )
            .height;
        let buttons = [(
            "Open in a terminal".to_owned(),
            ButtonKind::Secondary,
            Target::RunInTerminal,
        )];
        let bottom = self.button_rows(
            region,
            &buttons,
            x,
            top + text_height + self.m.px(6.0),
            width,
        );
        bottom - y
    }

    /// Lays buttons out left to right, wrapping, with the last one pushed
    /// right when it fits on the first row. Returns the bottom.
    fn button_rows(
        &mut self,
        region: &mut Region,
        buttons: &[(String, ButtonKind, Target)],
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let gap = self.m.px(8.0);
        let mut cursor_x = x;
        let mut cursor_y = y;
        let widths: Vec<f32> = buttons
            .iter()
            .map(|(text, kind, _)| {
                let face = if *kind == ButtonKind::Primary {
                    UiFace::SANS_BOLD
                } else {
                    UiFace::SANS
                };
                (self.text_width(text, face, self.m.body) + self.m.px(28.0))
                    .ceil()
                    .min(width)
            })
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len().saturating_sub(1)) as f32;
        let one_row = total <= width;
        for (index, ((text, kind, target), button_width)) in buttons.iter().zip(&widths).enumerate()
        {
            if cursor_x > x && cursor_x + button_width > x + width {
                cursor_x = x;
                cursor_y += self.m.control + gap;
            }
            let last = index + 1 == buttons.len();
            let at = if one_row && last && *kind == ButtonKind::Ghost {
                x + width - button_width
            } else {
                cursor_x
            };
            self.button(
                region,
                at,
                cursor_y,
                text,
                None,
                *kind,
                target.clone(),
                width,
            );
            cursor_x += button_width + gap;
        }
        cursor_y + self.m.control
    }

    fn question_card(
        &mut self,
        region: &mut Region,
        view: &ConversationView,
        approval: &ApprovalRequest,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let pad = self.m.px(12.0);
        let inner = width - pad * 2.0;
        let questions = present::questions(&approval.input);
        let mut body = Region::default();
        let mut cursor = y + pad;
        let heading = format!("{} asks", self.input.agent().short_name());
        self.label(
            &mut body,
            &heading,
            x + pad,
            cursor,
            self.m.small.line_px,
            inner,
            UiFace::SANS,
            self.m.small,
            self.c.text_muted,
        );
        cursor += self.m.small.line_px + self.m.px(4.0);
        let request_id = approval.request_id.clone();
        let mut complete = true;
        for (question_index, question) in questions.iter().enumerate() {
            cursor += self.read_paragraph(
                &mut body,
                &[(
                    &question.question,
                    SpanPaint {
                        face: UiFace::SANS_BOLD,
                        color: self.c.text_strong,
                    },
                )],
                x + pad,
                cursor,
                inner,
                self.m.body,
                true,
            );
            cursor += self.m.px(6.0);
            let chosen = view.chosen(&request_id, question_index);
            let typed = view.typed_answer(&request_id, question_index);
            if chosen.is_none_or(|chosen| chosen.is_empty()) && typed.is_none() {
                complete = false;
            }
            let options = question
                .options
                .iter()
                .map(|option| (option.label.as_str(), option.description.as_str()));
            let other_description = typed.unwrap_or("Type your own answer below");
            for (option_index, (label, description)) in options
                .chain(std::iter::once(("Other", other_description)))
                .enumerate()
            {
                let is_other = option_index == question.options.len();
                let selected = if is_other {
                    typed.is_some()
                } else {
                    chosen.is_some_and(|chosen| chosen.contains(&option_index))
                };
                let target = if is_other {
                    Target::AnswerOther {
                        request_id: request_id.clone(),
                        question: question_index,
                    }
                } else {
                    Target::ChooseOption {
                        request_id: request_id.clone(),
                        question: question_index,
                        option: option_index,
                    }
                };
                let marker = self.m.px(14.0);
                let text_x = x + pad + marker + self.m.px(12.0);
                let text_width = inner - marker - self.m.px(12.0);
                let mut option = Region::default();
                let label_height = self
                    .paragraph(
                        &mut option,
                        &[(
                            label,
                            SpanPaint {
                                face: UiFace::SANS,
                                color: self.c.text_strong,
                            },
                        )],
                        text_x,
                        cursor + self.m.px(6.0),
                        text_width,
                        self.m.body,
                    )
                    .height;
                let description_height = if description.is_empty() {
                    0.0
                } else {
                    self.paragraph(
                        &mut option,
                        &[(
                            description,
                            SpanPaint {
                                face: UiFace::SANS,
                                color: self.c.text_muted,
                            },
                        )],
                        text_x,
                        cursor + self.m.px(6.0) + label_height,
                        text_width,
                        self.m.small,
                    )
                    .height
                };
                let row = Bounds::new(
                    x + pad - self.m.px(6.0),
                    cursor,
                    inner + self.m.px(12.0),
                    label_height + description_height + self.m.px(12.0),
                );
                if selected || self.hovered(&target) {
                    body.prims.push(Prim::Fill {
                        bounds: row,
                        radius: self.m.px(6.0),
                        color: self.c.surface_hover,
                    });
                }
                let marker_bounds = Bounds::new(
                    x + pad,
                    cursor + self.m.px(6.0) + (self.m.body.line_px - marker) / 2.0,
                    marker,
                    marker,
                );
                let radius = if question.multiple {
                    self.m.px(3.0)
                } else {
                    marker / 2.0
                };
                body.prims.push(Prim::Outline {
                    bounds: marker_bounds,
                    radius,
                    stroke: self.m.px(1.5),
                    color: if selected {
                        self.c.accent
                    } else {
                        self.c.text_muted
                    },
                });
                if selected {
                    body.prims.push(Prim::Fill {
                        bounds: marker_bounds.inset(self.m.px(3.5), self.m.px(3.5)),
                        radius: if question.multiple {
                            self.m.px(1.5)
                        } else {
                            marker
                        },
                        color: self.c.accent,
                    });
                }
                merge(&mut body, option);
                let role = HitRole::Option { selected };
                let spoken = if description.is_empty() {
                    label.to_owned()
                } else {
                    format!("{label}. {description}")
                };
                self.hit(&mut body, row, target, role, spoken, true);
                cursor += row.height + self.m.px(2.0);
            }
            cursor += self.m.px(8.0);
        }
        let submit = Target::SubmitAnswers {
            request_id: request_id.clone(),
        };
        let buttons = vec![
            (
                if complete {
                    "Submit".to_owned()
                } else {
                    "Choose an answer".to_owned()
                },
                if complete {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                },
                submit,
            ),
            (
                "Skip".to_owned(),
                ButtonKind::Ghost,
                Target::Deny { request_id },
            ),
        ];
        cursor = self.button_rows(&mut body, &buttons, x + pad, cursor, inner);
        cursor += pad;
        let bounds = Bounds::new(x, y, width, (cursor - y).round());
        self.card(
            region,
            bounds,
            self.m.radius,
            self.c.attention_surface,
            self.c.attention_border,
        );
        merge(region, body);
        bounds.height
    }

    fn answered_question(
        &mut self,
        region: &mut Region,
        tool: &ToolCall,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let pad = self.m.px(12.0);
        let inner = width - pad * 2.0;
        let questions = present::questions(&tool.input);
        let answers = present::recorded_answers(tool);
        let mut body = Region::default();
        let mut cursor = y + self.m.px(10.0);
        let heading = format!("{} asked", self.input.agent().short_name());
        self.label(
            &mut body,
            &heading,
            x + pad,
            cursor,
            self.m.small.line_px,
            inner,
            UiFace::SANS,
            self.m.small,
            self.c.text_muted,
        );
        cursor += self.m.small.line_px + self.m.px(2.0);
        for question in &questions {
            cursor += self.read_paragraph(
                &mut body,
                &[(
                    &question.question,
                    SpanPaint {
                        face: UiFace::SANS,
                        color: self.c.text,
                    },
                )],
                x + pad,
                cursor,
                inner,
                self.m.body,
                false,
            );
            let answer = answers
                .iter()
                .find(|(asked, _)| *asked == question.question)
                .map(|(_, answer)| answer.clone());
            let (text, color, icon) = match (&answer, tool.state(false)) {
                (Some(answer), _) => (
                    format!("You chose: {answer}"),
                    self.c.text_strong,
                    Icon::Check,
                ),
                (None, ToolState::Denied) => ("Skipped".to_owned(), self.c.text_muted, Icon::Cross),
                (None, _) => ("Not answered".to_owned(), self.c.text_muted, Icon::Stop),
            };
            body.prims.push(Prim::Icon {
                icon,
                center: [
                    x + pad + self.m.px(6.0),
                    cursor + self.m.small.line_px / 2.0,
                ],
                size: self.m.px(12.0),
                color: if answer.is_some() {
                    self.c.success
                } else {
                    color
                },
            });
            cursor += self.read_paragraph(
                &mut body,
                &[(
                    &text,
                    SpanPaint {
                        face: UiFace::SANS,
                        color,
                    },
                )],
                x + pad + self.m.px(18.0),
                cursor,
                inner - self.m.px(18.0),
                self.m.small,
                false,
            );
            cursor += self.m.px(6.0);
        }
        cursor += self.m.px(4.0);
        let bounds = Bounds::new(x, y, width, (cursor - y).round());
        region.prims.push(Prim::Outline {
            bounds,
            radius: self.m.radius,
            stroke: self.m.stroke,
            color: self.c.border,
        });
        merge(region, body);
        bounds.height
    }

    fn plan_card(
        &mut self,
        region: &mut Region,
        tool: &ToolCall,
        approval: Option<&ApprovalRequest>,
        modes: &[ModeChoice],
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let pad = self.m.px(14.0);
        let inner = width - pad * 2.0;
        let header = self.m.px(38.0);
        let mut body = Region::default();
        let mut cursor = y + header + self.m.px(10.0);
        match present::plan_text(&tool.input) {
            Some(plan) => cursor += self.markdown(&mut body, plan, x + pad, cursor, inner),
            None => {
                let finished = format!("{} finished planning.", self.input.agent().short_name());
                cursor += self.read_paragraph(
                    &mut body,
                    &[(
                        &finished,
                        SpanPaint {
                            face: UiFace::SANS,
                            color: self.c.text_secondary,
                        },
                    )],
                    x + pad,
                    cursor,
                    inner,
                    self.m.body,
                    false,
                );
            }
        }
        cursor += self.m.px(12.0);
        match approval {
            Some(approval) => {
                let request_id = approval.request_id.clone();
                // Work starts in one of the agent's first two ordinary modes,
                // such as asking before edits or editing without asking.
                let mut buttons: Vec<(String, ButtonKind, Target)> = modes
                    .iter()
                    .filter(|mode| !mode.plans && !mode.unrestricted)
                    .take(2)
                    .enumerate()
                    .map(|(index, mode)| {
                        let mut label = mode.label.chars();
                        let label = label
                            .next()
                            .map(|first| first.to_lowercase().chain(label).collect::<String>())
                            .unwrap_or_default();
                        (
                            format!("Start, {label}"),
                            if index == 0 {
                                ButtonKind::Primary
                            } else {
                                ButtonKind::Secondary
                            },
                            Target::ApprovePlan {
                                request_id: request_id.clone(),
                                mode: mode.id.clone(),
                            },
                        )
                    })
                    .collect();
                buttons.push((
                    "Keep planning".to_owned(),
                    ButtonKind::Ghost,
                    Target::KeepPlanning { request_id },
                ));
                cursor = self.button_rows(&mut body, &buttons, x + pad, cursor, inner);
                cursor += pad;
            }
            None => {
                let (text, color, icon) = match tool.state(false) {
                    ToolState::Succeeded => ("Approved", self.c.success, Icon::Check),
                    ToolState::Denied | ToolState::Failed => {
                        ("Kept planning", self.c.text_muted, Icon::Plan)
                    }
                    _ => ("Not answered", self.c.text_muted, Icon::Stop),
                };
                body.prims.push(Prim::Icon {
                    icon,
                    center: [
                        x + pad + self.m.px(6.0),
                        cursor + self.m.small.line_px / 2.0,
                    ],
                    size: self.m.px(12.0),
                    color,
                });
                self.label(
                    &mut body,
                    text,
                    x + pad + self.m.px(18.0),
                    cursor,
                    self.m.small.line_px,
                    inner,
                    UiFace::SANS,
                    self.m.small,
                    color,
                );
                body.nodes.push(RegionNode::Text(TextNode {
                    bounds: Bounds::new(x + pad, cursor, inner, self.m.small.line_px),
                    clip: None,
                    text: text.to_owned(),
                    heading: false,
                }));
                cursor += self.m.small.line_px + self.m.px(10.0);
            }
        }
        let bounds = Bounds::new(x, y, width, (cursor - y).round());
        let (border, surface) = if approval.is_some() {
            (self.c.attention_border, self.c.panel)
        } else {
            (self.c.border, self.c.panel)
        };
        self.header_card(region, bounds, header, true, surface, border);
        region.prims.push(Prim::Icon {
            icon: Icon::Plan,
            center: [x + pad + self.m.px(7.0), y + header / 2.0],
            size: self.m.px(14.0),
            color: self.c.attention,
        });
        self.label(
            region,
            "Plan",
            x + pad + self.m.px(22.0),
            y,
            header,
            inner,
            UiFace::SANS_BOLD,
            self.m.body,
            self.c.text_strong,
        );
        region.nodes.push(RegionNode::Text(TextNode {
            bounds: Bounds::new(x, y, width, header),
            clip: None,
            text: "Plan".into(),
            heading: true,
        }));
        merge(region, body);
        bounds.height
    }

    // ── Turn endings and notices ─────────────────────────────────────────

    fn turn_end(
        &mut self,
        region: &mut Region,
        end: &TurnEnd,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        region.prims.push(Prim::Fill {
            bounds: Bounds::new(x, y, width, self.m.stroke),
            radius: 0.0,
            color: self.c.border,
        });
        let top = y + self.m.px(8.0);
        let height = self.m.control;
        let (icon, color, text) = if end.interrupted {
            (Icon::Stop, self.c.text_muted, "Stopped".to_owned())
        } else if end.outcome.success {
            (
                Icon::Check,
                self.c.success,
                format!("Done in {}", duration(end.outcome.duration)),
            )
        } else {
            (
                Icon::Cross,
                self.c.danger,
                end.outcome
                    .error
                    .clone()
                    .unwrap_or_else(|| "The turn failed".into()),
            )
        };
        let mut summary = text;
        if !end.files_changed.is_empty() {
            summary.push_str(" · ");
            summary.push_str(&plural(
                end.files_changed.len(),
                "file changed",
                "files changed",
            ));
        }
        region.prims.push(Prim::Icon {
            icon,
            center: [x + self.m.px(8.0), top + height / 2.0],
            size: self.m.px(14.0),
            color,
        });
        let copy = Bounds::new(
            x + width - self.m.control,
            top,
            self.m.control,
            self.m.control,
        );
        let mut right = copy.x - self.m.px(6.0);
        if !end.files_changed.is_empty() {
            let review_width =
                self.text_width("Review changes", UiFace::SANS, self.m.body) + self.m.px(28.0);
            right -= review_width;
            self.button(
                region,
                right,
                top,
                "Review changes",
                None,
                ButtonKind::Secondary,
                Target::ReviewChanges(index),
                f32::INFINITY,
            );
            right -= self.m.px(8.0);
        }
        self.icon_button(
            region,
            copy,
            Icon::Copy,
            Target::CopyItem(index),
            "Copy the answer",
            self.c.text_muted,
        );
        let text_x = x + self.m.px(22.0);
        let text_color = if end.outcome.success || end.interrupted {
            self.c.text_secondary
        } else {
            self.c.danger
        };
        let layout = self.layout(
            &[TextSpan {
                text: &summary,
                face: UiFace::SANS,
            }],
            self.m.small,
            Some((right - text_x).max(self.m.px(60.0))),
        );
        let text_top = if layout.lines.len() <= 1 {
            top + (height - self.m.small.line_px) / 2.0
        } else {
            top
        };
        region.prims.push(Prim::Paragraph {
            layout: Arc::clone(&layout),
            x: text_x,
            y: text_top.round(),
            font_px: self.m.small.font_px,
            paint: Arc::new([SpanPaint {
                face: UiFace::SANS,
                color: text_color,
            }]),
        });
        region.nodes.push(RegionNode::Text(TextNode {
            bounds: Bounds::new(text_x, text_top, right - text_x, layout.height),
            clip: None,
            text: summary,
            heading: false,
        }));
        (top - y) + height.max(layout.height)
    }

    fn notice(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        notice: &Notice,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let (icon, color) = match notice.kind {
            NoticeKind::Info => (None, self.c.text_muted),
            NoticeKind::Warning => (Some(Icon::Warning), self.c.attention),
            NoticeKind::Error => (Some(Icon::Warning), self.c.danger),
        };
        let indent = if icon.is_some() { self.m.px(22.0) } else { 0.0 };
        if let Some(icon) = icon {
            region.prims.push(Prim::Icon {
                icon,
                center: [x + self.m.px(8.0), y + self.m.body.line_px / 2.0],
                size: self.m.px(14.0),
                color,
            });
        }
        let text_color = if notice.kind == NoticeKind::Info {
            self.c.text_muted
        } else {
            self.c.text
        };
        let mut height = self.read_paragraph(
            region,
            &[(
                &notice.text,
                SpanPaint {
                    face: UiFace::SANS,
                    color: text_color,
                },
            )],
            x + indent,
            y,
            width - indent,
            self.m.body,
            false,
        );
        // The last error of a closed agent offers ways to continue.
        let is_last = session
            .transcript()
            .items()
            .last()
            .is_some_and(|item| matches!(item, Item::Notice(last) if last == notice));
        if notice.kind == NoticeKind::Error && is_last && !session.is_running() {
            height += self.m.px(8.0);
            let buttons = vec![
                (
                    "Continue".to_owned(),
                    ButtonKind::Secondary,
                    Target::Relaunch,
                ),
                (
                    "Open in a terminal".to_owned(),
                    ButtonKind::Ghost,
                    Target::OpenInTerminal,
                ),
            ];
            height = self.button_rows(region, &buttons, x + indent, y + height, width - indent) - y;
        }
        height
    }

    fn status_line(
        &mut self,
        region: &mut Region,
        session: &AgentSession,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let transcript = session.transcript();
        let text = if session.activity() == Activity::Starting && session.is_running() {
            Some(format!("Starting {}…", session.agent().name()))
        } else if transcript.turn_active() && transcript.approvals().is_empty() {
            Some(if transcript.interrupting() {
                "Stopping…".to_owned()
            } else if transcript.compacting() {
                "Compacting the conversation…".to_owned()
            } else if let Some((retry, since)) = transcript.retry() {
                // An agent may not say how long it waits or how often it tries.
                let wait = retry.delay.saturating_sub(since.elapsed());
                let when = if retry.delay.is_zero() {
                    "Retrying".to_owned()
                } else {
                    format!("Retrying in {}s", wait.as_secs() + 1)
                };
                let attempt = match retry.max_attempts {
                    0 => format!("attempt {}", retry.attempt),
                    max => format!("attempt {} of {max}", retry.attempt),
                };
                format!("{} {when} ({attempt})", retry.error)
            } else {
                let elapsed = transcript.turn_elapsed().map(duration).unwrap_or_default();
                format!("Working · {elapsed}")
            })
        } else {
            None
        };
        let Some(text) = text else { return y };
        let height = self.m.control;
        region.prims.push(Prim::Spinner {
            center: [x + self.m.px(8.0), y + height / 2.0],
            radius: self.m.px(6.0),
            thickness: self.m.px(1.8),
            phase: spinner_phase(self.input.now, self.input.state.reduced_motion()),
            color: self.c.accent,
        });
        self.label(
            region,
            &text,
            x + self.m.px(24.0),
            y,
            height,
            width - self.m.px(24.0),
            UiFace::SANS,
            self.m.small,
            self.c.text_secondary,
        );
        region.nodes.push(RegionNode::Text(TextNode {
            bounds: Bounds::new(x, y, width, height),
            clip: None,
            text,
            heading: false,
        }));
        y + height
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SpanTone {
    Body,
    Heading,
    Quiet,
}

/// What a row's right edge shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowStatus {
    None,
    Preparing,
    Running,
    NeedsApproval,
    Succeeded,
    Failed,
    Denied,
    Stopped,
}

impl RowStatus {
    fn text(self) -> &'static str {
        match self {
            RowStatus::None | RowStatus::Running | RowStatus::Succeeded => "",
            RowStatus::Preparing => "Writing…",
            RowStatus::NeedsApproval => "Needs approval",
            RowStatus::Failed => "Failed",
            RowStatus::Denied => "Denied",
            RowStatus::Stopped => "Stopped",
        }
    }
}

fn row_status(tool: &ToolCall, awaiting: bool) -> RowStatus {
    match tool.state(awaiting) {
        ToolState::Preparing => RowStatus::Preparing,
        ToolState::AwaitingApproval => RowStatus::NeedsApproval,
        ToolState::Running => RowStatus::Running,
        ToolState::Succeeded => RowStatus::Succeeded,
        ToolState::Failed => RowStatus::Failed,
        ToolState::Denied => RowStatus::Denied,
        ToolState::Stopped => RowStatus::Stopped,
    }
}

/// A line under a tool call saying who decided and why, when that matters:
/// the supervisor decided, or a rule was remembered.
fn decision_note(tool: &ToolCall) -> Option<(String, bool)> {
    let decision = tool.decision.as_ref()?;
    let text = match (decision.by, decision.allowed) {
        (DecidedBy::Supervisor, true) => match &decision.note {
            Some(note) => format!("Allowed by the supervisor: {note}"),
            None => "Allowed by the supervisor".to_owned(),
        },
        (DecidedBy::Supervisor, false) => match &decision.note {
            Some(note) => format!("Denied by the supervisor: {note}"),
            None => "Denied by the supervisor".to_owned(),
        },
        (DecidedBy::Person, true) => decision.remembered.clone()?,
        (DecidedBy::Person, false) => format!("You denied this: {}", decision.note.as_ref()?),
    };
    Some((text, decision.allowed))
}

fn approval_title(approval: &ApprovalRequest, cwd: &Path) -> String {
    let path = |key: &str| {
        approval
            .input
            .get(key)
            .and_then(|value| value.as_str())
            .map(|path| present::relative(path, cwd))
    };
    match approval.tool_name.as_str() {
        present::BASH_TOOL => "Run this command?".to_owned(),
        present::EDIT_TOOL | "MultiEdit" | "NotebookEdit" => {
            match path("file_path").or_else(|| path("notebook_path")) {
                Some(path) => format!("Edit {path}?"),
                None => "Make this edit?".to_owned(),
            }
        }
        present::WRITE_TOOL => match path("file_path") {
            Some(path) => format!("Write {path}?"),
            None => "Write this file?".to_owned(),
        },
        present::READ_TOOL => match path("file_path") {
            Some(path) => format!("Read {path}?"),
            None => "Read this file?".to_owned(),
        },
        present::WEB_FETCH_TOOL => "Fetch this page?".to_owned(),
        present::WEB_SEARCH_TOOL => "Search the web?".to_owned(),
        PATCH_TOOL => match present::patch_paths(&approval.input).as_slice() {
            [] => "Make these changes?".to_owned(),
            [path] => format!("Change {}?", present::relative(path, cwd)),
            paths => format!("Change {} files?", paths.len()),
        },
        present::PERMISSIONS_TOOL => "Grant these permissions?".to_owned(),
        _ => match &approval.title {
            Some(title) => title.clone(),
            None => format!("Allow {}?", present::summarize_name(&approval.tool_name)),
        },
    }
}

fn approval_subject(approval: &ApprovalRequest) -> Option<String> {
    let field = |key: &str| {
        approval
            .input
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    match approval.tool_name.as_str() {
        present::BASH_TOOL => field("command"),
        present::WEB_FETCH_TOOL => field("url"),
        present::WEB_SEARCH_TOOL => field("query"),
        present::GREP_TOOL | present::GLOB_TOOL => field("pattern"),
        _ => None,
    }
}

/// Claude wraps some tool errors in a tag; show only the message.
fn strip_tool_error(line: &str) -> String {
    line.trim_start_matches("<tool_use_error>")
        .trim_end_matches("</tool_use_error>")
        .trim()
        .to_owned()
}

/// "48s", "2m 5s", "1h 3m".
pub(super) fn duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// Spinners turn once a second, from a fixed origin so frames agree; with
/// reduced motion they hold still.
pub(super) fn spinner_phase(now: std::time::Instant, reduced_motion: bool) -> f32 {
    if reduced_motion {
        return 0.0;
    }
    use std::sync::OnceLock;
    static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(std::time::Instant::now);
    now.saturating_duration_since(origin).as_secs_f32().fract()
}

/// The command a person's latest message invoked, when the agent did not
/// list it among the commands it runs without its terminal. Only the latest
/// message offers the terminal: earlier ones were already answered.
fn unlisted_command<'a>(session: &AgentSession, index: usize, text: &'a str) -> Option<&'a str> {
    if session.handed_off() {
        return None;
    }
    let items = session.transcript().items();
    let latest = !items[index + 1..]
        .iter()
        .any(|item| matches!(item, Item::User { .. }));
    let listed = &session.transcript().handshake()?.commands;
    let name = crate::agent_view::event::invoked_name(text)?;
    (latest && !listed.iter().any(|command| command.name == name)).then_some(name)
}
