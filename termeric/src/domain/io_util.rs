//! File helpers shared by the domain client, the daemon, and the directory
//! archive.

use super::protocol;
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Random bytes in a [`create_unique`] name: enough that a clash means
/// someone guessed or copied the name, not chance.
const UNIQUE_NAME_RANDOM_BYTES: usize = 16;
/// Names [`create_unique`] tries before giving up; each clash is another
/// file that appeared under a fresh random name.
const UNIQUE_NAME_ATTEMPTS: usize = 16;

/// Creates a new entry named `prefix` and random hex in `directory` with
/// `create`, which must refuse an existing one
/// ([`crate::safe_fs::create_private`] or [`private_directory`]), and
/// returns its path with what `create` made.
pub(super) fn create_unique<T>(
    directory: &Path,
    prefix: &str,
    create: fn(&Path) -> io::Result<T>,
) -> anyhow::Result<(PathBuf, T)> {
    for _ in 0..UNIQUE_NAME_ATTEMPTS {
        let name = termeric_core::hex::random(UNIQUE_NAME_RANDOM_BYTES)
            .context("generate a unique file name")?;
        let path = directory.join(format!("{prefix}{name}"));
        match create(&path) {
            Ok(created) => return Ok((path, created)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("create {}", path.display()));
            }
        }
    }
    anyhow::bail!(
        "could not create a unique {prefix}* entry in {}",
        directory.display()
    )
}

/// A new directory at `path` that only this user can use; never an
/// existing one.
pub(super) fn private_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// Lowercase SHA-256 of the first `bytes` of `file`, leaving its cursor at
/// `bytes`: the offset an upload resumes from.
pub(super) fn sha256_prefix(file: &mut File, bytes: u64) -> anyhow::Result<String> {
    file.seek(SeekFrom::Start(0))
        .context("rewind upload for prefix verification")?;
    let mut hasher = Sha256::new();
    let mut remaining = bytes;
    let mut buffer = vec![0_u8; protocol::MAX_STREAM_CHUNK_BYTES];
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded upload digest chunk fits usize");
        let read = file
            .read(&mut buffer[..wanted])
            .context("read upload prefix")?;
        if read == 0 {
            anyhow::bail!("upload ended before its resume offset");
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Whether a file is still the one first inspected: the same inode, size,
/// and modification and change times.
pub(super) fn metadata_unchanged(before: &Metadata, after: &Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_fs::create_private;

    #[test]
    fn metadata_check_detects_same_size_changes() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source.bin");
        fs::write(&source, b"same size").unwrap();
        let before = fs::metadata(&source).unwrap();
        assert!(metadata_unchanged(&before, &before));
        fs::set_permissions(&source, fs::Permissions::from_mode(0o400)).unwrap();
        let after = fs::metadata(&source).unwrap();
        assert_eq!(before.len(), after.len());
        assert!(!metadata_unchanged(&before, &after));
    }

    #[test]
    fn unique_entries_are_private_new_and_named_by_prefix() {
        let temporary = tempfile::tempdir().unwrap();
        let (first, _) = create_unique(temporary.path(), ".fixture-", create_private).unwrap();
        let (second, ()) = create_unique(temporary.path(), ".fixture-", private_directory).unwrap();
        assert_ne!(first, second);
        for (path, mode) in [(&first, 0o600), (&second, 0o700)] {
            let name = path.file_name().unwrap().to_str().unwrap();
            let random = name.strip_prefix(".fixture-").unwrap();
            assert!(termeric_core::hex::is_lower(random, 32), "{name}");
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
        // Neither kind ever opens or reuses an existing entry.
        assert_eq!(
            create_private(&first).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            private_directory(&second).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        // Other failures end at once rather than trying more names.
        let missing = temporary.path().join("missing");
        let error = create_unique(&missing, ".fixture-", create_private).unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn prefix_digest_is_standard_and_leaves_the_resume_cursor() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source.bin");
        fs::write(&source, b"abcdef").unwrap();
        let mut file = File::open(source).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(
            sha256_prefix(&mut file, 3).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(file.stream_position().unwrap(), 3);
        assert!(sha256_prefix(&mut file, 7).is_err());
    }
}
