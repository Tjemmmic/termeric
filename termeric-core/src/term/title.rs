pub(crate) const MAX_TITLE_BYTES: usize = 4_096;
pub const XTERM_TITLE_STACK_DEPTH: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TitleTarget {
    Both,
    Icon,
    Window,
}

impl TitleTarget {
    pub(crate) fn from_parameter(parameter: u16) -> Option<Self> {
        match parameter {
            0 => Some(Self::Both),
            1 => Some(Self::Icon),
            2 => Some(Self::Window),
            _ => None,
        }
    }

    fn includes_icon(self) -> bool {
        matches!(self, Self::Both | Self::Icon)
    }

    fn includes_window(self) -> bool {
        matches!(self, Self::Both | Self::Window)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SavedTitles {
    icon: Option<String>,
    window: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RestoredTitles {
    pub(crate) icon: Option<String>,
    pub(crate) window: Option<String>,
}

/// Application-authored OSC title state and xterm-compatible save slots.
///
/// This intentionally excludes UI/native-window metadata. Keeping the state
/// beside the parser also makes queries deterministic for the originating PTY.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApplicationTitles {
    icon: String,
    window: String,
    window_set: bool,
    saved: [SavedTitles; XTERM_TITLE_STACK_DEPTH],
    cursor: usize,
    used: u64,
}

impl Default for ApplicationTitles {
    fn default() -> Self {
        Self {
            icon: String::new(),
            window: String::new(),
            window_set: false,
            saved: std::array::from_fn(|_| SavedTitles::default()),
            cursor: 0,
            used: 0,
        }
    }
}

impl ApplicationTitles {
    pub(crate) fn icon(&self) -> &str {
        &self.icon
    }

    pub(crate) fn window(&self) -> &str {
        &self.window
    }

    pub(crate) fn used(&self) -> u64 {
        self.used
    }

    pub(crate) fn set_icon(&mut self, icon: String) {
        self.icon = icon;
    }

    pub(crate) fn set_window(&mut self, window: String) -> bool {
        if self.window_set && self.window == window {
            return false;
        }
        self.window = window;
        self.window_set = true;
        true
    }

    pub(crate) fn set_both(&mut self, title: String) -> bool {
        self.icon.clone_from(&title);
        self.set_window(title)
    }

    pub(crate) fn save(&mut self, target: TitleTarget, slot: u16) {
        let index = if slot == 0 {
            let index = self.cursor;
            self.cursor = (self.cursor + 1) % XTERM_TITLE_STACK_DEPTH;
            self.used = self.used.saturating_add(1);
            index
        } else {
            usize::from(slot - 1) % XTERM_TITLE_STACK_DEPTH
        };

        // xterm replaces the whole slot, even when only one label is selected.
        self.saved[index] = SavedTitles {
            icon: target.includes_icon().then(|| self.icon.clone()),
            window: target.includes_window().then(|| self.window.clone()),
        };
    }

    pub(crate) fn restore(&mut self, target: TitleTarget, slot: u16) -> RestoredTitles {
        let (index, pop) = if slot == 0 {
            if self.used == 0 {
                return RestoredTitles::default();
            }
            self.used -= 1;
            self.cursor = (self.cursor + XTERM_TITLE_STACK_DEPTH - 1) % XTERM_TITLE_STACK_DEPTH;
            (self.cursor, true)
        } else {
            (usize::from(slot - 1) % XTERM_TITLE_STACK_DEPTH, false)
        };

        let restored = RestoredTitles {
            icon: target
                .includes_icon()
                .then(|| self.find_icon(index))
                .flatten(),
            window: target
                .includes_window()
                .then(|| self.find_window(index))
                .flatten(),
        };
        if pop {
            self.saved[index] = SavedTitles::default();
        }
        restored
    }

    fn find_icon(&self, index: usize) -> Option<String> {
        self.find_from(index, |saved| saved.icon.as_ref())
    }

    fn find_window(&self, index: usize) -> Option<String> {
        self.find_from(index, |saved| saved.window.as_ref())
    }

    fn find_from<'a>(
        &'a self,
        index: usize,
        select: impl Fn(&'a SavedTitles) -> Option<&'a String>,
    ) -> Option<String> {
        (0..XTERM_TITLE_STACK_DEPTH)
            .map(|distance| (index + XTERM_TITLE_STACK_DEPTH - distance) % XTERM_TITLE_STACK_DEPTH)
            .find_map(|candidate| select(&self.saved[candidate]).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop_wrap_the_ten_slot_ring() {
        let mut titles = ApplicationTitles::default();
        for index in 0..12 {
            titles.set_window(format!("title-{index}"));
            titles.save(TitleTarget::Window, 0);
        }

        assert_eq!(titles.used(), 12);
        assert_eq!(
            titles.restore(TitleTarget::Window, 0).window.as_deref(),
            Some("title-11")
        );
        assert_eq!(
            titles.restore(TitleTarget::Window, 0).window.as_deref(),
            Some("title-10")
        );
        assert_eq!(
            titles.restore(TitleTarget::Window, 0).window.as_deref(),
            Some("title-9")
        );
    }

    #[test]
    fn partial_entries_fall_back_to_older_saved_fields() {
        let mut titles = ApplicationTitles::default();
        titles.set_icon("old-icon".into());
        titles.set_window("old-window".into());
        titles.save(TitleTarget::Both, 0);
        titles.set_window("new-window".into());
        titles.save(TitleTarget::Window, 0);

        let restored = titles.restore(TitleTarget::Both, 0);
        assert_eq!(restored.icon.as_deref(), Some("old-icon"));
        assert_eq!(restored.window.as_deref(), Some("new-window"));
    }

    #[test]
    fn direct_slots_do_not_change_the_stack_position_or_clear_on_restore() {
        let mut titles = ApplicationTitles::default();
        titles.set_icon("saved".into());
        titles.save(TitleTarget::Icon, 4);
        titles.set_icon("current".into());

        assert_eq!(titles.used(), 0);
        assert_eq!(
            titles.restore(TitleTarget::Icon, 4).icon.as_deref(),
            Some("saved")
        );
        assert_eq!(
            titles.restore(TitleTarget::Icon, 4).icon.as_deref(),
            Some("saved")
        );
        assert_eq!(titles.used(), 0);
    }

    #[test]
    fn repeated_maximum_labels_retain_only_current_and_ten_saved_pairs() {
        let mut titles = ApplicationTitles::default();
        let label = "x".repeat(MAX_TITLE_BYTES);
        for _ in 0..25 {
            titles.set_both(label.clone());
            titles.save(TitleTarget::Both, 0);
        }

        let saved_bytes: usize = titles
            .saved
            .iter()
            .map(|saved| {
                saved.icon.as_ref().map_or(0, String::len)
                    + saved.window.as_ref().map_or(0, String::len)
            })
            .sum();
        let current_bytes = titles.icon.len() + titles.window.len();
        assert_eq!(saved_bytes, MAX_TITLE_BYTES * XTERM_TITLE_STACK_DEPTH * 2);
        assert_eq!(current_bytes, MAX_TITLE_BYTES * 2);
        assert_eq!(titles.used(), 25);
    }
}
