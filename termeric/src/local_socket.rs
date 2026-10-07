//! The security plumbing Termeric's two local socket servers share: the
//! live-domain daemon and each GUI process's control endpoint. Both live in
//! one private runtime directory, authenticate clients with a private token,
//! and serve only peers of the same user.

use anyhow::Context;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const TOKEN_BYTES: usize = 64;

/// One server's socket and token: `{stem}.sock` and `{stem}.token` in a
/// private runtime directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketPaths {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub token: PathBuf,
}

impl SocketPaths {
    /// The endpoint named `stem` in `directory`, which is created or secured
    /// to mode 0700 first.
    pub fn in_private_directory(directory: PathBuf, stem: &str) -> anyhow::Result<Self> {
        ensure_private_directory(&directory)?;
        Ok(Self::in_directory(directory, stem))
    }

    /// The endpoint named `stem` in `directory`, which is neither created nor
    /// checked.
    pub fn in_directory(directory: PathBuf, stem: &str) -> Self {
        Self {
            socket: directory.join(format!("{stem}.sock")),
            token: directory.join(format!("{stem}.token")),
            directory,
        }
    }

    pub fn load_or_create_token(&self) -> anyhow::Result<String> {
        load_or_create_token(&self.token)
    }

    pub fn read_token(&self) -> anyhow::Result<String> {
        read_token(&self.token)
    }
}

/// This user's runtime directory: `termeric` under `XDG_RUNTIME_DIR` when
/// that is a private directory of theirs, else `/tmp/termeric-{uid}`.
pub(crate) fn runtime_directory() -> PathBuf {
    let uid = effective_uid();
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| runtime_parent_is_safe(path, uid))
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/termeric-{uid}")));
    if base.ends_with(format!("termeric-{uid}")) {
        base
    } else {
        base.join("termeric")
    }
}

pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not retain pointers.
    unsafe { libc::geteuid() }
}

fn runtime_parent_is_safe(path: &Path, uid: u32) -> bool {
    path.is_absolute()
        && fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == uid
                && metadata.permissions().mode() & 0o077 == 0
        })
}

pub(crate) fn ensure_private_directory(path: &Path) -> anyhow::Result<()> {
    let directory = match open_directory(path) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .with_context(|| format!("create runtime directory {}", path.display()))?;
            open_directory(path).with_context(|| {
                format!("open newly created runtime directory {}", path.display())
            })?
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("open runtime directory {}", path.display()));
        }
    };
    validate_directory(&directory, path)?;
    directory
        .set_permissions(fs::Permissions::from_mode(0o700))
        .with_context(|| format!("secure runtime directory {}", path.display()))?;
    validate_directory(&directory, path)?;
    if directory.metadata()?.permissions().mode() & 0o777 != 0o700 {
        anyhow::bail!(
            "runtime directory {} could not be secured to mode 0700",
            path.display()
        );
    }
    Ok(())
}

fn open_directory(path: &Path) -> std::io::Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

fn validate_directory(directory: &fs::File, path: &Path) -> anyhow::Result<()> {
    let metadata = directory
        .metadata()
        .with_context(|| format!("inspect runtime directory {}", path.display()))?;
    if !metadata.file_type().is_dir() || metadata.uid() != effective_uid() {
        anyhow::bail!(
            "runtime directory {} is not a user-owned directory",
            path.display()
        );
    }
    Ok(())
}

fn load_or_create_token(path: &Path) -> anyhow::Result<String> {
    match read_token(path) {
        Ok(token) => Ok(token),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            match create_token(path) {
                Ok(token) => Ok(token),
                // Another same-user client may have won create_new between
                // our read and create. Re-read and revalidate its result.
                Err(create_error) => read_token(path).map_err(|_| create_error),
            }
        }
        Err(error) => Err(error),
    }
}

fn create_token(path: &Path) -> anyhow::Result<String> {
    // The publication nonce is independent of the credential so temporary
    // filenames never disclose even a prefix of the authentication token.
    let mut random = [0u8; 40];
    fs::File::open("/dev/urandom")
        .context("open operating-system random source")?
        .read_exact(&mut random)
        .context("read Termeric authentication token")?;
    let token = termeric_core::hex::encode(&random[..32]);
    let nonce = termeric_core::hex::encode(&random[32..]);
    let temporary = token_temporary_path(path, std::process::id(), &nonce);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .mode(0o600)
        .open(&temporary)
        .with_context(|| format!("create temporary Termeric token {}", temporary.display()))?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    drop(file);
    let linked = fs::hard_link(&temporary, path);
    let _ = fs::remove_file(&temporary);
    match linked {
        Ok(()) => Ok(token),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_token(path),
        Err(error) => Err(error).context("publish Termeric authentication token"),
    }
}

fn read_token(path: &Path) -> anyhow::Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        // A replaced FIFO must not block before descriptor type validation.
        // O_NONBLOCK does not change reads from the required regular file.
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("open Termeric token {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("read Termeric token metadata {}", path.display()))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() != TOKEN_BYTES as u64
    {
        anyhow::bail!(
            "Termeric token {} has unsafe type, owner, permissions, or size",
            path.display()
        );
    }

    // Read a fixed buffer from the already validated descriptor. This both
    // closes the metadata/open substitution window and prevents a corrupt
    // token path from causing an attacker-sized allocation.
    let mut bytes = [0u8; TOKEN_BYTES];
    file.read_exact(&mut bytes)
        .with_context(|| format!("read Termeric token {}", path.display()))?;
    let mut trailing = [0u8; 1];
    if file.read(&mut trailing)? != 0
        || !std::str::from_utf8(&bytes)
            .is_ok_and(|token| termeric_core::hex::is_lower(token, TOKEN_BYTES))
    {
        anyhow::bail!("Termeric token file is malformed");
    }
    Ok(std::str::from_utf8(&bytes)
        .context("Termeric token is not valid UTF-8")?
        .to_owned())
}

fn token_temporary_path(path: &Path, process_id: u32, nonce: &str) -> PathBuf {
    path.with_extension(format!("tmp-{process_id}-{nonce}"))
}

/// Refuses a peer of another user, and returns the peer's process ID when
/// the kernel reports one (a peer in another PID namespace reads as 0).
/// `peer` names the other end in errors.
pub(crate) fn same_user_peer(stream: &UnixStream, peer: &str) -> anyhow::Result<Option<u32>> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the connected descriptor and initialized output storage remain
    // valid for the duration of getsockopt.
    let result = unsafe {
        libc::getsockopt(
            std::os::fd::AsRawFd::as_raw_fd(stream),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("authenticate {peer}"));
    }
    if credentials.uid != effective_uid() {
        anyhow::bail!("{peer} belongs to a different user");
    }
    Ok(u32::try_from(credentials.pid).ok().filter(|pid| *pid > 0))
}

/// Admits a client that speaks `server_version` and presents
/// `expected_token`, checked in that order; otherwise the refusal to send it.
/// `service` names the server in the refusal.
pub(crate) fn authenticate(
    service: &str,
    server_version: u32,
    version: u32,
    token: &str,
    expected_token: &str,
) -> Result<(), String> {
    if version != server_version {
        return Err(format!(
            "protocol version {version} is incompatible with server version {server_version}"
        ));
    }
    if token != expected_token {
        return Err(format!("{service} authentication failed"));
    }
    Ok(())
}

/// A server's bounded count of connected clients.
pub(crate) struct ClientSlots {
    taken: Arc<AtomicUsize>,
    limit: usize,
}

/// One connected client's place in [`ClientSlots`], given back on drop.
pub(crate) struct ClientSlot(Arc<AtomicUsize>);

impl ClientSlots {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            taken: Arc::new(AtomicUsize::new(0)),
            limit,
        }
    }

    /// A slot, unless `limit` clients already hold one.
    pub(crate) fn claim(&self) -> Option<ClientSlot> {
        if self.taken.fetch_add(1, Ordering::AcqRel) >= self.limit {
            self.taken.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(ClientSlot(Arc::clone(&self.taken)))
    }

    #[cfg(test)]
    pub(crate) fn taken(&self) -> usize {
        self.taken.load(Ordering::Acquire)
    }
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Accepts clients on `listener` until `shutdown` is set, handing each to
/// `admit` with a slot from `slots`, or to `refuse` once every slot is taken.
/// A connection accepted once `shutdown` is set is the server being woken
/// (see [`wake`]) and is dropped unserved. An error from `admit` ends the
/// loop with it.
pub(crate) fn accept_clients(
    listener: &UnixListener,
    shutdown: &AtomicBool,
    slots: &ClientSlots,
    mut refuse: impl FnMut(UnixStream),
    mut admit: impl FnMut(UnixStream, ClientSlot) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    while !shutdown.load(Ordering::Acquire) {
        let (stream, _) = listener.accept().context("accept a local socket client")?;
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        match slots.claim() {
            Some(slot) => admit(stream, slot)?,
            None => refuse(stream),
        }
    }
    Ok(())
}

/// Wakes a server blocked accepting on `socket` once its shutdown flag is
/// set, without relying on a polling loop.
pub(crate) fn wake(socket: &Path) {
    let _ = UnixStream::connect(socket);
}

/// What a server found at its socket path before binding there.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SocketPath {
    /// Nothing, or a stale socket of this user's that is now removed.
    Free,
    /// A socket another server still answers on, left in place.
    InUse,
}

/// Clears `path` for a server to bind: removes a stale socket of this
/// user's, keeps one another server still answers on, and refuses anything
/// else there, such as a symlink or another user's socket.
pub(crate) fn clear_stale_socket(path: &Path) -> anyhow::Result<SocketPath> {
    if !is_owned(path, OwnedFile::Socket)? {
        return Ok(SocketPath::Free);
    }
    if UnixStream::connect(path).is_ok() {
        return Ok(SocketPath::InUse);
    }
    remove(path)?;
    Ok(SocketPath::Free)
}

/// The kind of file a server removes from the runtime directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnedFile {
    Socket,
    Regular,
}

/// Removes this user's `kind` of file at `path`, if there is one, and
/// refuses anything else there, such as a symlink.
pub(crate) fn remove_owned(path: &Path, kind: OwnedFile) -> anyhow::Result<()> {
    if is_owned(path, kind)? {
        remove(path)?;
    }
    Ok(())
}

/// Whether this user's `kind` of file is at `path`: false when nothing is,
/// an error when something else is.
fn is_owned(path: &Path, kind: OwnedFile) -> anyhow::Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    };
    let (expected_type, kind_name) = match kind {
        OwnedFile::Socket => (metadata.file_type().is_socket(), "socket"),
        OwnedFile::Regular => (metadata.file_type().is_file(), "regular file"),
    };
    if !expected_type || metadata.uid() != effective_uid() {
        anyhow::bail!(
            "refusing to remove {}, which is not this user's {kind_name}",
            path.display()
        );
    }
    Ok(true)
}

fn remove(path: &Path) -> anyhow::Result<()> {
    fs::remove_file(path).with_context(|| format!("remove stale {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::Duration;

    #[test]
    fn runtime_directory_and_token_are_private_and_stable() {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            SocketPaths::in_private_directory(temp.path().join("runtime"), "fixture").unwrap();
        let first = paths.load_or_create_token().unwrap();
        let second = paths.load_or_create_token().unwrap();
        assert_eq!(first, second);
        assert_eq!(
            fs::metadata(&paths.directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&paths.token).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn symlink_runtime_paths_and_public_tokens_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(SocketPaths::in_private_directory(link, "fixture").is_err());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755,
            "a rejected runtime-directory symlink must not chmod its target"
        );

        let paths = SocketPaths::in_private_directory(temp.path().join("safe"), "fixture").unwrap();
        fs::write(&paths.token, "0".repeat(64)).unwrap();
        fs::set_permissions(&paths.token, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(paths.read_token().is_err());
    }

    #[test]
    fn token_reads_are_descriptor_verified_and_strictly_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            SocketPaths::in_private_directory(temp.path().join("runtime"), "fixture").unwrap();

        let oversized = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&paths.token)
            .unwrap();
        oversized.set_len(1024 * 1024 * 1024).unwrap();
        drop(oversized);
        assert!(paths.read_token().is_err());

        fs::remove_file(&paths.token).unwrap();
        fs::write(&paths.token, "g".repeat(TOKEN_BYTES)).unwrap();
        fs::set_permissions(&paths.token, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(paths.read_token().is_err());
    }

    #[test]
    fn temporary_token_names_do_not_disclose_the_credential() {
        let token = "0123456789abcdef".repeat(4);
        let temporary = token_temporary_path(
            Path::new("/run/user/1000/termeric/domain-v1.token"),
            42,
            "fedcba9876543210",
        );
        assert_eq!(
            temporary.file_name().unwrap(),
            "domain-v1.tmp-42-fedcba9876543210"
        );
        assert!(!temporary.to_string_lossy().contains(&token[..16]));
    }

    #[test]
    fn concurrent_first_clients_publish_one_complete_token() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Arc::new(
            SocketPaths::in_private_directory(temp.path().join("runtime"), "fixture").unwrap(),
        );
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let paths = Arc::clone(&paths);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    paths.load_or_create_token().unwrap()
                })
            })
            .collect();
        let tokens: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(tokens.iter().all(|token| token == &tokens[0]));
        assert_eq!(tokens[0].len(), 64);
    }

    #[test]
    fn peers_of_this_user_are_admitted_with_their_process() {
        let (local, _peer) = UnixStream::pair().unwrap();
        assert_eq!(
            same_user_peer(&local, "fixture peer").unwrap(),
            Some(std::process::id())
        );
    }

    #[test]
    fn authentication_requires_the_exact_version_before_the_token() {
        assert_eq!(authenticate("fixture", 1, 1, "secret", "secret"), Ok(()));
        assert_eq!(
            authenticate("fixture", 1, 2, "wrong", "secret"),
            Err("protocol version 2 is incompatible with server version 1".into())
        );
        assert_eq!(
            authenticate("fixture", 1, 1, "wrong", "secret"),
            Err("fixture authentication failed".into())
        );
    }

    #[test]
    fn client_slots_are_bounded_and_given_back_on_drop() {
        let slots = ClientSlots::new(2);
        let first = slots.claim().unwrap();
        let second = slots.claim().unwrap();
        assert!(slots.claim().is_none());
        assert_eq!(slots.taken(), 2);
        drop(first);
        let third = slots.claim().unwrap();
        drop((second, third));
        assert_eq!(slots.taken(), 0);
    }

    #[test]
    fn accept_loop_refuses_beyond_its_slots_and_drops_its_waking_connection() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("fixture.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let (events, received) = std::sync::mpsc::channel();
        let server = {
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                let slots = ClientSlots::new(1);
                let refused = events.clone();
                let mut held = Vec::new();
                accept_clients(
                    &listener,
                    &shutdown,
                    &slots,
                    move |_| refused.send("refused").unwrap(),
                    |stream, slot| {
                        held.push((stream, slot));
                        events.send("admitted").unwrap();
                        Ok(())
                    },
                )
                .unwrap();
                slots.taken()
            })
        };
        let _first = UnixStream::connect(&socket).unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)),
            Ok("admitted")
        );
        let _second = UnixStream::connect(&socket).unwrap();
        assert_eq!(received.recv_timeout(Duration::from_secs(2)), Ok("refused"));
        shutdown.store(true, Ordering::Release);
        wake(&socket);
        // The loop returns holding only the first client's slot.
        assert_eq!(server.join().unwrap(), 1);
        assert!(
            received.try_recv().is_err(),
            "the waking connection was served"
        );
    }

    #[test]
    fn stale_sockets_are_removed_live_ones_kept_and_others_refused() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("fixture.sock");
        assert_eq!(clear_stale_socket(&socket).unwrap(), SocketPath::Free);

        let listener = UnixListener::bind(&socket).unwrap();
        assert_eq!(clear_stale_socket(&socket).unwrap(), SocketPath::InUse);
        drop(listener);
        assert_eq!(clear_stale_socket(&socket).unwrap(), SocketPath::Free);
        assert!(!socket.exists());

        let target = temp.path().join("target");
        fs::write(&target, "leave me").unwrap();
        symlink(&target, &socket).unwrap();
        assert!(clear_stale_socket(&socket).is_err());
        assert!(remove_owned(&socket, OwnedFile::Socket).is_err());
        assert!(remove_owned(&socket, OwnedFile::Regular).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "leave me");

        remove_owned(&target, OwnedFile::Regular).unwrap();
        assert!(!target.exists());
        remove_owned(&target, OwnedFile::Regular).unwrap();
    }
}
