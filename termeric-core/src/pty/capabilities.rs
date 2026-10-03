//! Terminal identity and terminfo discovery for child PTYs.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(crate) const TERMERIC_TERM: &str = "xterm-termeric";
pub(crate) const COMPATIBILITY_TERM: &str = "xterm-256color";
pub(crate) const TERM_OVERRIDE_ENV: &str = "TERMERIC_TERM";
pub(crate) const TERMINFO_OVERRIDE_ENV: &str = "TERMERIC_TERMINFO_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalEnvironment {
    pub term: &'static str,
    pub terminfo: Option<OsString>,
    pub terminfo_dirs: Option<OsString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermPreference {
    Auto,
    Compatibility,
    Termeric,
    Invalid,
}

impl TermPreference {
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("auto") => Self::Auto,
            Some("compat") | Some("compatibility") | Some(COMPATIBILITY_TERM) => {
                Self::Compatibility
            }
            Some(TERMERIC_TERM) => Self::Termeric,
            Some(_) => Self::Invalid,
        }
    }
}

pub(crate) fn terminal_environment() -> TerminalEnvironment {
    let requested = env::var(TERM_OVERRIDE_ENV).ok();
    let preference = TermPreference::parse(requested.as_deref());

    if preference == TermPreference::Invalid {
        log::warn!(
            "Ignoring invalid {TERM_OVERRIDE_ENV}={:?}; using {COMPATIBILITY_TERM}",
            requested.as_deref().unwrap_or_default()
        );
        return compatibility_environment();
    }

    let root = (preference != TermPreference::Compatibility)
        .then(discover_terminfo_root)
        .flatten();
    if selected_term(preference, root.is_some()) == COMPATIBILITY_TERM {
        if preference == TermPreference::Termeric {
            log::warn!(
                "{TERM_OVERRIDE_ENV} requested {TERMERIC_TERM}, but its compiled terminfo entry was not found; using {COMPATIBILITY_TERM}"
            );
        }
        return compatibility_environment();
    }

    let root = root.expect("custom TERM selection requires a discovered terminfo root");

    termeric_environment(&root, env::var_os("TERMINFO_DIRS").as_deref())
}

fn termeric_environment(
    root: &Path,
    inherited_terminfo_dirs: Option<&std::ffi::OsStr>,
) -> TerminalEnvironment {
    TerminalEnvironment {
        term: TERMERIC_TERM,
        // Pin ncurses to the same root whose entry justified exporting the
        // custom TERM. Otherwise an inherited TERMINFO can outrank a
        // TERMeric-specific override and make the selected identity stale or
        // undiscoverable inside the child.
        terminfo: Some(root.as_os_str().to_owned()),
        terminfo_dirs: terminfo_search_path(root, inherited_terminfo_dirs),
    }
}

fn selected_term(preference: TermPreference, entry_available: bool) -> &'static str {
    match preference {
        TermPreference::Auto | TermPreference::Termeric if entry_available => TERMERIC_TERM,
        _ => COMPATIBILITY_TERM,
    }
}

fn compatibility_environment() -> TerminalEnvironment {
    TerminalEnvironment {
        term: COMPATIBILITY_TERM,
        terminfo: None,
        terminfo_dirs: None,
    }
}

fn discover_terminfo_root() -> Option<PathBuf> {
    let mut roots = Vec::new();

    push_env_path(&mut roots, TERMINFO_OVERRIDE_ENV);
    push_env_path(&mut roots, "TERMINFO");

    if let Some(home) = env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".terminfo"));
    }
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        roots.push(PathBuf::from(data_home).join("terminfo"));
    }
    if let Some(dirs) = env::var_os("TERMINFO_DIRS") {
        roots.extend(env::split_paths(&dirs).filter(|path| !path.as_os_str().is_empty()));
    }
    if let Ok(executable) = env::current_exe()
        && let Some(bin_dir) = executable.parent()
        && let Some(prefix) = bin_dir.parent()
    {
        roots.push(prefix.join("share/terminfo"));
    }

    for root in default_terminfo_roots() {
        roots.push(PathBuf::from(root));
    }

    roots.into_iter().find(|root| terminfo_entry_exists(root))
}

fn push_env_path(roots: &mut Vec<PathBuf>, key: &str) {
    if let Some(value) = env::var_os(key).filter(|value| !value.is_empty()) {
        roots.push(PathBuf::from(value));
    }
}

fn terminfo_entry_exists(root: &Path) -> bool {
    let initial = &TERMERIC_TERM[..1];
    root.join(initial).join(TERMERIC_TERM).is_file()
        || root
            .join(format!("{:x}", TERMERIC_TERM.as_bytes()[0]))
            .join(TERMERIC_TERM)
            .is_file()
}

fn terminfo_search_path(root: &Path, existing: Option<&std::ffi::OsStr>) -> Option<OsString> {
    let mut paths = vec![root.to_path_buf()];
    if let Some(existing) = existing {
        paths.extend(env::split_paths(&existing));
    } else {
        // An empty TERMINFO_DIRS component asks ncurses to retain its compiled
        // system defaults after Termeric's discovered entry directory.
        paths.push(PathBuf::new());
    }
    env::join_paths(paths).ok()
}

fn default_terminfo_roots() -> &'static [&'static str] {
    &[
        "/usr/local/share/terminfo",
        "/usr/share/terminfo",
        "/lib/terminfo",
        "/etc/terminfo",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_vocabulary_is_small_and_explicit() {
        assert_eq!(TermPreference::parse(None), TermPreference::Auto);
        assert_eq!(TermPreference::parse(Some("auto")), TermPreference::Auto);
        assert_eq!(
            TermPreference::parse(Some("xterm-256color")),
            TermPreference::Compatibility
        );
        assert_eq!(
            TermPreference::parse(Some("compatibility")),
            TermPreference::Compatibility
        );
        assert_eq!(
            TermPreference::parse(Some("xterm-termeric")),
            TermPreference::Termeric
        );
        assert_eq!(
            TermPreference::parse(Some("screen")),
            TermPreference::Invalid
        );
    }

    #[test]
    fn custom_identity_is_never_selected_without_an_entry() {
        assert_eq!(
            selected_term(TermPreference::Auto, false),
            COMPATIBILITY_TERM
        );
        assert_eq!(
            selected_term(TermPreference::Termeric, false),
            COMPATIBILITY_TERM
        );
        assert_eq!(
            selected_term(TermPreference::Compatibility, true),
            COMPATIBILITY_TERM
        );
        assert_eq!(selected_term(TermPreference::Auto, true), TERMERIC_TERM);
        assert_eq!(selected_term(TermPreference::Termeric, true), TERMERIC_TERM);
    }

    #[test]
    fn discovery_accepts_character_and_hex_database_layouts() {
        for directory in ["x", "78"] {
            let temp = tempfile::tempdir().unwrap();
            let entry_dir = temp.path().join(directory);
            std::fs::create_dir(&entry_dir).unwrap();
            std::fs::write(entry_dir.join(TERMERIC_TERM), b"compiled entry").unwrap();
            assert!(terminfo_entry_exists(temp.path()));
        }
    }

    #[test]
    fn discovery_rejects_source_files_and_empty_directories() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("xterm-termeric.terminfo"), b"source").unwrap();
        assert!(!terminfo_entry_exists(temp.path()));
    }

    #[test]
    fn custom_environment_pins_the_discovered_root_and_retains_fallbacks() {
        let root = Path::new("/opt/termeric/share/terminfo");
        let existing =
            env::join_paths([Path::new("/custom/one"), Path::new("/custom/two")]).unwrap();
        let environment = termeric_environment(root, Some(&existing));

        assert_eq!(environment.terminfo.as_deref(), Some(root.as_os_str()));
        let paths: Vec<_> = env::split_paths(environment.terminfo_dirs.as_ref().unwrap()).collect();
        assert_eq!(
            paths,
            vec![
                root.to_path_buf(),
                PathBuf::from("/custom/one"),
                PathBuf::from("/custom/two")
            ]
        );
    }
}
