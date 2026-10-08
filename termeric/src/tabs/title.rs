//! Tab title generation helpers
//!
//! Functions for deriving tab titles from a terminal's reported directory.

use std::path::Path;
use termeric_core::Terminal;

/// The directory the shell last reported, on whichever host: a title shows
/// a remote directory too.
pub(crate) fn terminal_reported_directory_path(terminal: &Terminal) -> Option<&str> {
    terminal
        .reported_working_directory()
        .map(|directory| directory.path())
}

/// Extract the directory name from a path, showing ~ for home directory
pub(crate) fn dir_name_from_path(path: &str) -> String {
    // Check if this is the home directory
    if let Some(home) = std::env::var_os("HOME") {
        if path == home.to_str().unwrap_or("") {
            return "~".to_string();
        }
    }

    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Abbreviate a full path by replacing $HOME prefix with ~
pub(crate) fn abbreviate_path(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        if let Some(home_str) = home.to_str() {
            if path == home_str {
                return "~".to_string();
            }
            if let Some(rest) = path.strip_prefix(home_str) {
                return format!("~{}", rest);
            }
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_reported_directory_still_names_the_title() {
        let mut terminal = Terminal::new(8, 2);
        terminal.process(b"\x1b]7;file://remote.example/tmp/not-local\x07");
        assert_eq!(
            terminal_reported_directory_path(&terminal),
            Some("/tmp/not-local")
        );
    }
}
