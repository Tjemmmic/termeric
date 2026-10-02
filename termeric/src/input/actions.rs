//! App-internal action enum

/// Action that can be triggered by a key binding
#[derive(Clone, Debug)]
pub enum Action {
    /// Copy selected text to clipboard
    Copy,
    /// Paste from clipboard
    Paste,
    /// Scroll back by one screen of the focused terminal
    ScrollPageUp,
    /// Scroll forward by one screen of the focused terminal
    ScrollPageDown,
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
    /// Toggle borderless fullscreen for the current native window
    ToggleFullscreen,
    /// Clear selection
    ClearSelection,
    /// Create a new tab
    NewTab,
    /// Close current tab
    CloseTab,
    /// Switch to next tab
    NextTab,
    /// Switch to previous tab
    PrevTab,
    /// Switch to tab by number (0-8)
    SwitchToTab(usize),
    /// Toggle search mode
    ToggleSearch,
    /// Choose a coding agent to start in a new tab
    NewAgent,
    /// Open the docked agent view with the keyboard, or close it.
    ToggleAgentView,
    /// Enter or leave keyboard copy mode.
    ToggleCopyMode,
    /// Enter quick select for visible semantic matches.
    QuickSelect,
    /// Open the searchable Unicode character picker.
    OpenUnicodePicker,
    /// Toggle settings panel
    ToggleSettings,
    /// Navigate to previous prompt (shell integration)
    PreviousPrompt,
    /// Navigate to next prompt (shell integration)
    NextPrompt,
    /// Jump to the nearest earlier command that exited non-zero
    PreviousFailedCommand,
    /// Jump to the nearest later command that exited non-zero
    NextFailedCommand,
    /// Execute a plugin command by its ID
    PluginCommand(String),
    /// Toggle command palette
    ToggleCommandPalette,
    /// Split pane vertically
    SplitVertical,
    /// Split pane horizontally
    SplitHorizontal,
    /// Navigate to pane in direction
    NavigatePane(String),
    /// Close current pane
    ClosePane,
    RenameTab,
    MoveTabLeft,
    MoveTabRight,
    MoveTabToNextWindow,
    TogglePinTab,
    TogglePaneZoom,
    TogglePaneReadOnly,
    SwapPaneNext,
    BalancePanes,
    MovePaneToNextTab,
    MovePaneToNextWindow,
    LayoutColumns,
    LayoutRows,
    LayoutGrid,
    LayoutTall,
    LayoutFat,
    CyclePaneCwdMode,
    ToggleBroadcast,
    /// Clear scrollback buffer
    ClearScrollback,
    /// Reset terminal
    ResetTerminal,
}
