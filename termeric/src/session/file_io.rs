//! Per-file legacy snapshot safety. This is not a cross-file transaction.

use std::fs::{self, OpenOptions};
use std::io::{self, Read};
#[cfg(test)]
use std::io::{BufWriter, Write};
use std::path::Path;

pub(crate) const MAX_SESSION_METADATA_BYTES: usize = 1024 * 1024;

pub(super) fn read_text(path: &Path, limit: usize) -> io::Result<String> {
    read_capped_text(open_regular(path, limit)?, limit)
}

pub(super) fn open_regular(path: &Path, limit: usize) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(not(unix))]
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other("snapshot input must not be a symlink"));
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("snapshot input must be a regular file"));
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
        format!("snapshot file exceeds its {limit}-byte limit"),
    )
}

/// Write only to an exclusive same-directory staging file. Persistence replaces
/// a final symlink itself, never follows it to truncate the link's target.
#[cfg(test)]
pub(super) fn atomic_replace<T>(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> io::Result<T>,
) -> io::Result<T> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("snapshot path has no parent"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".snapshot-")
        .tempfile_in(directory)?;
    let result = {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        let result = write(&mut writer)?;
        writer.flush()?;
        result
    };
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    // A failure after publication cannot truthfully mean the old file survived.
    // Directory durability is best effort and separately logged, as elsewhere.
    #[cfg(unix)]
    if let Err(error) = fs::File::open(directory).and_then(|file| file.sync_all()) {
        log::warn!("Snapshot file was replaced but directory sync failed: {error}");
    }
    Ok(result)
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
        sparse
            .set_len(MAX_SESSION_METADATA_BYTES as u64 + 1)
            .unwrap();
        assert!(read_text(&path, MAX_SESSION_METADATA_BYTES).is_err());
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

    #[cfg(unix)]
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

    #[cfg(target_os = "linux")]
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
        assert!(!rescued, "snapshot read waited for a FIFO writer");
        assert!(result.unwrap_err().to_string().contains("regular file"));
        assert!(path.exists());
    }
}
