//! Bounded, deterministic directory-transfer archive.

use anyhow::Context;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_DIRECTORY_ARCHIVE_BYTES: u64 = super::protocol::MAX_FILE_UPLOAD_BYTES;
pub const MAX_DIRECTORY_ENTRIES: u32 = 4_096;
pub const MAX_DIRECTORY_DEPTH: usize = 64;
pub const MAX_RELATIVE_PATH_BYTES: usize = 4_096;

const MAGIC: &[u8; 8] = b"TRMDIR01";
const ENTRY_DIRECTORY: u8 = 1;
const ENTRY_FILE: u8 = 2;
const ENTRY_END: u8 = 0xff;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub struct DirectoryArchive {
    pub file: File,
    path: Option<PathBuf>,
    pub entries: u32,
    pub bytes: u64,
}

impl Drop for DirectoryArchive {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
pub fn build_directory_archive(source: &Path) -> anyhow::Result<DirectoryArchive> {
    build_directory_archive_inner(source, None, None)
}

pub fn build_directory_archive_with_control(
    source: &Path,
    cancelled: &AtomicBool,
    expected_identity: Option<(u64, u64)>,
) -> anyhow::Result<DirectoryArchive> {
    build_directory_archive_inner(source, Some(cancelled), expected_identity)
}

fn build_directory_archive_inner(
    source: &Path,
    cancelled: Option<&AtomicBool>,
    expected_identity: Option<(u64, u64)>,
) -> anyhow::Result<DirectoryArchive> {
    check_cancelled(cancelled)?;
    let source_metadata = fs::symlink_metadata(source)
        .with_context(|| format!("inspect directory upload source {}", source.display()))?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        anyhow::bail!("directory upload source must be one real directory, not a symlink");
    }
    if expected_identity
        .is_some_and(|identity| identity != (source_metadata.dev(), source_metadata.ino()))
    {
        anyhow::bail!("directory upload source identity changed after confirmation");
    }
    let source_device = source_metadata.dev();
    let mut temporary = anonymous_archive_file().map(|file| DirectoryArchive {
        file,
        path: None,
        entries: 0,
        bytes: 0,
    });
    let archive_id = crate::session::generate_session_id();
    for attempt in 0..16_u8 {
        if temporary.is_some() {
            break;
        }
        let path = std::env::temp_dir().join(format!(
            ".termeric-directory-archive-{archive_id}-{attempt}"
        ));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => {
                fs::remove_file(&path).context("unlink private directory upload archive")?;
                temporary = Some(DirectoryArchive {
                    file,
                    path: None,
                    entries: 0,
                    bytes: 0,
                });
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create private directory upload archive"),
        }
    }
    let mut archive = temporary.context("could not allocate directory upload archive")?;
    write_bounded(&mut archive, MAGIC)?;
    append_directory(
        &mut archive,
        source,
        Path::new(""),
        source_device,
        0,
        cancelled,
    )?;
    write_bounded(&mut archive, &[ENTRY_END])?;
    archive
        .file
        .flush()
        .context("flush directory upload archive")?;
    archive
        .file
        .sync_all()
        .context("sync directory upload archive")?;
    archive
        .file
        .seek(SeekFrom::Start(0))
        .context("rewind directory upload archive")?;
    Ok(archive)
}

fn anonymous_archive_file() -> Option<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_TMPFILE)
        .open(std::env::temp_dir())
        .ok()
}

fn append_directory(
    archive: &mut DirectoryArchive,
    absolute: &Path,
    relative: &Path,
    source_device: u64,
    depth: usize,
    cancelled: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    check_cancelled(cancelled)?;
    if depth > MAX_DIRECTORY_DEPTH {
        anyhow::bail!("directory upload exceeds the {MAX_DIRECTORY_DEPTH}-level depth limit");
    }
    let before = fs::symlink_metadata(absolute)
        .with_context(|| format!("revalidate upload directory {}", absolute.display()))?;
    if before.file_type().is_symlink() || !before.is_dir() || before.dev() != source_device {
        anyhow::bail!("directory upload tree changed during local preparation");
    }
    let mut children = fs::read_dir(absolute)
        .with_context(|| format!("read directory upload source {}", absolute.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    children.sort_by(|left, right| {
        left.file_name()
            .as_bytes()
            .cmp(right.file_name().as_bytes())
    });
    for child in children {
        check_cancelled(cancelled)?;
        let name = child.file_name();
        let name = name
            .to_str()
            .context("directory upload paths must be valid UTF-8")?;
        if !is_safe_component(name) {
            anyhow::bail!("directory upload contains an unsafe path component");
        }
        let child_relative = relative.join(name);
        validate_relative_path(&child_relative)?;
        let child_absolute = absolute.join(name);
        let metadata = fs::symlink_metadata(&child_absolute)
            .with_context(|| format!("inspect directory entry {}", child_absolute.display()))?;
        if metadata.dev() != source_device {
            anyhow::bail!("directory upload does not cross filesystem mount boundaries");
        }
        if metadata.file_type().is_symlink() {
            anyhow::bail!("directory upload rejects symbolic links");
        }
        if metadata.is_dir() {
            append_header(archive, ENTRY_DIRECTORY, &child_relative, 0o700, 0)?;
            append_directory(
                archive,
                &child_absolute,
                &child_relative,
                source_device,
                depth + 1,
                cancelled,
            )?;
        } else if metadata.is_file() {
            let mode = if metadata.permissions().mode() & 0o111 != 0 {
                0o700
            } else {
                0o600
            };
            append_header(archive, ENTRY_FILE, &child_relative, mode, metadata.len())?;
            append_file(archive, &child_absolute, &metadata, cancelled)?;
        } else {
            anyhow::bail!("directory upload rejects devices, sockets, and FIFOs");
        }
    }
    let after = fs::symlink_metadata(absolute)
        .with_context(|| format!("revalidate upload directory {}", absolute.display()))?;
    if !metadata_unchanged(&before, &after) {
        anyhow::bail!("directory upload tree changed during local preparation");
    }
    Ok(())
}

fn append_header(
    archive: &mut DirectoryArchive,
    kind: u8,
    relative: &Path,
    mode: u16,
    size: u64,
) -> anyhow::Result<()> {
    archive.entries = archive.entries.saturating_add(1);
    if archive.entries > MAX_DIRECTORY_ENTRIES {
        anyhow::bail!("directory upload exceeds the {MAX_DIRECTORY_ENTRIES}-entry limit");
    }
    let path = relative
        .to_str()
        .context("directory upload paths must be valid UTF-8")?
        .as_bytes();
    let path_len = u32::try_from(path.len()).context("directory archive path is too long")?;
    write_bounded(archive, &[kind])?;
    write_bounded(archive, &path_len.to_be_bytes())?;
    write_bounded(archive, &mode.to_be_bytes())?;
    write_bounded(archive, &size.to_be_bytes())?;
    write_bounded(archive, path)
}

fn append_file(
    archive: &mut DirectoryArchive,
    path: &Path,
    expected: &fs::Metadata,
    cancelled: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    if archive.bytes.saturating_add(expected.len()) > MAX_DIRECTORY_ARCHIVE_BYTES {
        anyhow::bail!("directory upload exceeds the 64 MiB archive limit");
    }
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("open directory upload file {}", path.display()))?;
    let opened = source.metadata().context("inspect opened directory file")?;
    if !opened.is_file()
        || opened.dev() != expected.dev()
        || opened.ino() != expected.ino()
        || opened.len() != expected.len()
    {
        anyhow::bail!("directory upload entry changed while it was being opened");
    }
    let mut remaining = expected.len();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        check_cancelled(cancelled)?;
        let wanted = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        let read = source
            .read(&mut buffer[..wanted])
            .with_context(|| format!("read directory upload file {}", path.display()))?;
        if read == 0 {
            anyhow::bail!("directory upload entry was truncated while reading");
        }
        write_bounded(archive, &buffer[..read])?;
        remaining -= read as u64;
    }
    let mut extra = [0_u8; 1];
    if source.read(&mut extra)? != 0 {
        anyhow::bail!("directory upload entry grew while reading");
    }
    let after = source
        .metadata()
        .context("revalidate directory upload file")?;
    if !metadata_unchanged(&opened, &after) {
        anyhow::bail!("directory upload entry changed while reading");
    }
    Ok(())
}

fn metadata_unchanged(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

fn check_cancelled(cancelled: Option<&AtomicBool>) -> anyhow::Result<()> {
    if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Acquire)) {
        anyhow::bail!("directory upload was cancelled during local preparation");
    }
    Ok(())
}

fn write_bounded(archive: &mut DirectoryArchive, bytes: &[u8]) -> anyhow::Result<()> {
    let next = archive.bytes.saturating_add(bytes.len() as u64);
    if next > MAX_DIRECTORY_ARCHIVE_BYTES {
        anyhow::bail!("directory upload exceeds the 64 MiB archive limit");
    }
    archive
        .file
        .write_all(bytes)
        .context("write directory upload archive")?;
    archive.bytes = next;
    Ok(())
}

pub fn extract_directory_archive(
    archive: &mut File,
    staging: &Path,
    expected_entries: u32,
) -> anyhow::Result<()> {
    if expected_entries > MAX_DIRECTORY_ENTRIES {
        anyhow::bail!("directory archive entry count exceeds the supported limit");
    }
    archive.seek(SeekFrom::Start(0))?;
    let mut magic = [0_u8; MAGIC.len()];
    archive.read_exact(&mut magic)?;
    if &magic != MAGIC {
        anyhow::bail!("directory archive has an invalid header");
    }
    let mut seen = HashSet::with_capacity(expected_entries as usize);
    let mut entries = 0_u32;
    loop {
        let kind = read_u8(archive)?;
        if kind == ENTRY_END {
            break;
        }
        if !matches!(kind, ENTRY_DIRECTORY | ENTRY_FILE) {
            anyhow::bail!("directory archive contains an unknown entry type");
        }
        entries = entries.saturating_add(1);
        if entries > expected_entries || entries > MAX_DIRECTORY_ENTRIES {
            anyhow::bail!("directory archive contains more entries than declared");
        }
        let path_len = read_u32(archive)? as usize;
        if path_len == 0 || path_len > MAX_RELATIVE_PATH_BYTES {
            anyhow::bail!("directory archive path length is invalid");
        }
        let mode = read_u16(archive)?;
        let size = read_u64(archive)?;
        let mut path = vec![0_u8; path_len];
        archive.read_exact(&mut path)?;
        let relative = PathBuf::from(
            std::str::from_utf8(&path).context("directory archive path is not UTF-8")?,
        );
        validate_relative_path(&relative)?;
        if !seen.insert(relative.clone()) {
            anyhow::bail!("directory archive contains a duplicate path");
        }
        let target = staging.join(&relative);
        let parent = target
            .parent()
            .context("directory archive path has no parent")?;
        if parent != staging && !parent.is_dir() {
            anyhow::bail!("directory archive child appears before its parent");
        }
        match kind {
            ENTRY_DIRECTORY => {
                if size != 0 || mode != 0o700 {
                    anyhow::bail!("directory archive contains invalid directory metadata");
                }
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder
                    .create(&target)
                    .context("create staged upload directory")?;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
            }
            ENTRY_FILE => {
                if !matches!(mode, 0o600 | 0o700) {
                    anyhow::bail!("directory archive contains invalid file metadata");
                }
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(mode as u32)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&target)
                    .context("create staged upload file")?;
                copy_exact(archive, &mut file, size)?;
                file.set_permissions(fs::Permissions::from_mode(mode as u32))?;
                file.flush()?;
                file.sync_all()?;
            }
            _ => unreachable!(),
        }
    }
    if entries != expected_entries {
        anyhow::bail!("directory archive entry count does not match its declaration");
    }
    let mut extra = [0_u8; 1];
    if archive.read(&mut extra)? != 0 {
        anyhow::bail!("directory archive contains trailing data");
    }
    Ok(())
}

fn copy_exact(source: &mut File, target: &mut File, mut remaining: u64) -> anyhow::Result<()> {
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        let read = source.read(&mut buffer[..wanted])?;
        if read == 0 {
            anyhow::bail!("directory archive ended inside file content");
        }
        target.write_all(&buffer[..read])?;
        remaining -= read as u64;
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> anyhow::Result<()> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_RELATIVE_PATH_BYTES {
        anyhow::bail!("directory archive path is empty or too long");
    }
    let mut depth = 0_usize;
    for component in path.components() {
        match component {
            Component::Normal(value) if value.to_str().is_some_and(is_safe_component) => {
                depth += 1;
            }
            _ => anyhow::bail!("directory archive path is not a safe relative path"),
        }
    }
    if depth == 0 || depth > MAX_DIRECTORY_DEPTH {
        anyhow::bail!("directory archive path depth is invalid");
    }
    Ok(())
}

fn is_safe_component(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && !value.chars().any(crate::safe_text::is_unsafe_display_char)
}

fn read_u8(reader: &mut File) -> anyhow::Result<u8> {
    let mut bytes = [0_u8; 1];
    reader.read_exact(&mut bytes)?;
    Ok(bytes[0])
}

fn read_u16(reader: &mut File) -> anyhow::Result<u16> {
    let mut bytes = [0_u8; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u32(reader: &mut File) -> anyhow::Result<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(reader: &mut File) -> anyhow::Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_round_trip_preserves_tree_and_only_executable_policy() {
        let source_parent = tempfile::tempdir().unwrap();
        let source = source_parent.path().join("project");
        fs::create_dir(&source).unwrap();
        fs::create_dir(source.join("empty")).unwrap();
        fs::create_dir(source.join("bin")).unwrap();
        fs::write(source.join("README.md"), b"hello\n").unwrap();
        fs::write(source.join("bin/run"), b"#!/bin/sh\n").unwrap();
        fs::set_permissions(source.join("bin/run"), fs::Permissions::from_mode(0o751)).unwrap();

        let mut archive = build_directory_archive(&source).unwrap();
        assert_eq!(archive.entries, 4);
        let destination_parent = tempfile::tempdir().unwrap();
        let destination = destination_parent.path().join("staging");
        fs::create_dir(&destination).unwrap();
        extract_directory_archive(&mut archive.file, &destination, archive.entries).unwrap();

        assert_eq!(fs::read(destination.join("README.md")).unwrap(), b"hello\n");
        assert_eq!(
            fs::read(destination.join("bin/run")).unwrap(),
            b"#!/bin/sh\n"
        );
        assert!(destination.join("empty").is_dir());
        assert_eq!(
            fs::metadata(destination.join("README.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(destination.join("bin/run"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn archive_rejects_symlinks_and_special_files() {
        let source_parent = tempfile::tempdir().unwrap();
        let source = source_parent.path().join("project");
        fs::create_dir(&source).unwrap();
        std::os::unix::fs::symlink("missing", source.join("link")).unwrap();
        assert!(
            build_directory_archive(&source)
                .unwrap_err()
                .to_string()
                .contains("symbolic links")
        );
        fs::remove_file(source.join("link")).unwrap();
        let fifo = source.join("pipe");
        let fifo_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: the path is a valid NUL-free CString and mode is conventional.
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        assert!(
            build_directory_archive(&source)
                .unwrap_err()
                .to_string()
                .contains("devices, sockets, and FIFOs")
        );
    }

    #[test]
    fn archive_rejects_confirmed_root_replacement_and_preparation_cancellation() {
        let source_parent = tempfile::tempdir().unwrap();
        let source = source_parent.path().join("project");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), b"content").unwrap();
        let cancelled = AtomicBool::new(false);
        let error =
            build_directory_archive_with_control(&source, &cancelled, Some((u64::MAX, u64::MAX)))
                .unwrap_err();
        assert!(error.to_string().contains("identity changed"));

        cancelled.store(true, Ordering::Release);
        let error = build_directory_archive_with_control(&source, &cancelled, None).unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn extractor_rejects_parent_traversal() {
        let temporary = tempfile::tempdir().unwrap();
        let archive_path = temporary.path().join("bad.archive");
        let mut archive = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&archive_path)
            .unwrap();
        archive.write_all(MAGIC).unwrap();
        archive.write_all(&[ENTRY_FILE]).unwrap();
        archive.write_all(&6_u32.to_be_bytes()).unwrap();
        archive.write_all(&0o600_u16.to_be_bytes()).unwrap();
        archive.write_all(&0_u64.to_be_bytes()).unwrap();
        archive.write_all(b"../bad").unwrap();
        archive.write_all(&[ENTRY_END]).unwrap();
        archive.seek(SeekFrom::Start(0)).unwrap();
        let staging = temporary.path().join("staging");
        fs::create_dir(&staging).unwrap();
        assert!(extract_directory_archive(&mut archive, &staging, 1).is_err());
        assert!(!temporary.path().join("bad").exists());
    }
}
