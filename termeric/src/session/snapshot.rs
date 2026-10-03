//! Version 5: bounded, single-file publication with metadata-only listing.
//!
//! The file remains valid TOML for old binaries to reject by version. The
//! length-prefixed comment lets us avoid parsing the embedded base64 in lists.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

use super::MAX_SESSION_METADATA_BYTES;
use super::{MAX_SCROLLBACK_FILE_SIZE, Session, SnapshotContent};
use crate::safe_fs::open_regular;

const MAGIC: &str = "# termeric-snapshot-v5 metadata-bytes=";
const HEADER_LEN: usize = MAGIC.len() + 8 + 1;
const PAYLOAD_PREFIX: &[u8] = b"\nscrollback_base64 = \"";
const PAYLOAD_SUFFIX: &[u8] = b"\"\n";
const PENDING: &str = ".snapshot-v5.pending";
const LOCK: &str = ".snapshot-v5.lock";
const MAX_CONTAINER_BYTES: usize = HEADER_LEN
    + MAX_SESSION_METADATA_BYTES
    + PAYLOAD_PREFIX.len()
    + MAX_SCROLLBACK_FILE_SIZE.div_ceil(3) * 4
    + PAYLOAD_SUFFIX.len();

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Returns a pinned descriptor positioned at the embedded payload (v5).
fn open_metadata(path: &Path) -> Result<(Session, File)> {
    let mut file = open_regular(path, MAX_CONTAINER_BYTES)?;
    let mut header = Vec::with_capacity(HEADER_LEN);
    (&mut file)
        .take(HEADER_LEN as u64)
        .read_to_end(&mut header)?;
    let framed = header.starts_with(MAGIC.as_bytes());
    let mut metadata = String::new();
    if framed {
        ensure!(
            header.len() == HEADER_LEN && header[HEADER_LEN - 1] == b'\n',
            "invalid snapshot framing"
        );
        let digits = &header[MAGIC.len()..HEADER_LEN - 1];
        ensure!(
            digits.iter().all(u8::is_ascii_digit),
            "invalid snapshot header length"
        );
        let length: usize = std::str::from_utf8(digits)?.parse()?;
        ensure!(
            length <= MAX_SESSION_METADATA_BYTES,
            "snapshot metadata exceeds its byte limit"
        );
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes)?;
        metadata = String::from_utf8(bytes)?;
    } else {
        file.seek(SeekFrom::Start(0))?;
        (&mut file)
            .take(MAX_SESSION_METADATA_BYTES as u64 + 1)
            .read_to_string(&mut metadata)?;
        ensure!(
            metadata.len() <= MAX_SESSION_METADATA_BYTES,
            "snapshot metadata exceeds its byte limit"
        );
    }
    let mut session: Session = toml::from_str(&metadata)?;
    ensure!(
        session.is_compatible(),
        "Session file version {} is newer than supported version {}",
        session.version,
        Session::CURRENT_VERSION
    );
    if framed {
        ensure!(
            session.version == 5 && session.scrollback_file.is_none(),
            "invalid v5 snapshot metadata"
        );
        let content = session
            .snapshot_content
            .as_ref()
            .context("missing snapshot content metadata")?;
        ensure!(
            content.bytes <= MAX_SCROLLBACK_FILE_SIZE,
            "snapshot text exceeds its byte limit"
        );
        ensure!(
            content.sha256.len() == 64
                && content.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid snapshot content hash"
        );
        let expected = HEADER_LEN
            + metadata.len()
            + PAYLOAD_PREFIX.len()
            + content.bytes.div_ceil(3) * 4
            + PAYLOAD_SUFFIX.len();
        ensure!(
            file.metadata()?.len() == expected as u64,
            "snapshot length does not match metadata"
        );
        session.snapshot_identity = Some(digest(metadata.as_bytes()));
    } else {
        ensure!(
            session.version < 5,
            "version 5 requires framed snapshot content"
        );
        ensure!(
            session.snapshot_content.is_none(),
            "legacy snapshot has unexpected embedded content"
        );
    }
    Ok((session, file))
}

pub(super) fn read_metadata(path: &Path) -> Result<Session> {
    open_metadata(path).map(|(session, _)| session)
}

/// A selection made before a newer save is explicitly stale, never retargeted.
pub(super) fn read_selected(path: &Path, selected: &Session) -> Result<Vec<String>> {
    let (current, file) = open_metadata(path)?;
    ensure!(
        &current == selected,
        "Snapshot changed since selection; refresh the session list and retry"
    );
    read_payload(file, &current)
}

fn read_payload(mut file: File, session: &Session) -> Result<Vec<String>> {
    let content = session
        .snapshot_content
        .as_ref()
        .context("missing snapshot content metadata")?;
    let length = PAYLOAD_PREFIX.len() + content.bytes.div_ceil(3) * 4 + PAYLOAD_SUFFIX.len();
    let mut encoded = Vec::with_capacity(length);
    (&mut file)
        .take(length as u64 + 1)
        .read_to_end(&mut encoded)?;
    ensure!(
        encoded.len() == length
            && encoded.starts_with(PAYLOAD_PREFIX)
            && encoded.ends_with(PAYLOAD_SUFFIX),
        "invalid snapshot text framing"
    );
    let bytes = STANDARD.decode(&encoded[PAYLOAD_PREFIX.len()..length - PAYLOAD_SUFFIX.len()])?;
    ensure!(
        bytes.len() == content.bytes && digest(&bytes) == content.sha256,
        "snapshot text checksum mismatch"
    );
    Ok(String::from_utf8(bytes)?
        .lines()
        .map(String::from)
        .collect())
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    options
}

fn check_owned(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "snapshot staging/lock must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0
                && metadata.nlink() == 1,
            "snapshot staging/lock must be private, owned, and not hard-linked"
        );
    }
    Ok(())
}

/// A save owns the lock until scope exit, not until every possibly inherited
/// descriptor has closed. Closing only this file can leave a duplicate holding
/// the advisory lock while another thread's child is still between fork/exec.
struct WriterLock {
    file: File,
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        // Unlock while the owning descriptor is still alive. Do not unlink the
        // stable rendezvous inode or panic if cleanup fails during unwinding.
        if let Err(error) = self.file.unlock() {
            log::warn!("Could not release snapshot writer lock: {:?}", error.kind());
        }
    }
}

fn writer_lock(directory: &Path) -> Result<WriterLock> {
    let path = directory.join(LOCK);
    #[cfg(not(unix))]
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        anyhow::bail!("snapshot lock must not be a symlink");
    }
    let file = private_options().create(true).truncate(false).open(path)?;
    check_owned(&file)?;
    ensure!(
        file.metadata()?.len() == 0,
        "unexpected data in snapshot lock file"
    );
    file.try_lock().map_err(|error| {
        anyhow::anyhow!("Snapshot writer lock unavailable; retry the save: {error}")
    })?;
    Ok(WriterLock { file })
}

/// Only this reserved, bounded, recognizable file is recoverable. Never sweep
/// the directory or unlink the persistent lock inode used by other processes.
fn staging(directory: &Path) -> Result<tempfile::NamedTempFile> {
    let path = directory.join(PENDING);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let mut existing = open_regular(&path, MAX_CONTAINER_BYTES)?;
            check_owned(&existing)?;
            let mut prefix = Vec::new();
            (&mut existing)
                .take(MAGIC.len() as u64)
                .read_to_end(&mut prefix)?;
            ensure!(
                MAGIC.as_bytes().starts_with(&prefix),
                "unrecognized reserved snapshot staging file; preserve it and resolve manually"
            );
            fs::remove_file(&path)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let file = private_options().create_new(true).open(&path)?;
    Ok(tempfile::NamedTempFile::from_parts(
        file,
        tempfile::TempPath::from_path(path),
    ))
}

/// One exclusive writer and one recoverable pending file per session. `before_commit`
/// is also the deterministic fault/publication-boundary seam used by tests.
fn save_with_hook(
    session: &Session,
    directory: &Path,
    lines: &[String],
    before_commit: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    fs::create_dir_all(directory)?;
    let _lock = writer_lock(directory)?;
    let destination = directory.join("session.toml");
    let mut persisted = session.clone();
    // Preserve the original creation time without parsing embedded text.
    match read_metadata(&destination) {
        Ok(existing) => {
            ensure!(
                existing.id == session.id,
                "snapshot destination belongs to a different session"
            );
            persisted.created_at = existing.created_at;
        }
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) => {}
        Err(error) => {
            return Err(error.context("existing snapshot is unreadable; refusing to overwrite it"));
        }
    }
    let mut bytes = Vec::new();
    let mut truncated = false;
    for line in lines {
        if bytes.len().saturating_add(line.len()).saturating_add(1) > MAX_SCROLLBACK_FILE_SIZE {
            truncated = true;
            break;
        }
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    persisted.version = 5;
    persisted.scrollback_file = None;
    persisted.snapshot_content = Some(SnapshotContent {
        bytes: bytes.len(),
        sha256: digest(&bytes),
    });
    let metadata = toml::to_string_pretty(&persisted)?;
    ensure!(
        metadata.len() <= MAX_SESSION_METADATA_BYTES,
        "session metadata exceeds its {MAX_SESSION_METADATA_BYTES}-byte limit"
    );
    let mut pending = staging(directory)?;
    {
        let mut writer = io::BufWriter::new(pending.as_file_mut());
        writeln!(writer, "{MAGIC}{:08}", metadata.len())?;
        writer.write_all(metadata.as_bytes())?;
        writer.write_all(PAYLOAD_PREFIX)?;
        let mut encoder = base64::write::EncoderWriter::new(&mut writer, &STANDARD);
        encoder.write_all(&bytes)?;
        encoder.finish()?;
        drop(encoder);
        writer.write_all(PAYLOAD_SUFFIX)?;
        writer.flush()?;
    }
    pending.as_file().sync_all()?;
    before_commit()?;
    pending.persist(&destination).map_err(|error| error.error)?;
    #[cfg(unix)]
    if let Err(error) = File::open(directory).and_then(|file| file.sync_all()) {
        log::warn!("Snapshot published but directory sync failed: {error}");
    }
    Ok(truncated)
}

pub(super) fn save(session: &Session, directory: &Path, lines: &[String]) -> Result<bool> {
    save_with_hook(session, directory, lines, || Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Session {
        Session::with_id("fixture".into(), "old".into(), Some("/old".into()))
    }

    #[test]
    fn staging_failure_preserves_complete_old_generation_and_cleans_only_owned_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        let mut session = fixture();
        save(&session, dir.path(), &["old text".into()]).unwrap();
        let old = read_metadata(&path).unwrap();
        let original = fs::read(&path).unwrap();
        fs::write(dir.path().join("unrelated"), "sentinel").unwrap();
        session.title = "new".into();
        session.working_directory = Some("/new".into());
        for _ in 0..10 {
            let result = save_with_hook(&session, dir.path(), &["new text".into()], || {
                assert_eq!(read_selected(&path, &old).unwrap(), ["old text"]);
                assert!(dir.path().join(PENDING).is_file());
                anyhow::bail!("injected failure after fully staged text, before publication")
            });
            assert!(result.unwrap_err().to_string().contains("injected failure"));
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
            assert_eq!(
                fs::read_to_string(dir.path().join("unrelated")).unwrap(),
                "sentinel"
            );
        }
    }

    #[test]
    fn pinned_reader_survives_publication_and_stale_selection_requires_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        let mut session = fixture();
        save(&session, dir.path(), &["old text".into()]).unwrap();
        let (old, pinned) = open_metadata(&path).unwrap();
        session.title = "new".into();
        session.last_command = Some("new command".into());
        save_with_hook(&session, dir.path(), &["new text".into()], || {
            assert_eq!(read_selected(&path, &old).unwrap(), ["old text"]);
            Ok(())
        })
        .unwrap();
        assert_eq!(read_payload(pinned, &old).unwrap(), ["old text"]);
        assert!(
            read_selected(&path, &old)
                .unwrap_err()
                .to_string()
                .contains("changed since selection")
        );
        let new = read_metadata(&path).unwrap();
        assert_eq!(new.title, "new");
        assert_eq!(new.last_command.as_deref(), Some("new command"));
        assert_eq!(read_selected(&path, &new).unwrap(), ["new text"]);
        let whole_file: Session = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            whole_file.version, 5,
            "older TOML readers can reject by version"
        );
    }

    #[test]
    fn interrupted_staging_is_recovered_with_constant_file_count() {
        let dir = tempfile::tempdir().unwrap();
        let session = fixture();
        save(&session, dir.path(), &["old text".into()]).unwrap();
        for prefix_length in 0..=MAGIC.len() {
            // Models a crash at every possible header-write boundary. The
            // lock descriptor drops, but the staging file is intentionally kept.
            {
                let _lock = writer_lock(dir.path()).unwrap();
                let mut pending = staging(dir.path()).unwrap();
                pending
                    .write_all(&MAGIC.as_bytes()[..prefix_length])
                    .unwrap();
                pending.keep().unwrap();
            }
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
            save(&session, dir.path(), &["new text".into()]).unwrap();
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        }
        // A crash after a complete staged container has the same one-slot bound.
        let complete = fs::read(dir.path().join("session.toml")).unwrap();
        let mut pending = staging(dir.path()).unwrap();
        pending.write_all(&complete).unwrap();
        pending.keep().unwrap();
        save(&session, dir.path(), &[]).unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn unknown_reserved_staging_and_future_versions_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let session = fixture();
        let mut pending = staging(dir.path()).unwrap();
        pending.write_all(b"not a Termeric staging file").unwrap();
        pending.keep().unwrap();
        assert!(save(&session, dir.path(), &[]).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join(PENDING)).unwrap(),
            "not a Termeric staging file"
        );
        let future_dir = tempfile::tempdir().unwrap();
        let mut future = session.clone();
        future.version = 999;
        let future_bytes = toml::to_string_pretty(&future).unwrap();
        fs::write(future_dir.path().join("session.toml"), &future_bytes).unwrap();
        assert!(save(&session, future_dir.path(), &[]).is_err());
        assert_eq!(
            fs::read_to_string(future_dir.path().join("session.toml")).unwrap(),
            future_bytes
        );
    }

    #[cfg(unix)]
    #[test]
    fn reserved_paths_never_follow_symlinks_and_committed_file_is_private() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        for name in [LOCK, PENDING, "session.toml"] {
            let dir = tempfile::tempdir().unwrap();
            let sentinel = dir.path().join("sentinel");
            fs::write(&sentinel, "untouched").unwrap();
            symlink(&sentinel, dir.path().join(name)).unwrap();
            assert!(save(&fixture(), dir.path(), &[]).is_err());
            assert_eq!(fs::read_to_string(sentinel).unwrap(), "untouched");
            assert!(
                fs::symlink_metadata(dir.path().join(name))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        let dir = tempfile::tempdir().unwrap();
        save(&fixture(), dir.path(), &[]).unwrap();
        for name in [LOCK, "session.toml"] {
            assert_eq!(
                fs::metadata(dir.path().join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn production_transaction_enforces_exact_whole_line_cap_and_metadata_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        let mut session = fixture();
        let line = "x".repeat(8_191);
        let lines = vec![line; MAX_SCROLLBACK_FILE_SIZE / 8_192 + 1];
        assert!(save(&session, dir.path(), &lines).unwrap());
        let selected = read_metadata(&path).unwrap();
        assert_eq!(
            selected.snapshot_content.as_ref().unwrap().bytes,
            MAX_SCROLLBACK_FILE_SIZE
        );
        assert_eq!(read_selected(&path, &selected).unwrap(), lines[..640]);
        assert!(fs::metadata(&path).unwrap().len() <= MAX_CONTAINER_BYTES as u64);
        let old = fs::read(&path).unwrap();
        session.title = "x".repeat(MAX_SESSION_METADATA_BYTES);
        assert!(save(&session, dir.path(), &[]).is_err());
        assert_eq!(fs::read(&path).unwrap(), old);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn metadata_listing_does_not_parse_payload_but_restore_validates_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        save(&fixture(), dir.path(), &["界\0quoted \"text\"".into()]).unwrap();
        let (selected, mut file) = open_metadata(&path).unwrap();
        let payload_start = file.stream_position().unwrap() as usize;
        assert!(payload_start <= HEADER_LEN + MAX_SESSION_METADATA_BYTES);
        assert_eq!(
            read_selected(&path, &selected).unwrap(),
            ["界\0quoted \"text\""]
        );
        let original = fs::read(&path).unwrap();
        for (offset, byte) in [
            (payload_start + PAYLOAD_PREFIX.len(), b'!'),
            (payload_start, b'x'),
            (original.len() - 1, b'x'),
        ] {
            let mut corrupt = original.clone();
            corrupt[offset] = byte;
            fs::write(&path, corrupt).unwrap();
            assert_eq!(read_metadata(&path).unwrap(), selected);
            assert!(read_selected(&path, &selected).is_err());
        }
        // A valid-base64 mutation is detected by the content checksum.
        let mut corrupt = original.clone();
        corrupt[payload_start + PAYLOAD_PREFIX.len()] = b'A';
        fs::write(&path, corrupt).unwrap();
        assert!(
            read_selected(&path, &selected)
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
        for length in [0, MAGIC.len(), HEADER_LEN, original.len() - 1] {
            fs::write(&path, &original[..length]).unwrap();
            assert!(read_metadata(&path).is_err());
        }
    }

    #[test]
    fn legacy_read_is_non_mutating_and_save_upgrades_without_deleting_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        for version in 1..=4 {
            let mut legacy = fixture();
            legacy.version = version;
            legacy.scrollback_file = Some("scrollback.txt".into());
            let serialized = toml::to_string_pretty(&legacy).unwrap();
            fs::write(&path, &serialized).unwrap();
            fs::write(dir.path().join("scrollback.txt"), "legacy text\n").unwrap();
            let loaded = read_metadata(&path).unwrap();
            assert_eq!(loaded, legacy);
            assert_eq!(fs::read_to_string(&path).unwrap(), serialized);
            save(&loaded, dir.path(), &["new text".into()]).unwrap();
            let updated = read_metadata(&path).unwrap();
            assert_eq!(updated.version, 5);
            assert_eq!(updated.created_at, loaded.created_at);
            assert_eq!(read_selected(&path, &updated).unwrap(), ["new text"]);
            assert_eq!(
                fs::read_to_string(dir.path().join("scrollback.txt")).unwrap(),
                "legacy text\n"
            );
        }
    }

    #[test]
    fn competing_writers_and_continuous_reader_never_mix_generations() {
        let dir = tempfile::tempdir().unwrap();
        let mut initial = fixture();
        initial.title = "initial".into();
        save(&initial, dir.path(), &["initial".into()]).unwrap();
        std::thread::scope(|scope| {
            let handles: Vec<_> = ["a", "b"]
                .into_iter()
                .map(|label| {
                    let directory = dir.path();
                    scope.spawn(move || {
                        for index in 0..40 {
                            let mut session = fixture();
                            session.title = format!("{label}-{index}");
                            if let Err(error) = save(&session, directory, &[session.title.clone()])
                            {
                                assert!(error.to_string().contains("writer lock unavailable"));
                            }
                        }
                    })
                })
                .collect();
            for _ in 0..100 {
                let (session, file) = open_metadata(&dir.path().join("session.toml")).unwrap();
                assert_eq!(read_payload(file, &session).unwrap(), [session.title]);
            }
            for handle in handles {
                handle.join().unwrap();
            }
        });
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn lock_child_fixture() {
        let Some(path) = std::env::var_os("TERMERIC_SNAPSHOT_LOCK_FIXTURE") else {
            return;
        };
        assert!(
            save(&fixture(), Path::new(&path), &[])
                .unwrap_err()
                .to_string()
                .contains("writer lock unavailable")
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn writer_scope_ends_lock_even_with_duplicate_descriptor_alive() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let lock = writer_lock(dir.path()).unwrap();
        let inode = fs::metadata(dir.path().join(LOCK)).unwrap().ino();
        let duplicate = lock.file.try_clone().unwrap();
        assert!(
            writer_lock(dir.path()).is_err(),
            "active writer must exclude others"
        );
        drop(lock);
        let next = writer_lock(dir.path())
            .expect("scope end must release the writer lock even while a duplicate exists");
        drop(duplicate);
        assert!(
            writer_lock(dir.path()).is_err(),
            "dropping the old duplicate must not release the next writer's lock"
        );
        drop(next);
        let _next = writer_lock(dir.path()).unwrap();
        assert_eq!(fs::metadata(dir.path().join(LOCK)).unwrap().ino(), inode);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn writer_scope_unlocks_after_error_and_unwind_with_duplicate_alive() {
        for panic in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut duplicate = None;
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                    let lock = writer_lock(dir.path())?;
                    duplicate = Some(lock.file.try_clone()?);
                    assert!(writer_lock(dir.path()).is_err());
                    if panic {
                        panic!("injected writer-scope unwind");
                    }
                    anyhow::bail!("injected writer-scope error")
                }));
            match (panic, result) {
                (false, Ok(error)) => assert_eq!(
                    error.unwrap_err().to_string(),
                    "injected writer-scope error"
                ),
                (true, Err(error)) => assert_eq!(
                    error.downcast_ref::<&str>(),
                    Some(&"injected writer-scope unwind")
                ),
                _ => panic!("unexpected scope outcome"),
            }
            assert!(duplicate.is_some());
            let next = writer_lock(dir.path()).expect("failed scope must release its lock");
            drop(duplicate);
            assert!(writer_lock(dir.path()).is_err());
            drop(next);
            save(&fixture(), dir.path(), &[]).unwrap();
        }
    }

    #[test]
    fn failed_save_hooks_release_lock_and_preserve_the_previous_snapshot() {
        for panic in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let session = fixture();
            save(&session, dir.path(), &["previous".into()]).unwrap();
            let original = fs::read(dir.path().join("session.toml")).unwrap();
            let result = std::panic::catch_unwind(|| {
                save_with_hook(&session, dir.path(), &["unpublished".into()], || {
                    assert!(writer_lock(dir.path()).is_err());
                    if panic {
                        panic!("injected save-hook unwind");
                    }
                    anyhow::bail!("injected save-hook error")
                })
            });
            match (panic, result) {
                (false, Ok(error)) => {
                    assert_eq!(error.unwrap_err().to_string(), "injected save-hook error")
                }
                (true, Err(error)) => assert_eq!(
                    error.downcast_ref::<&str>(),
                    Some(&"injected save-hook unwind")
                ),
                _ => panic!("unexpected save-hook outcome"),
            }
            assert_eq!(fs::read(dir.path().join("session.toml")).unwrap(), original);
            assert!(!dir.path().join(PENDING).exists());
            save(&session, dir.path(), &["next".into()]).unwrap();
            let selected = read_metadata(&dir.path().join("session.toml")).unwrap();
            assert_eq!(
                read_selected(&dir.path().join("session.toml"), &selected).unwrap(),
                ["next"]
            );
        }
    }

    #[test]
    fn writer_lock_is_nonblocking_across_processes() {
        let dir = tempfile::tempdir().unwrap();
        let lock = writer_lock(dir.path()).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "session::snapshot::tests::lock_child_fixture"])
            .env_clear()
            .env("TERMERIC_SNAPSHOT_LOCK_FIXTURE", dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        drop(lock);
        save(&fixture(), dir.path(), &[]).unwrap();
    }
}
