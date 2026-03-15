//! App-internal action enum

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
    /// Toggle settings panel
    ToggleSettings,
    /// Navigate to previous prompt (shell integration)
    PreviousPrompt,
    /// Navigate to next prompt (shell integration)
    NextPrompt,
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
    /// Clear scrollback buffer
    ClearScrollback,
    /// Reset terminal
    ResetTerminal,
}
