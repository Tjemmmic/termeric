//! The desktop's own file chooser, through the XDG desktop portal
//! (`org.freedesktop.portal.FileChooser`), so choosing a file looks like the
//! rest of the desktop. KDE, GNOME, and most other desktops provide it.

use std::collections::HashMap;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const PORTAL_SERVICE: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const FILE_CHOOSER: &str = "org.freedesktop.portal.FileChooser";
const REQUEST: &str = "org.freedesktop.portal.Request";
/// How long the portal may take to start a chooser; the person then takes
/// as long as they like.
const METHOD_TIMEOUT: Duration = Duration::from_secs(10);

/// One kind of file the chooser offers, by name and glob patterns.
pub struct FileFilter {
    pub name: &'static str,
    pub patterns: &'static [&'static str],
}

/// What the person did in the chooser.
#[derive(Debug, PartialEq, Eq)]
pub enum FileChoice {
    Chosen(PathBuf),
    /// They closed it without choosing.
    Cancelled,
}

/// Ask the desktop for one local file of the kind `filter` describes. It
/// blocks until the person answers, so call it off the event loop. An error
/// means no chooser could be shown, such as when no portal runs, and the
/// caller should ask another way.
pub fn choose_file(title: &str, filter: &FileFilter) -> Result<FileChoice, String> {
    let connection = zbus::blocking::connection::Builder::session()
        .and_then(|builder| builder.method_timeout(METHOD_TIMEOUT).build())
        .map_err(|error| format!("session bus connection failed: {error}"))?;
    let token = format!(
        "termeric_{}",
        termeric_core::hex::random(8).map_err(|error| error.to_string())?
    );
    let sender = connection
        .unique_name()
        .ok_or("the session bus gave no name")?
        .to_string();
    // Listen before asking, so a quick answer isn't missed.
    let request = zbus::blocking::Proxy::new(
        &connection,
        PORTAL_SERVICE,
        request_path(&sender, &token),
        REQUEST,
    )
    .map_err(|error| error.to_string())?;
    let mut responses = request
        .receive_signal("Response")
        .map_err(|error| error.to_string())?;

    let chooser =
        zbus::blocking::Proxy::new(&connection, PORTAL_SERVICE, PORTAL_PATH, FILE_CHOOSER)
            .map_err(|error| error.to_string())?;
    let _: OwnedObjectPath = chooser
        .call("OpenFile", &("", title, open_file_options(&token, filter)))
        .map_err(|error| format!("the file chooser portal is unavailable: {error}"))?;

    let message = responses
        .next()
        .ok_or("the file chooser closed without answering")?;
    let (response, results): (u32, HashMap<String, OwnedValue>) = message
        .body()
        .deserialize()
        .map_err(|error| error.to_string())?;
    match response {
        0 => {
            let uris: Vec<String> = results
                .get("uris")
                .and_then(|uris| Vec::<String>::try_from(uris.clone()).ok())
                .unwrap_or_default();
            uris.first()
                .and_then(|uri| local_path(uri))
                .map(FileChoice::Chosen)
                .ok_or_else(|| "the file chooser returned no local file".into())
        }
        1 => Ok(FileChoice::Cancelled),
        _ => Err("the file chooser failed".into()),
    }
}

/// `OpenFile`'s options: the token that names the request, and the one
/// filter, as `a(sa(us))` (a name, then glob patterns tagged 0).
fn open_file_options<'a>(
    token: &'a str,
    filter: &'a FileFilter,
) -> HashMap<&'static str, Value<'a>> {
    let patterns: Vec<(u32, &str)> = filter
        .patterns
        .iter()
        .map(|pattern| (0, *pattern))
        .collect();
    HashMap::from([
        ("handle_token", Value::from(token)),
        ("modal", Value::from(true)),
        ("filters", Value::from(vec![(filter.name, patterns)])),
    ])
}

/// Where the portal posts its answer to the request `token` names, for the
/// connection named `sender` (such as ":1.42").
fn request_path(sender: &str, token: &str) -> String {
    let sender = sender.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

/// The path a `file://` URI names on this computer, or `None` for another
/// host, another scheme, or a malformed escape.
fn local_path(uri: &str) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_comes_to_a_path_built_from_the_name_and_token() {
        assert_eq!(
            request_path(":1.42", "termeric_ab12"),
            "/org/freedesktop/portal/desktop/request/1_42/termeric_ab12"
        );
    }

    #[test]
    fn the_options_have_the_types_the_portal_reads() {
        let filter = FileFilter {
            name: "Images",
            patterns: &["*.png"],
        };
        let options = open_file_options("termeric_ab12", &filter);
        let signature = |key: &str| options[key].value_signature().to_string();
        assert_eq!(signature("handle_token"), "s");
        assert_eq!(signature("modal"), "b");
        assert_eq!(signature("filters"), "a(sa(us))");
    }

    #[test]
    fn only_local_file_uris_become_paths() {
        assert_eq!(
            local_path("file:///home/me/My%20Pictures/sky.png"),
            Some(PathBuf::from("/home/me/My Pictures/sky.png"))
        );
        assert_eq!(
            local_path("file://localhost/tmp/a.jpg"),
            Some(PathBuf::from("/tmp/a.jpg"))
        );
        // Bytes that aren't UTF-8 are still a path on Linux.
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
}
