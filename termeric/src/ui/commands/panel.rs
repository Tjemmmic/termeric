//! Custom commands panel — slide-in panel for managing user-defined shortcuts
//!
//! Provides a right-side slide-in panel where users can:
//! - View all custom commands with name + command preview
//! - Click a command to run it (opens in a new tab)
//! - Add new commands via an inline form
//! - Delete commands with a small (×) button
//! - Close via scrim click, × button, or Escape

use crate::config::CustomCommand;
use crate::ui::animation::PanelAnimation;
use crate::ui::text_input::TextInput;

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
    /// User clicked the Name field
    ClickName,
    /// User clicked the Command field
    ClickCommand,
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
    /// Mouse-hovered command index (None if not hovering a row)
    pub hovered_index: Option<usize>,
    /// Whether hovering the "Add Command" button
    pub hovered_add: bool,
    /// Whether hovering the delete (×) on a specific row
    pub hovered_delete: Option<usize>,
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
    /// List of commands to display
    pub commands: Vec<CustomCommand>,
    /// Mouse-hovered command row index
    pub hovered_index: Option<usize>,
    /// Whether the "Add Command" button is hovered
    pub hovered_add: bool,
    /// Which row's delete (×) is hovered
    pub hovered_delete: Option<usize>,
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
    pub hint_y: f32,
}

/// Single source of truth for custom-command panel geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandsPanelLayout {
    pub panel: CommandsPanelRect,
    pub padding: f32,
    pub small_padding: f32,
    pub header_height: f32,
    pub close_button: CommandsPanelRect,
    pub new_button: CommandsPanelRect,
    pub form: Option<CommandsFormLayout>,
    pub list_start_y: f32,
    pub row_height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl CommandsPanelLayout {
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        editing: bool,
        command_count: usize,
    ) -> Self {
        let panel_width = (cell_width * 36.0).max(340.0).min(window_width * 0.45);
        let panel_x = window_width - panel_width * animation;
        let padding = cell_width * 1.8;
        let small_padding = cell_width * 0.8;
        let header_height = cell_height * 2.8;
        let header_text_y = (header_height - cell_height) / 2.0;
        let close_x = panel_x + panel_width - padding - cell_width;
        let close_button = CommandsPanelRect {
            x: close_x,
            y: header_text_y,
            width: cell_width,
            height: cell_height * 1.5,
        };
        let new_button = CommandsPanelRect {
            x: panel_x + padding,
            y: header_height + cell_height * 0.8,
            width: panel_width - padding * 2.0,
            height: cell_height * 2.0,
        };
        let mut list_start_y = new_button.y + new_button.height + cell_height * 0.8;
        let form = editing.then(|| {
            let form_x = panel_x + padding;
            let form_width = panel_width - padding * 2.0;
            let field_height = cell_height * 1.8;
            let label_width = cell_width * 10.0;
            let form_height = field_height * 2.0 + cell_height * 3.2;
            let field_y = list_start_y + small_padding;
            let input_x = form_x + label_width;
            let input_width = form_width - label_width - small_padding;
            let command_y = field_y + field_height + small_padding;
            let buttons_y = command_y + field_height + small_padding * 1.5;
            let save_width = cell_width * 7.0;
            let save_x = form_x + form_width - save_width;
            let save_height = cell_height * 1.6;
            let cancel_width = cell_width * 8.0;
            let cancel_x = save_x - cancel_width - small_padding;
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
                hint_y: buttons_y + save_height + small_padding,
            };
            list_start_y += form_height + cell_height * 0.5;
            layout
        });
        if command_count > 0 {
            list_start_y += cell_height * 1.5;
        }
        Self {
            panel: CommandsPanelRect {
                x: panel_x,
                y: 0.0,
                width: panel_width,
                height: window_height,
            },
            padding,
            small_padding,
            header_height,
            close_button,
            new_button,
            form,
            list_start_y,
            row_height: cell_height * 3.0,
            cell_width,
            cell_height,
        }
    }

    pub fn command_row(self, index: usize) -> CommandsPanelRect {
        CommandsPanelRect {
            x: self.panel.x,
            y: self.list_start_y + index as f32 * self.row_height,
            width: self.panel.width,
            height: self.row_height,
        }
    }

    pub fn command_run(self, index: usize) -> CommandsPanelRect {
        let row = self.command_row(index);
        let delete = self.command_delete(index);
        CommandsPanelRect {
            width: (delete.x - row.x).max(0.0),
            ..row
        }
    }

    pub fn command_delete(self, index: usize) -> CommandsPanelRect {
        let row = self.command_row(index);
        let glyph_x = self.panel.x + self.panel.width - self.padding - self.cell_width * 1.5;
        CommandsPanelRect {
            x: glyph_x - self.cell_width * 0.5,
            y: row.y,
            width: self.cell_width * 2.0,
            height: row.height,
        }
    }
}

impl CommandsPanel {
    const MAX_FORM_NAME_BYTES: usize = 256;
    const MAX_FORM_COMMAND_BYTES: usize = 16 * 1024;

    pub fn new() -> Self {
        Self {
            animation: PanelAnimation::with_duration(0.25),
            hovered_index: None,
            hovered_add: false,
            hovered_delete: None,
            edit_mode: EditMode::Browse,
            form_name: TextInput::new(),
            form_command: TextInput::new(),
        }
    }

    /// Toggle the panel open/closed
    pub fn toggle(&mut self) {
        self.cancel_edit();
        if !self.animation.is_open() {
            // Reset hover state when opening
            self.hovered_index = None;
            self.hovered_add = false;
            self.hovered_delete = None;
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
    pub fn set_hover(&mut self, index: Option<usize>, add: bool, delete: Option<usize>) {
        self.hovered_index = index;
        self.hovered_add = add;
        self.hovered_delete = delete;
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

    /// Handle a character input in the form
    pub fn form_char(&mut self, c: char) {
        let limit = match self.edit_mode {
            EditMode::EditingName => Self::MAX_FORM_NAME_BYTES,
            EditMode::EditingCommand => Self::MAX_FORM_COMMAND_BYTES,
            EditMode::Browse => return,
        };
        if c == '\n' || c == '\r' || c == '\0' {
            return;
        }
        if let Some(input) = self.active_input()
            && input.text().len() + c.len_utf8() <= limit
        {
            input.insert_char(c);
        }
    }

    /// Handle backspace in the form
    pub fn form_backspace(&mut self) {
        if let Some(input) = self.active_input() {
            input.backspace();
        }
    }

    /// Replace a form field from an assistive-technology value action. The
    /// same single-line and size limits apply as to typed input.
    pub fn accessibility_set_name(&mut self, value: &str) -> bool {
        if value.len() > Self::MAX_FORM_NAME_BYTES || value.contains(['\0', '\n', '\r']) {
            return false;
        }
        self.focus_name();
        self.form_name.replace(value);
        true
    }

    pub fn accessibility_set_command(&mut self, value: &str) -> bool {
        if value.len() > Self::MAX_FORM_COMMAND_BYTES || value.contains(['\0', '\n', '\r']) {
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
            commands: commands.to_vec(),
            hovered_index: self.hovered_index,
            hovered_add: self.hovered_add,
            hovered_delete: self.hovered_delete,
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
    fn shared_layout_keeps_form_and_row_actions_disjoint() {
        let layout = CommandsPanelLayout::calculate(1000.0, 800.0, 10.0, 20.0, 1.0, true, 2);
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
    fn form_values_are_utf8_aware_single_line_and_bounded() {
        let mut panel = CommandsPanel::new();
        panel.start_add();
        assert!(panel.accessibility_set_name("Deploy 🫚"));
        assert_eq!(panel.form_name.text(), "Deploy 🫚");
        assert!(!panel.accessibility_set_name("bad\nname"));
        assert_eq!(panel.form_name.text(), "Deploy 🫚");

        panel.focus_command();
        for _ in 0..CommandsPanel::MAX_FORM_COMMAND_BYTES + 10 {
            panel.form_char('x');
        }
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
        panel.form_char('x');
        panel.cancel_edit();
        assert!(panel.is_open());
        assert!(!panel.is_editing());
        assert!(panel.form_name.text().is_empty());
        assert!(panel.form_command.text().is_empty());
    }
}
