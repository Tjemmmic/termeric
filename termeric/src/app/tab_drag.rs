//! Deadline-driven drag-through for the existing tab overflow controls.

use super::*;
use crate::renderer::{TabBarClick, TabSpan};
use std::time::Duration;

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
        crate::renderer::TabDragVisual { dragged, sliding }
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
                    || self
                        .pending_live_domain_controls
                        .iter()
                        .any(|pending| pending.domain_id() == live.domain_id)
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

#[cfg(test)]
mod tests {
    use super::*;

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
