//! Custom commands panel — slide-in panel for managing user-defined shortcuts
//!
//! Provides a right-side slide-in panel where users can:
//! - View all custom commands with name + command preview
//! - Run a command in the focused terminal by clicking its row
//! - Add new commands via the header's "+" and an inline form
//! - Delete commands with the Delete button shown on the hovered row
//! - Close via scrim or toolbar click, × button, or Escape

use super::navigation::{CommandControl, CommandNavigation, CommandNavigationKey};
use crate::config::CustomCommand;
use crate::ui::animation::PanelAnimation;
use crate::ui::text_input::{TextInput, TextInputEdit};
use crate::ui::text_input_viewport::TextInputViewport;
use unicode_segmentation::UnicodeSegmentation;

/// Example text shown in the empty add-form fields.
pub const NAME_PLACEHOLDER: &str = "Deploy staging";
pub const COMMAND_PLACEHOLDER: &str = "ssh deploy@staging";

/// Action returned by the commands panel after a click
#[derive(Debug, Clone)]
pub enum CommandsPanelAction {
    /// Run the command at the given index
    Run(usize),
    /// Delete the command at the given index
    Delete(usize),
    /// Start or reset the inline add-command form.
    StartAdd,
    /// Submit the inline add-command form.
    Submit,
    /// Focus the Name field (for semantic focus actions).
    ClickName,
    /// Focus the Command field (for semantic focus actions).
    ClickCommand,
    /// Place the caret at a visible whole-grapheme boundary in one field.
    PlaceCaret { field: EditMode, cursor: usize },
    /// Cancel the inline form without closing the panel.
    CancelEdit,
    /// Close the panel
    Close,
    /// No action
    None,
}

/// Editing mode for inline add-command form
#[derive(Debug, Clone, PartialEq)]
pub enum EditMode {
    /// Not editing — normal browse mode
    Browse,
    /// Editing the Name field of a new command
    EditingName,
    /// Editing the Command field of a new command
    EditingCommand,
}

/// State for the custom commands panel
pub struct CommandsPanel {
    /// Animation state
    animation: PanelAnimation,
    /// First requested list row; geometry clamps this after resize/config changes.
    scroll_offset: usize,
    navigation: CommandNavigation,
    /// Control under the pointer.
    pub hovered: Option<CommandControl>,
    /// Current edit mode
    pub edit_mode: EditMode,
    /// Name input for inline form
    pub form_name: TextInput,
    /// Command input for inline form
    pub form_command: TextInput,
}

/// Info piped to the renderer
#[derive(Debug, Clone)]
pub struct CommandsPanelInfo {
    /// Animation progress (0.0 = hidden, 1.0 = fully visible)
    pub animation: f32,
    pub scroll_offset: usize,
    pub browse_focus: Option<CommandControl>,
    /// List of commands to display
    pub commands: Vec<CustomCommand>,
    /// Control under the pointer.
    pub hovered: Option<CommandControl>,
    /// Current edit mode
    pub edit_mode: EditMode,
    /// Name field content (for inline form)
    pub form_name: String,
    /// Command field content (for inline form)
    pub form_command: String,
    /// Cursor position in active field (byte offset)
    pub form_cursor: usize,
}

/// Rectangle used by the commands panel's shared render, hit-test, and
/// accessibility geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandsPanelRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl CommandsPanelRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

/// Inline form geometry within the commands panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandsFormLayout {
    pub container: CommandsPanelRect,
    pub name_input: CommandsPanelRect,
    pub command_input: CommandsPanelRect,
    pub save_button: CommandsPanelRect,
    pub cancel_button: CommandsPanelRect,
}

/// Single source of truth for custom-command panel geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandsPanelLayout {
    pub panel: CommandsPanelRect,
    pub padding: f32,
    pub small_padding: f32,
    pub title_y: f32,
    /// Line of explanatory text under the title.
    pub note_y: f32,
    pub separator_y: f32,
    pub close_button: CommandsPanelRect,
    /// The header's "+" control, beside close.
    pub new_button: CommandsPanelRect,
    pub form: Option<CommandsFormLayout>,
    pub list_start_y: f32,
    pub row_height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    pub scroll_offset: usize,
    pub visible_count: usize,
    command_count: usize,
}

impl CommandsPanelLayout {
    /// Resolve a field click from the exact viewport used for painting.
    /// A collapsed field consumes its click without moving a hidden caret.
    pub fn input_click(
        &self,
        info: &CommandsPanelInfo,
        x: f32,
        y: f32,
    ) -> Option<CommandsPanelAction> {
        let form = self.form?;
        for (field, rect, text) in [
            (EditMode::EditingName, form.name_input, &info.form_name),
            (
                EditMode::EditingCommand,
                form.command_input,
                &info.form_command,
            ),
        ] {
            if rect.contains(x, y) {
                return Some(
                    self.input_viewport(info, field.clone())
                        .and_then(|view| view.cursor_at(text, x, self.cell_width))
                        .map_or(CommandsPanelAction::None, |cursor| {
                            CommandsPanelAction::PlaceCaret { field, cursor }
                        }),
                );
            }
        }
        None
    }

    /// Whole-grapheme field geometry shared by painting, pointer, and IME placement.
    /// Inactive fields show their beginning; only the active caret scrolls.
    pub fn input_viewport(
        &self,
        info: &CommandsPanelInfo,
        field: EditMode,
    ) -> Option<TextInputViewport> {
        let form = self.form?;
        let (text, rect) = match field {
            EditMode::EditingName => (&info.form_name, form.name_input),
            EditMode::EditingCommand => (&info.form_command, form.command_input),
            EditMode::Browse => return None,
        };
        let cursor = if info.edit_mode == field {
            info.form_cursor
        } else {
            0
        };
        TextInputViewport::calculate(
            text,
            cursor,
            rect.x + self.small_padding,
            rect.y + (rect.height - self.cell_height) / 2.0,
            rect.x + rect.width - self.small_padding,
            self.cell_width,
        )
    }

    /// Geometry of the panel, which slides in from the right edge and spans
    /// from `top` (below the toolbar) to the bottom of the window.
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        top: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        editing: bool,
        command_count: usize,
    ) -> Self {
        // Shares the Workspace Manager's width so side panels line up.
        let panel_width = (cell_width * 44.0).max(380.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * animation;
        let padding = cell_width * 1.5;
        let small_padding = cell_width * 0.8;
        let title_y = top + cell_height * 0.9;
        let control = cell_height * 1.5;
        let close_button = CommandsPanelRect {
            x: panel_x + panel_width - padding * 0.5 - control,
            y: title_y,
            width: control,
            height: control,
        };
        let new_button = CommandsPanelRect {
            x: close_button.x - control - cell_width * 0.25,
            ..close_button
        };
        let note_y = title_y + control;
        let separator_y = note_y + cell_height * 1.7;
        let mut list_start_y = separator_y + cell_height * 0.5;
        let form = editing.then(|| {
            let form_x = panel_x + padding;
            let form_width = panel_width - padding * 2.0;
            let field_height = cell_height * 1.8;
            let label_width = cell_width * 10.0;
            let field_y = list_start_y + small_padding;
            let input_x = form_x + label_width;
            let input_width = form_width - label_width - small_padding;
            let command_y = field_y + field_height + small_padding;
            let buttons_y = command_y + field_height + small_padding * 1.5;
            let save_width = cell_width * 7.0;
            let save_x = form_x + form_width - small_padding - save_width;
            let save_height = cell_height * 1.5;
            let cancel_width = cell_width * 8.0;
            let cancel_x = save_x - cancel_width - small_padding;
            let form_height = buttons_y + save_height + small_padding - list_start_y;
            let layout = CommandsFormLayout {
                container: CommandsPanelRect {
                    x: form_x,
                    y: list_start_y,
                    width: form_width,
                    height: form_height,
                },
                name_input: CommandsPanelRect {
                    x: input_x,
                    y: field_y,
                    width: input_width,
                    height: field_height,
                },
                command_input: CommandsPanelRect {
                    x: input_x,
                    y: command_y,
                    width: input_width,
                    height: field_height,
                },
                save_button: CommandsPanelRect {
                    x: save_x,
                    y: buttons_y,
                    width: save_width,
                    height: save_height,
                },
                cancel_button: CommandsPanelRect {
                    x: cancel_x,
                    y: buttons_y,
                    width: cancel_width,
                    height: save_height,
                },
            };
            list_start_y += form_height + cell_height * 0.5;
            layout
        });
        let row_height = cell_height * 3.0;
        Self {
            panel: CommandsPanelRect {
                x: panel_x,
                y: top,
                width: panel_width,
                height: (window_height - top).max(0.0),
            },
            padding,
            small_padding,
            title_y,
            note_y,
            separator_y,
            close_button,
            new_button,
            form,
            list_start_y,
            row_height,
            cell_width,
            cell_height,
            scroll_offset: 0,
            visible_count: (((window_height - list_start_y).max(0.0) / row_height).floor()
                as usize)
                .min(command_count),
            command_count,
        }
    }

    /// Whole rows only: no clipped command may expose an invisible Run/Delete target.
    pub fn scrolled(mut self, offset: usize) -> Self {
        self.scroll_offset = if self.visible_count == 0 {
            0
        } else {
            offset.min(self.command_count.saturating_sub(self.visible_count))
        };
        self
    }

    pub fn visible_range(self) -> std::ops::Range<usize> {
        self.scroll_offset..self.scroll_offset + self.visible_count
    }

    /// Resolve only painted rows, preserving their absolute command identity.
    pub fn row_action(self, x: f32, y: f32) -> CommandsPanelAction {
        if !self.panel.contains(x, y) {
            return CommandsPanelAction::None;
        }
        for index in self.visible_range() {
            let row = self.command_row(index);
            if y < row.y || y >= row.y + row.height {
                continue;
            }
            if self.command_delete(index).contains(x, y) {
                return CommandsPanelAction::Delete(index);
            }
            if self.command_run(index).contains(x, y) {
                return CommandsPanelAction::Run(index);
            }
        }
        CommandsPanelAction::None
    }

    /// The control under the pointer, for hover: header buttons first, then
    /// the painted rows.
    pub fn control_at(self, x: f32, y: f32) -> Option<CommandControl> {
        if self.close_button.contains(x, y) {
            return Some(CommandControl::Close);
        }
        if self.new_button.contains(x, y) {
            return Some(CommandControl::New);
        }
        match self.row_action(x, y) {
            CommandsPanelAction::Run(index) => Some(CommandControl::Run(index)),
            CommandsPanelAction::Delete(index) => Some(CommandControl::Delete(index)),
            _ => None,
        }
    }

    pub fn command_row(self, index: usize) -> CommandsPanelRect {
        let relative = if index >= self.scroll_offset {
            (index - self.scroll_offset) as f32
        } else {
            -((self.scroll_offset - index) as f32)
        };
        CommandsPanelRect {
            x: self.panel.x,
            y: self.list_start_y + relative * self.row_height,
            width: self.panel.width,
            height: self.row_height,
        }
    }

    pub fn control_rect(self, control: CommandControl) -> Option<CommandsPanelRect> {
        match control {
            CommandControl::New => Some(self.new_button),
            CommandControl::Close => Some(self.close_button),
            CommandControl::Run(index) if self.visible_range().contains(&index) => {
                Some(self.command_run(index))
            }
            CommandControl::Delete(index) if self.visible_range().contains(&index) => {
                Some(self.command_delete(index))
            }
            _ => None,
        }
    }

    /// Everything on the row left of Delete runs the command.
    pub fn command_run(self, index: usize) -> CommandsPanelRect {
        let row = self.command_row(index);
        let delete = self.command_delete(index);
        CommandsPanelRect {
            width: (delete.x - row.x).max(0.0),
            ..row
        }
    }

    /// The row's Delete button, drawn while the row is hovered or focused.
    pub fn command_delete(self, index: usize) -> CommandsPanelRect {
        let row = self.command_row(index);
        let width = self.cell_width * 8.0;
        let height = self.cell_height * 1.5;
        CommandsPanelRect {
            x: self.panel.x + self.panel.width - self.padding - width,
            y: row.y + (row.height - height) / 2.0,
            width,
            height,
        }
    }
}

impl CommandsPanel {
    const MAX_FORM_NAME_BYTES: usize = 256;
    pub(crate) const MAX_FORM_COMMAND_BYTES: usize = 16 * 1024;

    pub fn new() -> Self {
        Self {
            animation: PanelAnimation::with_duration(0.25),
            scroll_offset: 0,
            navigation: CommandNavigation::default(),
            hovered: None,
            edit_mode: EditMode::Browse,
            form_name: TextInput::new(),
            form_command: TextInput::new(),
        }
    }

    /// Toggle the panel open/closed
    pub fn toggle(&mut self) {
        self.cancel_edit();
        if !self.animation.is_open() {
            self.scroll_offset = 0;
            // Reset hover state when opening
            self.hovered = None;
        }
        self.animation.toggle();
    }

    /// Close the panel
    pub fn close(&mut self) {
        if self.animation.is_open() {
            self.cancel_edit();
            self.animation.close();
        }
    }

    /// Whether the panel is currently open
    pub fn is_open(&self) -> bool {
        self.animation.is_open()
    }

    /// Whether the panel is fully closed (animation complete)
    pub fn is_closed(&self) -> bool {
        self.animation.is_closed()
    }

    /// Whether the panel animation needs a redraw
    pub fn needs_redraw(&self) -> bool {
        self.animation.needs_redraw()
    }

    /// Whether the inline form is active
    pub fn is_editing(&self) -> bool {
        self.edit_mode != EditMode::Browse
    }

    /// Current animation progress (0.0 to 1.0)
    pub fn animation_progress(&self) -> f32 {
        self.animation.progress()
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.animation.set_reduced_motion(reduced_motion);
    }

    /// Update hover state from mouse position
    pub fn set_hover(&mut self, control: Option<CommandControl>) {
        self.hovered = control;
    }

    pub fn layout(
        &self,
        width: f32,
        height: f32,
        top: f32,
        cell_width: f32,
        cell_height: f32,
        command_count: usize,
    ) -> CommandsPanelLayout {
        CommandsPanelLayout::calculate(
            width,
            height,
            top,
            cell_width,
            cell_height,
            self.animation_progress(),
            self.is_editing(),
            command_count,
        )
        .scrolled(self.scroll_offset)
    }

    /// Scroll against current geometry, not a stale row count from a previous frame.
    pub fn scroll_rows(&mut self, delta: isize, layout: CommandsPanelLayout) -> bool {
        if !self.is_open() || delta == 0 || layout.visible_count == 0 {
            return false;
        }
        let current = layout.scrolled(self.scroll_offset).scroll_offset;
        let next = layout
            .scrolled(current.saturating_add_signed(delta))
            .scroll_offset;
        self.scroll_offset = next;
        if next == current {
            return false;
        }
        self.set_hover(None);
        true
    }

    /// Assistive focus must bring the exact revalidated command into the viewport.
    pub fn reveal_row(&mut self, index: usize, layout: CommandsPanelLayout) -> bool {
        if !self.is_open() || index >= layout.command_count || layout.visible_count == 0 {
            return false;
        }
        let current = layout.scrolled(self.scroll_offset);
        self.scroll_offset = if index < current.scroll_offset {
            index
        } else if index >= current.visible_range().end {
            index + 1 - current.visible_count
        } else {
            current.scroll_offset
        };
        self.set_hover(None);
        true
    }

    pub fn browse_focus(&self, commands: &[CustomCommand]) -> Option<CommandControl> {
        (!self.is_editing())
            .then(|| self.navigation.resolve(commands))
            .flatten()
    }

    /// Configuration replacement may make duplicate occurrences ambiguous.
    /// Retire only those browse targets; keep open form text and its caret.
    pub fn retain_browse_targets(
        &mut self,
        keys: &std::collections::HashSet<super::navigation::CommandKey>,
    ) {
        self.navigation.retain(keys);
    }

    pub fn focus_browse_control(
        &mut self,
        control: CommandControl,
        commands: &[CustomCommand],
        layout: CommandsPanelLayout,
    ) -> bool {
        if !self.is_open() || self.is_editing() {
            return false;
        }
        if let Some(index) = control.row()
            && !self.reveal_row(index, layout)
        {
            return false;
        }
        self.navigation.focus(control, commands)
    }

    pub fn navigate_browse(
        &mut self,
        key: CommandNavigationKey,
        commands: &[CustomCommand],
        layout: CommandsPanelLayout,
    ) {
        if !self.is_open() || self.is_editing() {
            return;
        }
        let control = self.navigation.navigate(key, commands);
        if let Some(index) = control.row() {
            self.reveal_row(index, layout);
        }
    }

    /// Revalidate exact command identity before returning a mutating action.
    /// An off-screen target is revealed on the first activation, never run blind.
    pub fn activate_browse(
        &mut self,
        commands: &[CustomCommand],
        layout: CommandsPanelLayout,
    ) -> CommandsPanelAction {
        if !self.is_open() || self.is_editing() {
            return CommandsPanelAction::None;
        }
        let Some(control) = self.navigation.resolve(commands) else {
            self.navigation.focus(CommandControl::New, commands);
            return CommandsPanelAction::None;
        };
        if let Some(index) = control.row()
            && !layout.visible_range().contains(&index)
        {
            self.reveal_row(index, layout);
            return CommandsPanelAction::None;
        }
        match control {
            CommandControl::New => CommandsPanelAction::StartAdd,
            CommandControl::Close => CommandsPanelAction::Close,
            CommandControl::Run(index) => CommandsPanelAction::Run(index),
            CommandControl::Delete(index) => {
                // Holding Enter must not delete the row that shifts into this slot.
                self.navigation.focus(CommandControl::New, commands);
                CommandsPanelAction::Delete(index)
            }
        }
    }

    // ── Inline form editing ──

    /// Start adding a new command
    pub fn start_add(&mut self) {
        self.edit_mode = EditMode::EditingName;
        self.form_name.clear();
        self.form_command.clear();
    }

    /// Cancel the inline form
    pub fn cancel_edit(&mut self) {
        self.navigation = CommandNavigation::default();
        self.edit_mode = EditMode::Browse;
        self.form_name.clear();
        self.form_command.clear();
    }

    /// Switch focus to the next field (Tab key)
    pub fn next_field(&mut self) {
        match self.edit_mode {
            EditMode::EditingName => {
                self.edit_mode = EditMode::EditingCommand;
                self.form_command.cursor_end();
            }
            EditMode::EditingCommand => {
                self.edit_mode = EditMode::EditingName;
                self.form_name.cursor_end();
            }
            EditMode::Browse => {}
        }
    }

    /// Focus the Name field (e.g. from a click)
    pub fn focus_name(&mut self) {
        self.edit_mode = EditMode::EditingName;
        self.form_name.cursor_end();
    }

    /// Focus the Command field (e.g. from a click)
    pub fn focus_command(&mut self) {
        self.edit_mode = EditMode::EditingCommand;
        self.form_command.cursor_end();
    }

    /// Get the active text input for the current edit mode
    fn active_input(&mut self) -> Option<&mut TextInput> {
        match self.edit_mode {
            EditMode::EditingName => Some(&mut self.form_name),
            EditMode::EditingCommand => Some(&mut self.form_command),
            EditMode::Browse => None,
        }
    }

    /// Edit only the active field, preserving the other field and form limits.
    pub fn form_edit(&mut self, edit: TextInputEdit) {
        if let Some(input) = self.active_input() {
            input.edit_grapheme(edit);
        }
    }

    /// Reject hidden forms and invalid/interior-cluster positions before
    /// changing either field focus or caret. Text and form limits are untouched.
    pub fn place_form_caret(&mut self, field: EditMode, cursor: usize) -> bool {
        if !self.is_open() || !self.is_editing() {
            return false;
        }
        let input = match field {
            EditMode::EditingName => &mut self.form_name,
            EditMode::EditingCommand => &mut self.form_command,
            EditMode::Browse => return false,
        };
        if cursor != input.text().len()
            && !input
                .text()
                .grapheme_indices(true)
                .any(|(offset, _)| offset == cursor)
        {
            return false;
        }
        if !input.set_cursor_byte(cursor) {
            return false;
        }
        self.edit_mode = field;
        true
    }

    fn valid_form_text(text: &str, limit: usize) -> bool {
        text.len() <= limit && !text.chars().any(crate::safe_text::breaks_single_line)
    }

    /// Atomic keyboard, IME, and clipboard insertion into the open form.
    /// Never truncate a command, execute it, or change focus on rejection.
    pub fn insert_form_text(&mut self, text: &str) -> bool {
        if !self.is_open() {
            return false;
        }
        let limit = match self.edit_mode {
            EditMode::EditingName => Self::MAX_FORM_NAME_BYTES,
            EditMode::EditingCommand => Self::MAX_FORM_COMMAND_BYTES,
            EditMode::Browse => return false,
        };
        if text.is_empty() || !Self::valid_form_text(text, limit) {
            return false;
        }
        let input = self.active_input().expect("editable field checked above");
        if input.text().len().saturating_add(text.len()) > limit {
            return false;
        }
        input.insert_text(text);
        true
    }

    /// Replace a form field from an assistive-technology value action. The
    /// same single-line and size limits apply as to typed input.
    pub fn accessibility_set_name(&mut self, value: &str) -> bool {
        if !Self::valid_form_text(value, Self::MAX_FORM_NAME_BYTES) {
            return false;
        }
        self.focus_name();
        self.form_name.replace(value);
        true
    }

    pub fn accessibility_set_command(&mut self, value: &str) -> bool {
        if !Self::valid_form_text(value, Self::MAX_FORM_COMMAND_BYTES) {
            return false;
        }
        self.focus_command();
        self.form_command.replace(value);
        true
    }

    /// Try to submit the form — returns the new command if valid
    pub fn try_submit(&mut self) -> Option<CustomCommand> {
        let name = self.form_name.text().trim().to_string();
        let command = self.form_command.text().trim().to_string();
        if name.is_empty()
            || command.is_empty()
            || name.len() > Self::MAX_FORM_NAME_BYTES
            || command.len() > Self::MAX_FORM_COMMAND_BYTES
            || name.contains(['\0', '\n', '\r'])
            || command.contains(['\0', '\n', '\r'])
        {
            return None;
        }
        self.cancel_edit();
        Some(CustomCommand {
            name,
            command,
            alias: None,
        })
    }

    /// Build info for the renderer
    pub fn build_info(&self, commands: &[CustomCommand]) -> CommandsPanelInfo {
        CommandsPanelInfo {
            animation: self.animation_progress(),
            scroll_offset: self.scroll_offset,
            browse_focus: self
                .navigation
                .visible
                .then(|| self.browse_focus(commands))
                .flatten(),
            commands: commands.to_vec(),
            hovered: self.hovered,
            edit_mode: self.edit_mode.clone(),
            form_name: self.form_name.text().to_string(),
            form_command: self.form_command.text().to_string(),
            form_cursor: match self.edit_mode {
                EditMode::EditingName => self.form_name.cursor(),
                EditMode::EditingCommand => self.form_command.cursor(),
                EditMode::Browse => 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_browse_navigation_reveals_and_revalidates_before_activation() {
        let mut commands: Vec<_> = (0..100)
            .map(|index| CustomCommand {
                name: format!("Command {index}"),
                command: "inert".into(),
                alias: None,
            })
            .collect();
        let mut panel = CommandsPanel::new();
        panel.set_reduced_motion(true);
        let layout = panel.layout(800.0, 600.0, 0.0, 8.0, 16.0, commands.len());
        assert!(matches!(
            panel.activate_browse(&commands, layout),
            CommandsPanelAction::None
        ));
        panel.toggle();
        assert_eq!(panel.build_info(&commands).browse_focus, None);
        for _ in 0..199 {
            let layout = panel.layout(800.0, 600.0, 0.0, 8.0, 16.0, commands.len());
            panel.navigate_browse(CommandNavigationKey::Next, &commands, layout);
        }
        let layout = panel.layout(800.0, 600.0, 0.0, 8.0, 16.0, commands.len());
        assert_eq!(
            panel.build_info(&commands).browse_focus,
            Some(CommandControl::Run(99))
        );
        assert!(layout.visible_range().contains(&99));
        assert_eq!(
            layout.control_rect(CommandControl::Run(99)),
            Some(layout.command_run(99))
        );
        assert_eq!(layout.control_rect(CommandControl::Run(0)), None);
        assert!(matches!(
            panel.activate_browse(&commands, layout),
            CommandsPanelAction::Run(99)
        ));
        panel.scroll_rows(isize::MIN, layout);
        let hidden = panel.layout(800.0, 600.0, 0.0, 8.0, 16.0, commands.len());
        assert!(matches!(
            panel.activate_browse(&commands, hidden),
            CommandsPanelAction::None
        ));
        assert!(
            panel
                .layout(800.0, 600.0, 0.0, 8.0, 16.0, commands.len())
                .visible_range()
                .contains(&99)
        );
        let tiny = panel.layout(800.0, 100.0, 0.0, 8.0, 16.0, commands.len());
        assert!(matches!(
            panel.activate_browse(&commands, tiny),
            CommandsPanelAction::None
        ));
        let visible = panel.layout(800.0, 600.0, 0.0, 8.0, 16.0, commands.len());
        assert!(panel.focus_browse_control(CommandControl::Delete(99), &commands, visible));
        assert_eq!(
            visible.control_rect(CommandControl::Delete(99)),
            Some(visible.command_delete(99))
        );
        assert!(matches!(
            panel.activate_browse(&commands, visible),
            CommandsPanelAction::Delete(99)
        ));
        assert_eq!(panel.browse_focus(&commands), Some(CommandControl::New));
        assert!(panel.focus_browse_control(CommandControl::Run(99), &commands, visible));
        commands[99].command = "changed".into();
        assert_eq!(panel.build_info(&commands).browse_focus, None);
        assert!(matches!(
            panel.activate_browse(&commands, visible),
            CommandsPanelAction::None
        ));
        assert_eq!(panel.browse_focus(&commands), Some(CommandControl::New));
        panel.start_add();
        assert!(matches!(
            panel.activate_browse(&commands, visible),
            CommandsPanelAction::None
        ));
        panel.cancel_edit();
        assert_eq!(panel.browse_focus(&commands), Some(CommandControl::New));
        panel.close();
        panel.toggle();
        assert_eq!(panel.build_info(&commands).browse_focus, None);
    }

    #[test]
    fn scrolled_rows_share_visible_run_delete_geometry_at_all_panel_sizes() {
        for (width, height, cell) in [
            (800.0, 600.0, 8.0),
            (1600.0, 1200.0, 16.0),
            (800.0, 180.0, 8.0),
        ] {
            for editing in [false, true] {
                for animation in [0.25, 1.0] {
                    for count in [0, 1, 100] {
                        let layout = CommandsPanelLayout::calculate(
                            width,
                            height,
                            0.0,
                            cell,
                            cell * 2.0,
                            animation,
                            editing,
                            count,
                        )
                        .scrolled(usize::MAX);
                        assert!(layout.visible_range().end <= count);
                        if layout.visible_count > 0 {
                            assert_eq!(layout.visible_range().end, count);
                        }
                        for index in layout.visible_range() {
                            let row = layout.command_row(index);
                            assert!(row.y >= layout.list_start_y);
                            assert!(row.y + row.height <= height);
                            let run = layout.command_run(index);
                            let delete = layout.command_delete(index);
                            let y = row.y + row.height / 2.0;
                            assert!(
                                matches!(layout.row_action(run.x + 1.0, y), CommandsPanelAction::Run(i) if i == index)
                            );
                            // During sliding, the delete button may still be outside the window.
                            if delete.x + delete.width / 2.0 <= width {
                                assert!(
                                    matches!(layout.row_action(delete.x + delete.width / 2.0, y), CommandsPanelAction::Delete(i) if i == index)
                                );
                            }
                        }
                        for (x, y) in [
                            (layout.panel.x + 1.0, layout.list_start_y - 1.0),
                            (
                                layout.panel.x + 1.0,
                                layout.list_start_y
                                    + layout.visible_count as f32 * layout.row_height,
                            ),
                            (layout.panel.x - 1.0, layout.list_start_y),
                            (
                                layout.panel.x + layout.panel.width + 1.0,
                                layout.list_start_y,
                            ),
                            (f32::NAN, layout.list_start_y),
                        ] {
                            assert!(matches!(layout.row_action(x, y), CommandsPanelAction::None));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn command_scroll_clamps_after_resize_form_and_inventory_changes_and_resets_on_open() {
        let layout = |height, editing, count| {
            CommandsPanelLayout::calculate(800.0, height, 0.0, 8.0, 16.0, 1.0, editing, count)
        };
        let mut panel = CommandsPanel::new();
        assert!(!panel.scroll_rows(1, layout(600.0, false, 100)));
        panel.toggle();
        panel.set_hover(Some(CommandControl::Delete(0)));
        assert!(panel.scroll_rows(isize::MAX, layout(600.0, false, 100)));
        assert_eq!(
            panel.scroll_offset,
            100 - layout(600.0, false, 100).visible_count
        );
        assert_eq!(panel.hovered, None);
        assert!(!panel.scroll_rows(1, layout(600.0, false, 100)));
        assert!(panel.scroll_rows(isize::MIN, layout(600.0, false, 100)));
        assert_eq!(panel.scroll_offset, 0);
        assert!(panel.reveal_row(99, layout(600.0, false, 100)));
        let old_offset = panel.scroll_offset;
        assert!(!panel.reveal_row(100, layout(600.0, false, 100)));
        assert_eq!(panel.scroll_offset, old_offset);
        panel.start_add();
        panel.form_name.replace("kept draft");
        assert!(panel.reveal_row(99, layout(600.0, true, 100)));
        assert!(panel.scroll_offset > old_offset);
        assert_eq!(panel.form_name.text(), "kept draft");
        let tiny = layout(100.0, true, 100).scrolled(panel.scroll_offset);
        assert!(tiny.visible_range().is_empty());
        assert!(!panel.scroll_rows(-1, tiny));
        assert!(!panel.reveal_row(99, tiny));
        let smaller_inventory = layout(1200.0, false, 3).scrolled(panel.scroll_offset);
        assert_eq!(smaller_inventory.visible_range(), 0..3);
        assert!(!panel.scroll_rows(1, smaller_inventory));
        assert_eq!(panel.scroll_offset, 0);
        assert!(panel.scroll_rows(20, layout(600.0, false, 100)));
        panel.close();
        assert!(!panel.is_open());
        panel.toggle();
        assert_eq!(panel.scroll_offset, 0);
    }

    #[test]
    fn panel_sits_below_the_toolbar_and_hover_names_the_control_under_the_pointer() {
        let centre =
            |rect: CommandsPanelRect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let layout = CommandsPanelLayout::calculate(1000.0, 800.0, 34.0, 10.0, 20.0, 1.0, false, 3);
        assert_eq!(layout.panel.y, 34.0);
        assert_eq!(layout.panel.height, 766.0);
        assert!(layout.close_button.y > layout.panel.y);
        assert!(layout.new_button.x + layout.new_button.width <= layout.close_button.x);
        assert!(layout.list_start_y > layout.separator_y);

        let (x, y) = centre(layout.close_button);
        assert_eq!(layout.control_at(x, y), Some(CommandControl::Close));
        let (x, y) = centre(layout.new_button);
        assert_eq!(layout.control_at(x, y), Some(CommandControl::New));
        let (x, y) = centre(layout.command_delete(1));
        assert_eq!(layout.control_at(x, y), Some(CommandControl::Delete(1)));
        let row = layout.command_row(2);
        assert_eq!(
            layout.control_at(row.x + 20.0, row.y + row.height / 2.0),
            Some(CommandControl::Run(2))
        );
        assert_eq!(layout.control_at(layout.panel.x + 20.0, 10.0), None);
    }

    #[test]
    fn shared_layout_keeps_form_and_row_actions_disjoint() {
        let layout = CommandsPanelLayout::calculate(1000.0, 800.0, 0.0, 10.0, 20.0, 1.0, true, 2);
        let form = layout.form.unwrap();
        assert!(layout.panel.contains(form.name_input.x, form.name_input.y));
        assert!(
            !form
                .name_input
                .contains(form.command_input.x, form.command_input.y)
        );
        assert!(!form.save_button.contains(
            form.cancel_button.x + form.cancel_button.width / 2.0,
            form.cancel_button.y + form.cancel_button.height / 2.0,
        ));
        let run = layout.command_run(0);
        let delete = layout.command_delete(0);
        assert!(run.x + run.width <= delete.x);
        assert_eq!(
            layout.command_row(1).y,
            layout.command_row(0).y + layout.row_height
        );
    }

    #[test]
    fn form_clicks_choose_visible_grapheme_boundaries_in_both_scaled_fields() {
        let text = "a界e\u{301}👩\u{200d}💻🇺🇸z";
        let mut panel = CommandsPanel::new();
        panel.toggle();
        panel.start_add();
        panel.form_name.replace(text);
        panel.form_command.replace(text);
        for (width, cell) in [(800.0, 8.0), (1600.0, 16.0), (900.0, 10.0)] {
            for animation in [0.25, 1.0] {
                let layout = CommandsPanelLayout::calculate(
                    width,
                    800.0,
                    0.0,
                    cell,
                    cell * 2.0,
                    animation,
                    true,
                    0,
                );
                for field in [EditMode::EditingName, EditMode::EditingCommand] {
                    assert!(panel.place_form_caret(field.clone(), 0));
                    let info = panel.build_info(&[]);
                    let rect = if field == EditMode::EditingName {
                        layout.form.unwrap().name_input
                    } else {
                        layout.form.unwrap().command_input
                    };
                    let view = layout.input_viewport(&info, field.clone()).unwrap();
                    for (column, expected) in [
                        (0.25, 0),
                        (0.75, 1),
                        (1.25, 1),
                        (2.25, "a界".len()),
                        (3.25, "a界".len()),
                        (3.75, "a界e\u{301}".len()),
                        (4.25, "a界e\u{301}".len()),
                        (5.25, "a界e\u{301}👩\u{200d}💻".len()),
                    ] {
                        let action = layout
                            .input_click(
                                &info,
                                view.text_x + column * cell,
                                rect.y + rect.height / 2.0,
                            )
                            .unwrap();
                        let CommandsPanelAction::PlaceCaret {
                            field: target,
                            cursor,
                        } = action
                        else {
                            panic!("not a caret action");
                        };
                        assert_eq!(target, field);
                        assert_eq!(cursor, expected);
                        assert!(panel.place_form_caret(target, cursor));
                        assert_eq!(panel.active_input().unwrap().text(), text);
                    }
                    for (x, expected) in [(rect.x, 0), (rect.x + rect.width, text.len())] {
                        assert!(matches!(layout.input_click(&info, x, rect.y),
                            Some(CommandsPanelAction::PlaceCaret { cursor, .. }) if cursor == expected));
                    }
                    assert!(
                        layout
                            .input_click(&info, view.text_x, rect.y - 1.0)
                            .is_none()
                    );
                    assert!(layout.input_click(&info, f32::NAN, rect.y).is_none());
                }
            }
        }
    }

    #[test]
    fn form_clicks_use_scrolled_active_and_unscrolled_inactive_text_and_consume_collapsed_fields() {
        let mut panel = CommandsPanel::new();
        panel.toggle();
        panel.start_add();
        let text = "a界e\u{301}👩\u{200d}💻".repeat(8);
        panel.form_name.replace(&text);
        panel.form_command.replace(&text);
        let layout = CommandsPanelLayout::calculate(800.0, 800.0, 0.0, 8.0, 16.0, 1.0, true, 0);
        for field in [EditMode::EditingName, EditMode::EditingCommand] {
            assert!(panel.place_form_caret(field.clone(), text.len()));
            let info = panel.build_info(&[]);
            let view = layout.input_viewport(&info, field.clone()).unwrap();
            assert!(view.text_range.start > 0);
            let rect = if field == EditMode::EditingName {
                layout.form.unwrap().name_input
            } else {
                layout.form.unwrap().command_input
            };
            assert!(matches!(layout.input_click(&info, rect.x, rect.y),
                Some(CommandsPanelAction::PlaceCaret { cursor, .. }) if cursor == view.text_range.start));
            let other = if field == EditMode::EditingName {
                EditMode::EditingCommand
            } else {
                EditMode::EditingName
            };
            let other_view = layout.input_viewport(&info, other.clone()).unwrap();
            assert_eq!(other_view.text_range.start, 0);
            let CommandsPanelAction::PlaceCaret {
                field: target,
                cursor,
            } = layout
                .input_click(&info, other_view.text_x + 0.75 * 8.0, other_view.text_y)
                .unwrap()
            else {
                panic!("not a caret action");
            };
            assert_eq!(target, other);
            assert_eq!(cursor, 1);
            assert!(panel.place_form_caret(target, cursor));
            assert_eq!(panel.active_input().unwrap().text(), text);
        }
        let layout = CommandsPanelLayout::calculate(260.0, 800.0, 0.0, 8.0, 16.0, 1.0, true, 0);
        let rect = layout.form.unwrap().name_input;
        assert!(rect.width > 0.0);
        assert!(matches!(
            layout.input_click(&panel.build_info(&[]), rect.x, rect.y),
            Some(CommandsPanelAction::None)
        ));
    }

    #[test]
    fn form_caret_rejects_closed_browse_and_non_grapheme_positions_without_changing_focus() {
        let mut panel = CommandsPanel::new();
        panel.start_add();
        assert!(!panel.place_form_caret(EditMode::EditingCommand, 0));
        panel.toggle();
        assert!(!panel.place_form_caret(EditMode::EditingCommand, 0));
        panel.start_add();
        panel.form_command.replace("界e\u{301}👩\u{200d}💻");
        let end = panel.form_command.cursor();
        for cursor in [1, 4, end + 1, usize::MAX] {
            assert!(!panel.place_form_caret(EditMode::EditingCommand, cursor));
            assert_eq!(panel.edit_mode, EditMode::EditingName);
            assert_eq!(panel.form_command.cursor(), end);
        }
        assert!(!panel.place_form_caret(EditMode::Browse, 0));
        assert!(panel.place_form_caret(EditMode::EditingCommand, 3));
        assert_eq!(panel.form_command.text(), "界e\u{301}👩\u{200d}💻");
    }

    #[test]
    fn form_paste_is_atomic_at_the_caret_and_obeys_each_field_byte_limit() {
        for name in [true, false] {
            let mut panel = CommandsPanel::new();
            panel.start_add();
            assert!(!panel.insert_form_text("hidden"));
            panel.toggle();
            assert!(!panel.insert_form_text("browse"));
            panel.start_add();
            if !name {
                panel.focus_command();
            }
            let limit = if name {
                CommandsPanel::MAX_FORM_NAME_BYTES
            } else {
                CommandsPanel::MAX_FORM_COMMAND_BYTES
            };
            assert!(panel.insert_form_text("az"));
            panel.form_edit(TextInputEdit::Left);
            assert!(panel.insert_form_text("界e\u{301}👩\u{200d}💻"));
            let value = "a界e\u{301}👩\u{200d}💻z";
            assert_eq!(panel.active_input().unwrap().text(), value);
            let cursor = value.len() - 1;
            for rejected in [
                "",
                "two\nlines",
                "two\rlines",
                "\t",
                "\0",
                "\u{1b}[200~",
                "\u{7f}",
                "\u{85}",
                "\u{2028}",
                "\u{2029}",
                &"x".repeat(limit + 1),
            ] {
                assert!(!panel.insert_form_text(rejected));
                assert_eq!(panel.active_input().unwrap().text(), value);
                assert_eq!(panel.active_input().unwrap().cursor(), cursor);
            }
            assert!(panel.insert_form_text(&"x".repeat(limit - value.len() - 3)));
            assert!(!panel.insert_form_text("🫚")); // four bytes cannot fit in three
            assert!(panel.insert_form_text("界"));
            assert_eq!(panel.active_input().unwrap().text().len(), limit);
            assert_eq!(panel.active_input().unwrap().cursor(), limit - 1);
            assert!(!panel.insert_form_text("x"));
            assert!(panel.active_input().unwrap().text().ends_with('z'));
            let other = if name {
                &panel.form_command
            } else {
                &panel.form_name
            };
            assert!(other.text().is_empty());
            panel.cancel_edit();
            assert!(!panel.insert_form_text("browse"));
        }
    }

    #[test]
    fn form_edits_keep_clusters_whole_and_change_only_the_active_field() {
        use TextInputEdit::*;
        for name in [true, false] {
            let mut panel = CommandsPanel::new();
            panel.toggle();
            panel.start_add();
            panel.form_name.replace("untouched");
            panel.form_command.replace("untouched");
            if !name {
                panel.focus_command();
            }
            let input = "a界e\u{301}👩\u{200d}💻🇺🇸z";
            panel.active_input().unwrap().replace(input);
            for edit in [Home, Right, Delete, Right] {
                panel.form_edit(edit);
            }
            assert!(panel.insert_form_text("X"));
            assert_eq!(
                panel.active_input().unwrap().text(),
                "ae\u{301}X👩\u{200d}💻🇺🇸z"
            );
            for edit in [Delete, Delete, End, Backspace] {
                panel.form_edit(edit);
            }
            assert_eq!(panel.active_input().unwrap().text(), "ae\u{301}X");
            panel
                .active_input()
                .unwrap()
                .replace("one e\u{301} 👩\u{200d}💻 tail");
            for edit in [WordLeft, BackspaceWord, DeleteWord] {
                panel.form_edit(edit);
            }
            assert_eq!(panel.active_input().unwrap().text(), "one e\u{301} ");
            for edit in [Home, DeleteWord, WordRight, BackspaceWord] {
                panel.form_edit(edit);
            }
            assert_eq!(panel.active_input().unwrap().text(), "  ");
            let other = if name {
                &panel.form_command
            } else {
                &panel.form_name
            };
            assert_eq!(other.text(), "untouched");
        }
    }

    #[test]
    fn form_viewports_follow_whole_graphemes_at_scaled_animated_geometry() {
        use unicode_segmentation::UnicodeSegmentation;
        let text = "a界e\u{301}👩\u{200d}💻🇺🇸".repeat(30);
        let mut panel = CommandsPanel::new();
        panel.start_add();
        panel.form_name.replace(&text);
        panel.form_command.replace(&text);
        for (width, cell) in [(800.0, 8.0), (1600.0, 16.0), (320.0, 8.0)] {
            for animation in [0.25, 1.0] {
                let layout = CommandsPanelLayout::calculate(
                    width,
                    800.0,
                    0.0,
                    cell,
                    cell * 2.0,
                    animation,
                    true,
                    0,
                );
                for field in [EditMode::EditingName, EditMode::EditingCommand] {
                    panel.edit_mode = field.clone();
                    for cursor in text
                        .grapheme_indices(true)
                        .map(|(i, _)| i)
                        .chain([text.len()])
                    {
                        panel.active_input().unwrap().set_cursor_byte(cursor);
                        let info = panel.build_info(&[]);
                        let Some(view) = layout.input_viewport(&info, field.clone()) else {
                            assert_eq!(width, 320.0); // collapsed input: no out-of-box caret
                            continue;
                        };
                        assert!(view.text_range.start <= cursor && cursor <= view.text_range.end);
                        for edge in [view.text_range.start, view.text_range.end] {
                            assert!(
                                edge == text.len()
                                    || text.grapheme_indices(true).any(|(i, _)| i == edge)
                            );
                        }
                        assert!(
                            view.cursor_x >= view.text_x
                                && view.cursor_x + cell <= view.right + 0.001
                        );
                        if cursor == text.len() {
                            assert!(view.text_range.start > 0);
                        }
                    }
                    let other = if field == EditMode::EditingName {
                        EditMode::EditingCommand
                    } else {
                        EditMode::EditingName
                    };
                    if let Some(view) = layout.input_viewport(&panel.build_info(&[]), other) {
                        assert_eq!(view.text_range.start, 0);
                    }
                }
            }
        }
        panel.cancel_edit();
        panel.form_edit(TextInputEdit::Delete);
        assert!(!panel.is_editing());
    }

    #[test]
    fn form_values_are_utf8_aware_single_line_and_bounded() {
        let mut panel = CommandsPanel::new();
        panel.toggle();
        panel.start_add();
        assert!(panel.accessibility_set_name("Deploy 🫚"));
        assert_eq!(panel.form_name.text(), "Deploy 🫚");
        assert!(!panel.accessibility_set_name("bad\nname"));
        assert_eq!(panel.form_name.text(), "Deploy 🫚");

        panel.focus_command();
        assert!(panel.insert_form_text(&"x".repeat(CommandsPanel::MAX_FORM_COMMAND_BYTES)));
        assert!(!panel.insert_form_text("x"));
        assert_eq!(
            panel.form_command.text().len(),
            CommandsPanel::MAX_FORM_COMMAND_BYTES
        );
        assert!(
            !panel
                .accessibility_set_command(&"x".repeat(CommandsPanel::MAX_FORM_COMMAND_BYTES + 1))
        );
    }

    #[test]
    fn explicit_cancel_preserves_the_panel_and_clears_the_form() {
        let mut panel = CommandsPanel::new();
        panel.toggle();
        panel.start_add();
        assert!(panel.insert_form_text("x"));
        panel.cancel_edit();
        assert!(panel.is_open());
        assert!(!panel.is_editing());
        assert!(panel.form_name.text().is_empty());
        assert!(panel.form_command.text().is_empty());
    }
}
