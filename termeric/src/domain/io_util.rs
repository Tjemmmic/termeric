//! File helpers shared by the domain client, the daemon, and the directory
//! archive.

use super::protocol;
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;

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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

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
