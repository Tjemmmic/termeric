//! Deadline-driven drag-through for the existing tab overflow controls.

use super::*;
use crate::renderer::TabBarClick;
use std::time::Duration;

const INITIAL_DWELL: Duration = Duration::from_millis(400);
const REPEAT_DWELL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy)]
pub(super) struct OverflowDwell {
    tab_id: usize,
    control: TabBarClick,
    pub(super) deadline: Instant,
}

impl App {
    fn overflow_drag_target(&self, control: TabBarClick) -> Option<(usize, usize)> {
        if !self.window_focused || self.presentation_suspended() {
            return None;
        }
        let drag = self.tab_dragging.as_ref()?;
        let tabs = self.tabs.as_ref()?;
        let source = tabs.active_tab()?;
        if source.id != drag.tab_id || source.closed || source.is_closing() {
            return None;
        }
        let index = match control {
            TabBarClick::PreviousTab => tabs.active_index().checked_sub(1)?,
            TabBarClick::NextTab => tabs.active_index().checked_add(1)?,
            _ => return None,
        };
        let target = tabs.iter().find(|(i, _)| *i == index)?.1;
        if target.closed || target.is_closing() || target.pinned != source.pinned {
            return None;
        }
        if let Some(live) = &drag.live
            && (target
                .live_domain_tab_route()
                .is_none_or(|(domain, _)| domain != live.domain_id)
                || self
                    .pending_live_domain_controls
                    .iter()
                    .any(|pending| pending.domain_id() == live.domain_id))
        {
            return None;
        }
        Some((drag.tab_id, index))
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
        let moved = self
            .tabs
            .as_mut()
            .is_some_and(|tabs| tabs.move_dragged_tab_to(tab_id, target));
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
