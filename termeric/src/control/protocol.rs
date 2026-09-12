//! Bounded wire types for inspecting and explicitly controlling GUI processes.

use anyhow::Context;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashSet;
use std::io::{Read, Write};

pub const PROTOCOL_VERSION: u32 = 14;
pub const JSON_SCHEMA_VERSION: u32 = 3;
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_WINDOWS: usize = 32;
pub const MAX_TABS_PER_WINDOW: usize = 256;
pub const MAX_PANES_PER_TAB: usize = 512;
pub const MAX_DIVIDERS_PER_TAB: usize = MAX_PANES_PER_TAB - 1;
pub const MAX_TOTAL_TABS: usize = 256;
pub const MAX_TOTAL_PANES: usize = 1_024;
pub const MAX_TOTAL_DIVIDERS: usize = MAX_TOTAL_PANES - 1;
pub const MAX_METADATA_BYTES: usize = 32 * 1024;
pub const MIN_WINDOW_WIDTH: u32 = 160;
pub const MIN_WINDOW_HEIGHT: u32 = 120;
pub const MAX_WINDOW_DIMENSION: u32 = 16_384;
pub const MAX_TEXT_HISTORY_LINES: u16 = 256;
pub const MAX_TERMINAL_TEXT_BYTES: u32 = 64 * 1024;
pub const MAX_CONTROL_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_CONTROL_LAUNCH_ARGS: usize = 128;
pub const MAX_CONTROL_LAUNCH_ARGV_BYTES: usize = 16 * 1024;
pub const MAX_CONTROL_LAUNCH_CWD_BYTES: usize = 4 * 1024;
pub const MAX_CONTROL_LAUNCH_TITLE_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientFrame {
    pub version: u32,
    pub token: String,
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum Request {
    Inventory,
    ReadText {
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        history_lines: u16,
        max_bytes: u32,
    },
    SendText {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        text: String,
        paste: bool,
        enter: bool,
    },
    CreateWindow {
        request_id: String,
        source_window: u64,
    },
    LaunchWindow {
        request_id: String,
        source_window: u64,
        program: String,
        args: Vec<String>,
        working_directory: Option<String>,
        title: Option<String>,
    },
    CreateTab {
        request_id: String,
        window_id: u64,
    },
    LaunchTab {
        request_id: String,
        window_id: u64,
        program: String,
        args: Vec<String>,
        working_directory: Option<String>,
        title: Option<String>,
    },
    SplitPane {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        direction: SplitDirection,
    },
    FocusWindow {
        request_id: String,
        window_id: u64,
    },
    SetQuickAccessVisibility {
        request_id: String,
        window_id: u64,
        visible: bool,
    },
    SelectTab {
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    FocusPane {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    },
    CloseWindow {
        request_id: String,
        window_id: u64,
    },
    CloseTab {
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    ClosePane {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    },
    MoveTab {
        request_id: String,
        source_window_id: u64,
        tab_id: u64,
        destination_window_id: u64,
        destination_index: u16,
    },
    MovePane {
        request_id: String,
        source_window_id: u64,
        source_tab_id: u64,
        pane_id: u32,
        destination_window_id: u64,
        destination_tab_id: u64,
        target_pane_id: u32,
        direction: SplitDirection,
    },
    ResizeWindow {
        request_id: String,
        window_id: u64,
        width: u32,
        height: u32,
    },
    ResizeDivider {
        request_id: String,
        window_id: u64,
        tab_id: u64,
        divider_id: u64,
        ratio_millis: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextScreen {
    Primary,
    Alternate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputDisposition {
    Sent,
    PendingConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Inventory(ProcessInventory),
    Text {
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        screen: TextScreen,
        history_lines: u16,
        text: String,
        truncated: bool,
    },
    InputAccepted {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        input_bytes: u32,
        wire_bytes: u32,
        paste: bool,
        enter: bool,
        disposition: InputDisposition,
        confirmation_id: Option<u64>,
    },
    WindowCreated {
        process_id: u32,
        request_id: String,
        window_id: u64,
    },
    WindowLaunched {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    TabCreated {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    PaneCreated {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    },
    WindowFocusRequested {
        process_id: u32,
        request_id: String,
        window_id: u64,
        already_focused: bool,
    },
    QuickAccessChanged {
        process_id: u32,
        request_id: String,
        window_id: u64,
        visible: bool,
    },
    TabSelected {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        changed: bool,
    },
    PaneFocused {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        tab_changed: bool,
        pane_changed: bool,
    },
    WindowCloseRequested {
        process_id: u32,
        request_id: String,
        window_id: u64,
    },
    TabCloseRequested {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    PaneClosed {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        remaining_panes: u16,
        focused_pane_id: u32,
    },
    TabMoved {
        process_id: u32,
        request_id: String,
        source_window_id: u64,
        source_tab_id: u64,
        destination_window_id: u64,
        destination_tab_id: u64,
        destination_index: u16,
        source_window_closing: bool,
        changed: bool,
    },
    PaneMoved {
        process_id: u32,
        request_id: String,
        source_window_id: u64,
        source_tab_id: u64,
        source_pane_id: u32,
        destination_window_id: u64,
        destination_tab_id: u64,
        destination_pane_id: u32,
        target_pane_id: u32,
    },
    WindowResizeRequested {
        process_id: u32,
        request_id: String,
        window_id: u64,
        requested_width: u32,
        requested_height: u32,
        immediate_width: Option<u32>,
        immediate_height: Option<u32>,
    },
    DividerResized {
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        divider_id: u64,
        previous_ratio_millis: u16,
        ratio_millis: u16,
        changed: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum ServerFrame {
    Inventory {
        version: u32,
        inventory: ProcessInventory,
    },
    Text {
        version: u32,
        process_id: u32,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        screen: TextScreen,
        history_lines: u16,
        text: String,
        truncated: bool,
    },
    InputAccepted {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        input_bytes: u32,
        wire_bytes: u32,
        paste: bool,
        enter: bool,
        disposition: InputDisposition,
        confirmation_id: Option<u64>,
    },
    WindowCreated {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
    },
    WindowLaunched {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    TabCreated {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    PaneCreated {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
    },
    WindowFocusRequested {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        already_focused: bool,
    },
    QuickAccessChanged {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        visible: bool,
    },
    TabSelected {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        changed: bool,
    },
    PaneFocused {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        tab_changed: bool,
        pane_changed: bool,
    },
    WindowCloseRequested {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
    },
    TabCloseRequested {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
    },
    PaneClosed {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        pane_id: u32,
        remaining_panes: u16,
        focused_pane_id: u32,
    },
    TabMoved {
        version: u32,
        process_id: u32,
        request_id: String,
        source_window_id: u64,
        source_tab_id: u64,
        destination_window_id: u64,
        destination_tab_id: u64,
        destination_index: u16,
        source_window_closing: bool,
        changed: bool,
    },
    PaneMoved {
        version: u32,
        process_id: u32,
        request_id: String,
        source_window_id: u64,
        source_tab_id: u64,
        source_pane_id: u32,
        destination_window_id: u64,
        destination_tab_id: u64,
        destination_pane_id: u32,
        target_pane_id: u32,
    },
    WindowResizeRequested {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        requested_width: u32,
        requested_height: u32,
        immediate_width: Option<u32>,
        immediate_height: Option<u32>,
    },
    DividerResized {
        version: u32,
        process_id: u32,
        request_id: String,
        window_id: u64,
        tab_id: u64,
        divider_id: u64,
        previous_ratio_millis: u16,
        ratio_millis: u16,
        changed: bool,
    },
    Error {
        version: u32,
        message: String,
    },
}

impl Request {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Inventory => Ok(()),
            Self::ReadText {
                window_id,
                history_lines,
                max_bytes,
                ..
            } => {
                if *window_id == 0 {
                    return Err("GUI control target window ID must be nonzero".into());
                }
                if *history_lines > MAX_TEXT_HISTORY_LINES {
                    return Err(format!(
                        "GUI terminal-text history is limited to {MAX_TEXT_HISTORY_LINES} lines"
                    ));
                }
                if !(1..=MAX_TERMINAL_TEXT_BYTES).contains(max_bytes) {
                    return Err(format!(
                        "GUI terminal-text output must be between 1 and {MAX_TERMINAL_TEXT_BYTES} bytes"
                    ));
                }
                Ok(())
            }
            Self::SendText {
                request_id,
                window_id,
                text,
                enter,
                ..
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *window_id == 0 {
                    return Err("GUI control target window ID must be nonzero".into());
                }
                if text.len() > MAX_CONTROL_INPUT_BYTES {
                    return Err(format!(
                        "GUI control input is limited to {MAX_CONTROL_INPUT_BYTES} UTF-8 bytes"
                    ));
                }
                if text.is_empty() && !enter {
                    return Err("GUI control input must include text or --enter".into());
                }
                if !valid_control_input_text(text) {
                    return Err(
                        "GUI control input permits printable UTF-8, tab, and newline only".into(),
                    );
                }
                Ok(())
            }
            Self::CreateWindow {
                request_id,
                source_window,
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *source_window == 0 {
                    return Err("GUI control source window ID must be nonzero".into());
                }
                Ok(())
            }
            Self::CreateTab {
                request_id,
                window_id,
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *window_id == 0 {
                    return Err("GUI control target window ID must be nonzero".into());
                }
                Ok(())
            }
            Self::LaunchTab {
                request_id,
                window_id,
                program,
                args,
                working_directory,
                title,
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *window_id == 0 {
                    return Err("GUI control target window ID must be nonzero".into());
                }
                validate_control_launch(
                    program,
                    args,
                    working_directory.as_deref(),
                    title.as_deref(),
                )
            }
            Self::LaunchWindow {
                request_id,
                source_window,
                program,
                args,
                working_directory,
                title,
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *source_window == 0 {
                    return Err("GUI control source window ID must be nonzero".into());
                }
                validate_control_launch(
                    program,
                    args,
                    working_directory.as_deref(),
                    title.as_deref(),
                )
            }
            Self::SplitPane {
                request_id,
                window_id,
                ..
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *window_id == 0 {
                    return Err("GUI control target window ID must be nonzero".into());
                }
                Ok(())
            }
            Self::FocusWindow {
                request_id,
                window_id,
            }
            | Self::SetQuickAccessVisibility {
                request_id,
                window_id,
                ..
            }
            | Self::SelectTab {
                request_id,
                window_id,
                ..
            }
            | Self::FocusPane {
                request_id,
                window_id,
                ..
            }
            | Self::CloseWindow {
                request_id,
                window_id,
            }
            | Self::CloseTab {
                request_id,
                window_id,
                ..
            }
            | Self::ClosePane {
                request_id,
                window_id,
                ..
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *window_id == 0 {
                    return Err("GUI control target window ID must be nonzero".into());
                }
                Ok(())
            }
            Self::MoveTab {
                request_id,
                source_window_id,
                destination_window_id,
                destination_index,
                ..
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *source_window_id == 0 || *destination_window_id == 0 {
                    return Err(
                        "GUI control source and destination window IDs must be nonzero".into(),
                    );
                }
                if usize::from(*destination_index) > MAX_TABS_PER_WINDOW {
                    return Err("GUI control destination tab index exceeds the window limit".into());
                }
                Ok(())
            }
            Self::MovePane {
                request_id,
                source_window_id,
                destination_window_id,
                ..
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *source_window_id == 0 || *destination_window_id == 0 {
                    return Err(
                        "GUI control source and destination window IDs must be nonzero".into(),
                    );
                }
                Ok(())
            }
            Self::ResizeWindow {
                request_id,
                window_id,
                width,
                height,
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *window_id == 0 {
                    return Err("GUI control target window ID must be nonzero".into());
                }
                if !(MIN_WINDOW_WIDTH..=MAX_WINDOW_DIMENSION).contains(width)
                    || !(MIN_WINDOW_HEIGHT..=MAX_WINDOW_DIMENSION).contains(height)
                {
                    return Err(format!(
                        "GUI window dimensions must be between {MIN_WINDOW_WIDTH}x{MIN_WINDOW_HEIGHT} and {MAX_WINDOW_DIMENSION}x{MAX_WINDOW_DIMENSION} pixels"
                    ));
                }
                Ok(())
            }
            Self::ResizeDivider {
                request_id,
                window_id,
                ratio_millis,
                ..
            } => {
                if !valid_request_id(request_id) {
                    return Err(
                        "GUI control request ID must be exactly 32 lowercase hex digits".into(),
                    );
                }
                if *window_id == 0 {
                    return Err("GUI control target window ID must be nonzero".into());
                }
                if !(100..=900).contains(ratio_millis) {
                    return Err("GUI divider ratio must be between 100 and 900 thousandths".into());
                }
                Ok(())
            }
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Inventory | Self::ReadText { .. } => None,
            Self::CreateWindow { request_id, .. }
            | Self::LaunchWindow { request_id, .. }
            | Self::CreateTab { request_id, .. }
            | Self::LaunchTab { request_id, .. }
            | Self::SplitPane { request_id, .. }
            | Self::FocusWindow { request_id, .. }
            | Self::SetQuickAccessVisibility { request_id, .. }
            | Self::SelectTab { request_id, .. }
            | Self::FocusPane { request_id, .. }
            | Self::CloseWindow { request_id, .. }
            | Self::CloseTab { request_id, .. }
            | Self::ClosePane { request_id, .. }
            | Self::MoveTab { request_id, .. }
            | Self::MovePane { request_id, .. }
            | Self::ResizeWindow { request_id, .. }
            | Self::ResizeDivider { request_id, .. }
            | Self::SendText { request_id, .. } => Some(request_id),
        }
    }
}

fn validate_control_launch(
    program: &str,
    args: &[String],
    working_directory: Option<&str>,
    title: Option<&str>,
) -> Result<(), String> {
    if program.is_empty() || program.chars().any(unsafe_control_metadata_character) {
        return Err(
            "GUI control launch program must be nonempty UTF-8 without control or bidirectional formatting text"
                .into(),
        );
    }
    if args.len() > MAX_CONTROL_LAUNCH_ARGS {
        return Err(format!(
            "GUI control launch is limited to {MAX_CONTROL_LAUNCH_ARGS} arguments"
        ));
    }
    if args.iter().any(|argument| argument.contains('\0')) {
        return Err("GUI control launch arguments cannot contain NUL bytes".into());
    }
    let argv_bytes = args.iter().fold(program.len(), |total, argument| {
        total.saturating_add(argument.len())
    });
    if argv_bytes > MAX_CONTROL_LAUNCH_ARGV_BYTES {
        return Err(format!(
            "GUI control launch program and arguments are limited to {MAX_CONTROL_LAUNCH_ARGV_BYTES} UTF-8 bytes"
        ));
    }
    if let Some(directory) = working_directory
        && (directory.is_empty()
            || directory.len() > MAX_CONTROL_LAUNCH_CWD_BYTES
            || !std::path::Path::new(directory).is_absolute()
            || directory.chars().any(unsafe_control_metadata_character))
    {
        return Err(format!(
            "GUI control launch working directory must be an absolute UTF-8 path of at most {MAX_CONTROL_LAUNCH_CWD_BYTES} bytes without control or bidirectional formatting text"
        ));
    }
    if let Some(title) = title
        && (title.trim().is_empty()
            || title.len() > MAX_CONTROL_LAUNCH_TITLE_BYTES
            || title.chars().any(unsafe_control_metadata_character))
    {
        return Err(format!(
            "GUI control launch title must be nonempty UTF-8 of at most {MAX_CONTROL_LAUNCH_TITLE_BYTES} bytes without control or bidirectional formatting text"
        ));
    }
    Ok(())
}

pub fn valid_control_input_text(value: &str) -> bool {
    value
        .chars()
        .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
}

fn unsafe_control_metadata_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

pub fn valid_request_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessInventory {
    pub process_id: u32,
    pub windows: Vec<WindowInventory>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowInventory {
    /// Stable within the lifetime of this GUI process.
    pub id: u64,
    pub focused: bool,
    pub visible: bool,
    pub quick_access: bool,
    pub workspace_id: Option<String>,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub tabs: Vec<TabInventory>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabInventory {
    /// Stable within the lifetime of this window.
    pub id: u64,
    pub title: String,
    pub kind: TabKind,
    pub active: bool,
    pub pinned: bool,
    pub activity: bool,
    pub closed: bool,
    pub domain_id: Option<String>,
    pub panes: Vec<PaneInventory>,
    pub dividers: Vec<DividerInventory>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabKind {
    Shell,
    Profile,
    Plugin,
    LiveDomain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneInventory {
    /// Stable within the lifetime of this tab.
    pub id: u32,
    pub title: Option<String>,
    pub focused: bool,
    pub closed: bool,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DividerInventory {
    /// Stable within the lifetime of this tab topology.
    pub id: u64,
    pub kind: DividerKind,
    pub direction: SplitDirection,
    /// First-child ratio in thousandths, bounded to 100 through 900.
    pub ratio_millis: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DividerKind {
    Local,
    LiveDomain,
}

impl ProcessInventory {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.process_id == 0 || self.process_id > i32::MAX as u32 {
            anyhow::bail!("GUI inventory has an invalid process ID");
        }
        if self.windows.len() > MAX_WINDOWS {
            anyhow::bail!("GUI inventory exceeds the window limit");
        }
        let mut window_ids = HashSet::with_capacity(self.windows.len());
        let mut tab_count = 0usize;
        let mut pane_count = 0usize;
        let mut divider_count = 0usize;
        let mut metadata_bytes = 0usize;
        let mut quick_access_windows = 0usize;
        for window in &self.windows {
            if window.id == 0 || !window_ids.insert(window.id) {
                anyhow::bail!("GUI inventory contains an invalid or duplicate window ID");
            }
            if window.pixel_width == 0 || window.pixel_height == 0 {
                anyhow::bail!("GUI inventory contains zero-sized window geometry");
            }
            quick_access_windows += usize::from(window.quick_access);
            if quick_access_windows > 1 {
                anyhow::bail!("GUI inventory contains multiple quick-access windows");
            }
            validate_metadata(window.workspace_id.as_deref(), &mut metadata_bytes)?;
            if window.tabs.len() > MAX_TABS_PER_WINDOW {
                anyhow::bail!("GUI inventory exceeds the per-window tab limit");
            }
            tab_count = tab_count
                .checked_add(window.tabs.len())
                .context("GUI inventory tab count overflow")?;
            let mut tab_ids = HashSet::with_capacity(window.tabs.len());
            for tab in &window.tabs {
                if !tab_ids.insert(tab.id) {
                    anyhow::bail!("GUI inventory contains a duplicate tab ID");
                }
                validate_metadata(Some(&tab.title), &mut metadata_bytes)?;
                validate_metadata(tab.domain_id.as_deref(), &mut metadata_bytes)?;
                if tab.panes.len() > MAX_PANES_PER_TAB {
                    anyhow::bail!("GUI inventory exceeds the per-tab pane limit");
                }
                if tab.dividers.len() > MAX_DIVIDERS_PER_TAB {
                    anyhow::bail!("GUI inventory exceeds the per-tab divider limit");
                }
                if tab.dividers.len() > tab.panes.len().saturating_sub(1) {
                    anyhow::bail!("GUI inventory has more dividers than its pane tree permits");
                }
                pane_count = pane_count
                    .checked_add(tab.panes.len())
                    .context("GUI inventory pane count overflow")?;
                let mut pane_ids = HashSet::with_capacity(tab.panes.len());
                for pane in &tab.panes {
                    if !pane_ids.insert(pane.id) || pane.cols == 0 || pane.rows == 0 {
                        anyhow::bail!(
                            "GUI inventory contains a duplicate pane ID or zero cell geometry"
                        );
                    }
                    validate_metadata(pane.title.as_deref(), &mut metadata_bytes)?;
                }
                divider_count = divider_count
                    .checked_add(tab.dividers.len())
                    .context("GUI inventory divider count overflow")?;
                let mut divider_ids = HashSet::with_capacity(tab.dividers.len());
                for divider in &tab.dividers {
                    if !divider_ids.insert(divider.id)
                        || !(100..=900).contains(&divider.ratio_millis)
                    {
                        anyhow::bail!("GUI inventory contains an invalid or duplicate divider");
                    }
                }
            }
        }
        if tab_count > MAX_TOTAL_TABS
            || pane_count > MAX_TOTAL_PANES
            || divider_count > MAX_TOTAL_DIVIDERS
            || metadata_bytes > MAX_METADATA_BYTES
        {
            anyhow::bail!("GUI inventory exceeds its process-wide resource limits");
        }
        Ok(())
    }
}

fn validate_metadata(value: Option<&str>, total: &mut usize) -> anyhow::Result<()> {
    const MAX_FIELD_BYTES: usize = 1_024;
    let Some(value) = value else {
        return Ok(());
    };
    if value.len() > MAX_FIELD_BYTES || value.chars().any(char::is_control) {
        anyhow::bail!("GUI inventory contains unsafe or oversized metadata");
    }
    *total = total
        .checked_add(value.len())
        .context("GUI inventory metadata size overflow")?;
    Ok(())
}

pub fn write_frame<T: Serialize>(writer: &mut impl Write, frame: &T) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(frame).context("serialize GUI control frame")?;
    if payload.len() > MAX_FRAME_BYTES {
        anyhow::bail!("GUI control frame exceeds {MAX_FRAME_BYTES} bytes");
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .context("write GUI control frame length")?;
    writer
        .write_all(&payload)
        .context("write GUI control frame payload")?;
    writer.flush().context("flush GUI control frame")
}

pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> anyhow::Result<T> {
    let mut length = [0u8; 4];
    reader
        .read_exact(&mut length)
        .context("read GUI control frame length")?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        anyhow::bail!("invalid GUI control frame length {length}");
    }
    let mut payload = vec![0u8; length];
    reader
        .read_exact(&mut payload)
        .context("read GUI control frame payload")?;
    serde_json::from_slice(&payload).context("decode GUI control frame")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_round_trips_with_stable_versioned_shape() {
        let frame = ServerFrame::Inventory {
            version: PROTOCOL_VERSION,
            inventory: ProcessInventory {
                process_id: 42,
                windows: vec![WindowInventory {
                    id: 7,
                    focused: true,
                    visible: true,
                    quick_access: false,
                    workspace_id: Some("daily".into()),
                    pixel_width: 1_200,
                    pixel_height: 800,
                    tabs: vec![TabInventory {
                        id: 3,
                        title: "build".into(),
                        kind: TabKind::Shell,
                        active: true,
                        pinned: false,
                        activity: false,
                        closed: false,
                        domain_id: None,
                        panes: vec![
                            PaneInventory {
                                id: 0,
                                title: None,
                                focused: true,
                                closed: false,
                                cols: 120,
                                rows: 40,
                            },
                            PaneInventory {
                                id: 1,
                                title: Some("logs".into()),
                                focused: false,
                                closed: false,
                                cols: 120,
                                rows: 40,
                            },
                        ],
                        dividers: vec![DividerInventory {
                            id: 1,
                            kind: DividerKind::Local,
                            direction: SplitDirection::Vertical,
                            ratio_millis: 500,
                        }],
                        truncated: false,
                    }],
                    truncated: false,
                }],
                truncated: false,
            },
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );
    }

    #[test]
    fn create_window_requests_and_responses_are_explicitly_versioned() {
        let request_id = "0123456789abcdef0123456789abcdef".to_string();
        let request = Request::CreateWindow {
            request_id: request_id.clone(),
            source_window: 7,
        };
        request.validate().unwrap();
        let frame = ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );

        let response = ServerFrame::WindowCreated {
            version: PROTOCOL_VERSION,
            process_id: 42,
            request_id,
            window_id: 9,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert_eq!(
            read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
            response
        );

        assert!(
            Request::CreateWindow {
                request_id: "ABC".into(),
                source_window: 7,
            }
            .validate()
            .is_err()
        );
        assert!(
            Request::CreateWindow {
                request_id: "0".repeat(32),
                source_window: 0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn create_tab_requests_and_responses_are_explicitly_versioned() {
        let request_id = "fedcba9876543210fedcba9876543210".to_string();
        let request = Request::CreateTab {
            request_id: request_id.clone(),
            window_id: 7,
        };
        request.validate().unwrap();
        let frame = ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );

        let response = ServerFrame::TabCreated {
            version: PROTOCOL_VERSION,
            process_id: 42,
            request_id,
            window_id: 7,
            tab_id: 3,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert_eq!(
            read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
            response
        );

        assert!(
            Request::CreateTab {
                request_id: "0".repeat(32),
                window_id: 0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn launch_tab_requests_preserve_exact_argv_and_reject_unsafe_or_oversized_input() {
        let request = Request::LaunchTab {
            request_id: "0123456789abcdef0123456789abcdef".into(),
            window_id: 7,
            program: "/bin/printf".into(),
            args: vec![
                "".into(),
                "two words".into(),
                "$HOME;🙂".into(),
                "--json".into(),
            ],
            working_directory: Some("/tmp/launch space".into()),
            title: Some("Exact launch 🙂".into()),
        };
        request.validate().unwrap();
        let frame = ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request: request.clone(),
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );
        assert_eq!(
            request.request_id(),
            Some("0123456789abcdef0123456789abcdef")
        );

        let invalid = |program: String,
                       args: Vec<String>,
                       working_directory: Option<String>,
                       title: Option<String>| Request::LaunchTab {
            request_id: "0123456789abcdef0123456789abcdef".into(),
            window_id: 7,
            program,
            args,
            working_directory,
            title,
        };
        assert!(invalid("".into(), vec![], None, None).validate().is_err());
        assert!(
            invalid("bad\u{1b}".into(), vec![], None, None)
                .validate()
                .is_err()
        );
        assert!(
            invalid("ok".into(), vec!["bad\0arg".into()], None, None)
                .validate()
                .is_err()
        );
        assert!(
            invalid(
                "ok".into(),
                vec!["x".into(); MAX_CONTROL_LAUNCH_ARGS + 1],
                None,
                None,
            )
            .validate()
            .is_err()
        );
        assert!(
            invalid(
                "x".repeat(MAX_CONTROL_LAUNCH_ARGV_BYTES + 1),
                vec![],
                None,
                None,
            )
            .validate()
            .is_err()
        );
        assert!(
            invalid("ok".into(), vec![], Some("relative".into()), None)
                .validate()
                .is_err()
        );
        assert!(
            invalid(
                "ok".into(),
                vec![],
                Some("/tmp/\u{202e}hidden".into()),
                None,
            )
            .validate()
            .is_err()
        );
        assert!(
            invalid("ok".into(), vec![], None, Some("line\nbreak".into()))
                .validate()
                .is_err()
        );
        assert!(
            invalid(
                "ok".into(),
                vec![],
                None,
                Some("x".repeat(MAX_CONTROL_LAUNCH_TITLE_BYTES + 1)),
            )
            .validate()
            .is_err()
        );
    }

    #[test]
    fn launch_window_requests_and_responses_preserve_exact_stable_ids() {
        let request_id = "abcdef0123456789abcdef0123456789".to_string();
        let request = Request::LaunchWindow {
            request_id: request_id.clone(),
            source_window: 7,
            program: "/bin/printf".into(),
            args: vec!["".into(), "two words".into(), "$HOME;🙂".into()],
            working_directory: Some("/tmp/launch space".into()),
            title: Some("Exact window 🙂".into()),
        };
        request.validate().unwrap();
        let frame = ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request: request.clone(),
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );
        assert_eq!(request.request_id(), Some(request_id.as_str()));

        let response = ServerFrame::WindowLaunched {
            version: PROTOCOL_VERSION,
            process_id: 42,
            request_id,
            window_id: 9,
            tab_id: 0,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert_eq!(
            read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
            response
        );

        assert!(
            Request::LaunchWindow {
                request_id: "0".repeat(32),
                source_window: 0,
                program: "/bin/true".into(),
                args: Vec::new(),
                working_directory: None,
                title: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            Request::LaunchWindow {
                request_id: "0".repeat(32),
                source_window: 7,
                program: "/bin/true".into(),
                args: vec!["x".into(); MAX_CONTROL_LAUNCH_ARGS + 1],
                working_directory: None,
                title: None,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn split_pane_requests_and_responses_are_explicitly_versioned() {
        let request_id = "1234567890abcdef1234567890abcdef".to_string();
        let request = Request::SplitPane {
            request_id: request_id.clone(),
            window_id: 7,
            tab_id: 3,
            pane_id: 0,
            direction: SplitDirection::Vertical,
        };
        request.validate().unwrap();
        let frame = ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );

        let response = ServerFrame::PaneCreated {
            version: PROTOCOL_VERSION,
            process_id: 42,
            request_id,
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert_eq!(
            read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
            response
        );

        assert!(
            Request::SplitPane {
                request_id: "0".repeat(32),
                window_id: 0,
                tab_id: 0,
                pane_id: 0,
                direction: SplitDirection::Horizontal,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn focus_requests_and_responses_are_explicit_and_id_targeted() {
        let request_id = "abcdef0123456789abcdef0123456789".to_string();
        for request in [
            Request::FocusWindow {
                request_id: request_id.clone(),
                window_id: 7,
            },
            Request::SetQuickAccessVisibility {
                request_id: request_id.clone(),
                window_id: 7,
                visible: false,
            },
            Request::SelectTab {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
            },
            Request::FocusPane {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
            },
        ] {
            request.validate().unwrap();
            let frame = ClientFrame {
                version: PROTOCOL_VERSION,
                token: "secret".into(),
                request,
            };
            let mut wire = Vec::new();
            write_frame(&mut wire, &frame).unwrap();
            assert_eq!(
                read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
                frame
            );
        }

        for response in [
            ServerFrame::WindowFocusRequested {
                version: PROTOCOL_VERSION,
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                already_focused: false,
            },
            ServerFrame::QuickAccessChanged {
                version: PROTOCOL_VERSION,
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                visible: false,
            },
            ServerFrame::TabSelected {
                version: PROTOCOL_VERSION,
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                changed: true,
            },
            ServerFrame::PaneFocused {
                version: PROTOCOL_VERSION,
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                tab_changed: true,
                pane_changed: true,
            },
        ] {
            let mut wire = Vec::new();
            write_frame(&mut wire, &response).unwrap();
            assert_eq!(
                read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
                response
            );
        }
    }

    #[test]
    fn close_requests_distinguish_scheduled_and_completed_teardown() {
        let request_id = "9876543210abcdef9876543210abcdef".to_string();
        for request in [
            Request::CloseWindow {
                request_id: request_id.clone(),
                window_id: 7,
            },
            Request::CloseTab {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
            },
            Request::ClosePane {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
            },
        ] {
            request.validate().unwrap();
            let frame = ClientFrame {
                version: PROTOCOL_VERSION,
                token: "secret".into(),
                request,
            };
            let mut wire = Vec::new();
            write_frame(&mut wire, &frame).unwrap();
            assert_eq!(
                read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
                frame
            );
        }

        for response in [
            ServerFrame::WindowCloseRequested {
                version: PROTOCOL_VERSION,
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
            },
            ServerFrame::TabCloseRequested {
                version: PROTOCOL_VERSION,
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
            },
            ServerFrame::PaneClosed {
                version: PROTOCOL_VERSION,
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                remaining_panes: 1,
                focused_pane_id: 0,
            },
        ] {
            let mut wire = Vec::new();
            write_frame(&mut wire, &response).unwrap();
            assert_eq!(
                read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
                response
            );
        }
    }

    #[test]
    fn tab_move_wire_shape_returns_destination_local_identity() {
        let request_id = "0123456789abcdef0123456789abcdef".to_string();
        let request = Request::MoveTab {
            request_id: request_id.clone(),
            source_window_id: 7,
            tab_id: 3,
            destination_window_id: 9,
            destination_index: 2,
        };
        request.validate().unwrap();
        let frame = ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );

        let response = ServerFrame::TabMoved {
            version: PROTOCOL_VERSION,
            process_id: 42,
            request_id,
            source_window_id: 7,
            source_tab_id: 3,
            destination_window_id: 9,
            destination_tab_id: 11,
            destination_index: 2,
            source_window_closing: false,
            changed: true,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert_eq!(
            read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
            response
        );

        assert!(
            Request::MoveTab {
                request_id: "0".repeat(32),
                source_window_id: 0,
                tab_id: 3,
                destination_window_id: 9,
                destination_index: 0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn pane_move_wire_shape_names_both_complete_paths() {
        let request_id = "abcdefabcdefabcdefabcdefabcdefab".to_string();
        let request = Request::MovePane {
            request_id: request_id.clone(),
            source_window_id: 7,
            source_tab_id: 3,
            pane_id: 1,
            destination_window_id: 9,
            destination_tab_id: 4,
            target_pane_id: 2,
            direction: SplitDirection::Horizontal,
        };
        request.validate().unwrap();
        let frame = ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );

        let response = ServerFrame::PaneMoved {
            version: PROTOCOL_VERSION,
            process_id: 42,
            request_id,
            source_window_id: 7,
            source_tab_id: 3,
            source_pane_id: 1,
            destination_window_id: 9,
            destination_tab_id: 4,
            destination_pane_id: 5,
            target_pane_id: 2,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert_eq!(
            read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
            response
        );
    }

    #[test]
    fn terminal_text_wire_shape_is_bounded_and_read_only() {
        let request = Request::ReadText {
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            history_lines: 40,
            max_bytes: 4096,
        };
        request.validate().unwrap();
        assert_eq!(request.request_id(), None);
        let frame = ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );

        let response = ServerFrame::Text {
            version: PROTOCOL_VERSION,
            process_id: 42,
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            screen: TextScreen::Primary,
            history_lines: 12,
            text: "recent output".into(),
            truncated: false,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert_eq!(
            read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
            response
        );

        for invalid in [
            Request::ReadText {
                window_id: 0,
                tab_id: 3,
                pane_id: 1,
                history_lines: 0,
                max_bytes: 1,
            },
            Request::ReadText {
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                history_lines: MAX_TEXT_HISTORY_LINES + 1,
                max_bytes: 1,
            },
            Request::ReadText {
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                history_lines: 0,
                max_bytes: 0,
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn terminal_input_wire_shape_is_utf8_typed_and_replay_identified() {
        let request_id = "33333333333333333333333333333333".to_string();
        let request = Request::SendText {
            request_id: request_id.clone(),
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            text: "cargo test\nnext".into(),
            paste: true,
            enter: true,
        };
        request.validate().unwrap();
        assert_eq!(request.request_id(), Some(request_id.as_str()));
        let frame = ClientFrame {
            version: PROTOCOL_VERSION,
            token: "secret".into(),
            request,
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert_eq!(
            read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
            frame
        );

        let response = ServerFrame::InputAccepted {
            version: PROTOCOL_VERSION,
            process_id: 42,
            request_id,
            window_id: 7,
            tab_id: 3,
            pane_id: 1,
            input_bytes: 16,
            wire_bytes: 28,
            paste: true,
            enter: true,
            disposition: InputDisposition::PendingConfirmation,
            confirmation_id: Some(9),
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert_eq!(
            read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
            response
        );

        for text in ["\u{1b}[31m", "bad\rreturn", "\0"] {
            assert!(
                Request::SendText {
                    request_id: "4".repeat(32),
                    window_id: 7,
                    tab_id: 3,
                    pane_id: 1,
                    text: text.into(),
                    paste: false,
                    enter: false,
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Request::SendText {
                request_id: "4".repeat(32),
                window_id: 7,
                tab_id: 3,
                pane_id: 1,
                text: "x".repeat(MAX_CONTROL_INPUT_BYTES + 1),
                paste: false,
                enter: false,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn resize_wire_shapes_distinguish_native_request_from_completed_divider_change() {
        let request_id = "22222222222222222222222222222222".to_string();
        for request in [
            Request::ResizeWindow {
                request_id: request_id.clone(),
                window_id: 7,
                width: 900,
                height: 600,
            },
            Request::ResizeDivider {
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                divider_id: 11,
                ratio_millis: 650,
            },
        ] {
            request.validate().unwrap();
            let frame = ClientFrame {
                version: PROTOCOL_VERSION,
                token: "secret".into(),
                request,
            };
            let mut wire = Vec::new();
            write_frame(&mut wire, &frame).unwrap();
            assert_eq!(
                read_frame::<ClientFrame>(&mut wire.as_slice()).unwrap(),
                frame
            );
        }

        for response in [
            ServerFrame::WindowResizeRequested {
                version: PROTOCOL_VERSION,
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                requested_width: 900,
                requested_height: 600,
                immediate_width: None,
                immediate_height: None,
            },
            ServerFrame::DividerResized {
                version: PROTOCOL_VERSION,
                process_id: 42,
                request_id: request_id.clone(),
                window_id: 7,
                tab_id: 3,
                divider_id: 11,
                previous_ratio_millis: 500,
                ratio_millis: 650,
                changed: true,
            },
        ] {
            let mut wire = Vec::new();
            write_frame(&mut wire, &response).unwrap();
            assert_eq!(
                read_frame::<ServerFrame>(&mut wire.as_slice()).unwrap(),
                response
            );
        }

        assert!(
            Request::ResizeWindow {
                request_id: request_id.clone(),
                window_id: 7,
                width: 159,
                height: 600,
            }
            .validate()
            .is_err()
        );
        assert!(
            Request::ResizeDivider {
                request_id,
                window_id: 7,
                tab_id: 3,
                divider_id: 11,
                ratio_millis: 901,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn decoding_rejects_unknown_and_unbounded_frames() {
        let payload = br#"{"version":1,"token":"x","request":"inventory","extra":true}"#;
        let mut wire = (payload.len() as u32).to_be_bytes().to_vec();
        wire.extend_from_slice(payload);
        assert!(read_frame::<ClientFrame>(&mut wire.as_slice()).is_err());

        let oversized_length = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes();
        let mut oversized = oversized_length.as_slice();
        assert!(read_frame::<ClientFrame>(&mut oversized).is_err());
        let empty_length = 0u32.to_be_bytes();
        let mut empty = empty_length.as_slice();
        assert!(read_frame::<ClientFrame>(&mut empty).is_err());
    }

    #[test]
    fn process_wide_snapshot_caps_fit_the_wire_budget() {
        let mut windows = Vec::new();
        let mut tab_id = 0usize;
        let mut metadata_left = MAX_METADATA_BYTES;
        for window_id in 0..MAX_WINDOWS {
            let mut tabs = Vec::new();
            for _ in 0..(MAX_TOTAL_TABS / MAX_WINDOWS) {
                let title_bytes = metadata_left.min(128);
                metadata_left -= title_bytes;
                let panes_per_tab = MAX_TOTAL_PANES / MAX_TOTAL_TABS;
                let panes = (0..panes_per_tab)
                    .map(|pane_id| PaneInventory {
                        id: u32::MAX - pane_id as u32,
                        title: None,
                        focused: pane_id == 0,
                        closed: false,
                        cols: u16::MAX,
                        rows: u16::MAX,
                    })
                    .collect();
                let dividers = (0..panes_per_tab.saturating_sub(1))
                    .map(|divider_id| DividerInventory {
                        id: divider_id as u64,
                        kind: DividerKind::LiveDomain,
                        direction: SplitDirection::Vertical,
                        ratio_millis: 500,
                    })
                    .collect();
                tabs.push(TabInventory {
                    id: u64::MAX - tab_id as u64,
                    title: "x".repeat(title_bytes),
                    kind: TabKind::LiveDomain,
                    active: tab_id == 0,
                    pinned: false,
                    activity: true,
                    closed: false,
                    domain_id: None,
                    panes,
                    dividers,
                    truncated: false,
                });
                tab_id += 1;
            }
            windows.push(WindowInventory {
                id: u64::MAX - window_id as u64,
                focused: window_id == 0,
                visible: true,
                quick_access: false,
                workspace_id: None,
                pixel_width: MAX_WINDOW_DIMENSION,
                pixel_height: MAX_WINDOW_DIMENSION,
                tabs,
                truncated: false,
            });
        }
        assert_eq!(tab_id, MAX_TOTAL_TABS);
        let frame = ServerFrame::Inventory {
            version: PROTOCOL_VERSION,
            inventory: ProcessInventory {
                process_id: i32::MAX as u32,
                windows,
                truncated: false,
            },
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &frame).unwrap();
        assert!(wire.len() <= MAX_FRAME_BYTES + 4);
        if let ServerFrame::Inventory { inventory, .. } = frame {
            inventory.validate().unwrap();
        }
    }

    #[test]
    fn semantic_validation_rejects_duplicate_ids_controls_and_aggregate_overflow() {
        let mut inventory = ProcessInventory {
            process_id: 42,
            windows: vec![WindowInventory {
                id: 1,
                focused: true,
                visible: true,
                quick_access: false,
                workspace_id: None,
                pixel_width: 800,
                pixel_height: 600,
                tabs: vec![TabInventory {
                    id: 1,
                    title: "safe".into(),
                    kind: TabKind::Shell,
                    active: true,
                    pinned: false,
                    activity: false,
                    closed: false,
                    domain_id: None,
                    panes: vec![PaneInventory {
                        id: 0,
                        title: None,
                        focused: true,
                        closed: false,
                        cols: 80,
                        rows: 24,
                    }],
                    dividers: Vec::new(),
                    truncated: false,
                }],
                truncated: false,
            }],
            truncated: false,
        };
        inventory.validate().unwrap();
        inventory.windows[0].tabs[0]
            .dividers
            .push(DividerInventory {
                id: 1,
                kind: DividerKind::Local,
                direction: SplitDirection::Vertical,
                ratio_millis: 99,
            });
        assert!(inventory.validate().is_err());
        inventory.windows[0].tabs[0].dividers.clear();
        inventory.windows[0].tabs[0].title = "unsafe\u{1b}".into();
        assert!(inventory.validate().is_err());
        inventory.windows[0].tabs[0].title = "safe".into();
        inventory.windows.push(inventory.windows[0].clone());
        assert!(inventory.validate().is_err());

        let template = inventory.windows[0].tabs[0].clone();
        inventory.windows.truncate(1);
        inventory.windows.push(WindowInventory {
            id: 2,
            focused: false,
            visible: true,
            quick_access: false,
            workspace_id: None,
            pixel_width: 800,
            pixel_height: 600,
            tabs: (0..MAX_TOTAL_TABS)
                .map(|index| TabInventory {
                    id: index as u64 + 2,
                    panes: Vec::new(),
                    ..template.clone()
                })
                .collect(),
            truncated: false,
        });
        assert!(inventory.validate().is_err());
    }
}
