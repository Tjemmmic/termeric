//! Input handling - keyboard and mouse

use winit::event::{ElementState, KeyEvent, MouseButton};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use termeric_core::{Selection, SelectionType, Point};

/// Convert a key event to bytes to send to the PTY
pub fn key_to_bytes(event: &KeyEvent) -> Option<Vec<u8>> {
    // Only handle key presses
    if event.state != ElementState::Pressed {
        return None;
    }

    match &event.logical_key {
        Key::Character(c) => {
            // Regular character input
            Some(c.as_bytes().to_vec())
        }

        Key::Named(named) => {
            let bytes = match named {
                // Basic control keys
                NamedKey::Enter => b"\r".to_vec(),
                NamedKey::Tab => b"\t".to_vec(),
                NamedKey::Backspace => b"\x7f".to_vec(),
                NamedKey::Escape => b"\x1b".to_vec(),
                NamedKey::Space => b" ".to_vec(),

                // Arrow keys
                NamedKey::ArrowUp => b"\x1b[A".to_vec(),
                NamedKey::ArrowDown => b"\x1b[B".to_vec(),
                NamedKey::ArrowRight => b"\x1b[C".to_vec(),
                NamedKey::ArrowLeft => b"\x1b[D".to_vec(),

                // Navigation keys
                NamedKey::Home => b"\x1b[H".to_vec(),
                NamedKey::End => b"\x1b[F".to_vec(),
                NamedKey::PageUp => b"\x1b[5~".to_vec(),
                NamedKey::PageDown => b"\x1b[6~".to_vec(),
                NamedKey::Insert => b"\x1b[2~".to_vec(),
                NamedKey::Delete => b"\x1b[3~".to_vec(),

                // Function keys
                NamedKey::F1 => b"\x1bOP".to_vec(),
                NamedKey::F2 => b"\x1bOQ".to_vec(),
                NamedKey::F3 => b"\x1bOR".to_vec(),
                NamedKey::F4 => b"\x1bOS".to_vec(),
                NamedKey::F5 => b"\x1b[15~".to_vec(),
                NamedKey::F6 => b"\x1b[17~".to_vec(),
                NamedKey::F7 => b"\x1b[18~".to_vec(),
                NamedKey::F8 => b"\x1b[19~".to_vec(),
                NamedKey::F9 => b"\x1b[20~".to_vec(),
                NamedKey::F10 => b"\x1b[21~".to_vec(),
                NamedKey::F11 => b"\x1b[23~".to_vec(),
                NamedKey::F12 => b"\x1b[24~".to_vec(),

                _ => return None,
            };
            Some(bytes)
        }

        _ => None,
    }
}

/// Action that can be triggered by a key binding
#[derive(Clone, Debug)]
pub enum Action {
    /// Copy selected text to clipboard
    Copy,
    /// Paste from clipboard
    Paste,
    /// Scroll up by N lines
    ScrollUp(usize),
    /// Scroll down by N lines
    ScrollDown(usize),
    /// Scroll to top of scrollback
    ScrollToTop,
    /// Scroll to bottom (current output)
    ScrollToBottom,
    /// Increase font size
    IncreaseFontSize,
    /// Decrease font size
    DecreaseFontSize,
    /// Reset font size to default
    ResetFontSize,
    /// Clear selection
    ClearSelection,
    /// No action (pass through to PTY)
    None,
}

/// Check if a key event triggers a bound action
pub fn check_keybinding(event: &KeyEvent, modifiers: ModifiersState) -> Option<Action> {
    if event.state != ElementState::Pressed {
        return None;
    }

    let ctrl = modifiers.control_key();
    let shift = modifiers.shift_key();
    let alt = modifiers.alt_key();

    match &event.logical_key {
        Key::Character(c) => {
            let c = c.to_lowercase();
            match c.as_str() {
                "c" if ctrl && shift => Some(Action::Copy),
                "v" if ctrl && shift => Some(Action::Paste),
                "+" | "=" if ctrl => Some(Action::IncreaseFontSize),
                "-" if ctrl => Some(Action::DecreaseFontSize),
                "0" if ctrl => Some(Action::ResetFontSize),
                _ => None,
            }
        }
        Key::Named(named) => {
            match named {
                NamedKey::PageUp if shift => Some(Action::ScrollUp(10)),
                NamedKey::PageDown if shift => Some(Action::ScrollDown(10)),
                NamedKey::Home if ctrl && shift => Some(Action::ScrollToTop),
                NamedKey::End if ctrl && shift => Some(Action::ScrollToBottom),
                NamedKey::Escape => Some(Action::ClearSelection),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Mouse input state tracker
pub struct MouseState {
    /// Current selection being made
    pub selection: Option<Selection>,
    /// Last mouse position in grid coordinates
    pub last_position: Option<Point>,
    /// Number of rapid clicks (for double/triple click detection)
    pub click_count: u32,
    /// Time of last click (for double/triple click detection)
    pub last_click_time: std::time::Instant,
    /// Position of last click
    pub last_click_pos: Option<Point>,
}

impl Default for MouseState {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseState {
    pub fn new() -> Self {
        Self {
            selection: None,
            last_position: None,
            click_count: 0,
            last_click_time: std::time::Instant::now(),
            last_click_pos: None,
        }
    }

    /// Handle mouse button press
    pub fn handle_press(
        &mut self,
        button: MouseButton,
        position: Point,
        modifiers: ModifiersState,
    ) -> Option<Selection> {
        if button != MouseButton::Left {
            return None;
        }

        // Check for multi-click (double/triple)
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_click_time);
        let same_position = self.last_click_pos.map(|p| {
            (p.row - position.row).abs() <= 1 &&
            (p.col as i32 - position.col as i32).abs() <= 2
        }).unwrap_or(false);

        if elapsed.as_millis() < 400 && same_position {
            self.click_count = (self.click_count % 3) + 1;
        } else {
            self.click_count = 1;
        }

        self.last_click_time = now;
        self.last_click_pos = Some(position);

        // Determine selection type based on click count and modifiers
        let selection_type = if modifiers.alt_key() {
            SelectionType::Block
        } else {
            match self.click_count {
                2 => SelectionType::Semantic,
                3 => SelectionType::Lines,
                _ => SelectionType::Simple,
            }
        };

        let selection = Selection::new(position, selection_type);
        self.selection = Some(selection.clone());
        Some(selection)
    }

    /// Handle mouse movement (drag)
    pub fn handle_move(&mut self, position: Point) -> Option<Selection> {
        self.last_position = Some(position);

        if let Some(ref mut selection) = self.selection {
            selection.update(position);
            return Some(selection.clone());
        }

        None
    }

    /// Handle mouse button release
    pub fn handle_release(&mut self, button: MouseButton) -> Option<Selection> {
        if button != MouseButton::Left {
            return None;
        }

        if let Some(ref mut selection) = self.selection {
            selection.finish();
            return Some(selection.clone());
        }

        None
    }

    /// Clear current selection
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Get current selection
    pub fn current_selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }
}

/// Convert screen coordinates to grid cell position
pub fn screen_to_grid(
    x: f64,
    y: f64,
    cell_width: f32,
    cell_height: f32,
    scroll_offset: i32,
) -> Point {
    let col = (x as f32 / cell_width).floor() as usize;
    let row = (y as f32 / cell_height).floor() as i32 - scroll_offset;
    Point::new(row, col)
}
