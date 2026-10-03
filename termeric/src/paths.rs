//! Where Termeric keeps its configuration and data.

use std::path::PathBuf;

/// `~/.config/termeric`: configuration, themes, plugins, and shell
/// integration.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(test_root().join("config"))
    }
    #[cfg(not(test))]
    {
        directories::ProjectDirs::from("", "", "termeric").map(|dirs| dirs.config_dir().into())
    }
}

/// `~/.local/share/termeric`: sessions, workspaces, and application state.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(test_root().join("data"))
    }
    #[cfg(not(test))]
    {
        directories::ProjectDirs::from("", "", "termeric").map(|dirs| dirs.data_dir().into())
    }
}

/// Tests never read or write the person's own configuration or data: every
/// path resolves under this test process's own directory, unless a test
/// points a store somewhere of its own.
#[cfg(test)]
fn test_root() -> &'static std::path::Path {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| std::env::temp_dir().join(format!("termeric-tests-{}", std::process::id())))
}

#[cfg(test)]
mod tests {
    #[test]
    fn tests_never_resolve_the_persons_own_directories() {
        let root = std::env::temp_dir();
        for dir in [super::config_dir(), super::data_dir()] {
            assert!(dir.unwrap().starts_with(&root));
        }
        for path in [
            crate::config::Config::config_path(),
            crate::config::Config::themes_dir(),
            crate::plugin::runtime::registry_path(),
            crate::plugin::runtime::plugins_dir(),
            crate::app::state::AppState::state_path(),
        ] {
            assert!(path.starts_with(&root), "{}", path.display());
        }
    }
}
