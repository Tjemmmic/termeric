//! Custom commands panel — slide-in panel for managing user-defined shortcuts
//!
//! Provides a right-side slide-in panel where users can:
//! - View all custom commands with name + command preview
//! - Click a command to run it (opens in a new tab)
//! - Add new commands via an inline form
//! - Delete commands with a small (×) button
//! - Close via scrim click, × button, or Escape

use std::time::Instant;
use crate::config::CustomCommand;

/// Action returned by the commands panel after a click
#[derive(Debug, Clone)]
pub enum CommandsPanelAction {
    /// Run the command at the given index
    Run(usize),
    /// Delete the command at the given index
    Delete(usize),
    /// User submitted the inline "add command" form
    AddSubmit { name: String, command: String },
    /// User clicked the Name field
    ClickName,
    /// User clicked the Command field
    ClickCommand,
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
    /// Whether the panel is open
    open: bool,
    /// Animation start time
    animation_start: Instant,
    /// Whether we're animating open (vs closed)
    animating_open: bool,
    /// Base animation value at start of animation
    animation_base: f32,
    /// Mouse-hovered command index (None if not hovering a row)
    pub hovered_index: Option<usize>,
    /// Whether hovering the "Add Command" button
    pub hovered_add: bool,
    /// Whether hovering the delete (×) on a specific row
    pub hovered_delete: Option<usize>,
    /// Current edit mode
    pub edit_mode: EditMode,
    /// Name buffer for inline form
    pub form_name: String,
    /// Command buffer for inline form
    pub form_command: String,
    /// Cursor position in active text field
    pub form_cursor: usize,
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
    /// Cursor position in active field
    pub form_cursor: usize,
}

const ANIMATION_DURATION: f32 = 0.25;

impl CommandsPanel {
    pub fn new() -> Self {
        Self {
            open: false,
            animation_start: Instant::now(),
            animating_open: false,
            animation_base: 0.0,
            hovered_index: None,
            hovered_add: false,
            hovered_delete: None,
            edit_mode: EditMode::Browse,
            form_name: String::new(),
            form_command: String::new(),
            form_cursor: 0,
        }
    }

    /// Toggle the panel open/closed
    pub fn toggle(&mut self) {
        self.animation_base = self.animation_progress();
        self.animation_start = Instant::now();
        self.open = !self.open;
        self.animating_open = self.open;
        if self.open {
            self.hovered_index = None;
            self.hovered_add = false;
            self.hovered_delete = None;
            self.cancel_edit();
        }
    }

    /// Open the panel
    pub fn open(&mut self) {
        if !self.open {
            self.toggle();
        }
    }

    /// Close the panel
    pub fn close(&mut self) {
        if self.open {
            self.cancel_edit();
            self.toggle();
        }
    }

    /// Whether the panel is currently open
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the panel is fully closed (animation complete)
    pub fn is_closed(&self) -> bool {
        !self.open && self.animation_progress() <= 0.0
    }

    /// Whether the inline form is active
    pub fn is_editing(&self) -> bool {
        self.edit_mode != EditMode::Browse
    }

    /// Current animation progress (0.0 to 1.0)
    pub fn animation_progress(&self) -> f32 {
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        let t = (elapsed / ANIMATION_DURATION).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);

        if self.animating_open {
            self.animation_base + (1.0 - self.animation_base) * eased
        } else {
            self.animation_base * (1.0 - eased)
        }
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
        self.form_cursor = 0;
    }

    /// Cancel the inline form
    pub fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::Browse;
        self.form_name.clear();
        self.form_command.clear();
        self.form_cursor = 0;
    }

    /// Switch focus to the next field (Tab key)
    pub fn next_field(&mut self) {
        match self.edit_mode {
            EditMode::EditingName => {
                self.edit_mode = EditMode::EditingCommand;
                self.form_cursor = self.form_command.len();
            }
            EditMode::EditingCommand => {
                self.edit_mode = EditMode::EditingName;
                self.form_cursor = self.form_name.len();
            }
            EditMode::Browse => {}
        }
    }

    /// Focus the Name field (e.g. from a click)
    pub fn focus_name(&mut self) {
        self.edit_mode = EditMode::EditingName;
        self.form_cursor = self.form_name.len();
    }

    /// Focus the Command field (e.g. from a click)
    pub fn focus_command(&mut self) {
        self.edit_mode = EditMode::EditingCommand;
        self.form_cursor = self.form_command.len();
    }

    /// Handle a character input in the form
    pub fn form_char(&mut self, c: char) {
        let buf = match self.edit_mode {
            EditMode::EditingName => &mut self.form_name,
            EditMode::EditingCommand => &mut self.form_command,
            EditMode::Browse => return,
        };
        buf.insert(self.form_cursor, c);
        self.form_cursor += c.len_utf8();
    }

    /// Handle backspace in the form
    pub fn form_backspace(&mut self) {
        let buf = match self.edit_mode {
            EditMode::EditingName => &mut self.form_name,
            EditMode::EditingCommand => &mut self.form_command,
            EditMode::Browse => return,
        };
        if self.form_cursor > 0 {
            let prev = buf[..self.form_cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            buf.remove(prev);
            self.form_cursor = prev;
        }
    }

    /// Try to submit the form — returns the new command if valid
    pub fn try_submit(&mut self) -> Option<CustomCommand> {
        let name = self.form_name.trim().to_string();
        let command = self.form_command.trim().to_string();
        if name.is_empty() || command.is_empty() {
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
            form_name: self.form_name.clone(),
            form_command: self.form_command.clone(),
            form_cursor: self.form_cursor,
        }
    }
}
