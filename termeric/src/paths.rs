//! Where Termeric keeps its configuration and data, and where its own
//! executable is.
//!
//! Both directories are found from the home directory. Without one there
//! is nowhere to keep Termeric's files, and nothing falls back to the
//! current directory: what needs a directory is skipped or refused.

use anyhow::Context;
use std::path::PathBuf;

/// This Termeric's own executable, with links resolved, for the commands it
/// starts or names for others to start (the domain server and relay, the
/// serial bridge, agent hooks, the supervisor's MCP server) and for telling
/// one build from another.
pub fn executable() -> anyhow::Result<PathBuf> {
    std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .context("locate the Termeric executable")
}

/// Whether `executable` runs from App Translocation's temporary copy, which
/// macOS uses for an app opened from where it was downloaded while it is
/// still quarantined. The copy's path changes from launch to launch, so
/// nothing that outlives this run (agent hooks) may name it.
pub fn is_translocated(executable: &std::path::Path) -> bool {
    executable
        .components()
        .any(|component| component.as_os_str() == "AppTranslocation")
}

/// [`executable`] as UTF-8, for command lines kept as text.
pub fn executable_text() -> anyhow::Result<String> {
    executable()?
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow::anyhow!("the Termeric executable's path is not valid UTF-8"))
}

/// Why something that needs [`config_dir`] or [`data_dir`] cannot work.
pub const NO_HOME_DIRECTORY: &str =
    "No home directory was found, so Termeric has nowhere to keep its files";

/// `~/.config/termeric`: configuration, themes, plugins, and shell
/// integration.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        (!NO_HOME.get()).then(|| test_root().join("config"))
    }
    #[cfg(not(test))]
    {
        xdg_directory("XDG_CONFIG_HOME", ".config")
    }
}

/// `~/.local/share/termeric`: sessions, workspaces, and application state.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        (!NO_HOME.get()).then(|| test_root().join("data"))
    }
    #[cfg(not(test))]
    {
        xdg_directory("XDG_DATA_HOME", ".local/share")
    }
}

/// `termeric` under the XDG base directory `variable` names, else under
/// `fallback` in the home directory. macOS uses the same places as Linux, as
/// other terminals do there, so dotfiles carry over between the two.
#[cfg(not(test))]
fn xdg_directory(variable: &str, fallback: &str) -> Option<PathBuf> {
    let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    xdg_directory_from(std::env::var_os(variable), home.as_deref(), fallback)
}

/// [`xdg_directory`] for a given variable value and home directory. The XDG
/// specification ignores a relative value.
fn xdg_directory_from(
    value: Option<std::ffi::OsString>,
    home: Option<&std::path::Path>,
    fallback: &str,
) -> Option<PathBuf> {
    value
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| home.join(fallback)))
        .map(|base| base.join("termeric"))
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
thread_local! {
    static NO_HOME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `test` on this thread as if no home directory could be found.
#[cfg(test)]
pub(crate) fn without_home<R>(test: impl FnOnce() -> R) -> R {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            NO_HOME.set(false);
        }
    }
    NO_HOME.set(true);
    let _restore = Restore;
    test()
}

#[cfg(test)]
mod tests {
    #[test]
    fn tests_never_resolve_the_persons_own_directories() {
        let root = std::env::temp_dir();
        for dir in [super::config_dir(), super::data_dir()] {
            assert!(dir.unwrap().starts_with(&root));
        }
        let plugins = crate::plugin::runtime::PluginPaths::configured().unwrap();
        for path in [
            crate::config::Config::config_path().unwrap(),
            crate::config::Config::themes_dir().unwrap(),
            plugins.registry,
            plugins.root,
            crate::app::state::AppState::state_path().unwrap(),
        ] {
            assert!(path.starts_with(&root), "{}", path.display());
        }
    }

    #[test]
    fn translocated_copies_are_recognized() {
        assert!(super::is_translocated(std::path::Path::new(
            "/private/var/folders/ab/cd/T/AppTranslocation/0F3A/d/Termeric.app/Contents/MacOS/termeric"
        )));
        assert!(!super::is_translocated(std::path::Path::new(
            "/Applications/Termeric.app/Contents/MacOS/termeric"
        )));
    }

    #[test]
    fn xdg_directories_follow_the_specification_on_every_system() {
        let home = Some(std::path::Path::new("/home/person"));
        assert_eq!(
            super::xdg_directory_from(None, home, ".config"),
            Some(std::path::PathBuf::from("/home/person/.config/termeric"))
        );
        assert_eq!(
            super::xdg_directory_from(Some("/elsewhere".into()), home, ".config"),
            Some(std::path::PathBuf::from("/elsewhere/termeric"))
        );
        // A relative value is ignored, as the specification asks.
        assert_eq!(
            super::xdg_directory_from(Some("relative".into()), home, ".local/share"),
            Some(std::path::PathBuf::from(
                "/home/person/.local/share/termeric"
            ))
        );
        assert_eq!(super::xdg_directory_from(None, None, ".config"), None);
    }

    #[test]
    fn without_a_home_directory_nothing_resolves() {
        super::without_home(|| {
            assert_eq!(super::config_dir(), None);
            assert_eq!(super::data_dir(), None);
            assert_eq!(crate::config::Config::config_path(), None);
            assert_eq!(crate::config::Config::themes_dir(), None);
            assert_eq!(crate::plugin::runtime::PluginPaths::configured(), None);
            assert_eq!(crate::app::state::AppState::state_path(), None);
        });
        assert!(super::config_dir().is_some());
    }

    #[test]
    fn the_executable_is_this_process_with_links_resolved() {
        let executable = super::executable().unwrap();
        assert!(executable.is_absolute());
        assert_eq!(
            std::env::current_exe().unwrap().canonicalize().unwrap(),
            executable
        );
        assert_eq!(
            super::executable_text().unwrap(),
            executable.to_str().unwrap()
        );
    }
}
