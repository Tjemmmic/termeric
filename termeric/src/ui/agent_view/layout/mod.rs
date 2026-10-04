//! Turns the panel state and conversations into a [`Scene`].
//!
//! Layout happens in physical pixels. Text is measured through
//! [`TextMeasure`], so the same code runs against real fonts in the app and
//! fixed advances in tests. The scrolling conversation is laid out in content
//! coordinates first and then moved by the scroll offset, which lets the
//! panel follow new output exactly.

mod composer;
mod feed;
mod header;
mod menu;
mod screens;

#[cfg(test)]
use menu::{MENU_LIST_MAX, filtered_menu_items, menu_items};
use menu::{
    MenuList, auto_approve, current_effort, current_model, mode_state, offered_efforts,
    offered_models,
};
pub use screens::session_summary;

use crate::ui::geometry::Bounds;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::agent_view::agent::AgentKind;
use crate::agent_view::event::{
    EffortChoice, Handshake, ModeChoice, ModelChoice, model_name, selectable_modes,
};
use crate::agent_view::session::AgentSession;
use crate::agent_view::transcript::Activity;
use crate::ui::color::scale_alpha;
use crate::ui::text_layout::{
    TextLayout, TextMeasure, TextSize, TextSpan, UiFace, fit_with_ellipsis,
};

use super::colors::Colors;
use crate::ui::icon::Icon;

use super::scene::{
    Caret, ComposerGeometry, Hit, HitRole, Layer, MenuAction, MenuKind, Node, Prim, Scene,
    SpanPaint, Target, TextNode, composer_layout,
};
use super::{AgentViewState, Focus, Layout, OpenMenu, Screen};

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
    pub agent: AgentKind,
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
    /// The agent chosen to start.
    pub agent: AgentKind,
    /// Whether the chosen agent can start.
    pub availability: AgentAvailability,
    /// Every agent the panel drives, and whether each can start.
    pub agents: Vec<(AgentKind, AgentAvailability)>,
}

/// Whether an agent can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentAvailability {
    Ready,
    NotInstalled,
    /// Still asking the login shell where programs are.
    Looking,
    /// Turned off in the agent settings.
    Disabled,
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

    /// The agent the panel speaks of.
    fn agent(&self) -> AgentKind {
        self.state.agent(self.sessions)
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
        let menu = self
            .input
            .state
            .menu
            .as_ref()
            .and_then(|menu| self.menu(&mut overlay, panel, menu));
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
            menu,
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

fn assemble(
    panel: Bounds,
    base: Region,
    feed: Region,
    overlay: Region,
    menu: Option<MenuList>,
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
        menu: Layer::default(),
        menu_viewport: None,
        menu_height: 0.0,
        menu_scroll: 0.0,
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
    if let Some(menu) = menu {
        scene.menu = Layer {
            clip: Some(menu.viewport),
            prims: menu.region.prims,
        };
        scene.menu_viewport = Some(menu.viewport);
        scene.menu_height = menu.height;
        scene.menu_scroll = menu.scroll;
        let menu_start = scene.hits.len();
        scene.hits.extend(menu.region.hits);
        scene.nodes.extend(
            menu.region
                .nodes
                .into_iter()
                .map(|node| convert(node, &|index| menu_start + index)),
        );
    }
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

    /// The width a pill takes to show all of `text`.
    fn chip_width(&mut self, text: &str, icon: bool, trailing: bool) -> f32 {
        let icon_size = self.m.px(13.0);
        let leading = if icon {
            icon_size + self.m.px(6.0)
        } else {
            0.0
        };
        let trailing = if trailing {
            icon_size + self.m.px(4.0)
        } else {
            0.0
        };
        self.m.px(10.0) * 2.0
            + leading
            + self.text_width(text, UiFace::SANS, self.m.small)
            + trailing
    }

    /// A small pill. Returns its width.
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
        let width = self
            .chip_width(text, icon.is_some(), trailing.is_some())
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
                    scale_alpha(self.c.border, 0.9)
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

impl Builder<'_, '_> {
    /// A card: `bounds` filled and outlined at one corner `radius`.
    fn card(
        &self,
        region: &mut Region,
        bounds: Bounds,
        radius: f32,
        fill: [f32; 4],
        border: [f32; 4],
    ) {
        region.prims.push(Prim::Fill {
            bounds,
            radius,
            color: fill,
        });
        region.prims.push(Prim::Outline {
            bounds,
            radius,
            stroke: self.m.stroke,
            color: border,
        });
    }

    /// A card whose top `header_height` is a raised surface strip; where a
    /// body `joined` below it, the strip's lower corners are squared off.
    fn header_card(
        &self,
        region: &mut Region,
        bounds: Bounds,
        header_height: f32,
        joined: bool,
        body: [f32; 4],
        border: [f32; 4],
    ) {
        let radius = self.m.radius;
        region.prims.push(Prim::Fill {
            bounds,
            radius,
            color: body,
        });
        region.prims.push(Prim::Fill {
            bounds: Bounds::new(bounds.x, bounds.y, bounds.width, header_height),
            radius,
            color: self.c.surface,
        });
        if joined {
            region.prims.push(Prim::Fill {
                bounds: Bounds::new(
                    bounds.x,
                    bounds.y + header_height - radius,
                    bounds.width,
                    radius,
                ),
                radius: 0.0,
                color: self.c.surface,
            });
        }
        region.prims.push(Prim::Outline {
            bounds,
            radius,
            stroke: self.m.stroke,
            color: border,
        });
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

// ── Header and banner ────────────────────────────────────────────────────

// ── Composer ─────────────────────────────────────────────────────────────

/// `count` with the singular or plural noun, as in "1 file" or "3 files".
pub(crate) fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

// ── Menus ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
