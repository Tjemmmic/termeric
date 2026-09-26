//! Turns the panel state and conversations into a [`Scene`].
//!
//! Layout happens in physical pixels. Text is measured through
//! [`TextMeasure`], so the same code runs against real fonts in the app and
//! fixed advances in tests. The scrolling conversation is laid out in content
//! coordinates first and then moved by the scroll offset, which lets the
//! panel follow new output exactly.

mod feed;
mod screens;

pub use screens::session_summary;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::agent_view::event::{FALLBACK_MODELS, ModelChoice, PermissionMode, model_name};
use crate::agent_view::session::AgentSession;
use crate::agent_view::transcript::Activity;
use crate::ui::text_layout::{
    TextLayout, TextMeasure, TextSize, TextSpan, UiFace, fit_with_ellipsis,
};

use super::colors::Colors;
use super::scene::{
    Bounds, Caret, ComposerGeometry, Hit, HitRole, Icon, Layer, MenuAction, MenuKind, Node, Prim,
    Scene, SpanPaint, Target, TextNode, composer_layout,
};
use super::{AgentViewState, Focus, Layout, Screen};

/// Points per UI body line, the desktop's usual chrome size.
const BODY_POINTS: f32 = 10.0;
const SMALL_POINTS: f32 = 9.0;
const TITLE_POINTS: f32 = 10.5;
const PIXELS_PER_POINT: f32 = 96.0 / 72.0;

/// A terminal-hosted agent, listed beside the panel's own sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalAgentRow {
    pub identity: crate::tabs::TerminalIdentity,
    pub name: String,
    pub place: String,
    pub summary: String,
    pub needs_you: bool,
}

/// An earlier conversation that can be continued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastSession {
    pub id: String,
    pub title: String,
    pub when: String,
}

/// Context the person can attach from the focused terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suggestions {
    /// Lines in the terminal selection.
    pub selection_lines: Option<usize>,
    /// The last command's text, its output lines, and whether it failed.
    pub last_command: Option<(String, usize, bool)>,
}

/// A notice across the top of a conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub title: String,
    pub detail: String,
    pub action: Option<(String, Target)>,
    /// The supervisor's voice rather than a problem.
    pub supervisor: bool,
}

/// The supervisor's state, for the sessions page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupervisorCard {
    pub status: String,
    pub latest: Option<String>,
    pub held: bool,
}

/// What a new session would start with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSessionInfo {
    pub cwd: PathBuf,
    /// Where the directory came from, e.g. "From the focused terminal".
    pub cwd_source: String,
    /// Other directories to offer, from open terminals.
    pub cwd_choices: Vec<PathBuf>,
    pub availability: AgentAvailability,
}

/// Whether Claude Code can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentAvailability {
    Ready,
    NotInstalled,
    /// Still asking the login shell where programs are.
    Looking,
}

/// A message the supervisor asked to send, waiting for the person in the
/// supervisor's conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupervisorRequest {
    pub session: crate::agent_view::session::SessionId,
    pub recipient: String,
    pub text: String,
}

/// Everything a scene is built from.
pub struct Input<'a> {
    /// The panel's area in physical window pixels.
    pub panel: Bounds,
    pub scale: f32,
    pub now: Instant,
    pub colors: &'a Colors,
    pub state: &'a AgentViewState,
    /// This window's sessions, most recent first.
    pub sessions: &'a [&'a AgentSession],
    pub suggestions: Suggestions,
    pub banner: Option<Banner>,
    pub terminal_agents: &'a [TerminalAgentRow],
    pub past: &'a [PastSession],
    pub supervisor: Option<SupervisorCard>,
    pub new_session: NewSessionInfo,
    pub home: Option<PathBuf>,
    /// The person opted in to modes that skip every check.
    pub allow_bypass: bool,
    /// A message the supervisor waits for the person to approve.
    pub supervisor_request: Option<SupervisorRequest>,
}

impl Input<'_> {
    /// The shown session, when it holds a conversation: a session started
    /// ahead of its first message belongs to the new-session page.
    fn active(&self) -> Option<&AgentSession> {
        self.tab_session().filter(|session| !session.is_blank())
    }

    /// The tab's session, including one started ahead of its first message.
    fn tab_session(&self) -> Option<&AgentSession> {
        let id = self.state.active?;
        self.sessions
            .iter()
            .copied()
            .find(|session| session.id() == id)
    }
}

/// Widest the conversation grows, in logical pixels, so lines stay easy to
/// read when the panel fills a wide tab.
const COLUMN_WIDTH: f32 = 860.0;

/// Sizes in physical pixels.
#[derive(Clone, Copy, Debug)]
struct Metrics {
    scale: f32,
    body: TextSize,
    small: TextSize,
    mono: TextSize,
    title: TextSize,
    pad: f32,
    gap: f32,
    radius: f32,
    control: f32,
    chip: f32,
    icon: f32,
    stroke: f32,
}

impl Metrics {
    fn new(scale: f32) -> Self {
        let px = |logical: f32| (logical * scale).round();
        let text = |points: f32, line: f32| TextSize {
            font_px: points * PIXELS_PER_POINT * scale,
            line_px: px(line),
        };
        Self {
            scale,
            body: text(BODY_POINTS, 20.0),
            small: text(SMALL_POINTS, 17.0),
            mono: text(SMALL_POINTS, 18.0),
            title: text(TITLE_POINTS, 20.0),
            pad: px(16.0),
            gap: px(14.0),
            radius: px(8.0),
            control: px(30.0),
            chip: px(26.0),
            icon: px(16.0),
            stroke: px(1.0).max(1.0),
        }
    }

    fn px(&self, logical: f32) -> f32 {
        (logical * self.scale).round()
    }
}

/// Primitives, hits, and reading order for one part of the panel.
#[derive(Default)]
struct Region {
    prims: Vec<Prim>,
    hits: Vec<Hit>,
    nodes: Vec<RegionNode>,
}

enum RegionNode {
    Text(TextNode),
    Hit(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
}

struct Builder<'a, 'b> {
    input: &'a Input<'b>,
    m: Metrics,
    c: &'a Colors,
    measure: &'a mut dyn TextMeasure,
    /// Clip applied to hits pushed into the region being built.
    clip: Option<Bounds>,
    caret: Option<Caret>,
    composer_geometry: Option<ComposerGeometry>,
    /// The message box, which the command menu opens above.
    composer_box: Option<Bounds>,
}

/// Builds the scene for one frame.
pub fn build(input: &Input<'_>, measure: &mut dyn TextMeasure) -> Scene {
    let mut builder = Builder {
        input,
        m: Metrics::new(input.scale),
        c: input.colors,
        measure,
        clip: None,
        caret: None,
        composer_geometry: None,
        composer_box: None,
    };
    builder.scene()
}

impl Builder<'_, '_> {
    fn scene(&mut self) -> Scene {
        let input = self.input;
        let panel = self.slid_panel();
        let mut base = Region::default();
        let mut overlay = Region::default();
        base.prims.push(Prim::Fill {
            bounds: panel,
            radius: 0.0,
            color: self.c.panel,
        });
        base.prims.push(Prim::Fill {
            bounds: Bounds::new(panel.x, panel.y, self.m.stroke, panel.height),
            radius: 0.0,
            color: self.c.border,
        });

        let header = self.header(&mut base, panel);
        // A panel filling a wide tab keeps its content to a readable column.
        let column = self.column(panel);
        let mut top = header.bottom();
        if let Some(banner) = self.input.banner.clone()
            && self.input.state.screen == Screen::Conversation
        {
            top = self.banner(&mut base, column, top, &banner);
        }
        let composer_top = match self.input.state.screen {
            Screen::Sessions => panel.bottom(),
            _ => self.composer(&mut base, column),
        };
        let viewport = Bounds::new(column.x, top, column.width, (composer_top - top).max(0.0));

        let (mut feed, feed_height) = match input.state.screen {
            Screen::Conversation => match input.active() {
                Some(session) => self.conversation(session, viewport),
                None => (Region::default(), 0.0),
            },
            Screen::Sessions => self.sessions_page(viewport),
            Screen::NewSession => self.new_session_page(viewport),
        };
        let scroll = match self.input.state.screen {
            Screen::Conversation => self
                .input
                .state
                .active
                .and_then(|id| self.input.state.conversation(id))
                .map(|view| &view.scroll),
            Screen::Sessions => Some(&self.input.state.sessions_scroll),
            Screen::NewSession => Some(&self.input.state.new_session.scroll),
        };
        let max = (feed_height - viewport.height).max(0.0);
        let offset = scroll.map_or(0.0, |scroll| scroll.offset(max, self.input.now));
        let shift = (viewport.y - offset).round();
        translate(&mut feed, shift, viewport);

        if max > 0.0 {
            self.scrollbar(&mut feed, viewport, feed_height, offset);
        }
        if self.input.state.screen == Screen::Conversation
            && scroll.is_some_and(|scroll| !scroll.following())
            && max - offset > self.m.px(40.0)
        {
            self.jump_to_end(&mut overlay, viewport);
        }
        if let Some(menu) = &self.input.state.menu {
            self.menu(
                &mut overlay,
                panel,
                menu.kind,
                menu.anchor,
                menu.highlighted,
            );
        }
        if let Some(anchor) = self.composer_box {
            self.command_menu(&mut overlay, anchor);
        }
        self.focus_ring(&mut base, &mut feed, &mut overlay, viewport);

        let caret = self.caret;
        let mut scene = assemble(
            panel,
            base,
            feed,
            overlay,
            viewport,
            feed_height,
            caret,
            self.composer_geometry.take(),
        );
        scene.background = self.c.panel;
        scene
    }
}

impl Builder<'_, '_> {
    fn slid_panel(&self) -> Bounds {
        let progress = self.input.state.open_progress(self.input.now);
        let offset = ((1.0 - progress) * self.m.px(24.0)).round();
        Bounds {
            x: self.input.panel.x + offset,
            ..self.input.panel
        }
    }
}

fn translate(region: &mut Region, dy: f32, viewport: Bounds) {
    for prim in &mut region.prims {
        match prim {
            Prim::Fill { bounds, .. } | Prim::Outline { bounds, .. } => bounds.y += dy,
            Prim::Paragraph { y, .. } => *y += dy,
            Prim::Label { baseline, .. } => *baseline += dy,
            Prim::Icon { center, .. } | Prim::Spinner { center, .. } | Prim::Arc { center, .. } => {
                center[1] += dy;
            }
        }
    }
    region.prims.retain(|prim| visible(prim, viewport));
    for hit in &mut region.hits {
        hit.bounds.y += dy;
        hit.clip = Some(viewport);
    }
    for node in &mut region.nodes {
        if let RegionNode::Text(text) = node {
            text.bounds.y += dy;
            text.clip = Some(viewport);
        }
    }
    region.nodes.retain(|node| match node {
        RegionNode::Text(text) => text.bounds.intersect(viewport).is_some(),
        RegionNode::Hit(_) => true,
    });
}

fn visible(prim: &Prim, viewport: Bounds) -> bool {
    let (top, bottom) = match prim {
        Prim::Fill { bounds, .. } | Prim::Outline { bounds, .. } => (bounds.y, bounds.bottom()),
        Prim::Paragraph { layout, y, .. } => (*y, *y + layout.height),
        Prim::Label {
            baseline, font_px, ..
        } => (*baseline - *font_px * 1.5, *baseline + *font_px),
        Prim::Icon { center, size, .. } => (center[1] - size, center[1] + size),
        Prim::Spinner { center, radius, .. } | Prim::Arc { center, radius, .. } => {
            (center[1] - radius, center[1] + radius)
        }
    };
    bottom >= viewport.y && top <= viewport.bottom()
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    panel: Bounds,
    base: Region,
    feed: Region,
    overlay: Region,
    viewport: Bounds,
    feed_height: f32,
    caret: Option<Caret>,
    composer_text: Option<ComposerGeometry>,
) -> Scene {
    let mut scene = Scene {
        panel,
        background: [0.0; 4],
        base: Layer {
            clip: Some(panel),
            prims: base.prims,
        },
        feed: Layer {
            clip: Some(viewport),
            prims: feed.prims,
        },
        overlay: Layer {
            clip: Some(panel),
            prims: overlay.prims,
        },
        hits: Vec::new(),
        nodes: Vec::new(),
        feed_viewport: viewport,
        feed_height,
        caret,
        composer_text,
    };
    // Reading and Tab order: header, conversation, composer, menus. The base
    // region holds both header and composer, split at the viewport.
    let (header_hits, composer_hits): (Vec<_>, Vec<_>) = base
        .hits
        .into_iter()
        .enumerate()
        .partition(|(_, hit)| hit.bounds.y < viewport.y);
    let mut remap_base = vec![0; header_hits.len() + composer_hits.len()];
    let push_hits = |scene: &mut Scene, hits: Vec<(usize, Hit)>, remap: &mut Vec<usize>| {
        for (old, hit) in hits {
            remap[old] = scene.hits.len();
            scene.hits.push(hit);
        }
    };
    push_hits(&mut scene, header_hits, &mut remap_base);
    let feed_start = scene.hits.len();
    scene.hits.extend(feed.hits);
    push_hits(&mut scene, composer_hits, &mut remap_base);
    let overlay_start = scene.hits.len();
    scene.hits.extend(overlay.hits);

    let (header_nodes, composer_nodes): (Vec<_>, Vec<_>) =
        base.nodes.into_iter().partition(|node| match node {
            RegionNode::Text(text) => text.bounds.y < viewport.y,
            RegionNode::Hit(index) => scene.hits[remap_base[*index]].bounds.y < viewport.y,
        });
    let convert = |node: RegionNode, remap: &dyn Fn(usize) -> usize| match node {
        RegionNode::Text(text) => Node::Text(text),
        RegionNode::Hit(index) => Node::Hit(remap(index)),
    };
    let base_remap = |index: usize| remap_base[index];
    scene.nodes.extend(
        header_nodes
            .into_iter()
            .map(|node| convert(node, &base_remap)),
    );
    scene.nodes.extend(
        feed.nodes
            .into_iter()
            .map(|node| convert(node, &|index| feed_start + index)),
    );
    scene.nodes.extend(
        composer_nodes
            .into_iter()
            .map(|node| convert(node, &base_remap)),
    );
    scene.nodes.extend(
        overlay
            .nodes
            .into_iter()
            .map(|node| convert(node, &|index| overlay_start + index)),
    );
    scene
}

// ── Drawing helpers ──────────────────────────────────────────────────────

impl Builder<'_, '_> {
    fn layout(
        &mut self,
        spans: &[TextSpan<'_>],
        size: TextSize,
        width: Option<f32>,
    ) -> Arc<TextLayout> {
        self.measure.layout(spans, size, width)
    }

    /// Wrapped text at (`x`, `y`), returning its height.
    #[allow(clippy::too_many_arguments)]
    fn paragraph(
        &mut self,
        region: &mut Region,
        spans: &[(&str, SpanPaint)],
        x: f32,
        y: f32,
        width: f32,
        size: TextSize,
    ) -> Arc<TextLayout> {
        let text_spans: Vec<TextSpan<'_>> = spans
            .iter()
            .map(|(text, paint)| TextSpan {
                text,
                face: paint.face,
            })
            .collect();
        let layout = self.layout(&text_spans, size, Some(width.max(1.0)));
        region.prims.push(Prim::Paragraph {
            layout: Arc::clone(&layout),
            x: x.round(),
            y: y.round(),
            font_px: size.font_px,
            paint: spans.iter().map(|(_, paint)| *paint).collect(),
        });
        layout
    }

    /// Wrapped text that assistive technology also reads.
    #[allow(clippy::too_many_arguments)]
    fn read_paragraph(
        &mut self,
        region: &mut Region,
        spans: &[(&str, SpanPaint)],
        x: f32,
        y: f32,
        width: f32,
        size: TextSize,
        heading: bool,
    ) -> f32 {
        let layout = self.paragraph(region, spans, x, y, width, size);
        let text: String = spans.iter().map(|(text, _)| *text).collect();
        region.nodes.push(RegionNode::Text(TextNode {
            bounds: Bounds::new(x, y, width, layout.height),
            clip: None,
            text,
            heading,
        }));
        layout.height
    }

    /// One line of text, vertically centred in `top..top + height` and
    /// shortened with an ellipsis to `max_width`. Returns its width.
    #[allow(clippy::too_many_arguments)]
    fn label(
        &mut self,
        region: &mut Region,
        text: &str,
        x: f32,
        top: f32,
        height: f32,
        max_width: f32,
        face: UiFace,
        size: TextSize,
        color: [f32; 4],
    ) -> f32 {
        let measure = &mut *self.measure;
        let fitted = fit_with_ellipsis(text, max_width, |candidate| {
            measure.width(candidate, face, size.font_px)
        })
        .into_owned();
        if fitted.is_empty() {
            return 0.0;
        }
        let layout = self.layout(
            &[TextSpan {
                text: &fitted,
                face,
            }],
            size,
            None,
        );
        let line_top = top + ((height - size.line_px) / 2.0).round();
        let baseline = line_top
            + layout
                .lines
                .first()
                .map_or(size.font_px, |line| line.baseline);
        let width = layout.width;
        region.prims.push(Prim::Label {
            text: fitted,
            x: x.round(),
            baseline: baseline.round(),
            font_px: size.font_px,
            face,
            color,
        });
        width
    }

    fn text_width(&mut self, text: &str, face: UiFace, size: TextSize) -> f32 {
        self.measure.width(text, face, size.font_px)
    }

    fn hovered(&self, target: &Target) -> bool {
        self.input.state.hover.as_ref() == Some(target)
    }

    fn hit(
        &mut self,
        region: &mut Region,
        bounds: Bounds,
        target: Target,
        role: HitRole,
        label: impl Into<String>,
        focusable: bool,
    ) {
        region.nodes.push(RegionNode::Hit(region.hits.len()));
        region.hits.push(Hit {
            bounds,
            clip: self.clip,
            target,
            role,
            label: label.into(),
            focusable,
        });
    }

    /// A labelled button, at most `max_width` wide. Returns its width.
    #[allow(clippy::too_many_arguments)]
    fn button(
        &mut self,
        region: &mut Region,
        x: f32,
        y: f32,
        text: &str,
        icon: Option<Icon>,
        kind: ButtonKind,
        target: Target,
        max_width: f32,
    ) -> f32 {
        let size = self.m.body;
        let face = if kind == ButtonKind::Primary {
            UiFace::SANS_BOLD
        } else {
            UiFace::SANS
        };
        let padding = self.m.px(14.0);
        let icon_width = icon.map_or(0.0, |_| self.m.icon + self.m.px(6.0));
        let width = (self.text_width(text, face, size) + padding * 2.0 + icon_width)
            .ceil()
            .min(max_width.floor());
        let bounds = Bounds::new(x.round(), y.round(), width, self.m.control);
        let radius = self.m.control / 2.0;
        let hovered = self.hovered(&target);
        let (fill, text_color, border) = match kind {
            ButtonKind::Primary => (Some(self.c.accent), self.c.on_accent, None),
            ButtonKind::Secondary => (
                Some(self.c.surface),
                self.c.text_strong,
                Some(self.c.border),
            ),
            ButtonKind::Ghost => (None, self.c.text, None),
        };
        if let Some(fill) = fill {
            region.prims.push(Prim::Fill {
                bounds,
                radius,
                color: fill,
            });
        }
        if let Some(border) = border {
            region.prims.push(Prim::Outline {
                bounds,
                radius,
                stroke: self.m.stroke,
                color: border,
            });
        }
        if hovered {
            let hover = if kind == ButtonKind::Primary {
                [1.0, 1.0, 1.0, 0.12]
            } else {
                self.c.surface_hover
            };
            region.prims.push(Prim::Fill {
                bounds,
                radius,
                color: hover,
            });
        }
        let mut text_x = bounds.x + padding;
        if let Some(icon) = icon {
            region.prims.push(Prim::Icon {
                icon,
                center: [text_x + self.m.icon / 2.0, bounds.y + bounds.height / 2.0],
                size: self.m.icon,
                color: text_color,
            });
            text_x += icon_width;
        }
        self.label(
            region,
            text,
            text_x,
            bounds.y,
            bounds.height,
            bounds.right() - padding - text_x,
            face,
            size,
            text_color,
        );
        self.hit(region, bounds, target, HitRole::Button, text, true);
        width
    }

    /// A square icon-only button with an accessible name.
    fn icon_button(
        &mut self,
        region: &mut Region,
        bounds: Bounds,
        icon: Icon,
        target: Target,
        label: &str,
        color: [f32; 4],
    ) {
        if self.hovered(&target) {
            region.prims.push(Prim::Fill {
                bounds,
                radius: self.m.px(6.0),
                color: self.c.surface_hover,
            });
        }
        region.prims.push(Prim::Icon {
            icon,
            center: bounds.center(),
            size: self.m.icon,
            color,
        });
        self.hit(region, bounds, target, HitRole::Button, label, true);
    }

    /// A small pill. Returns its width.
    #[allow(clippy::too_many_arguments)]
    fn chip(
        &mut self,
        region: &mut Region,
        x: f32,
        y: f32,
        text: &str,
        icon: Option<Icon>,
        trailing: Option<Icon>,
        outlined: bool,
        dashed: bool,
        max_width: f32,
        target: Option<(Target, &str)>,
    ) -> f32 {
        let size = self.m.small;
        let padding = self.m.px(10.0);
        let icon_size = self.m.px(13.0);
        let leading = icon.map_or(0.0, |_| icon_size + self.m.px(6.0));
        let trailing_width = trailing.map_or(0.0, |_| icon_size + self.m.px(4.0));
        let text_width = self.text_width(text, UiFace::SANS, size);
        let width = (padding * 2.0 + leading + text_width + trailing_width)
            .min(max_width)
            .ceil();
        let bounds = Bounds::new(x.round(), y.round(), width, self.m.chip);
        let radius = self.m.chip / 2.0;
        let hovered = target
            .as_ref()
            .is_some_and(|(target, _)| self.hovered(target));
        if outlined {
            region.prims.push(Prim::Fill {
                bounds,
                radius,
                color: self.c.surface,
            });
        }
        if hovered {
            region.prims.push(Prim::Fill {
                bounds,
                radius,
                color: self.c.surface_hover,
            });
        }
        if outlined || dashed {
            region.prims.push(Prim::Outline {
                bounds,
                radius,
                stroke: self.m.stroke,
                color: if dashed {
                    with_alpha(self.c.border, 0.9)
                } else {
                    self.c.border
                },
            });
        }
        let color = if hovered {
            self.c.text_strong
        } else {
            self.c.text_secondary
        };
        let mut cursor = bounds.x + padding;
        if let Some(icon) = icon {
            region.prims.push(Prim::Icon {
                icon,
                center: [cursor + icon_size / 2.0, bounds.y + bounds.height / 2.0],
                size: icon_size,
                color,
            });
            cursor += leading;
        }
        let available = bounds.right() - padding - trailing_width - cursor;
        self.label(
            region,
            text,
            cursor,
            bounds.y,
            bounds.height,
            available,
            UiFace::SANS,
            size,
            color,
        );
        if let Some(icon) = trailing {
            region.prims.push(Prim::Icon {
                icon,
                center: [
                    bounds.right() - padding - icon_size / 2.0 + self.m.px(2.0),
                    bounds.y + bounds.height / 2.0,
                ],
                size: icon_size,
                color,
            });
        }
        if let Some((target, label)) = target {
            self.hit(region, bounds, target, HitRole::Button, label, true);
        }
        width
    }

    fn status_dot(&self, region: &mut Region, center: [f32; 2], color: [f32; 4]) {
        let size = self.m.px(8.0);
        region.prims.push(Prim::Fill {
            bounds: Bounds::new(center[0] - size / 2.0, center[1] - size / 2.0, size, size),
            radius: size / 2.0,
            color,
        });
    }

    fn activity_color(&self, activity: Activity) -> [f32; 4] {
        match activity {
            Activity::NeedsYou => self.c.attention,
            Activity::Working | Activity::Starting => self.c.accent,
            Activity::Finished { success: true } => self.c.success,
            Activity::Finished { success: false } => self.c.danger,
            Activity::Idle | Activity::Stopped | Activity::Exited => self.c.text_muted,
        }
    }

    fn short_path(&self, path: &Path) -> String {
        if let Some(home) = &self.input.home
            && let Ok(relative) = path.strip_prefix(home)
        {
            return if relative.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", relative.display())
            };
        }
        path.display().to_string()
    }
}

/// Appends `other` to `region`, keeping its hits' reading order.
fn merge(region: &mut Region, other: Region) {
    let offset = region.hits.len();
    region.prims.extend(other.prims);
    region.hits.extend(other.hits);
    region
        .nodes
        .extend(other.nodes.into_iter().map(|node| match node {
            RegionNode::Hit(index) => RegionNode::Hit(index + offset),
            text => text,
        }));
}

fn with_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] *= alpha;
    color
}

// ── Header and banner ────────────────────────────────────────────────────

impl Builder<'_, '_> {
    /// The part of the panel its content uses: all of it, or a centred
    /// column when the panel is much wider than lines read well.
    fn column(&self, panel: Bounds) -> Bounds {
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
    fn layout_toggle(&self) -> Option<(Icon, Target, &'static str)> {
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

    fn header(&mut self, region: &mut Region, panel: Bounds) -> Bounds {
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
                    "Claude Code · {} · {} · {}",
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

    fn banner(&mut self, region: &mut Region, panel: Bounds, top: f32, banner: &Banner) -> f32 {
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

    fn scrollbar(&self, region: &mut Region, viewport: Bounds, content: f32, offset: f32) {
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
        region.prims.push(Prim::Fill {
            bounds: Bounds::new(
                viewport.right() - width - self.m.px(3.0),
                y.round(),
                width,
                length.round(),
            ),
            radius: width / 2.0,
            color: with_alpha(self.c.text_muted, 0.45),
        });
    }

    fn jump_to_end(&mut self, region: &mut Region, viewport: Bounds) {
        let size = self.m.px(32.0);
        let bounds = Bounds::new(
            (viewport.x + (viewport.width - size) / 2.0).round(),
            viewport.bottom() - size - self.m.px(10.0),
            size,
            size,
        );
        region.prims.push(Prim::Fill {
            bounds,
            radius: size / 2.0,
            color: self.c.surface_hover,
        });
        region.prims.push(Prim::Outline {
            bounds,
            radius: size / 2.0,
            stroke: self.m.stroke,
            color: self.c.border,
        });
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
    fn focus_ring(
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

// ── Composer ─────────────────────────────────────────────────────────────

/// Fewest and most text lines the composer shows before scrolling.
const COMPOSER_MIN_LINES: f32 = 2.0;
const COMPOSER_MAX_LINES: f32 = 10.0;

impl Builder<'_, '_> {
    /// Lays the composer out at the bottom of the panel and returns its top.
    fn composer(&mut self, region: &mut Region, panel: Bounds) -> f32 {
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
                with_alpha(self.c.accent, 0.8)
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
                placeholder,
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
            "Message Claude",
            true,
        );

        // Toolbar.
        let toolbar_y = box_bounds.bottom() - toolbar_height + self.m.px(2.0);
        let mut x = inner_x + self.m.px(6.0);
        let (mode, model_label) = match (state.screen, session) {
            (Screen::Conversation, Some(session)) => {
                (session.transcript().mode(), session.model_label())
            }
            _ => (
                state.new_session.mode,
                model_name(
                    &offered_models(self.input),
                    state.new_session.model.as_deref().unwrap_or("default"),
                ),
            ),
        };
        let mode_icon = match mode {
            PermissionMode::PlanOnly => Icon::Plan,
            mode if mode.skips_every_check() => Icon::Warning,
            _ => Icon::Shield,
        };
        let mode_name = format!("Permissions: {}", mode.label());
        let model_name = format!("Model: {model_label}");
        let width = self.chip(
            region,
            x,
            toolbar_y,
            mode.label(),
            Some(mode_icon),
            Some(Icon::ChevronDown),
            false,
            false,
            inner_width / 2.0,
            Some((Target::OpenMenu(MenuKind::Mode), &mode_name)),
        );
        x += width + self.m.px(2.0);
        self.chip(
            region,
            x,
            toolbar_y,
            &model_label,
            None,
            Some(Icon::ChevronDown),
            false,
            false,
            inner_width / 3.0,
            Some((Target::OpenMenu(MenuKind::Model), &model_name)),
        );
        let send_size = self.m.control;
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
            with_alpha(fill, 0.45)
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

    fn placeholder(&self, hint: Option<&'static str>, working: bool) -> &'static str {
        if let Some(hint) = hint {
            return hint;
        }
        match self.input.state.screen {
            Screen::NewSession => "What should Claude work on?",
            _ if working => "Queue a message for when Claude finishes…",
            _ => "Ask Claude to change something…",
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn selection_rects(
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
fn caret_visible(moved: Instant, now: Instant) -> bool {
    let elapsed = now.saturating_duration_since(moved);
    elapsed < super::CARET_HOLD
        || (elapsed.as_millis() / super::CARET_PHASE.as_millis()).is_multiple_of(2)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

// ── Menus ────────────────────────────────────────────────────────────────

/// One menu row.
pub struct MenuItem {
    pub text: String,
    pub detail: String,
    pub action: MenuAction,
    pub checked: bool,
}

/// The rows of a menu, for layout and keyboard navigation alike.
pub fn menu_items(input: &Input<'_>, kind: MenuKind) -> Vec<MenuItem> {
    let session = input.active();
    match kind {
        MenuKind::Mode => {
            let current = match (input.state.screen, session) {
                (Screen::Conversation, Some(session)) => session.transcript().mode(),
                _ => input.state.new_session.mode,
            };
            PermissionMode::selectable(input.allow_bypass)
                .into_iter()
                .map(|mode| MenuItem {
                    text: mode.label().to_owned(),
                    detail: mode.description().to_owned(),
                    action: MenuAction::SetMode(mode),
                    checked: mode == current,
                })
                .collect()
        }
        MenuKind::Model => {
            let current = match (input.state.screen, session) {
                (Screen::Conversation, Some(session)) => session.model().unwrap_or("default"),
                _ => input
                    .state
                    .new_session
                    .model
                    .as_deref()
                    .unwrap_or("default"),
            };
            offered_models(input)
                .into_iter()
                .map(|model| MenuItem {
                    checked: model.value == current,
                    text: model.display_name,
                    detail: model.description,
                    action: MenuAction::SetModel(model.value),
                })
                .collect()
        }
        MenuKind::More => {
            let mut items = vec![
                MenuItem {
                    text: "Open in a terminal".into(),
                    detail: "Continue this conversation in Claude Code's own interface".into(),
                    action: MenuAction::OpenInTerminal,
                    checked: false,
                },
                MenuItem {
                    text: "Copy conversation".into(),
                    detail: String::new(),
                    action: MenuAction::CopyConversation,
                    checked: false,
                },
            ];
            if session.is_some() {
                items.push(MenuItem {
                    text: "End session".into(),
                    detail: "Stop Claude Code and remove this session; continue it later from \
                             New session"
                        .into(),
                    action: MenuAction::EndSession,
                    checked: false,
                });
            }
            items.push(MenuItem {
                text: "Hide panel".into(),
                detail: String::new(),
                action: MenuAction::HidePanel,
                checked: false,
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
                })
                .collect()
        }
    }
}

/// The models the agent offers: the shown session's, else any running
/// session's (they share an account), else a fallback list until one starts.
fn offered_models(input: &Input) -> Vec<ModelChoice> {
    let shown = match input.state.screen {
        Screen::Sessions => None,
        _ => input.tab_session(),
    };
    let reported = shown
        .into_iter()
        .chain(input.sessions.iter().copied())
        .find_map(|session| session.transcript().handshake())
        .map(|handshake| handshake.models.clone())
        .filter(|models| !models.is_empty());
    reported.unwrap_or_else(|| {
        FALLBACK_MODELS
            .into_iter()
            .map(|value| ModelChoice {
                value: value.to_owned(),
                display_name: model_name(&[], value),
                description: String::new(),
            })
            .collect()
    })
}

impl Builder<'_, '_> {
    /// The raised card menus and the command list draw on.
    fn popover_frame(&self, region: &mut Region, bounds: Bounds) {
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
    }

    /// The agent's commands matching what follows a leading `/`, above the
    /// message box. Typing continues in the box while it shows.
    fn command_menu(&mut self, region: &mut Region, anchor: Bounds) {
        let input = self.input;
        let offered = super::commands::offered(input.state, input.sessions);
        let shown = super::commands::shown(input.state, offered);
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

    fn menu(
        &mut self,
        region: &mut Region,
        panel: Bounds,
        kind: MenuKind,
        anchor: Bounds,
        highlighted: usize,
    ) {
        let items = menu_items(self.input, kind);
        if items.is_empty() {
            return;
        }
        // A click anywhere else in the panel closes the menu.
        self.hit(
            region,
            panel,
            Target::DismissMenu,
            HitRole::Button,
            "Close menu",
            false,
        );
        let width = (panel.width - self.m.pad * 2.0).min(self.m.px(320.0));
        let row_pad = self.m.px(12.0);
        let text_width = width - row_pad * 2.0 - self.m.icon - self.m.px(8.0);
        let mut rows = Vec::with_capacity(items.len());
        let mut height = self.m.px(6.0) * 2.0;
        for item in &items {
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
            let row = self.m.body.line_px
                + detail.as_ref().map_or(0.0, |detail| detail.height)
                + self.m.px(12.0);
            rows.push((row, detail));
            height += row;
        }
        let x = anchor
            .x
            .min(panel.right() - self.m.pad - width)
            .max(panel.x + self.m.pad);
        // Open upward from controls in the lower half, as the composer's do.
        let y = if anchor.y > panel.y + panel.height / 2.0 {
            anchor.y - height - self.m.px(6.0)
        } else {
            anchor.bottom() + self.m.px(6.0)
        }
        .max(panel.y + self.m.px(4.0));
        let bounds = Bounds::new(x.round(), y.round(), width, height.round());
        self.popover_frame(region, bounds);
        let mut row_y = bounds.y + self.m.px(6.0);
        for (index, (item, (row_height, detail))) in items.iter().zip(rows).enumerate() {
            let row = Bounds::new(
                bounds.x + self.m.px(4.0),
                row_y,
                bounds.width - self.m.px(8.0),
                row_height,
            );
            let target = Target::Menu(item.action.clone());
            if self.hovered(&target) || index == highlighted {
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
            let label = if item.detail.is_empty() {
                item.text.clone()
            } else {
                format!("{}. {}", item.text, item.detail)
            };
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
            row_y += row_height;
        }
    }
}

#[cfg(test)]
mod tests;
