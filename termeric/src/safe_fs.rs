//! Hardened access to Termeric's own files: reads never follow a symlink,
//! block on a FIFO, or read past a limit, writes replace a file atomically
//! through a private staging file beside it, publishing a staged entry
//! never replaces another, private files are only ever created new, and
//! private directories are never reached through a symlink.

use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// How [`atomic_replace`]'s staging files are named: this prefix and a
/// random suffix. One left behind was interrupted before it was published.
pub(crate) const STAGING_PREFIX: &str = ".termeric-";

/// The whole of `path` as UTF-8, opened through [`open_regular`] and capped at
/// `limit` bytes even if the file grows while it is read.
pub(crate) fn read_text(path: &Path, limit: usize) -> io::Result<String> {
    read_capped_text(open_regular(path, limit)?, limit)
}

/// [`read_text`] for a file the person owns, such as an agent's
/// `settings.json`, which a dotfile manager may have made a symbolic link:
/// the link is resolved, and only the regular file it names is read.
pub(crate) fn read_linked_text(path: &Path, limit: usize) -> io::Result<String> {
    read_text(&fs::canonicalize(path)?, limit)
}

/// The whole of `path`, opened through [`open_regular`] and capped at `limit`
/// bytes even if the file grows while it is read.
pub(crate) fn read_bytes(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_regular(path, limit)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(size_error(limit));
    }
    Ok(bytes)
}

/// Open `path` for reading only if it is a regular file of at most `limit`
/// bytes, without following a symlink or waiting on a FIFO.
pub(crate) fn open_regular(path: &Path, limit: usize) -> io::Result<fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("input must be a regular file"));
    }
    if metadata.len() > limit as u64 {
        return Err(size_error(limit));
    }
    // The read itself is capped: metadata alone does not bound a growing file.
    Ok(file)
}

fn read_capped_text(reader: impl Read, limit: usize) -> io::Result<String> {
    let maximum = (limit as u64)
        .checked_add(1)
        .ok_or_else(|| size_error(limit))?;
    let mut text = String::new();
    reader.take(maximum).read_to_string(&mut text)?;
    if text.len() > limit {
        return Err(size_error(limit));
    }
    Ok(text)
}

fn size_error(limit: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("file exceeds its {limit}-byte limit"),
    )
}

/// Create `path` as a new file only this user can read and write (mode
/// 0600 exactly, whatever the umask), opened for both. Anything already at
/// `path`, even a symbolic link, fails with [`io::ErrorKind::AlreadyExists`]
/// and is left alone, so the file is never opened through a link or reused.
pub(crate) fn create_private(path: &Path) -> io::Result<fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    // The umask can only narrow the creation mode; set it exactly, since
    // readers such as a resumed upload or the token check expect 0600.
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// Replace `path` with what `write` produces: write a private (0600)
/// staging file beside it, sync it, and rename it over `path`. A symlink at
/// `path` is replaced, never written through. Until the rename nothing at
/// `path` changes, so a failed write leaves the old file whole.
pub(crate) fn atomic_replace<T>(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> io::Result<T>,
) -> io::Result<T> {
    atomic_replace_keeping(path, None, write)
}

/// [`atomic_replace`] for a file the person owns and may have given other
/// permissions, such as `config.toml`: the replacement takes `permissions`
/// (the old file's), or stays private when there are none.
pub(crate) fn atomic_replace_keeping<T>(
    path: &Path,
    permissions: Option<fs::Permissions>,
    write: impl FnOnce(&mut dyn Write) -> io::Result<T>,
) -> io::Result<T> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("replaced file has no parent directory"))?;
    let mut staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempfile_in(directory)?;
    if let Some(permissions) = permissions {
        staging.as_file().set_permissions(permissions)?;
    }
    let result = {
        let mut writer = BufWriter::new(staging.as_file_mut());
        let result = write(&mut writer)?;
        writer.flush()?;
        result
    };
    staging.as_file().sync_all()?;
    staging.persist(path).map_err(|error| error.error)?;
    // The new file is already in place, so failing here would wrongly say
    // the old one survived. Directory durability is reported, not returned.
    if let Err(error) = sync_directory(directory) {
        log::warn!(
            "{} was replaced, but its directory could not be synced: {error}",
            path.display()
        );
    }
    Ok(result)
}

/// Rename `from` to `to` unless something, even an empty directory, is
/// already at `to`; that fails with [`io::ErrorKind::AlreadyExists`].
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both arguments are NUL-terminated C strings that outlive the
    // call. RENAME_NOREPLACE makes the no-overwrite check part of the rename.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Sync `directory` itself, so the entries created, renamed, or removed in
/// it survive a crash.
pub(crate) fn sync_directory(directory: &Path) -> io::Result<()> {
    fs::File::open(directory)?.sync_all()
}

/// Make `path` a directory only this user can use (mode 0700): create it,
/// after any missing parents, or tighten an existing one. A symbolic link,
/// anything but a directory, or a directory another user owns is refused
/// and left as it was.
pub(crate) fn ensure_private_directory(path: &Path) -> io::Result<()> {
    let directory = match open_directory(path) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            match fs::DirBuilder::new().mode(0o700).create(path) {
                // One created by someone else meanwhile is checked like any
                // existing directory.
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            open_directory(path)?
        }
        Err(error) => return Err(error),
    };
    // Checked and changed through the descriptor, so a directory swapped in
    // after the open is never the one changed.
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.uid() != crate::local_socket::effective_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a directory this user owns", path.display()),
        ));
    }
    directory.set_permissions(fs::Permissions::from_mode(0o700))?;
    if directory.metadata()?.permissions().mode() & 0o777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} could not be made private (mode 0700)", path.display()),
        ));
    }
    Ok(())
}

fn open_directory(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn partial_staging_and_publication_failures_preserve_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot");
        fs::write(&path, "previous complete snapshot").unwrap();
        let error = atomic_replace(&path, |writer| -> io::Result<()> {
            writer.write_all(&vec![b'x'; 32 * 1024])?;
            Err(io::Error::other("injected disk failure"))
        })
        .unwrap_err();
        assert!(error.to_string().contains("disk failure"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "previous complete snapshot"
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        let blocked = dir.path().join("directory");
        fs::create_dir(&blocked).unwrap();
        assert!(atomic_replace(&blocked, |writer| writer.write_all(b"new")).is_err());
        assert!(blocked.is_dir());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn concurrent_replacements_never_expose_mixed_or_partial_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot");
        fs::write(&path, "initial").unwrap();
        let finished = Arc::new(AtomicUsize::new(0));
        let workers: Vec<_> = b"abc"
            .iter()
            .copied()
            .map(|byte| {
                let path = path.clone();
                let finished = finished.clone();
                std::thread::spawn(move || {
                    let result = (0..8).try_for_each(|_| {
                        atomic_replace(&path, |writer| {
                            writer.write_all(&vec![byte; 8192])?;
                            std::thread::yield_now();
                            writer.write_all(&vec![byte; 8192])
                        })
                    });
                    finished.fetch_add(1, Ordering::Release);
                    result
                })
            })
            .collect();
        let mut invalid = false;
        loop {
            let text = read_text(&path, 16384).unwrap();
            invalid |= text != "initial"
                && !(text.len() == 16384
                    && b"abc"
                        .iter()
                        .any(|byte| text.bytes().all(|candidate| candidate == *byte)));
            if finished.load(Ordering::Acquire) == 3 {
                break;
            }
            std::thread::yield_now();
        }
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
        assert!(!invalid, "reader saw a partial or mixed replacement");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn reads_enforce_exact_byte_limits_type_and_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot");
        fs::write(&path, "界".repeat(100)).unwrap();
        assert_eq!(read_text(&path, 300).unwrap(), "界".repeat(100));
        assert!(
            read_text(&path, 299)
                .unwrap_err()
                .to_string()
                .contains("299-byte limit")
        );
        assert!(read_text(dir.path(), 300).is_err());
        fs::write(&path, [0xff]).unwrap();
        assert!(read_text(&path, 300).is_err());
        let sparse = fs::File::create(&path).unwrap();
        sparse.set_len(1024 * 1024 + 1).unwrap();
        assert!(read_text(&path, 1024 * 1024).is_err());
    }

    #[test]
    fn growing_source_cannot_read_past_the_limit_plus_probe_byte() {
        struct Growing {
            read: Arc<AtomicUsize>,
        }
        impl Read for Growing {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                output.fill(b'x');
                self.read.fetch_add(output.len(), Ordering::Relaxed);
                Ok(output.len())
            }
        }
        let read = Arc::new(AtomicUsize::new(0));
        assert!(read_capped_text(Growing { read: read.clone() }, 4096).is_err());
        assert_eq!(read.load(Ordering::Relaxed), 4097);
    }

    #[test]
    fn reads_reject_symlinks_and_replacements_create_private_regular_files() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "unchanged").unwrap();
        let path = dir.path().join("snapshot");
        symlink(&target, &path).unwrap();
        assert!(read_text(&path, 1024).is_err());
        atomic_replace(&path, |writer| writer.write_all(b"new snapshot")).unwrap();
        assert!(fs::symlink_metadata(&path).unwrap().is_file());
        assert_eq!(fs::read_to_string(&target).unwrap(), "unchanged");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(read_text(&path, 1024).unwrap(), "new snapshot");
    }

    #[test]
    fn linked_reads_follow_links_to_regular_files_only() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "linked").unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();
        assert_eq!(read_linked_text(&link, 1024).unwrap(), "linked");
        assert_eq!(
            read_linked_text(&link, 3).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let dangling = dir.path().join("dangling");
        symlink(dir.path().join("gone"), &dangling).unwrap();
        assert_eq!(
            read_linked_text(&dangling, 1024).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let linked_directory = dir.path().join("directory");
        symlink(dir.path(), &linked_directory).unwrap();
        assert!(read_linked_text(&linked_directory, 1024).is_err());
    }

    #[test]
    fn fifo_read_is_rejected_without_waiting_for_a_writer() {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fifo");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a live C string within this private fixture directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let source = path.clone();
        let (send, receive) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(read_text(&source, 1024)).unwrap();
        });
        let mut rescued = false;
        let result = match receive.recv_timeout(Duration::from_millis(300)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                rescued = true;
                let _peer = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&path)
                    .unwrap();
                receive.recv_timeout(Duration::from_secs(2)).unwrap()
            }
            Err(error) => panic!("FIFO worker failed: {error}"),
        };
        worker.join().unwrap();
        assert!(!rescued, "the read waited for a FIFO writer");
        assert!(result.unwrap_err().to_string().contains("regular file"));
        assert!(path.exists());
    }

    #[test]
    fn private_files_are_only_ever_created_new_and_never_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private");
        let mut file = create_private(&path).unwrap();
        file.write_all(b"secret").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            create_private(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "secret");

        let target = dir.path().join("target");
        fs::write(&target, "untouched").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let dangling = dir.path().join("dangling");
        let missing = dir.path().join("missing");
        std::os::unix::fs::symlink(&missing, &dangling).unwrap();
        for linked in [&link, &dangling] {
            assert_eq!(
                create_private(linked).unwrap_err().kind(),
                io::ErrorKind::AlreadyExists
            );
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
        assert!(!missing.exists());
    }

    #[test]
    fn private_directories_are_created_or_tightened_but_never_reached_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let mode = |path: &Path| fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777;

        let created = dir.path().join("parent").join("private");
        ensure_private_directory(&created).unwrap();
        assert_eq!(mode(&created), 0o700);
        ensure_private_directory(&created).unwrap();
        assert_eq!(mode(&created), 0o700);

        let open = dir.path().join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_directory(&open).unwrap();
        assert_eq!(mode(&open), 0o700);

        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(ensure_private_directory(&link).is_err());
        assert_eq!(mode(&target), 0o755);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let file = dir.path().join("file");
        fs::write(&file, "kept").unwrap();
        assert!(ensure_private_directory(&file).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "kept");
    }

    #[test]
    fn rename_no_replace_never_replaces_even_an_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged");
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("file"), "staged").unwrap();
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "existing").unwrap();
        for occupied in [&empty, &file] {
            let error = rename_no_replace(&staged, occupied).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        }
        assert!(staged.join("file").exists());
        assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
        assert_eq!(fs::read_to_string(&file).unwrap(), "existing");
        let published = dir.path().join("published");
        rename_no_replace(&staged, &published).unwrap();
        assert_eq!(
            fs::read_to_string(published.join("file")).unwrap(),
            "staged"
        );
        assert!(!staged.exists());
    }
}
