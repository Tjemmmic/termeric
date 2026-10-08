//! Dragging tabs: following the pointer along the bar, deadline-driven
//! drag-through for the overflow controls, and the picture the pointer
//! carries once a tab leaves its window.

use super::*;
use crate::renderer::{DraggedTabPicture, TabBarClick, TabSpan};
use std::time::Duration;
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, CustomCursor, CustomCursorSource};

/// Where the pointer holds a dragged tab, as positions along the tab bar
/// (x for the top bar, y for the side rail).
#[derive(Debug, Clone, Copy)]
pub(super) struct TabGrab {
    pressed_at: f32,
    /// From the tab's leading edge to the pointer.
    offset: f32,
    pointer: f32,
    /// The pointer moved far enough for a drag rather than a click.
    moving: bool,
}

/// A tab sliding into its slot after the dragged tab passed it, or the
/// dragged tab after its release.
#[derive(Debug, Clone, Copy)]
pub(super) struct TabSlide {
    tab_id: usize,
    /// Its distance from the slot when the slide began.
    from: f32,
    started: Instant,
}

const INITIAL_DWELL: Duration = Duration::from_millis(400);
const REPEAT_DWELL: Duration = Duration::from_millis(200);
/// How far the pointer moves, in logical pixels, before a press on a tab
/// becomes a drag.
const DRAG_THRESHOLD: f32 = 4.0;
/// How long a tab takes to slide into its slot.
const SLIDE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy)]
pub(super) struct OverflowDwell {
    tab_id: usize,
    control: TabBarClick,
    pub(super) deadline: Instant,
}

/// What the pointer shows while a dragged tab is outside its window.
#[derive(Default)]
pub(super) struct TabDragPointer {
    /// The dragged tab's picture, by tab id.
    picture: Option<(usize, TabPicture)>,
    /// The tab the frame being drawn copies.
    requested: Option<usize>,
    /// What this window's pointer shows now.
    shown: DragPointer,
}

enum TabPicture {
    /// Pixels waiting for the event loop to make them a cursor.
    Pixels(CustomCursorSource),
    Cursor(CustomCursor),
    /// The frame could not be copied, so there is no picture.
    Unavailable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DragPointer {
    /// The pointer's usual shapes: no tab is dragged outside the window.
    #[default]
    Usual,
    /// A picture of the dragged tab, held where the tab was grabbed.
    Picture,
    /// A closed hand, when the tab could not be pictured.
    Grabbing,
    /// The tab cannot leave: a live-domain tab keeps its daemon-owned place.
    NoDrop,
}

/// The pointer for a tab dragged `outside` the window: its picture when it
/// has one, a closed hand when not, and no-drop for a tab that cannot leave.
fn drag_pointer(outside: bool, live: bool, pictured: bool) -> DragPointer {
    match (outside, live, pictured) {
        (false, _, _) => DragPointer::Usual,
        (true, true, _) => DragPointer::NoDrop,
        (true, false, true) => DragPointer::Picture,
        (true, false, false) => DragPointer::Grabbing,
    }
}

/// A cursor of `picture`, held at the `pointer` position it was copied
/// under, or at the nearest edge when the pointer was off the tab.
fn picture_cursor(picture: DraggedTabPicture, pointer: (f64, f64)) -> Option<CustomCursorSource> {
    let width = u16::try_from(picture.width).ok()?;
    let height = u16::try_from(picture.height).ok()?;
    let hotspot = |pointer: f64, origin: u32, length: u16| {
        (pointer - f64::from(origin))
            .floor()
            .clamp(0.0, f64::from(length.saturating_sub(1))) as u16
    };
    let (x, y) = (
        hotspot(pointer.0, picture.origin.0, width),
        hotspot(pointer.1, picture.origin.1, height),
    );
    CustomCursor::from_rgba(picture.rgba, width, height, x, y)
        .inspect_err(|error| log::warn!("Could not make the dragged tab a cursor: {error}"))
        .ok()
}

impl App {
    /// The visible tabs' slots along the bar, and the pointer's position
    /// along it.
    fn tab_bar_geometry(&self, position: (f64, f64)) -> Option<(Vec<TabSpan>, f32)> {
        let renderer = self.renderer.as_ref()?;
        let size = self.window.as_ref()?.inner_size();
        let tabs = self.tabs.as_ref()?.tab_info();
        let spans = renderer.tab_slot_spans(&tabs, size.width as f32, size.height as f32);
        let along = if renderer.tab_bar_is_vertical() {
            position.1
        } else {
            position.0
        } as f32;
        Some((spans, along))
    }

    /// Records where the pointer holds the tab just pressed.
    pub(super) fn begin_tab_grab(&mut self) {
        let Some(position) = self.last_cursor_pos else {
            return;
        };
        let active = self.tabs.as_ref().map(TabManager::active_index);
        let Some((spans, along)) = self.tab_bar_geometry(position) else {
            return;
        };
        let Some(&(_, start, _)) = spans.iter().find(|(index, _, _)| Some(*index) == active) else {
            return;
        };
        if let Some(drag) = &mut self.tab_dragging {
            drag.grab = Some(TabGrab {
                pressed_at: along,
                offset: along - start,
                pointer: along,
                moving: false,
            });
        }
    }

    /// Follows the pointer with the dragged tab and moves it to the slot its
    /// center is over. Tabs it passes slide into their new slots.
    pub(super) fn drag_tab_to_pointer(&mut self, position: (f64, f64)) {
        if self
            .tab_dragging
            .as_ref()
            .is_none_or(|drag| drag.grab.is_none())
        {
            return;
        }
        let scale = self
            .renderer
            .as_ref()
            .map_or(1.0, crate::renderer::Renderer::scale_factor);
        let Some((spans, along)) = self.tab_bar_geometry(position) else {
            return;
        };
        let Some(grab) = self
            .tab_dragging
            .as_mut()
            .and_then(|drag| drag.grab.as_mut())
        else {
            return;
        };
        grab.pointer = along;
        if !grab.moving && (along - grab.pressed_at).abs() < DRAG_THRESHOLD * scale {
            return;
        }
        grab.moving = true;
        let grab = *grab;
        // Outside the window the tab is on its way to another window, or
        // to a window of its own; its place here stays as it was.
        if self.outside_window(position) {
            return;
        }
        let Some(active) = self.tabs.as_ref().map(TabManager::active_index) else {
            return;
        };
        let Some(&(_, _, length)) = spans.iter().find(|(index, _, _)| *index == active) else {
            return;
        };
        let center = along - grab.offset + length / 2.0;
        let Some(target) = slot_under(&spans, center).filter(|target| *target != active) else {
            return;
        };
        let order: Vec<usize> = self
            .tabs
            .as_ref()
            .map(|tabs| tabs.iter().map(|(_, tab)| tab.id).collect())
            .unwrap_or_default();
        if !self.apply_tab_drag_target(target) {
            return;
        }
        // Each tab between the old and new slot moves one slot toward the
        // old one; it slides there from where it was drawn.
        let now = Instant::now();
        let start_of = |index: usize| {
            spans
                .iter()
                .find(|(slot, _, _)| *slot == index)
                .map(|(_, start, _)| *start)
        };
        let (passed, step): (Vec<usize>, isize) = if target > active {
            ((active + 1..=target).collect(), -1)
        } else {
            ((target..active).collect(), 1)
        };
        for index in passed {
            let new_index = index.saturating_add_signed(step);
            if let (Some(tab_id), Some(old), Some(new)) = (
                order.get(index).copied(),
                start_of(index),
                start_of(new_index),
            ) {
                self.start_tab_slide(tab_id, old - new, now);
            }
        }
    }

    /// Keeps the dragged tab's picture copied by the frame just drawn. A
    /// frame that could not be copied leaves the tab without one; a frame
    /// that was not presented asks again next time.
    pub(super) fn keep_dragged_tab_picture(&mut self, presented: bool) {
        let Some(tab_id) = self.tab_drag_pointer.requested.take() else {
            return;
        };
        if !presented {
            return;
        }
        let picture = self
            .renderer
            .as_mut()
            .and_then(crate::renderer::Renderer::take_dragged_tab_picture)
            .zip(self.last_cursor_pos)
            .and_then(|(picture, pointer)| picture_cursor(picture, pointer))
            .map_or(TabPicture::Unavailable, TabPicture::Pixels);
        self.tab_drag_pointer.picture = Some((tab_id, picture));
    }

    /// Shows the dragged tab's picture as the pointer while the tab is
    /// outside this window, and the usual pointer otherwise. The window
    /// keeps the pointer through the drag, so its cursor is the one seen
    /// over other windows and the desktop.
    pub(super) fn sync_tab_drag_pointer(&mut self, event_loop: &ActiveEventLoop) {
        let dragged = self.tab_dragging.as_ref().map(|drag| drag.tab_id);
        if self
            .tab_drag_pointer
            .picture
            .as_ref()
            .is_some_and(|(tab_id, _)| Some(*tab_id) != dragged)
        {
            self.tab_drag_pointer.picture = None;
        }
        let pictured = self
            .tab_drag_pointer
            .picture
            .as_ref()
            .is_some_and(|(_, picture)| !matches!(picture, TabPicture::Unavailable));
        let outside = self.tab_dragging.as_ref().is_some_and(|drag| {
            drag.grab.is_some_and(|grab| grab.moving)
                && self
                    .last_cursor_pos
                    .is_some_and(|position| self.outside_window(position))
        });
        let live = self
            .tab_dragging
            .as_ref()
            .is_some_and(|drag| drag.live.is_some());
        let wanted = drag_pointer(outside, live, pictured);
        if wanted == self.tab_drag_pointer.shown {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };
        match wanted {
            DragPointer::Usual => window.set_cursor(CursorIcon::Default),
            DragPointer::Grabbing => window.set_cursor(CursorIcon::Grabbing),
            DragPointer::NoDrop => window.set_cursor(CursorIcon::NoDrop),
            DragPointer::Picture => {
                let Some((_, picture)) = &mut self.tab_drag_pointer.picture else {
                    return;
                };
                if let TabPicture::Pixels(_) = picture
                    && let TabPicture::Pixels(source) =
                        std::mem::replace(picture, TabPicture::Unavailable)
                {
                    *picture = TabPicture::Cursor(event_loop.create_custom_cursor(source));
                }
                if let TabPicture::Cursor(cursor) = picture {
                    window.set_cursor(cursor.clone());
                }
            }
        }
        self.tab_drag_pointer.shown = wanted;
        if wanted == DragPointer::Usual {
            self.refresh_terminal_pointer_cursor();
        }
    }

    /// Whether `position` lies outside this window.
    fn outside_window(&self, position: (f64, f64)) -> bool {
        self.window.as_ref().is_some_and(|window| {
            let size = window.inner_size();
            position.0 < 0.0
                || position.1 < 0.0
                || position.0 >= f64::from(size.width)
                || position.1 >= f64::from(size.height)
        })
    }

    /// The dragged tab, when it is released outside this window: the process
    /// then moves it to the window the pointer enters next, or tears it off
    /// into a window of its own. Live-domain tabs keep their daemon-owned
    /// place.
    pub(super) fn tab_released_outside(&self, position: (f64, f64)) -> Option<usize> {
        let drag = self.tab_dragging.as_ref()?;
        drag.grab.filter(|grab| grab.moving)?;
        (drag.live.is_none() && self.outside_window(position)).then_some(drag.tab_id)
    }

    /// Where a tab dropped at `position` in this window goes: before the
    /// first tab whose middle is past the pointer when it is over the tab
    /// bar, else at the end; a pinned tab stays among the pinned ones and
    /// any other after them.
    pub(super) fn tab_drop_index(&self, position: (f64, f64), pinned: bool) -> usize {
        let pinned_count = self.tabs.as_ref().map_or(0, |tabs| {
            tabs.iter().take_while(|(_, tab)| tab.pinned).count()
        });
        let index = self.tab_drop_slot(position);
        if pinned {
            index.min(pinned_count)
        } else {
            index.max(pinned_count)
        }
    }

    fn tab_drop_slot(&self, position: (f64, f64)) -> usize {
        let count = self.tabs.as_ref().map_or(0, |tabs| tabs.iter().count());
        let Some(renderer) = self.renderer.as_ref() else {
            return count;
        };
        let over_bar = if renderer.tab_bar_is_vertical() {
            (position.0 as f32) < renderer.tab_bar_width()
        } else {
            (position.1 as f32) < renderer.tab_bar_height()
        };
        if !over_bar {
            return count;
        }
        let Some((spans, along)) = self.tab_bar_geometry(position) else {
            return count;
        };
        insertion_index(&spans, along).unwrap_or(count)
    }

    /// Slides a released tab from where it was drawn into its slot.
    pub(super) fn settle_dragged_tab(&mut self, drag: &TabDrag, now: Instant) {
        let Some(grab) = drag.grab.filter(|grab| grab.moving) else {
            return;
        };
        let Some(position) = self.last_cursor_pos else {
            return;
        };
        let Some((spans, _)) = self.tab_bar_geometry(position) else {
            return;
        };
        let Some(index) = self
            .tabs
            .as_ref()
            .and_then(|tabs| tabs.iter().position(|(_, tab)| tab.id == drag.tab_id))
        else {
            return;
        };
        let Some(&(_, start, length)) = spans.iter().find(|(slot, _, _)| *slot == index) else {
            return;
        };
        let first = spans.first().map_or(0.0, |(_, start, _)| *start);
        let end = spans
            .last()
            .map_or(0.0, |(_, start, length)| start + length);
        let drawn = (grab.pointer - grab.offset).min(end - length).max(first);
        self.start_tab_slide(drag.tab_id, drawn - start, now);
    }

    fn start_tab_slide(&mut self, tab_id: usize, from: f32, now: Instant) {
        self.tab_slides.retain(|slide| slide.tab_id != tab_id);
        if from.abs() >= 0.5 && !self.reduced_motion_enabled() {
            self.tab_slides.push(TabSlide {
                tab_id,
                from,
                started: now,
            });
        }
    }

    /// The dragged tab and the tabs sliding into their slots, for this
    /// frame. Finished slides are dropped.
    pub(super) fn tab_drag_visual(&mut self, now: Instant) -> crate::renderer::TabDragVisual {
        self.tab_slides
            .retain(|slide| now.saturating_duration_since(slide.started) < SLIDE);
        let Some(tabs) = self.tabs.as_ref() else {
            return crate::renderer::TabDragVisual::default();
        };
        let index_of = |tab_id: usize| tabs.iter().position(|(_, tab)| tab.id == tab_id);
        let dragged = self.tab_dragging.as_ref().and_then(|drag| {
            let grab = drag.grab.filter(|grab| grab.moving)?;
            Some((index_of(drag.tab_id)?, grab.pointer - grab.offset))
        });
        // A tab that can leave the window is pictured once, as it is first
        // drawn under the pointer.
        self.tab_drag_pointer.requested = self
            .tab_dragging
            .as_ref()
            .filter(|drag| dragged.is_some() && drag.live.is_none())
            .map(|drag| drag.tab_id)
            .filter(|tab_id| {
                self.tab_drag_pointer
                    .picture
                    .as_ref()
                    .is_none_or(|(pictured, _)| pictured != tab_id)
            });
        let sliding = self
            .tab_slides
            .iter()
            .filter_map(|slide| {
                let t = (now.saturating_duration_since(slide.started).as_secs_f32()
                    / SLIDE.as_secs_f32())
                .min(1.0);
                let eased = 1.0 - (1.0 - t).powi(3);
                Some((index_of(slide.tab_id)?, slide.from * (1.0 - eased)))
            })
            .collect();
        crate::renderer::TabDragVisual {
            dragged,
            sliding,
            capture: self.tab_drag_pointer.requested.is_some(),
        }
    }

    pub(super) fn apply_tab_drag_target(&mut self, target: usize) -> bool {
        self.finish_obscured_native_pointer_gesture();
        let Some(tab_id) = self.tab_drag_target(target) else {
            return false;
        };
        self.tabs
            .as_mut()
            .is_some_and(|tabs| tabs.move_dragged_tab_to(tab_id, target))
    }

    /// Pointer motion and overflow dwell must authorize the same exact source
    /// and destination. A pending request can arrive after button-down.
    fn tab_drag_target(&self, index: usize) -> Option<usize> {
        if self.presentation_suspended() {
            return None;
        }
        let drag = self.tab_dragging.as_ref()?;
        let tabs = self.tabs.as_ref()?;
        let source = tabs.active_tab()?;
        if source.id != drag.tab_id || source.closed || source.is_closing() {
            return None;
        }
        let target = tabs.iter().find(|(i, _)| *i == index)?.1;
        if target.closed || target.is_closing() || target.pinned != source.pinned {
            return None;
        }
        match (&drag.live, source.live_domain_tab_route()) {
            (None, None) => {}
            (Some(live), Some((domain, remote_tab)))
                if domain == live.domain_id && remote_tab == live.remote_tab_id =>
            {
                if target
                    .live_domain_tab_route()
                    .is_none_or(|(domain, _)| domain != live.domain_id)
                    || self.live_domain_busy(&live.domain_id)
                {
                    return None;
                }
            }
            _ => return None,
        }
        Some(drag.tab_id)
    }

    fn overflow_drag_target(&self, control: TabBarClick) -> Option<(usize, usize)> {
        // Autonomous repeat needs keyboard focus; direct native pointer input
        // can legitimately start a new drag on a pointer-only Wayland seat.
        // Actual focus loss still finalizes every existing native gesture.
        if !self.window_focused {
            return None;
        }
        let tabs = self.tabs.as_ref()?;
        let index = match control {
            TabBarClick::PreviousTab => tabs.active_index().checked_sub(1)?,
            TabBarClick::NextTab => tabs.active_index().checked_add(1)?,
            _ => return None,
        };
        self.tab_drag_target(index).map(|tab_id| (tab_id, index))
    }

    /// Both native motion and idle wakeups use the current rendered hit test.
    /// One late wakeup moves at most one slot; there is no catch-up burst.
    pub(super) fn update_tab_drag_scroll(&mut self, hit: Option<TabBarClick>, now: Instant) {
        self.finish_obscured_native_pointer_gesture();
        let Some((control, (tab_id, target))) = hit.and_then(|control| {
            self.overflow_drag_target(control)
                .map(|target| (control, target))
        }) else {
            self.tab_drag_scroll = None;
            return;
        };
        let Some(dwell) = self
            .tab_drag_scroll
            .filter(|dwell| dwell.tab_id == tab_id && dwell.control == control)
        else {
            self.tab_drag_scroll = Some(OverflowDwell {
                tab_id,
                control,
                deadline: now + INITIAL_DWELL,
            });
            return;
        };
        if now < dwell.deadline {
            return;
        }
        let moved = self.apply_tab_drag_target(target);
        self.tab_drag_scroll = if moved && self.overflow_drag_target(control).is_some() {
            Some(OverflowDwell {
                deadline: now + REPEAT_DWELL,
                ..dwell
            })
        } else {
            None
        };
        if moved {
            self.redraw_scheduler
                .request(redraw_scheduler::RedrawReason::WindowEvent);
        }
    }

    pub(super) fn poll_tab_drag_scroll(&mut self, now: Instant) {
        if self.tab_dragging.is_none() {
            self.tab_drag_scroll = None;
            return;
        }
        let hit = (|| {
            self.tab_bar_hover?;
            let (x, y) = self.last_cursor_pos?;
            let renderer = self.renderer.as_ref()?;
            let window = self.window.as_ref()?;
            let size = window.inner_size();
            if x < 0.0 || y < 0.0 || x >= size.width as f64 || y >= size.height as f64 {
                return None;
            }
            Some(renderer.tab_bar_hit_test(
                x as f32,
                y as f32,
                &self.tabs.as_ref()?.tab_info(),
                size.width as f32,
                size.height as f32,
            ))
        })();
        self.update_tab_drag_scroll(hit, now);
    }
}

/// The visible slot (index, start, length) holding `position` along the
/// bar; past either end of the strip, the slot at that end.
fn slot_under(spans: &[TabSpan], position: f32) -> Option<usize> {
    spans
        .iter()
        .find(|(_, start, length)| position >= *start && position < start + length)
        .or_else(|| {
            if spans.first().is_some_and(|(_, start, _)| position < *start) {
                spans.first()
            } else {
                spans.last()
            }
        })
        .map(|(index, _, _)| *index)
}

/// Where a tab dropped at `position` along the bar goes among the visible
/// slots: before the first whose middle is past it, or after the last.
fn insertion_index(spans: &[TabSpan], position: f32) -> Option<usize> {
    spans
        .iter()
        .find(|(_, start, length)| position < start + length / 2.0)
        .map(|(index, _, _)| *index)
        .or_else(|| spans.last().map(|(index, _, _)| index + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_opened_for_a_torn_off_tab_starts_empty() {
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        app.launch.adopts_tab = true;
        app.init_tabs().unwrap();
        assert!(app.tabs.as_ref().unwrap().is_empty());
    }

    #[test]
    fn the_pointer_carries_the_tab_only_outside_the_window() {
        assert_eq!(drag_pointer(false, false, true), DragPointer::Usual);
        assert_eq!(drag_pointer(false, true, false), DragPointer::Usual);
        assert_eq!(drag_pointer(true, false, true), DragPointer::Picture);
        assert_eq!(drag_pointer(true, false, false), DragPointer::Grabbing);
        assert_eq!(drag_pointer(true, true, true), DragPointer::NoDrop);
    }

    #[test]
    fn the_picture_is_held_where_the_tab_was_grabbed() {
        let picture = |origin| DraggedTabPicture {
            rgba: vec![255; 100 * 30 * 4],
            width: 100,
            height: 30,
            origin,
        };
        assert!(picture_cursor(picture((200, 4)), (240.7, 19.2)).is_some());
        // Off the tab, the nearest edge holds it; a picture too large for a
        // cursor gives none.
        assert!(picture_cursor(picture((200, 4)), (10.0, 900.0)).is_some());
        let huge = DraggedTabPicture {
            rgba: vec![0; 4000 * 4],
            width: 4000,
            height: 1,
            origin: (0, 0),
        };
        assert!(picture_cursor(huge, (0.0, 0.0)).is_none());
    }

    #[test]
    fn a_dragged_tab_is_pictured_once() {
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        tabs.new_tab().unwrap();
        tabs.new_tab().unwrap();
        let tab_id = tabs.active_tab().unwrap().id;
        app.tabs = Some(tabs);
        let grab = TabGrab {
            pressed_at: 150.0,
            offset: 40.0,
            pointer: 152.0,
            moving: false,
        };
        app.tab_dragging = Some(TabDrag {
            tab_id,
            live: None,
            grab: Some(grab),
        });
        let now = Instant::now();
        // A press is not yet a drag.
        assert!(!app.tab_drag_visual(now).capture);
        app.tab_dragging.as_mut().unwrap().grab = Some(TabGrab {
            moving: true,
            ..grab
        });
        assert!(app.tab_drag_visual(now).capture);
        // A frame that was not presented asks again.
        app.keep_dragged_tab_picture(false);
        assert!(app.tab_drag_visual(now).capture);
        // A presented frame settles it, even without a picture to show.
        app.keep_dragged_tab_picture(true);
        assert!(matches!(
            app.tab_drag_pointer.picture,
            Some((pictured, TabPicture::Unavailable)) if pictured == tab_id
        ));
        assert!(!app.tab_drag_visual(now).capture);
        // A live-domain tab cannot leave, so it is never pictured.
        app.tab_drag_pointer.picture = None;
        app.tab_dragging.as_mut().unwrap().live = Some(LiveDomainTabDrag {
            domain_id: "domain".into(),
            remote_tab_id: 1,
            initial_index: 0,
        });
        assert!(!app.tab_drag_visual(now).capture);
    }

    #[test]
    fn a_dropped_tab_goes_before_the_first_tab_whose_middle_is_past_it() {
        let spans = [(3, 10.0, 100.0), (4, 110.0, 100.0)];
        assert_eq!(insertion_index(&spans, 20.0), Some(3));
        assert_eq!(insertion_index(&spans, 70.0), Some(4));
        assert_eq!(insertion_index(&spans, 170.0), Some(5));
        assert_eq!(insertion_index(&[], 5.0), None);
    }

    #[test]
    fn the_dragged_tab_takes_the_slot_its_center_is_over() {
        let spans = [(0, 10.0, 100.0), (1, 110.0, 100.0), (2, 210.0, 100.0)];
        assert_eq!(slot_under(&spans, 60.0), Some(0));
        // Over the next tab's close button still counts as that tab.
        assert_eq!(slot_under(&spans, 305.0), Some(2));
        assert_eq!(slot_under(&spans, 110.0), Some(1));
        assert_eq!(slot_under(&spans, -40.0), Some(0));
        assert_eq!(slot_under(&spans, 900.0), Some(2));
        assert_eq!(slot_under(&[], 5.0), None);
    }

    #[test]
    fn a_drag_is_drawn_at_the_pointer_and_slides_ease_out() {
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        let mut tabs = TabManager::new(
            80,
            24,
            crate::config::ShellConfig::default(),
            100,
            1024 * 1024,
        );
        for _ in 0..3 {
            tabs.new_tab().unwrap();
        }
        tabs.switch_to(1);
        let ids: Vec<usize> = tabs.iter().map(|(_, tab)| tab.id).collect();
        app.tabs = Some(tabs);
        let grab = TabGrab {
            pressed_at: 150.0,
            offset: 40.0,
            pointer: 152.0,
            moving: false,
        };
        app.tab_dragging = Some(TabDrag {
            tab_id: ids[1],
            live: None,
            grab: Some(grab),
        });
        let now = Instant::now();
        // A press that has not moved far is a click, drawn in place.
        assert_eq!(app.tab_drag_visual(now).dragged, None);
        app.tab_dragging.as_mut().unwrap().grab = Some(TabGrab {
            pointer: 190.0,
            moving: true,
            ..grab
        });
        assert_eq!(app.tab_drag_visual(now).dragged, Some((1, 150.0)));

        app.start_tab_slide(ids[2], 100.0, now);
        let start = app.tab_drag_visual(now).sliding;
        assert_eq!(start, [(2, 100.0)]);
        let halfway = app.tab_drag_visual(now + SLIDE / 2).sliding;
        // Ease-out: well past halfway at half the time.
        assert!(halfway[0].1 < 50.0 && halfway[0].1 > 0.0, "{halfway:?}");
        assert!(app.tab_drag_visual(now + SLIDE).sliding.is_empty());
        assert!(app.tab_slides.is_empty(), "finished slides are dropped");
    }
}
