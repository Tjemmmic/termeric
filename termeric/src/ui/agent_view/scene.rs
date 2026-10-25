//! One frame of the agent view: what to draw, where input lands, and what
//! assistive technology reads.
//!
//! The scene is built once per change from the panel state and the
//! conversation, then used unchanged by the renderer, hit testing, keyboard
//! focus, and accessibility, so all four always agree.

use std::sync::Arc;

use crate::agent_view::session::SessionId;
use crate::ui::geometry::Bounds;
use crate::ui::icon::Icon;
use crate::ui::text_layout::{TextLayout, TextMeasure, TextSize, TextSpan, UiFace};

/// How one span of a paragraph is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanPaint {
    pub face: UiFace,
    pub color: [f32; 4],
}

/// A drawing primitive.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Fill {
        bounds: Bounds,
        radius: f32,
        color: [f32; 4],
    },
    Outline {
        bounds: Bounds,
        radius: f32,
        stroke: f32,
        color: [f32; 4],
    },
    /// Wrapped, styled text with its top-left corner at (`x`, `y`).
    Paragraph {
        layout: Arc<TextLayout>,
        x: f32,
        y: f32,
        font_px: f32,
        paint: Arc<[SpanPaint]>,
    },
    /// One line of text, already fitted.
    Label {
        text: String,
        x: f32,
        baseline: f32,
        font_px: f32,
        face: UiFace,
        color: [f32; 4],
    },
    Icon {
        icon: Icon,
        center: [f32; 2],
        /// Side of the icon's square box.
        size: f32,
        color: [f32; 4],
    },
    /// An indeterminate progress arc; `phase` turns once per unit.
    Spinner {
        center: [f32; 2],
        radius: f32,
        thickness: f32,
        phase: f32,
        color: [f32; 4],
    },
    /// Part of a ring from 0 to `fraction` of a full turn, from the top.
    Arc {
        center: [f32; 2],
        radius: f32,
        thickness: f32,
        fraction: f32,
        color: [f32; 4],
    },
}

/// Primitives sharing one clip rectangle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    pub clip: Option<Bounds>,
    pub prims: Vec<Prim>,
}

/// Something a hit target expands or collapses.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ExpandKey {
    /// A tool call's detail.
    Tool(String),
    /// A group of exploration calls, by its first call.
    Explored(String),
    /// A thinking block, by item index.
    Thinking(usize),
}

/// A menu the panel can open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuKind {
    Mode,
    Model,
    Effort,
    More,
    Folder,
}

/// An action a menu item performs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MenuAction {
    /// Switch to the mode with this id.
    SetMode(String),
    SetModel(String),
    /// Run at the level of effort with this value.
    SetEffort(String),
    /// Allow everything the agent asks about without asking, or ask again.
    SetAutoApprove(bool),
    SetFolder(std::path::PathBuf),
    EndSession,
    OpenInTerminal,
    CopyConversation,
    HidePanel,
}

/// What a click, key, or assistive action can do.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    // Header.
    ShowSessions,
    /// Show the panel beside the terminal, or filling the tab.
    SetLayout(super::Layout),
    /// The conversation's title, which also opens the sessions page. A
    /// target of its own because every target must be unique in a scene.
    SessionTitle,
    NewSession,
    OpenMenu(MenuKind),
    Back,
    // Conversation.
    Toggle(ExpandKey),
    /// The control at the end of a long expanded block that collapses it,
    /// so it can be closed without scrolling back to its header.
    Collapse(ExpandKey),
    Allow {
        request_id: String,
    },
    AllowAlways {
        request_id: String,
    },
    Deny {
        request_id: String,
    },
    ChooseOption {
        request_id: String,
        question: usize,
        option: usize,
    },
    AnswerOther {
        request_id: String,
        question: usize,
    },
    SubmitAnswers {
        request_id: String,
    },
    ApprovePlan {
        request_id: String,
        /// The id of the mode to work in once the plan is approved.
        mode: String,
    },
    KeepPlanning {
        request_id: String,
    },
    CopyItem(usize),
    ReviewChanges(usize),
    Relaunch,
    OpenInTerminal,
    /// Continue in Claude Code's terminal to run a command it runs only
    /// there.
    RunInTerminal,
    ScrollToEnd,
    // Composer.
    Composer,
    AttachSelection,
    AttachLastCommand,
    RemoveAttachment(usize),
    /// Send the message the supervisor asked to send.
    SupervisorSend,
    /// Refuse the message the supervisor asked to send.
    SupervisorDecline,
    /// Withdraw a message waiting for the current turn to end.
    CancelQueued(usize),
    /// Complete the message box to one of the agent's `/` commands.
    Command(String),
    Send,
    Stop,
    // Menus.
    Menu(MenuAction),
    DismissMenu,
    /// A long menu's search field; typing while the menu is open narrows it.
    MenuSearch,
    // Sessions.
    OpenSession(SessionId),
    /// Continue an earlier conversation of `agent` by its id.
    ResumePast {
        agent: crate::agent_view::agent::AgentKind,
        id: String,
    },
    OpenTerminalAgent(crate::tabs::TerminalIdentity),
    ToggleSupervisor,
    // New session.
    /// Choose the agent to start.
    ChooseAgent(crate::agent_view::agent::AgentKind),
    StartSession,
}

/// How a target presents itself to assistive technology.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitRole {
    Button,
    Toggle { expanded: bool },
    Option { selected: bool },
    TextInput,
    ListItem,
    MenuItem { checked: bool },
}

/// A region input can land on.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub bounds: Bounds,
    /// The visible part of the region, for regions inside the scrolling feed.
    pub clip: Option<Bounds>,
    pub target: Target,
    pub role: HitRole,
    /// Read by assistive technology and shown as a tooltip for icon buttons.
    pub label: String,
    /// Reachable with Tab.
    pub focusable: bool,
}

impl Hit {
    pub fn visible_bounds(&self) -> Option<Bounds> {
        match self.clip {
            Some(clip) => self.bounds.intersect(clip),
            None => Some(self.bounds),
        }
    }
}

/// Static text read by assistive technology, in reading order with hits.
#[derive(Clone, Debug, PartialEq)]
pub struct TextNode {
    pub bounds: Bounds,
    pub clip: Option<Bounds>,
    pub text: String,
    /// A heading, such as a message author or section title.
    pub heading: bool,
}

/// Something assistive technology reads, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(TextNode),
    Hit(usize),
}

/// Where the composer's caret is, for IME placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Caret {
    pub bounds: Bounds,
}

/// One frame of the panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub panel: Bounds,
    /// The panel's background, which translucent strokes are blended over.
    pub background: [f32; 4],
    /// Panel background, header, banner, and composer.
    pub base: Layer,
    /// The scrolling conversation, clipped to its viewport.
    pub feed: Layer,
    /// Menus and floating controls, drawn above everything else.
    pub overlay: Layer,
    /// An open menu's list, clipped to where it scrolls, drawn over its
    /// menu.
    pub menu: Layer,
    /// Where an open menu's list scrolls.
    pub menu_viewport: Option<Bounds>,
    /// Height of the menu's list, and how far it is scrolled.
    pub menu_height: f32,
    pub menu_scroll: f32,
    pub hits: Vec<Hit>,
    pub nodes: Vec<Node>,
    pub feed_viewport: Bounds,
    /// Height of everything in the feed, for scrolling.
    pub feed_height: f32,
    /// The feed's scrollbar, while the feed scrolls, for dragging it.
    pub feed_scrollbar: Option<ScrollbarGeometry>,
    pub caret: Option<Caret>,
    /// The composer's text box, for pointer selection.
    pub composer_text: Option<ComposerGeometry>,
}

/// A scrollbar as drawn: where its thumb travels, the thumb, and the wider
/// strip a press grabs it in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarGeometry {
    pub track: Bounds,
    pub thumb: Bounds,
    pub grab: Bounds,
}

impl ScrollbarGeometry {
    /// The scroll offset, out of `max`, that puts the thumb's top at
    /// `thumb_top`.
    pub fn offset_for(&self, thumb_top: f32, max: f32) -> f32 {
        let travel = self.track.height - self.thumb.height;
        if travel <= 0.0 {
            return 0.0;
        }
        ((thumb_top - self.track.y) / travel).clamp(0.0, 1.0) * max
    }
}

/// Where and how the composer's text is laid out, so keys and clicks can
/// find positions in it.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposerGeometry {
    pub origin: [f32; 2],
    pub wrap_width: f32,
    pub size: TextSize,
}

impl ComposerGeometry {
    /// Lays out `text` as the composer draws it. Input lays out the text as
    /// it is now instead of reusing the drawn layout, which a key earlier in
    /// the same batch of events may have made stale.
    pub fn layout(&self, text: &str, measure: &mut dyn TextMeasure) -> Arc<TextLayout> {
        composer_layout(text, self.size, self.wrap_width, measure)
    }
}

/// The composer's text layout. An empty box lays out one space so the caret
/// has a line.
pub fn composer_layout(
    text: &str,
    size: TextSize,
    wrap_width: f32,
    measure: &mut dyn TextMeasure,
) -> Arc<TextLayout> {
    let text = if text.is_empty() { " " } else { text };
    measure.layout(
        &[TextSpan {
            text,
            face: UiFace::SANS,
        }],
        size,
        Some(wrap_width),
    )
}

impl Scene {
    /// The topmost target at a point.
    pub fn hit_at(&self, x: f32, y: f32) -> Option<&Hit> {
        self.hits.iter().rev().find(|hit| {
            hit.visible_bounds()
                .is_some_and(|bounds| bounds.contains(x, y))
        })
    }

    /// Targets reachable with Tab, in order.
    pub fn focus_order(&self) -> impl Iterator<Item = &Target> {
        self.hits
            .iter()
            .filter(|hit| hit.focusable)
            .map(|hit| &hit.target)
    }

    pub fn hit_for(&self, target: &Target) -> Option<&Hit> {
        self.hits.iter().find(|hit| hit.target == *target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(bounds: Bounds, clip: Option<Bounds>, target: Target, focusable: bool) -> Hit {
        Hit {
            bounds,
            clip,
            target,
            role: HitRole::Button,
            label: String::new(),
            focusable,
        }
    }

    #[test]
    fn the_topmost_visible_hit_wins() {
        let viewport = Bounds::new(0.0, 50.0, 100.0, 100.0);
        let scene = Scene {
            hits: vec![
                hit(
                    Bounds::new(0.0, 0.0, 100.0, 300.0),
                    Some(viewport),
                    Target::ScrollToEnd,
                    false,
                ),
                hit(Bounds::new(0.0, 0.0, 50.0, 40.0), None, Target::Back, true),
                hit(
                    Bounds::new(10.0, 60.0, 20.0, 20.0),
                    None,
                    Target::Send,
                    true,
                ),
            ],
            ..Scene::default()
        };
        assert_eq!(scene.hit_at(15.0, 65.0).unwrap().target, Target::Send);
        assert_eq!(
            scene.hit_at(80.0, 100.0).unwrap().target,
            Target::ScrollToEnd
        );
        // Clipped away by the feed viewport.
        assert!(scene.hit_at(80.0, 200.0).is_none());
        assert_eq!(scene.hit_at(5.0, 5.0).unwrap().target, Target::Back);
        assert_eq!(
            scene.focus_order().cloned().collect::<Vec<_>>(),
            [Target::Back, Target::Send]
        );
    }
}
