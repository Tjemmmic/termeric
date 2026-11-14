//! The system's own file chooser, so choosing a file looks like the rest of
//! the desktop: the XDG desktop portal on Linux (`portal.rs`), asked off the
//! event loop; an `NSOpenPanel` sheet on macOS (`open_panel.rs`), shown from
//! it. `app/file_chooser.rs` opens either for a window and routes the answer
//! back to it.

#[cfg(target_os = "macos")]
mod open_panel;
#[cfg(target_os = "linux")]
mod portal;

#[cfg(target_os = "macos")]
pub(crate) use open_panel::present;
#[cfg(target_os = "linux")]
pub(crate) use portal::choose;

use std::path::PathBuf;

/// One kind of file the chooser offers, by name and glob patterns of the
/// form `*.extension`.
pub struct FileFilter {
    pub name: &'static str,
    pub patterns: &'static [&'static str],
}

/// What the person did in the chooser.
#[derive(Debug, PartialEq, Eq)]
pub enum FileChoice {
    /// What they chose: one path, unless they were offered several.
    Chosen(Vec<PathBuf>),
    /// They closed it without choosing.
    Cancelled,
}

/// What the chooser picks.
pub enum Pick {
    /// One file of the kind the filter describes.
    File(&'static FileFilter),
    /// One folder, starting in `start` when given.
    Folder { start: Option<PathBuf> },
    /// Any number of files, starting in `start` when given.
    Files { start: Option<PathBuf> },
}

/// A question for the chooser: its title and what it picks.
pub struct FileRequest {
    pub title: &'static str,
    pub pick: Pick,
}

/// The path a `file://` URI names on this computer, or `None` for another
/// host, another scheme, or a malformed escape.
pub(crate) fn local_path(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let rest = uri.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    let bytes = termeric_core::uri::percent_decode(&rest.as_bytes()[slash..])?;
    if bytes.contains(&0) {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// The extensions a filter's `*.extension` patterns name, for choosers that
/// filter by extension rather than by pattern.
#[cfg(any(target_os = "macos", test))]
fn filter_extensions(filter: &FileFilter) -> Vec<&'static str> {
    filter
        .patterns
        .iter()
        .filter_map(|pattern| pattern.strip_prefix("*."))
        .filter(|extension| !extension.is_empty() && !extension.contains(['*', '?', '[']))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_file_uris_become_paths() {
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(
            local_path("file:///home/me/My%20Pictures/sky.png"),
            Some(PathBuf::from("/home/me/My Pictures/sky.png"))
        );
        assert_eq!(
            local_path("file://localhost/tmp/a.jpg"),
            Some(PathBuf::from("/tmp/a.jpg"))
        );
        // Bytes that aren't UTF-8 are still a path.
        assert_eq!(
            local_path("file:///tmp/%FF.png"),
            Some(PathBuf::from(std::ffi::OsString::from_vec(
                b"/tmp/\xff.png".to_vec()
            )))
        );
        for refused in [
            "file://other-host/tmp/a.png",
            "https://example.org/a.png",
            "file:///tmp/%zz.png",
            "file:///tmp/%00.png",
        ] {
            assert_eq!(local_path(refused), None, "{refused}");
        }
    }

    #[test]
    fn filters_name_their_extensions() {
        static IMAGES: FileFilter = FileFilter {
            name: "Images",
            patterns: &["*.png", "*.jpg", "*.jpeg", "photo-*", "*.[ch]"],
        };
        assert_eq!(filter_extensions(&IMAGES), ["png", "jpg", "jpeg"]);
    }
}
