//! Where the local daemon listens: its socket and token in the private
//! runtime directory.

use crate::local_socket::{self, SocketPaths};
use std::path::PathBuf;

/// The daemon's endpoint is `domain-v1.sock` and `domain-v1.token`.
const ENDPOINT_STEM: &str = "domain-v1";

/// The daemon's endpoint in this user's runtime directory.
pub(crate) fn discover() -> anyhow::Result<SocketPaths> {
    in_directory(local_socket::runtime_directory())
}

/// The daemon's endpoint in `directory`, which is made private first.
pub(crate) fn in_directory(directory: PathBuf) -> anyhow::Result<SocketPaths> {
    SocketPaths::in_private_directory(directory, ENDPOINT_STEM)
}

/// Whether a daemon's socket exists, without creating the runtime directory.
pub(crate) fn daemon_socket_exists() -> bool {
    SocketPaths::in_directory(local_socket::runtime_directory(), ENDPOINT_STEM)
        .socket
        .exists()
}
