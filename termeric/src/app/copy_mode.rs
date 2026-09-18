//! App routing for keyboard copy mode and quick-select hints.

use termeric_core::{Point, SelectionType};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, NamedKey};

use crate::config::CopyModeBindings;
use crate::terminal::copy_mode::{CopyMotion, HintAction};

use super::{App, CopyModeTarget};

impl App {
    pub(super) fn copy_mode_render_info(&self) -> Option<crate::renderer::CopyModeRenderInfo> {
        if !self.copy_mode.is_active() || self.copy_mode_target != self.active_copy_target() {
            return None;
        }
        let (status, hints) = if let Some(quick) = self.copy_mode.quick_select() {
            let status = format!(
                "QUICK SELECT · {} · {}{}",
                quick.action.label().to_ascii_uppercase(),
                if quick.prefix.is_empty() {
                    "TYPE HINT"
                } else {
                    quick.prefix.as_str()
                },
                " · TAB ACTION · ESC BACK"
            );
            let hints = quick
                .matches
                .iter()
                .filter(|candidate| candidate.label.starts_with(&quick.prefix))
                .map(|candidate| crate::renderer::QuickSelectHintRenderInfo {
                    label: candidate.label.clone(),
                    row: candidate.start.row,
                    col: candidate.start.col,
                    matches_prefix: !quick.prefix.is_empty(),
                })
                .collect();
            (status, hints)
        } else {
            let bindings = match self.config.copy_mode.bindings {
                CopyModeBindings::Vi => "VI · HJKL MOVE · V SELECT · Y COPY · / SEARCH",
                CopyModeBindings::Emacs => {
                    "EMACS · ARROWS MOVE · SPACE SELECT · ALT+W COPY · CTRL+S SEARCH"
                }
            };
            (format!("COPY MODE · {bindings} · ESC EXIT"), Vec::new())
        };
        Some(crate::renderer::CopyModeRenderInfo {
            cursor: self
                .copy_mode
                .quick_select()
                .and_then(|quick| quick.matches.first())
                .map_or_else(|| self.copy_mode.cursor(), |first| first.start),
            status,
            hints,
        })
    }

    fn active_copy_target(&self) -> Option<CopyModeTarget> {
        let tab = self.tabs.as_ref()?.active_tab()?;
        Some(CopyModeTarget {
            tab: tab.id,
            pane: tab
                .pane_manager
                .as_ref()
                .map(crate::panes::PaneManager::focused_id),
        })
    }

    pub(super) fn enter_copy_mode(&mut self) {
        // Copy mode and search share one real grid. Clear any previously
        // confirmed filter projection so modal coordinates always refer to
        // the terminal's scrollback-aware rows.
        self.search.cancel();
        let target = self.active_copy_target();
        let (copy_mode, tabs) = (&mut self.copy_mode, &mut self.tabs);
        let Some(terminal) = tabs
            .as_mut()
            .and_then(crate::tabs::TabManager::active_tab_mut)
            .and_then(crate::tabs::Tab::focused_terminal_mut)
        else {
            return;
        };
        copy_mode.enter(terminal.grid());
        terminal.set_selection(None);
        self.copy_mode_target = target;
        self.mouse_state.clear_selection();
        self.toasts
            .info("Copy mode: move, v select, y copy, / search, Esc exit");
    }

    pub(super) fn leave_copy_mode(&mut self, clear_selection: bool) {
        self.copy_mode.exit();
        self.copy_mode_target = None;
        if clear_selection {
            self.mouse_state.clear_selection();
            if let Some(terminal) = self.active_terminal_mut() {
                terminal.set_selection(None);
            }
        }
    }

    pub(super) fn begin_quick_select(&mut self) {
        if !self.copy_mode.is_active() {
            self.enter_copy_mode();
        }
        let patterns = &self.compiled_copy_patterns;
        let (copy_mode, tabs) = (&mut self.copy_mode, &self.tabs);
        let Some(terminal) = tabs
            .as_ref()
            .and_then(crate::tabs::TabManager::active_tab)
            .and_then(crate::tabs::Tab::focused_terminal)
        else {
            return;
        };
        copy_mode.start_quick_select(
            terminal,
            patterns,
            HintAction::Open,
            crate::tabs::presentation_working_directory(terminal),
        );
        let count = copy_mode
            .quick_select()
            .map_or(0, |quick| quick.matches.len());
        if count == 0 {
            copy_mode.cancel_quick_select();
            self.toasts
                .warning("No links or configured patterns are visible");
        } else {
            self.toasts.info(format!(
                "Quick select: type a hint to open; Tab switches to copy ({count} matches)"
            ));
        }
    }

    fn sync_copy_mode_selection(&mut self) {
        let selection = self.copy_mode.selection();
        if let Some(terminal) = self.active_terminal_mut() {
            terminal.set_selection(selection);
        }
    }

    pub(super) fn prepare_copy_mode_reflow(&mut self) {
        let (copy_mode, tabs) = (&mut self.copy_mode, &self.tabs);
        if let Some(grid) = tabs
            .as_ref()
            .and_then(crate::tabs::TabManager::active_tab)
            .and_then(crate::tabs::Tab::focused_terminal)
            .map(termeric_core::Terminal::grid)
        {
            copy_mode.prepare_reflow(grid);
        }
    }

    pub(super) fn finish_copy_mode_reflow(&mut self) {
        if !self.copy_mode.is_active() {
            return;
        }
        let (copy_mode, tabs, patterns) = (
            &mut self.copy_mode,
            &mut self.tabs,
            &self.compiled_copy_patterns,
        );
        if let Some(terminal) = tabs
            .as_mut()
            .and_then(crate::tabs::TabManager::active_tab_mut)
            .and_then(crate::tabs::Tab::focused_terminal_mut)
        {
            copy_mode.finish_reflow(terminal.grid_mut());
            copy_mode.refresh_quick_select(terminal, patterns);
        }
        self.sync_copy_mode_selection();
    }

    fn move_copy_cursor(&mut self, motion: CopyMotion) {
        let (copy_mode, tabs) = (&mut self.copy_mode, &mut self.tabs);
        if let Some(grid) = tabs
            .as_mut()
            .and_then(crate::tabs::TabManager::active_tab_mut)
            .and_then(crate::tabs::Tab::focused_terminal_mut)
            .map(termeric_core::Terminal::grid_mut)
        {
            copy_mode.move_cursor(motion, grid);
        }
        self.sync_copy_mode_selection();
    }

    pub(super) fn jump_copy_cursor_to_search(&mut self) {
        let point = self
            .search
            .current()
            .map(|found| Point::new(found.row, found.start_col));
        let Some(point) = point else { return };
        let (copy_mode, tabs) = (&mut self.copy_mode, &mut self.tabs);
        if let Some(grid) = tabs
            .as_mut()
            .and_then(crate::tabs::TabManager::active_tab_mut)
            .and_then(crate::tabs::Tab::focused_terminal_mut)
            .map(termeric_core::Terminal::grid_mut)
        {
            copy_mode.jump_to(point, grid);
        }
        self.sync_copy_mode_selection();
    }

    fn activate_hint(&mut self, value: String, action: HintAction) {
        let working_directory = self
            .copy_mode
            .quick_select()
            .and_then(|quick| quick.working_directory.clone());
        self.copy_mode.cancel_quick_select();
        match action {
            HintAction::Copy => self.copy_to_clipboard(value, "Copied quick-select match"),
            HintAction::Open => {
                if let Err(error) = crate::terminal::url::UrlDetector::open_url_in_directory(
                    &value,
                    working_directory.as_deref().map(std::path::Path::new),
                ) {
                    self.toasts
                        .error(format!("Could not open {value:?}: {error}"));
                } else {
                    self.toasts.success(format!("Opened {value}"));
                }
            }
        }
    }

    fn handle_quick_select_input(&mut self, event: &KeyEvent) -> bool {
        if self.reconcile_quick_select_presentation() {
            return true;
        }
        if self.copy_mode.quick_select().is_none() {
            return false;
        }
        if event.state != ElementState::Pressed {
            return true;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.copy_mode.cancel_quick_select(),
            Key::Named(NamedKey::Tab) => {
                self.copy_mode.toggle_hint_action();
                let action = self
                    .copy_mode
                    .quick_select()
                    .map_or("open", |quick| quick.action.label());
                self.toasts.info(format!("Quick-select action: {action}"));
            }
            Key::Named(NamedKey::Backspace) => self.copy_mode.backspace_hint(),
            Key::Character(text) if text.chars().count() == 1 => {
                let character = text.chars().next().unwrap_or_default();
                if let Some((selected, action)) = self.copy_mode.push_hint_key(character) {
                    self.activate_hint(selected.value, action);
                }
            }
            _ => {}
        }
        true
    }

    /// Never rebind a partly typed label across a synchronized presentation
    /// boundary. Called before rendering and before consuming hint input.
    pub(super) fn reconcile_quick_select_presentation(&mut self) -> bool {
        if self.copy_mode.quick_select().is_none() {
            return false;
        }
        let current = self
            .active_terminal()
            .is_some_and(|terminal| self.copy_mode.quick_select_presentation_matches(terminal));
        if current {
            return false;
        }
        self.copy_mode.cancel_quick_select();
        self.toasts
            .info("Quick select closed because the displayed snapshot changed");
        true
    }

    /// Modal input has priority over application shortcuts and PTY encoding,
    /// but search input remains one level above it while `/` or Ctrl+S is open.
    pub(super) fn handle_copy_mode_input(&mut self, event: &KeyEvent) -> bool {
        if !self.copy_mode.is_active() {
            return false;
        }
        if self.copy_mode_target != self.active_copy_target() {
            self.leave_copy_mode(false);
            return false;
        }
        if self.handle_quick_select_input(event) {
            return true;
        }
        if event.state != ElementState::Pressed {
            return true;
        }

        let ctrl = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();
        let shift = self.modifiers.shift_key();
        let character = match &event.logical_key {
            Key::Character(text) if text.chars().count() == 1 => {
                text.chars().next().map(|ch| ch.to_ascii_lowercase())
            }
            _ => None,
        };

        if matches!(event.logical_key, Key::Named(NamedKey::Escape))
            || (self.config.copy_mode.bindings == CopyModeBindings::Emacs
                && ctrl
                && character == Some('g'))
        {
            self.leave_copy_mode(true);
            return true;
        }

        let common_motion = match &event.logical_key {
            Key::Named(NamedKey::Home) if ctrl => Some(CopyMotion::Top),
            Key::Named(NamedKey::End) if ctrl => Some(CopyMotion::Bottom),
            Key::Named(NamedKey::ArrowLeft) => Some(CopyMotion::Left),
            Key::Named(NamedKey::ArrowRight) => Some(CopyMotion::Right),
            Key::Named(NamedKey::ArrowUp) => Some(CopyMotion::Up),
            Key::Named(NamedKey::ArrowDown) => Some(CopyMotion::Down),
            Key::Named(NamedKey::Home) => Some(CopyMotion::LineStart),
            Key::Named(NamedKey::End) => Some(CopyMotion::LineEnd),
            Key::Named(NamedKey::PageUp) => Some(CopyMotion::PageUp),
            Key::Named(NamedKey::PageDown) => Some(CopyMotion::PageDown),
            _ => None,
        };
        let style_motion = match self.config.copy_mode.bindings {
            CopyModeBindings::Vi => match character {
                Some('g') if shift => Some(CopyMotion::Bottom),
                Some('h') => Some(CopyMotion::Left),
                Some('j') => Some(CopyMotion::Down),
                Some('k') => Some(CopyMotion::Up),
                Some('l') => Some(CopyMotion::Right),
                Some('0') => Some(CopyMotion::LineStart),
                Some('$') => Some(CopyMotion::LineEnd),
                Some('w') => Some(CopyMotion::WordForward),
                Some('b') => Some(CopyMotion::WordBackward),
                Some('g') if self.copy_mode.take_pending_g() => Some(CopyMotion::Top),
                Some('g') => {
                    self.copy_mode.set_pending_g(true);
                    return true;
                }
                _ => None,
            },
            CopyModeBindings::Emacs => match (ctrl, alt, character) {
                (true, _, Some('b')) => Some(CopyMotion::Left),
                (true, _, Some('f')) => Some(CopyMotion::Right),
                (true, _, Some('p')) => Some(CopyMotion::Up),
                (true, _, Some('n')) => Some(CopyMotion::Down),
                (true, _, Some('a')) => Some(CopyMotion::LineStart),
                (true, _, Some('e')) => Some(CopyMotion::LineEnd),
                (_, true, Some('f')) => Some(CopyMotion::WordForward),
                (_, true, Some('b')) => Some(CopyMotion::WordBackward),
                (true, _, Some('v')) => Some(CopyMotion::PageDown),
                (_, true, Some('v')) => Some(CopyMotion::PageUp),
                _ => None,
            },
        };
        if let Some(motion) = common_motion.or(style_motion) {
            self.copy_mode.set_pending_g(false);
            self.move_copy_cursor(motion);
            return true;
        }

        let search_key = (self.config.copy_mode.bindings == CopyModeBindings::Vi
            && character == Some('/'))
            || (self.config.copy_mode.bindings == CopyModeBindings::Emacs
                && ctrl
                && character == Some('s'));
        if search_key {
            self.search.start();
            self.search_cursor_pos = 0;
            return true;
        }
        if character == Some('n') && self.search.has_results() {
            if shift {
                self.search.prev();
            } else {
                self.search.next();
            }
            self.jump_copy_cursor_to_search();
            return true;
        }

        let selection_type = match self.config.copy_mode.bindings {
            CopyModeBindings::Vi if character == Some('v') && ctrl => Some(SelectionType::Block),
            CopyModeBindings::Vi if character == Some('v') && shift => Some(SelectionType::Lines),
            CopyModeBindings::Vi if character == Some('v') => Some(SelectionType::Simple),
            CopyModeBindings::Emacs
                if matches!(event.logical_key, Key::Named(NamedKey::Space))
                    || character == Some(' ') =>
            {
                Some(SelectionType::Simple)
            }
            _ => None,
        };
        if let Some(selection_type) = selection_type {
            self.copy_mode.toggle_selection(selection_type);
            self.sync_copy_mode_selection();
            return true;
        }

        let yank = (self.config.copy_mode.bindings == CopyModeBindings::Vi
            && character == Some('y'))
            || (self.config.copy_mode.bindings == CopyModeBindings::Emacs
                && alt
                && character == Some('w'))
            || matches!(event.logical_key, Key::Named(NamedKey::Enter));
        if yank {
            if self.copy_selection() {
                self.toasts.success("Copied selection");
                self.leave_copy_mode(false);
            } else {
                self.toasts.warning("Start a selection before copying");
            }
            return true;
        }

        true
    }
}
