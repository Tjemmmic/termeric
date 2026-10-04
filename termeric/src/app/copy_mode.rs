//! App routing for keyboard copy mode and quick-select hints.

use termeric_core::{Point, SelectionType};
use winit::event::ElementState;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::config::CopyModeBindings;
use crate::input::keybindings::BindableAction;
use crate::terminal::copy_mode::{CopyMotion, HintAction};

use super::App;

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
                if quick.entered_copy_mode {
                    " · TAB ACTION · ESC CANCEL"
                } else {
                    " · TAB ACTION · ESC BACK"
                }
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

    fn active_copy_target(&self) -> Option<crate::tabs::TerminalIdentity> {
        Some(
            self.tabs
                .as_ref()?
                .active_tab()?
                .focused_terminal_identity(),
        )
    }

    pub(super) fn enter_copy_mode(&mut self) {
        self.start_copy_mode();
        if self.copy_mode.is_active() {
            self.toasts
                .info("Copy mode: move, v select, y copy, / search, Esc exit");
        }
    }

    /// Enter copy mode without announcing it, for quick select, which
    /// announces itself.
    fn start_copy_mode(&mut self) {
        // Copy mode and search share the presented grid. Clear any previously
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
        copy_mode.enter_presentation(terminal);
        terminal.set_selection(None);
        copy_mode.sync_live_cursor(terminal);
        self.copy_mode_target = target;
        self.mouse_state.clear_selection();
        self.mouse_selection_source = None;
        self.mouse_pressed = false;
    }

    pub(super) fn leave_copy_mode(&mut self, clear_selection: bool) {
        if let Some(target) = self.copy_mode_target
            && let Some(tabs) = &mut self.tabs
            && let Some(terminal) = tabs.terminal_for_origin_mut(crate::tabs::TerminalEventOrigin {
                tab_id: target.tab_id,
                pane_id: target.pane_id,
            })
        {
            if self.copy_mode.owns_navigation_cursor(terminal) {
                terminal.set_navigation_cursor(None);
            }
            if clear_selection && self.copy_mode.presentation_matches(terminal) {
                terminal.set_selection(None);
            }
        }
        self.copy_mode.exit();
        self.copy_mode_target = None;
        if clear_selection {
            self.mouse_state.clear_selection();
        }
    }

    pub(super) fn begin_quick_select(&mut self) {
        self.reconcile_copy_mode_presentation();
        // Quick select returns to the mode it started from. Restarting it
        // keeps the first start's mode.
        let entered_copy_mode = self
            .copy_mode
            .quick_select()
            .map_or(!self.copy_mode.is_active(), |quick| quick.entered_copy_mode);
        if !self.copy_mode.is_active() {
            self.start_copy_mode();
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
            entered_copy_mode,
        );
        let count = copy_mode
            .quick_select()
            .map_or(0, |quick| quick.matches.len());
        if count == 0 {
            self.finish_quick_select();
            self.toasts
                .warning("No links or configured patterns are visible");
        } else {
            self.toasts.info(format!(
                "Quick select: type a hint to open; Tab switches to copy ({count} matches)"
            ));
        }
    }

    /// End quick select, whichever way it ends. Copy mode that quick select
    /// entered ends with it, back to the terminal; otherwise copy mode stays.
    fn finish_quick_select(&mut self) {
        if self.copy_mode.cancel_quick_select() {
            self.leave_copy_mode(false);
        }
    }

    fn sync_copy_mode_selection(&mut self) {
        let selection = self.copy_mode.selection();
        let (copy_mode, tabs) = (&mut self.copy_mode, &mut self.tabs);
        if let Some(terminal) = tabs
            .as_mut()
            .and_then(crate::tabs::TabManager::active_tab_mut)
            .and_then(crate::tabs::Tab::focused_terminal_mut)
        {
            // Frozen viewport coordinates must never be installed on the live
            // grid. The copy-mode overlay owns this selection until release.
            terminal.set_selection(if terminal.synchronized_output_deadline().is_some() {
                None
            } else {
                selection
            });
            copy_mode.sync_live_cursor(terminal);
        }
    }

    pub(super) fn copy_mode_selection_text(&self) -> Option<String> {
        if !self.copy_mode.is_active() || self.copy_mode_target != self.active_copy_target() {
            return None;
        }
        let terminal = self.active_terminal()?;
        if !self.copy_mode.presentation_matches(terminal) {
            return None;
        }
        if terminal.synchronized_output_deadline().is_some() {
            Some(terminal.presentation_selection_text(&self.copy_mode.selection()?))
        } else {
            // The live terminal shifts its selection as output scrolls. Keep
            // that established behavior outside a frozen viewport.
            terminal.get_selection_text()
        }
    }

    /// Reset modal coordinates, hints and search rather than applying them to
    /// a different presentation after begin/release/timeout/restart.
    pub(super) fn reconcile_copy_mode_presentation(&mut self) -> bool {
        if !self.copy_mode.is_active() {
            return false;
        }
        if self.copy_mode_target != self.active_copy_target() {
            self.leave_copy_mode(false);
            return true;
        }
        if self
            .active_terminal()
            .is_some_and(|term| self.copy_mode.presentation_matches(term))
        {
            let (copy_mode, tabs) = (&mut self.copy_mode, &mut self.tabs);
            if let Some(terminal) = tabs
                .as_mut()
                .and_then(crate::tabs::TabManager::active_tab_mut)
                .and_then(crate::tabs::Tab::focused_terminal_mut)
            {
                copy_mode.reconcile_live_output(terminal);
            }
            return false;
        }
        // Hints never carry over to a new view. Copy mode that quick select
        // entered ends with it rather than starting over.
        if self.copy_mode.quick_select().is_some() {
            self.finish_quick_select();
            if !self.copy_mode.is_active() {
                self.toasts
                    .info("Quick select closed because its displayed targets changed");
                return true;
            }
        }
        self.enter_copy_mode();
        self.toasts
            .info("Copy selection reset because the displayed terminal view changed");
        true
    }

    pub(super) fn prepare_copy_mode_reflow(&mut self) {
        self.reconcile_copy_mode_presentation();
        let (copy_mode, tabs) = (&mut self.copy_mode, &mut self.tabs);
        if let Some(grid) = tabs
            .as_mut()
            .and_then(crate::tabs::TabManager::active_tab_mut)
            .and_then(crate::tabs::Tab::focused_terminal_mut)
            .map(termeric_core::Terminal::presentation_grid_mut)
        {
            copy_mode.prepare_reflow(grid);
        }
    }

    pub(super) fn finish_copy_mode_reflow(&mut self) {
        if self.reconcile_copy_mode_presentation() {
            return;
        }
        if !self.copy_mode.is_active() {
            return;
        }
        let (copy_mode, tabs, patterns) = (
            &mut self.copy_mode,
            &mut self.tabs,
            &self.compiled_copy_patterns,
        );
        let mut targets_remain = true;
        if let Some(terminal) = tabs
            .as_mut()
            .and_then(crate::tabs::TabManager::active_tab_mut)
            .and_then(crate::tabs::Tab::focused_terminal_mut)
        {
            copy_mode.finish_reflow(terminal.presentation_grid_mut());
            targets_remain = copy_mode.refresh_quick_select(terminal, patterns);
        }
        if !targets_remain {
            self.finish_quick_select();
            if !self.copy_mode.is_active() {
                return;
            }
        }
        self.sync_copy_mode_selection();
    }

    pub(super) fn move_copy_cursor(&mut self, motion: CopyMotion) {
        if self.reconcile_copy_mode_presentation() {
            return;
        }
        let (copy_mode, tabs) = (&mut self.copy_mode, &mut self.tabs);
        if let Some(grid) = tabs
            .as_mut()
            .and_then(crate::tabs::TabManager::active_tab_mut)
            .and_then(crate::tabs::Tab::focused_terminal_mut)
            .map(termeric_core::Terminal::presentation_grid_mut)
        {
            copy_mode.move_cursor(motion, grid);
        }
        self.sync_copy_mode_selection();
    }

    pub(super) fn jump_copy_cursor_to_search(&mut self) {
        if self.reconcile_copy_mode_presentation() {
            return;
        }
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
            .map(termeric_core::Terminal::presentation_grid_mut)
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
        self.finish_quick_select();
        match action {
            HintAction::Copy => self.copy_to_clipboard(value, "Copied quick-select match"),
            HintAction::Open => {
                if self.activate_hyperlink(
                    self.active_terminal_origin(),
                    &value,
                    working_directory.as_deref(),
                ) {
                    self.toasts.success(format!("Opened {value}"));
                }
            }
        }
    }

    /// Quick select's own keys: Escape, Tab, Backspace, and hint
    /// characters. Any other key goes to `run_copy_mode_shortcut`.
    fn handle_quick_select_input(&mut self, key: &Key, state: ElementState) -> bool {
        if self.reconcile_quick_select_presentation() {
            return true;
        }
        if self.copy_mode.quick_select().is_none() {
            return false;
        }
        if state != ElementState::Pressed {
            return true;
        }
        let plain = self.modifiers.is_empty() && !self.alt_graph_active;
        match key {
            Key::Named(NamedKey::Escape) if plain => self.finish_quick_select(),
            Key::Named(NamedKey::Tab) if plain => {
                self.copy_mode.toggle_hint_action();
                let action = self
                    .copy_mode
                    .quick_select()
                    .map_or("open", |quick| quick.action.label());
                self.toasts.info(format!("Quick-select action: {action}"));
            }
            Key::Named(NamedKey::Backspace) if plain => self.copy_mode.backspace_hint(),
            Key::Character(text) if self.key_types_text() && text.chars().count() == 1 => {
                let character = text.chars().next().unwrap_or_default();
                if let Some((selected, action)) = self.copy_mode.push_hint_key(character) {
                    self.activate_hint(selected.value, action);
                }
            }
            _ => self.run_copy_mode_shortcut(key),
        }
        true
    }

    /// Revalidate targets before rendering or consuming hint input. If a
    /// pending target disappears, consume that input rather than rebinding it.
    pub(super) fn reconcile_quick_select_presentation(&mut self) -> bool {
        if self.copy_mode.quick_select().is_none() {
            return false;
        }
        let targets_remain = self.copy_mode_target == self.active_copy_target() && {
            let (copy_mode, tabs, patterns) = (
                &mut self.copy_mode,
                &self.tabs,
                &self.compiled_copy_patterns,
            );
            tabs.as_ref()
                .and_then(crate::tabs::TabManager::active_tab)
                .and_then(crate::tabs::Tab::focused_terminal)
                .is_some_and(|terminal| copy_mode.refresh_quick_select(terminal, patterns))
        };
        if targets_remain {
            return false;
        }
        self.finish_quick_select();
        self.toasts
            .info("Quick select closed because its displayed targets changed");
        true
    }

    /// Copy mode owns the keyboard below the find box: its own keys first,
    /// then application shortcuts, and nothing ever reaches the terminal.
    pub(super) fn handle_copy_mode_input(&mut self, key: &Key, state: ElementState) -> bool {
        if !self.copy_mode.is_active() {
            return false;
        }
        if self.copy_mode_target != self.active_copy_target() {
            self.leave_copy_mode(false);
            return false;
        }
        if self.reconcile_copy_mode_presentation() {
            return true;
        }
        if self.handle_quick_select_input(key, state) {
            return true;
        }
        if state != ElementState::Pressed {
            return true;
        }
        let pending_g = self.copy_mode.take_pending_g();
        match copy_mode_command(
            key,
            self.modifiers,
            self.alt_graph_active,
            self.config.copy_mode.bindings,
        ) {
            Some(command) => self.run_copy_mode_command(command, pending_g),
            None => self.run_copy_mode_shortcut(key),
        }
        true
    }

    fn run_copy_mode_command(&mut self, command: CopyModeCommand, pending_g: bool) {
        match command {
            CopyModeCommand::Leave => self.leave_copy_mode(true),
            CopyModeCommand::Move(motion) => self.move_copy_cursor(motion),
            CopyModeCommand::G if pending_g => self.move_copy_cursor(CopyMotion::Top),
            CopyModeCommand::G => self.copy_mode.set_pending_g(true),
            CopyModeCommand::Search => {
                self.search.start();
                self.search_cursor_pos = 0;
            }
            CopyModeCommand::NextMatch | CopyModeCommand::PreviousMatch => {
                if !self.search.has_results() {
                    return;
                }
                if command == CopyModeCommand::NextMatch {
                    self.search.next();
                } else {
                    self.search.prev();
                }
                self.jump_copy_cursor_to_search();
            }
            CopyModeCommand::Select(selection_type) => {
                self.copy_mode.toggle_selection(selection_type);
                self.sync_copy_mode_selection();
            }
            CopyModeCommand::Yank => {
                if self.copy_selection() {
                    self.toasts.success("Copied selection");
                    self.leave_copy_mode(false);
                } else {
                    self.toasts.warning("Start a selection before copying");
                }
            }
        }
    }

    /// A key copy mode and quick select don't use themselves runs its
    /// application shortcut, except Paste: copy mode reads the screen and
    /// never types into the terminal. A shortcut that moved to another tab or
    /// pane leaves copy mode through the usual target check.
    fn run_copy_mode_shortcut(&mut self, key: &Key) {
        if self.shortcut_registry.check(key, self.modifiers) == Some(BindableAction::Paste) {
            return;
        }
        if self.run_app_shortcut(key) {
            self.reconcile_copy_mode_presentation();
        }
    }
}

/// What a key does in copy mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyModeCommand {
    Leave,
    Move(CopyMotion),
    /// vi `g`: the first half of `gg`, or `gg` itself.
    G,
    Search,
    NextMatch,
    PreviousMatch,
    Select(SelectionType),
    /// Copy the selection and leave.
    Yank,
}

/// Copy mode's own chords. Modifiers must match exactly, so a chord such as
/// Ctrl+Shift+F is an application shortcut rather than Emacs Ctrl+F.
/// Characters typed alone, with Shift, or through AltGr match by the
/// character they type, such as vi `G`, `$`, and `V`.
fn copy_mode_command(
    key: &Key,
    modifiers: ModifiersState,
    alt_graph: bool,
    bindings: CopyModeBindings,
) -> Option<CopyModeCommand> {
    use CopyModeCommand::{G, Leave, Move, NextMatch, PreviousMatch, Search, Select, Yank};
    let plain = modifiers.is_empty() && !alt_graph;
    let ctrl = modifiers == ModifiersState::CONTROL && !alt_graph;
    let alt = modifiers == ModifiersState::ALT && !alt_graph;
    let typed = alt_graph
        || !modifiers
            .intersects(ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SUPER);
    let command = match key {
        Key::Named(named) => match named {
            NamedKey::Escape if plain => Leave,
            NamedKey::Enter if plain => Yank,
            NamedKey::ArrowLeft if plain => Move(CopyMotion::Left),
            NamedKey::ArrowRight if plain => Move(CopyMotion::Right),
            NamedKey::ArrowUp if plain => Move(CopyMotion::Up),
            NamedKey::ArrowDown if plain => Move(CopyMotion::Down),
            NamedKey::Home if plain => Move(CopyMotion::LineStart),
            NamedKey::End if plain => Move(CopyMotion::LineEnd),
            NamedKey::Home if ctrl => Move(CopyMotion::Top),
            NamedKey::End if ctrl => Move(CopyMotion::Bottom),
            NamedKey::PageUp if plain => Move(CopyMotion::PageUp),
            NamedKey::PageDown if plain => Move(CopyMotion::PageDown),
            NamedKey::Space if plain && bindings == CopyModeBindings::Emacs => {
                Select(SelectionType::Simple)
            }
            _ => return None,
        },
        Key::Character(text) => {
            let mut chars = text.chars();
            let (Some(character), None) = (chars.next(), chars.next()) else {
                return None;
            };
            // Ctrl and Alt chords ignore Caps Lock; typed vi keys don't,
            // since `G` and `V` differ from `g` and `v`.
            let chord = character.to_ascii_lowercase();
            match bindings {
                CopyModeBindings::Vi if ctrl && chord == 'v' => Select(SelectionType::Block),
                CopyModeBindings::Vi if typed => match character {
                    'h' => Move(CopyMotion::Left),
                    'j' => Move(CopyMotion::Down),
                    'k' => Move(CopyMotion::Up),
                    'l' => Move(CopyMotion::Right),
                    '0' => Move(CopyMotion::LineStart),
                    '$' => Move(CopyMotion::LineEnd),
                    'w' => Move(CopyMotion::WordForward),
                    'b' => Move(CopyMotion::WordBackward),
                    'g' => G,
                    'G' => Move(CopyMotion::Bottom),
                    '/' => Search,
                    'n' => NextMatch,
                    'N' => PreviousMatch,
                    'v' => Select(SelectionType::Simple),
                    'V' => Select(SelectionType::Lines),
                    'y' => Yank,
                    _ => return None,
                },
                CopyModeBindings::Emacs if ctrl => match chord {
                    'g' => Leave,
                    'b' => Move(CopyMotion::Left),
                    'f' => Move(CopyMotion::Right),
                    'p' => Move(CopyMotion::Up),
                    'n' => Move(CopyMotion::Down),
                    'a' => Move(CopyMotion::LineStart),
                    'e' => Move(CopyMotion::LineEnd),
                    'v' => Move(CopyMotion::PageDown),
                    's' => Search,
                    _ => return None,
                },
                CopyModeBindings::Emacs if alt => match chord {
                    'f' => Move(CopyMotion::WordForward),
                    'b' => Move(CopyMotion::WordBackward),
                    'v' => Move(CopyMotion::PageUp),
                    'w' => Yank,
                    _ => return None,
                },
                CopyModeBindings::Emacs if typed => match character {
                    'n' => NextMatch,
                    'N' => PreviousMatch,
                    ' ' => Select(SelectionType::Simple),
                    _ => return None,
                },
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_mode_chords_match_modifiers_exactly_and_typed_keys_by_character() {
        use CopyModeBindings::{Emacs, Vi};
        let command = |key: &str, modifiers, bindings| {
            copy_mode_command(&Key::Character(key.into()), modifiers, false, bindings)
        };
        let shift = ModifiersState::SHIFT;
        let ctrl = ModifiersState::CONTROL;
        let alt = ModifiersState::ALT;
        let none = ModifiersState::empty();

        // Shifted vi keys match by the character typed.
        assert_eq!(
            command("G", shift, Vi),
            Some(CopyModeCommand::Move(CopyMotion::Bottom))
        );
        assert_eq!(
            command("$", shift, Vi),
            Some(CopyModeCommand::Move(CopyMotion::LineEnd))
        );
        assert_eq!(
            command("V", shift, Vi),
            Some(CopyModeCommand::Select(SelectionType::Lines))
        );
        assert_eq!(command("g", none, Vi), Some(CopyModeCommand::G));
        // A Ctrl chord is never a typed vi key, and Shift is never ignored.
        assert_eq!(command("h", ctrl, Vi), None);
        assert_eq!(
            command("v", ctrl, Vi),
            Some(CopyModeCommand::Select(SelectionType::Block))
        );
        assert_eq!(command("V", ctrl | shift, Vi), None);
        // Emacs chords: exact Ctrl or Alt.
        assert_eq!(
            command("f", ctrl, Emacs),
            Some(CopyModeCommand::Move(CopyMotion::Right))
        );
        assert_eq!(command("F", ctrl | shift, Emacs), None);
        assert_eq!(command("w", alt, Emacs), Some(CopyModeCommand::Yank));
        assert_eq!(command("f", none, Emacs), None);
        // AltGr types characters even where it reports as Ctrl+Alt.
        assert_eq!(
            copy_mode_command(&Key::Character("$".into()), ctrl | alt, true, Vi),
            Some(CopyModeCommand::Move(CopyMotion::LineEnd))
        );
        // Named keys: plain, or Ctrl only on Home and End.
        let named = |key, modifiers| copy_mode_command(&Key::Named(key), modifiers, false, Vi);
        assert_eq!(
            named(NamedKey::PageUp, none),
            Some(CopyModeCommand::Move(CopyMotion::PageUp))
        );
        assert_eq!(named(NamedKey::PageUp, shift), None);
        assert_eq!(
            named(NamedKey::Home, ctrl),
            Some(CopyModeCommand::Move(CopyMotion::Top))
        );
        assert_eq!(named(NamedKey::Home, ctrl | shift), None);
        assert_eq!(named(NamedKey::Escape, none), Some(CopyModeCommand::Leave));
    }
}
