//! The download transaction: staging, publishing, and rolling back files
//! beneath a pinned destination.

use super::*;
use flate2::{Decompress, FlushDecompress, Status};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs::{self, File, Permissions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use tempfile::{NamedTempFile, TempPath};

const DECOMPRESS_BUFFER_BYTES: usize = 64 * 1024;

pub(super) struct DownloadTransaction {
    session_id: String,
    entries: HashMap<String, Entry>,
    paths: HashSet<PathBuf>,
    node_ids: HashMap<String, String>,
    declared_bytes: u64,
    received_bytes: u64,
}

struct Entry {
    path: PathBuf,
    kind: EntryKind,
    mode: u32,
    mtime_ns: Option<i64>,
    declared_size: u64,
    bytes_written: u64,
    complete: bool,
}

enum EntryKind {
    Directory,
    Regular {
        temporary: Option<StagedFile>,
        decompressor: Option<DownloadDecompressor>,
    },
    Symlink {
        target: Vec<u8>,
    },
    HardLink {
        target: Vec<u8>,
    },
}

struct DownloadDecompressor {
    decoder: Decompress,
    finished: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CreatedKind {
    Directory,
    File,
    Symlink,
}

struct CreatedPath {
    path: PathBuf,
    device: u64,
    inode: u64,
    kind: CreatedKind,
    _identity_guard: Option<File>,
}

pub(super) enum TransactionResult {
    Status(KittyFileTransferStatus),
    Finished { files: usize, bytes: u64 },
}

#[derive(Debug)]
pub(super) struct TransactionError {
    pub file_id: String,
    pub code: KittyFileTransferStatusCode,
    pub message: String,
}

impl TransactionError {
    fn new(
        file_id: impl Into<String>,
        code: KittyFileTransferStatusCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            file_id: file_id.into(),
            code,
            message: message.into(),
        }
    }

    fn invalid(file_id: &str, message: impl Into<String>) -> Self {
        Self::new(file_id, KittyFileTransferStatusCode::Invalid, message)
    }

    fn io(file_id: &str, context: &str, error: io::Error) -> Self {
        let code = if error.kind() == io::ErrorKind::AlreadyExists {
            KittyFileTransferStatusCode::AlreadyExists
        } else if error.kind() == io::ErrorKind::PermissionDenied {
            KittyFileTransferStatusCode::PermissionDenied
        } else if error.kind() == io::ErrorKind::FileTooLarge {
            KittyFileTransferStatusCode::TooLarge
        } else {
            KittyFileTransferStatusCode::Io
        };
        Self::new(file_id, code, format!("{context}: {error}"))
    }
}

impl DownloadTransaction {
    pub(super) fn new(session_id: String) -> Self {
        Self {
            session_id,
            entries: HashMap::new(),
            paths: HashSet::new(),
            node_ids: HashMap::new(),
            declared_bytes: 0,
            received_bytes: 0,
        }
    }

    pub(super) fn status(
        &self,
        file_id: &str,
        code: KittyFileTransferStatusCode,
        message: &str,
    ) -> KittyFileTransferStatus {
        transfer_status(&self.session_id, file_id, code, message)
    }

    pub(super) fn progress(&self) -> (usize, u64) {
        (self.entries.len(), self.received_bytes)
    }

    pub(super) fn handle(
        &mut self,
        command: KittyFileTransferCommand,
        cancellation: &AtomicBool,
    ) -> Result<TransactionResult, TransactionError> {
        match command.action {
            KittyFileTransferAction::File => self.add_file(command),
            KittyFileTransferAction::Data | KittyFileTransferAction::EndData => {
                self.add_data(command, cancellation)
            }
            KittyFileTransferAction::Finish => self.commit(command, cancellation),
            _ => Err(TransactionError::invalid(
                &command.file_id,
                "invalid worker action",
            )),
        }
    }

    fn add_file(
        &mut self,
        command: KittyFileTransferCommand,
    ) -> Result<TransactionResult, TransactionError> {
        if !command.data.is_empty() {
            return Err(TransactionError::invalid(
                &command.file_id,
                "file metadata unexpectedly contains payload data",
            ));
        }
        if command.file_type != KittyFileTransferType::Regular
            && command.compression != termeric_core::KittyFileTransferCompression::None
        {
            return Err(TransactionError::invalid(
                &command.file_id,
                "compression applies only to regular files",
            ));
        }
        if self.entries.len() >= MAX_KITTY_DOWNLOAD_FILES {
            return Err(TransactionError::new(
                command.file_id,
                KittyFileTransferStatusCode::TooLarge,
                "transfer has too many entries",
            ));
        }
        if command.file_id.is_empty() || self.entries.contains_key(&command.file_id) {
            return Err(TransactionError::invalid(
                &command.file_id,
                "missing or duplicate file id",
            ));
        }
        let name = command
            .name
            .as_deref()
            .ok_or_else(|| TransactionError::invalid(&command.file_id, "missing path"))?;
        let path = super::resolve_home_relative_path(name)
            .map_err(|message| TransactionError::invalid(&command.file_id, message))?;
        if self.paths.contains(&path) {
            return Err(TransactionError::invalid(
                &command.file_id,
                "duplicate destination path",
            ));
        }
        validate_destination(&path)
            .map_err(|error| TransactionError::io(&command.file_id, "unsafe destination", error))?;

        let status_id = command.status.as_deref().unwrap_or_default();
        if !status_id.is_empty()
            && !termeric_core::file_transfer::is_safe_transfer_id(
                status_id.as_bytes(),
                termeric_core::MAX_KITTY_TRANSFER_FILE_ID_BYTES,
            )
        {
            return Err(TransactionError::invalid(
                &command.file_id,
                "invalid metadata node id",
            ));
        }
        if !status_id.is_empty() && self.node_ids.contains_key(status_id) {
            return Err(TransactionError::invalid(
                &command.file_id,
                "duplicate metadata node id",
            ));
        }
        if let Some(parent_id) = command.parent.as_deref().filter(|value| !value.is_empty()) {
            let parent_file_id = self.node_ids.get(parent_id).ok_or_else(|| {
                TransactionError::invalid(&command.file_id, "unknown metadata parent")
            })?;
            let parent = self
                .entries
                .get(parent_file_id)
                .expect("node id names an entry");
            if !matches!(parent.kind, EntryKind::Directory)
                || !path.starts_with(&parent.path)
                || path == parent.path
            {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "metadata parent does not contain this path",
                ));
            }
        }

        for entry in self.entries.values() {
            if path.starts_with(&entry.path)
                && path != entry.path
                && !matches!(entry.kind, EntryKind::Directory)
            {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "destination descends through a non-directory entry",
                ));
            }
            if entry.path.starts_with(&path)
                && entry.path != path
                && command.file_type != KittyFileTransferType::Directory
            {
                return Err(TransactionError::invalid(
                    &command.file_id,
                    "non-directory destination contains an earlier entry",
                ));
            }
        }

        let declared_size = match command.file_type {
            KittyFileTransferType::Regular => command.size.ok_or_else(|| {
                TransactionError::invalid(&command.file_id, "regular file is missing size")
            })?,
            _ => command.size.unwrap_or(0),
        };
        if declared_size < 0 {
            return Err(TransactionError::invalid(
                &command.file_id,
                "negative file size",
            ));
        }
        let declared_size = declared_size as u64;
        if command.file_type == KittyFileTransferType::Regular {
            self.declared_bytes =
                self.declared_bytes
                    .checked_add(declared_size)
                    .ok_or_else(|| {
                        TransactionError::new(
                            &command.file_id,
                            KittyFileTransferStatusCode::TooLarge,
                            "declared transfer size overflowed",
                        )
                    })?;
            if self.declared_bytes > MAX_KITTY_DOWNLOAD_BYTES {
                return Err(TransactionError::new(
                    &command.file_id,
                    KittyFileTransferStatusCode::TooLarge,
                    "transfer exceeds the 64 MiB limit",
                ));
            }
        }

        let mode = command.permissions.unwrap_or(match command.file_type {
            KittyFileTransferType::Directory => 0o700,
            _ => 0o600,
        }) & 0o777;
        // Kitty modification times are signed nanoseconds since the epoch;
        // negative values are valid dates, not missing-metadata sentinels.
        let mtime_ns = command.mtime_ns;
        let kind = match command.file_type {
            KittyFileTransferType::Directory => EntryKind::Directory,
            KittyFileTransferType::Regular => EntryKind::Regular {
                temporary: None,
                decompressor: (command.compression
                    == termeric_core::KittyFileTransferCompression::Zlib)
                    .then(|| DownloadDecompressor {
                        decoder: Decompress::new(true),
                        finished: false,
                    }),
            },
            KittyFileTransferType::Symlink => EntryKind::Symlink { target: Vec::new() },
            KittyFileTransferType::HardLink => EntryKind::HardLink { target: Vec::new() },
        };
        let complete = command.file_type == KittyFileTransferType::Directory;
        let file_id = command.file_id.clone();
        self.paths.insert(path.clone());
        if !status_id.is_empty() {
            self.node_ids.insert(status_id.into(), file_id.clone());
        }
        self.entries.insert(
            file_id.clone(),
            Entry {
                path: path.clone(),
                kind,
                mode,
                mtime_ns,
                declared_size,
                bytes_written: 0,
                complete,
            },
        );
        let mut status = self.status(
            &file_id,
            if complete {
                KittyFileTransferStatusCode::Ok
            } else {
                KittyFileTransferStatusCode::Started
            },
            "",
        );
        status.name = Some(path.to_string_lossy().into_owned());
        if !complete {
            status.size = Some(-1);
        }
        Ok(TransactionResult::Status(status))
    }

    fn add_data(
        &mut self,
        command: KittyFileTransferCommand,
        cancellation: &AtomicBool,
    ) -> Result<TransactionResult, TransactionError> {
        if command.name.is_some()
            || command.status.is_some()
            || command.parent.is_some()
            || command.size.is_some()
            || command.mtime_ns.is_some()
            || command.permissions.is_some()
            || command.compression != termeric_core::KittyFileTransferCompression::None
            || command.file_type != KittyFileTransferType::Regular
        {
            return Err(TransactionError::invalid(
                &command.file_id,
                "data command contains unexpected metadata",
            ));
        }
        let is_last = command.action == KittyFileTransferAction::EndData;
        let file_id = command.file_id.clone();
        let entry = self.entries.get_mut(&file_id).ok_or_else(|| {
            TransactionError::invalid(&file_id, "data arrived before file metadata")
        })?;
        if entry.complete {
            return Err(TransactionError::invalid(
                &file_id,
                "data arrived for a completed entry",
            ));
        }
        if cancellation.load(Ordering::Acquire) {
            return Err(TransactionError::new(
                &file_id,
                KittyFileTransferStatusCode::Canceled,
                "transfer was canceled",
            ));
        }

        let before = entry.bytes_written;
        match &mut entry.kind {
            EntryKind::Directory => {
                return Err(TransactionError::invalid(
                    &file_id,
                    "directory entries cannot receive data",
                ));
            }
            EntryKind::Regular {
                temporary,
                decompressor,
            } => {
                if temporary.is_none() {
                    *temporary = Some(stage_file(&entry.path).map_err(|error| {
                        TransactionError::io(&file_id, "create staging file", error)
                    })?);
                }
                let temporary = temporary.as_mut().expect("staging file was created");
                if let Some(decompressor) = decompressor {
                    write_decompressed(
                        decompressor,
                        &command.data,
                        is_last,
                        temporary.as_file_mut(),
                        &mut entry.bytes_written,
                        entry.declared_size,
                        cancellation,
                    )
                    .map_err(|message| TransactionError::invalid(&file_id, message))?;
                } else {
                    let data_len = command.data.len() as u64;
                    if entry.bytes_written.saturating_add(data_len) > entry.declared_size {
                        return Err(TransactionError::invalid(
                            &file_id,
                            "received data exceeds declared size",
                        ));
                    }
                    temporary
                        .as_file_mut()
                        .write_all(&command.data)
                        .map_err(|error| {
                            TransactionError::io(&file_id, "write staging file", error)
                        })?;
                    entry.bytes_written += data_len;
                }
                if is_last && entry.bytes_written != entry.declared_size {
                    return Err(TransactionError::invalid(
                        &file_id,
                        format!(
                            "received {} bytes but metadata declared {}",
                            entry.bytes_written, entry.declared_size
                        ),
                    ));
                }
            }
            EntryKind::Symlink { target } | EntryKind::HardLink { target } => {
                if target.len().saturating_add(command.data.len())
                    > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES
                {
                    return Err(TransactionError::new(
                        &file_id,
                        KittyFileTransferStatusCode::TooLarge,
                        "link target is too large",
                    ));
                }
                target.extend_from_slice(&command.data);
                entry.bytes_written = target.len() as u64;
            }
        }
        self.received_bytes = self
            .received_bytes
            .saturating_add(entry.bytes_written.saturating_sub(before));
        if self.received_bytes > MAX_KITTY_DOWNLOAD_BYTES {
            return Err(TransactionError::new(
                &file_id,
                KittyFileTransferStatusCode::TooLarge,
                "transfer exceeds the 64 MiB limit",
            ));
        }
        if is_last {
            entry.complete = true;
        }
        let complete = entry.complete;
        let path = entry.path.to_string_lossy().into_owned();
        let bytes_written = entry.bytes_written;
        let mut status = self.status(
            &file_id,
            if complete {
                KittyFileTransferStatusCode::Ok
            } else {
                KittyFileTransferStatusCode::Progress
            },
            "",
        );
        status.name = Some(path);
        status.size = Some(bytes_written.min(i64::MAX as u64) as i64);
        Ok(TransactionResult::Status(status))
    }

    fn commit(
        &mut self,
        command: KittyFileTransferCommand,
        cancellation: &AtomicBool,
    ) -> Result<TransactionResult, TransactionError> {
        if !command.file_id.is_empty()
            || !command.data.is_empty()
            || command.name.is_some()
            || command.status.is_some()
            || command.parent.is_some()
            || command.size.is_some()
            || command.mtime_ns.is_some()
            || command.permissions.is_some()
            || command.compression != termeric_core::KittyFileTransferCompression::None
            || command.file_type != KittyFileTransferType::Regular
        {
            return Err(TransactionError::invalid(
                &command.file_id,
                "finish command contains unexpected file fields",
            ));
        }
        if self.entries.is_empty() {
            return Err(TransactionError::invalid(
                "",
                "transfer contains no entries",
            ));
        }
        if self.entries.values().any(|entry| !entry.complete) {
            return Err(TransactionError::invalid(
                "",
                "transfer finished with incomplete entries",
            ));
        }
        if cancellation.load(Ordering::Acquire) {
            return Err(TransactionError::new(
                "",
                KittyFileTransferStatusCode::Canceled,
                "transfer was canceled",
            ));
        }
        for entry in self.entries.values() {
            validate_destination(&entry.path).map_err(|error| {
                TransactionError::io("", "destination changed before commit", error)
            })?;
        }
        validate_creation_budget(&self.paths)
            .map_err(|error| TransactionError::io("", "expanded destination limit", error))?;

        let mut created = Vec::<CreatedPath>::new();
        let result = self.commit_inner(cancellation, &mut created);
        self.complete_commit(result, cancellation, &created)
    }

    fn complete_commit(
        &self,
        result: Result<(), TransactionError>,
        cancellation: &AtomicBool,
        created: &[CreatedPath],
    ) -> Result<TransactionResult, TransactionError> {
        // Final acceptance boundary: cancellation observed after the last
        // filesystem operation still rolls back. Preserve an earlier error
        // rather than replacing its diagnostics with a cancellation status.
        let result = result.and_then(|()| check_cancel(cancellation, ""));
        if let Err(error) = result {
            rollback_paths(created);
            return Err(error);
        }
        Ok(TransactionResult::Finished {
            files: self.entries.len(),
            bytes: self.received_bytes,
        })
    }

    fn commit_inner(
        &mut self,
        cancellation: &AtomicBool,
        created: &mut Vec<CreatedPath>,
    ) -> Result<(), TransactionError> {
        let mut directories: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.kind, EntryKind::Directory))
            .map(|(id, entry)| (id.clone(), entry.path.clone()))
            .collect();
        directories.sort_by_key(|(_, path)| path.components().count());
        for (id, path) in &directories {
            check_cancel(cancellation, id)?;
            create_missing_parents(path, &self.paths, created)
                .map_err(|error| TransactionError::io(id, "create destination parent", error))?;
            create_directory(path, created)
                .map_err(|error| TransactionError::io(id, "create directory", error))?;
        }

        let file_ids: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.kind, EntryKind::Regular { .. }))
            .map(|(id, _)| id.clone())
            .collect();
        for id in file_ids {
            check_cancel(cancellation, &id)?;
            let entry = self.entries.get_mut(&id).expect("file id exists");
            create_missing_parents(&entry.path, &self.paths, created)
                .map_err(|error| TransactionError::io(&id, "create destination parent", error))?;
            ensure_creation_capacity(created)
                .map_err(|error| TransactionError::io(&id, "publish file", error))?;
            let temporary = match &mut entry.kind {
                EntryKind::Regular { temporary, .. } => temporary.take(),
                _ => None,
            }
            .ok_or_else(|| TransactionError::invalid(&id, "missing staged file"))?;
            prepare_staged_file(temporary.as_file(), entry.mode, entry.mtime_ns)
                .map_err(|error| TransactionError::io(&id, "finalize staging file", error))?;
            let identity = temporary
                .as_file()
                .metadata()
                .map_err(|error| TransactionError::io(&id, "inspect staging file", error))?;
            let destination = PublicationTarget::open(&entry.path)
                .map_err(|error| TransactionError::io(&id, "pin destination parent", error))?;
            let published = destination
                .publish(temporary)
                .map_err(|error| TransactionError::io(&id, "publish file", error))?;
            created.push(CreatedPath {
                path: entry.path.clone(),
                device: identity.dev(),
                inode: identity.ino(),
                kind: CreatedKind::File,
                _identity_guard: Some(published),
            });
        }

        let link_ids: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                matches!(
                    entry.kind,
                    EntryKind::Symlink { .. } | EntryKind::HardLink { .. }
                )
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in link_ids {
            check_cancel(cancellation, &id)?;
            let (path, is_symlink, target_data, mtime) = {
                let entry = self.entries.get(&id).expect("link id exists");
                let (is_symlink, target) = match &entry.kind {
                    EntryKind::Symlink { target } => (true, target.clone()),
                    EntryKind::HardLink { target } => (false, target.clone()),
                    _ => unreachable!(),
                };
                (entry.path.clone(), is_symlink, target, entry.mtime_ns)
            };
            create_missing_parents(&path, &self.paths, created)
                .map_err(|error| TransactionError::io(&id, "create destination parent", error))?;
            let target = self.resolve_link_target(&id, &path, &target_data, is_symlink)?;
            if is_symlink {
                PublicationTarget::open(&path)
                    .and_then(|destination| destination.create_symlink(&target, created))
                    .map_err(|error| TransactionError::io(&id, "create symbolic link", error))?;
            } else {
                create_hard_link(&target, &path, created)
                    .map_err(|error| TransactionError::io(&id, "create hard link", error))?;
            }
            apply_link_mtime(created.last().expect("link was recorded"), mtime)
                .map_err(|error| TransactionError::io(&id, "set link modification time", error))?;
        }

        directories.sort_by_key(|(_, path)| std::cmp::Reverse(path.components().count()));
        for (id, path) in directories {
            check_cancel(cancellation, &id)?;
            let entry = self.entries.get(&id).expect("directory id exists");
            finalize_directory(&path, entry.mode, entry.mtime_ns, created)
                .map_err(|error| TransactionError::io(&id, "set directory metadata", error))?;
        }
        Ok(())
    }

    fn resolve_link_target(
        &self,
        file_id: &str,
        link_path: &Path,
        data: &[u8],
        symlink: bool,
    ) -> Result<PathBuf, TransactionError> {
        let text = std::str::from_utf8(data)
            .map_err(|_| TransactionError::invalid(file_id, "link target is not UTF-8"))?;
        if !symlink {
            return self
                .entries
                .get(text)
                .filter(|entry| matches!(entry.kind, EntryKind::Regular { .. }))
                .map(|entry| entry.path.clone())
                .ok_or_else(|| {
                    TransactionError::invalid(
                        file_id,
                        "hard link names an unknown or non-regular target file id",
                    )
                });
        }

        let absolute = if let Some(target_id) = text
            .strip_prefix("fid:")
            .or_else(|| text.strip_prefix("fid_abs:"))
        {
            // Symbolic links can name any transferred entry, including a
            // directory. Resolve its destination path without following it;
            // only hard links require the regular-file restriction above.
            self.entries
                .get(target_id)
                .map(|entry| entry.path.clone())
                .ok_or_else(|| TransactionError::invalid(file_id, "link names unknown file id"))?
        } else if let Some(path) = text.strip_prefix("path:") {
            let raw = Path::new(path);
            if path.is_empty()
                || path.len() > termeric_core::MAX_KITTY_TRANSFER_PATH_BYTES
                || path.as_bytes().contains(&0)
            {
                return Err(TransactionError::invalid(
                    file_id,
                    "symbolic link target is empty or invalid",
                ));
            }
            return Ok(raw.to_path_buf());
        } else {
            return Err(TransactionError::invalid(
                file_id,
                "unknown link target encoding",
            ));
        };
        if text.starts_with("fid:") {
            relative_path(
                link_path.parent().unwrap_or_else(|| Path::new("/")),
                &absolute,
            )
            .ok_or_else(|| TransactionError::invalid(file_id, "cannot relativize link"))
        } else {
            Ok(absolute)
        }
    }
}

fn validate_destination(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination already exists",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut ancestors = Vec::new();
    let mut current = path.parent();
    while let Some(candidate) = current {
        ancestors.push(candidate);
        current = candidate.parent();
    }
    for ancestor in ancestors.into_iter().rev() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "destination ancestor is a symbolic link",
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "destination ancestor is not a directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn create_hard_link(
    source: &Path,
    destination: &Path,
    created: &mut Vec<CreatedPath>,
) -> io::Result<()> {
    ensure_creation_capacity(created)?;
    // source is only a transaction lookup key. Its current pathname
    // cannot grant authority to link or timestamp a replacement inode.
    let entry = created
        .iter()
        .find(|entry| entry.path == source && entry.kind == CreatedKind::File)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "source was not recorded"))?;
    let file = entry._identity_guard.as_ref().ok_or_else(|| {
        io::Error::new(io::ErrorKind::PermissionDenied, "source handle is missing")
    })?;
    let identity = file.metadata()?;
    if !identity.is_file() || identity.dev() != entry.device || identity.ino() != entry.inode {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "source handle does not match its recorded regular-file identity",
        ));
    }
    let published = PublicationTarget::open(destination)?.link_file(file)?;
    // Record directly from the known source, not a fresh destination
    // lookup that could observe an intervening replacement.
    created.push(CreatedPath {
        path: destination.to_owned(),
        device: identity.dev(),
        inode: identity.ino(),
        kind: CreatedKind::File,
        _identity_guard: Some(published),
    });
    Ok(())
}

struct StagedFile {
    file: File,
    path: TempPath,
    // Must remain open through Drop's identity check and unlink.
    _parent: Option<File>,
}

impl From<NamedTempFile> for StagedFile {
    fn from(temporary: NamedTempFile) -> Self {
        let (file, mut path) = temporary.into_parts();
        // Never let TempPath blindly unlink a replacement during failure,
        // cancellation, or successful publication. Our Drop checks identity.
        path.disable_cleanup(true);
        Self {
            file,
            path,
            _parent: None,
        }
    }
}

impl StagedFile {
    fn as_file(&self) -> &File {
        &self.file
    }

    fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        let Ok(owned) = self.file.metadata() else {
            return;
        };
        let Ok(named) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if owned.is_file()
            && named.is_file()
            && (owned.dev(), owned.ino()) == (named.dev(), named.ino())
        {
            // Best effort, not an atomic compare-and-unlink. A moved source
            // is not searched for, and an unknown/replaced name is retained.
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn open_pinned_directory(path: &Path) -> io::Result<File> {
    use std::os::fd::FromRawFd;

    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in parent"))?;
    let how = OpenHow {
        flags: (libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) as u64,
        mode: 0,
        resolve: libc::RESOLVE_NO_SYMLINKS,
    };
    // SAFETY: path and initialized open_how live through the syscall.
    // Success transfers ownership of one new descriptor to this function.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            path.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful syscall's descriptor has no other owner.
    Ok(unsafe { File::from_raw_fd(descriptor as _) })
}

struct StagingDirectory {
    parent: File,
    path: PathBuf,
}

impl StagingDirectory {
    fn open(path: &Path) -> io::Result<Self> {
        use std::os::fd::AsRawFd;

        let parent = open_pinned_directory(path)?;
        let path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
        Ok(Self { parent, path })
    }

    fn create_file(self) -> io::Result<StagedFile> {
        let temporary = tempfile::Builder::new()
            .prefix(".termeric-transfer-")
            .tempfile_in(&self.path)?;
        let mut staged = StagedFile::from(temporary);
        // TempPath preserves this trusted procfs path. Keep its parent
        // descriptor alive until StagedFile::drop finishes cleanup.
        staged._parent = Some(self.parent);
        Ok(staged)
    }
}

struct PublicationTarget {
    parent: File,
    path: PathBuf,
    original_path: PathBuf,
}

impl PublicationTarget {
    fn open(path: &Path) -> io::Result<Self> {
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "missing destination name")
        })?;
        let bytes = name.as_bytes();
        if !path.is_absolute()
            || bytes.is_empty()
            || bytes.len() > 255
            || bytes.contains(&0)
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid publication destination",
            ));
        }
        use std::os::fd::AsRawFd;

        let parent = open_pinned_directory(path.parent().unwrap())?;
        let original_path = path.to_owned();
        let path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name);
        Ok(Self {
            parent,
            path,
            original_path,
        })
    }

    fn create_symlink(self, target: &Path, created: &mut Vec<CreatedPath>) -> io::Result<()> {
        ensure_creation_capacity(created)?;
        // Keep the pinned parent alive through both creation and identity
        // capture. The original path is only a rollback lookup key, never
        // a fallback if the trusted procfs reference fails.
        std::os::unix::fs::symlink(target, &self.path)?;
        record_created(&self.path, CreatedKind::Symlink, created)?;
        created.last_mut().expect("link was recorded").path = self.original_path;
        Ok(())
    }

    fn create_directory(self, created: &mut Vec<CreatedPath>) -> io::Result<()> {
        ensure_creation_capacity(created)?;
        // Creation and identity capture share the retained parent. Restore
        // access through the new directory's handle, never its old pathname.
        fs::DirBuilder::new().mode(0o700).create(&self.path)?;
        record_created(&self.path, CreatedKind::Directory, created)?;
        let entry = created.last_mut().expect("created directory was recorded");
        entry.path = self.original_path;
        ensure_directory_access(entry)
    }

    fn publish(self, temporary: StagedFile) -> io::Result<File> {
        self.link_file(temporary.as_file())
    }

    fn link_file(self, file: &File) -> io::Result<File> {
        use std::os::fd::AsRawFd;

        // Acquire the returned handle before publication so a descriptor
        // allocation failure cannot leave an unrecorded destination behind.
        let published = file.try_clone()?;
        let source = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .expect("descriptor path contains no NUL");
        let destination =
            CString::new(self.path.as_os_str().as_bytes()).expect("publication path was validated");
        // SAFETY: both CString pointers and both retained descriptors
        // stay live through linkat. Only our trusted procfs source link
        // is followed. linkat atomically refuses an existing destination.
        let result = unsafe {
            libc::linkat(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::AT_SYMLINK_FOLLOW,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        drop(self.parent);
        Ok(published)
    }
}

fn stage_file(path: &Path) -> io::Result<StagedFile> {
    let mut ancestor = path.parent();
    while let Some(candidate) = ancestor {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                return StagingDirectory::open(candidate)?.create_file();
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "staging ancestor is not a real directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                ancestor = candidate.parent();
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no existing destination ancestor",
    ))
}

fn write_decompressed(
    stream: &mut DownloadDecompressor,
    mut input: &[u8],
    is_last: bool,
    output: &mut File,
    bytes_written: &mut u64,
    declared_size: u64,
    cancellation: &AtomicBool,
) -> Result<(), String> {
    if stream.finished {
        // Compression EOF does not finish the protocol entry. Empty data
        // and end_data packets are valid; a second stream or any trailing
        // payload is not, even when it arrives in a later packet.
        return if input.is_empty() {
            Ok(())
        } else {
            Err("data follows the end of the zlib stream".into())
        };
    }
    let decompressor = &mut stream.decoder;
    let mut buffer = [0u8; DECOMPRESS_BUFFER_BYTES];
    loop {
        if cancellation.load(Ordering::Acquire) {
            return Err("transfer was canceled".into());
        }
        let before_in = decompressor.total_in();
        let before_out = decompressor.total_out();
        let status = decompressor
            .decompress(
                input,
                &mut buffer,
                if is_last {
                    FlushDecompress::Finish
                } else {
                    FlushDecompress::None
                },
            )
            .map_err(|_| "invalid zlib stream".to_string())?;
        let consumed = (decompressor.total_in() - before_in) as usize;
        let produced = (decompressor.total_out() - before_out) as usize;
        if bytes_written.saturating_add(produced as u64) > declared_size
            || bytes_written.saturating_add(produced as u64) > MAX_KITTY_DOWNLOAD_BYTES
        {
            return Err("decompressed data exceeds declared or session size".into());
        }
        output
            .write_all(&buffer[..produced])
            .map_err(|error| format!("write decompressed data: {error}"))?;
        *bytes_written += produced as u64;
        input = &input[consumed..];
        if status == Status::StreamEnd {
            if !input.is_empty() {
                return Err("data follows the end of the zlib stream".into());
            }
            stream.finished = true;
            return Ok(());
        }
        if input.is_empty() && produced < buffer.len() {
            if is_last {
                return Err("truncated zlib stream".into());
            }
            return Ok(());
        }
        if consumed == 0 && produced == 0 {
            return Err("zlib decoder made no progress".into());
        }
    }
}

fn creation_limit_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        "transfer exceeds the destination entry limit including implicit parents",
    )
}

fn ensure_creation_capacity(created: &[CreatedPath]) -> io::Result<()> {
    if created.len() >= MAX_KITTY_DOWNLOAD_FILES {
        Err(creation_limit_error())
    } else {
        Ok(())
    }
}

fn validate_creation_budget(paths: &HashSet<PathBuf>) -> io::Result<()> {
    if paths.len() > MAX_KITTY_DOWNLOAD_FILES {
        return Err(creation_limit_error());
    }
    let mut planned = paths.clone();
    for path in paths {
        let mut ancestor = path.parent();
        while let Some(candidate) = ancestor {
            match fs::symlink_metadata(candidate) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    break;
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "destination ancestor is not a real directory",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    planned.insert(candidate.to_owned());
                    if planned.len() > MAX_KITTY_DOWNLOAD_FILES {
                        return Err(creation_limit_error());
                    }
                    ancestor = candidate.parent();
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

fn create_missing_parents(
    path: &Path,
    transfer_paths: &HashSet<PathBuf>,
    created: &mut Vec<CreatedPath>,
) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut current = path.parent();
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => break,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "destination ancestor is not a real directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(candidate.to_path_buf());
                current = candidate.parent();
            }
            Err(error) => return Err(error),
        }
    }
    for directory in missing.into_iter().rev() {
        if transfer_paths.contains(&directory) {
            continue;
        }
        create_directory(&directory, created)?;
    }
    Ok(())
}

fn open_directory(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

fn open_directory_identity(path: &Path) -> io::Result<File> {
    let result = open_directory(path);
    if result
        .as_ref()
        .is_err_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
    {
        // O_PATH can pin a directory whose initial permissions were
        // removed by umask, without reading it or following a symlink.
        return fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path);
    }
    result
}

fn create_directory(path: &Path, created: &mut Vec<CreatedPath>) -> io::Result<()> {
    PublicationTarget::open(path)?.create_directory(created)
}

fn ensure_directory_access(entry: &mut CreatedPath) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let directory = entry._identity_guard.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory handle is missing",
        )
    })?;
    let metadata = directory.metadata()?;
    if entry.kind != CreatedKind::Directory
        || !metadata.is_dir()
        || metadata.dev() != entry.device
        || metadata.ino() != entry.inode
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory handle does not match its recorded identity",
        ));
    }
    // SAFETY: directory owns a live descriptor; F_GETFL takes no
    // additional argument and does not mutate descriptor state.
    let flags = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::O_PATH != 0 {
        // This trusted procfs magic link names the retained inode,
        // even if the destination/ancestor pathname was replaced.
        // Keep directory borrowed until reopening is complete. If
        // procfs is unavailable, fail without a destination fallback.
        let proc_path = format!("/proc/self/fd/{}", directory.as_raw_fd());
        fs::set_permissions(&proc_path, Permissions::from_mode(0o700))?;
        let upgraded = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK)
            .open(&proc_path)?;
        let metadata = upgraded.metadata()?;
        if !metadata.is_dir() || metadata.dev() != entry.device || metadata.ino() != entry.inode {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "reopened directory does not match its recorded identity",
            ));
        }
        entry._identity_guard = Some(upgraded);
        return Ok(());
    }
    directory.set_permissions(Permissions::from_mode(0o700))
}

fn prepare_staged_file(file: &File, mode: u32, mtime_ns: Option<i64>) -> io::Result<()> {
    // Finish metadata on the retained inode before making it visible, then
    // sync both data and metadata. Never re-open its eventual destination.
    apply_file_mtime(file, mtime_ns)?;
    file.set_permissions(Permissions::from_mode(mode))?;
    file.sync_all()
}

fn finalize_directory(
    path: &Path,
    mode: u32,
    mtime_ns: Option<i64>,
    created: &[CreatedPath],
) -> io::Result<()> {
    // The path is a transaction lookup key, never fresh filesystem
    // authority: another process may have replaced it since creation.
    let entry = created
        .iter()
        .find(|entry| entry.path == path && entry.kind == CreatedKind::Directory)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "directory was not recorded"))?;
    let directory = entry._identity_guard.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory handle is missing",
        )
    })?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.dev() != entry.device || metadata.ino() != entry.inode {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory handle does not match its recorded identity",
        ));
    }
    apply_file_mtime(directory, mtime_ns)?;
    directory.set_permissions(Permissions::from_mode(mode))
}

fn apply_file_mtime(file: &File, mtime_ns: Option<i64>) -> io::Result<()> {
    if let Some(mtime_ns) = mtime_ns {
        let distance = Duration::from_nanos(mtime_ns.unsigned_abs());
        let timestamp = if mtime_ns < 0 {
            std::time::UNIX_EPOCH.checked_sub(distance)
        } else {
            std::time::UNIX_EPOCH.checked_add(distance)
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "timestamp is out of range"))?;
        file.set_times(
            fs::FileTimes::new()
                .set_accessed(timestamp)
                .set_modified(timestamp),
        )?;
    }
    Ok(())
}

fn record_created(
    path: &Path,
    kind: CreatedKind,
    created: &mut Vec<CreatedPath>,
) -> io::Result<()> {
    record_created_with(path, kind, created, |path| fs::symlink_metadata(path))
}

/// [`record_created`], observing the path's identity with `inspect`, which
/// tests use to fail the observation while the path is being replaced.
fn record_created_with(
    path: &Path,
    kind: CreatedKind,
    created: &mut Vec<CreatedPath>,
    inspect: impl FnOnce(&Path) -> io::Result<fs::Metadata>,
) -> io::Result<()> {
    // Without an observed identity there is no safe cleanup authority: the
    // pathname might already refer to somebody else's replacement.
    let metadata = inspect(path)?;
    let kind_matches = match kind {
        CreatedKind::Directory => metadata.is_dir(),
        CreatedKind::File => metadata.is_file(),
        CreatedKind::Symlink => metadata.file_type().is_symlink(),
    };
    if !kind_matches {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "created path changed type before its identity was recorded",
        ));
    }
    // Bookkeeping must never open a symbolic link's target: it could be a
    // FIFO/device, and a followed handle would not pin the link's inode
    // anyway. Linux O_PATH pins even dangling symlinks without opening their
    // contents or requiring readers.
    let identity_guard = if kind == CreatedKind::Directory {
        // Retain an fchmod-capable handle before final permissions can
        // remove owner access needed by rollback.
        open_directory_identity(path)
    } else {
        let flags = libc::O_PATH | libc::O_NOFOLLOW;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags)
            .open(path)
    }
    .ok()
    .filter(|file| {
        file.metadata()
            .is_ok_and(|opened| opened.dev() == metadata.dev() && opened.ino() == metadata.ino())
    });
    created.push(CreatedPath {
        path: path.to_path_buf(),
        device: metadata.dev(),
        inode: metadata.ino(),
        kind,
        _identity_guard: identity_guard,
    });
    Ok(())
}

fn rollback_paths(created: &[CreatedPath]) {
    // Restore access top-down through retained handles before unlinking
    // children. Never chmod the current path, which might be a replacement.
    for entry in created {
        if entry.kind == CreatedKind::Directory
            && let Some(directory) = &entry._identity_guard
            && let Ok(metadata) = directory.metadata()
            && metadata.is_dir()
            && metadata.dev() == entry.device
            && metadata.ino() == entry.inode
        {
            let _ = directory.set_permissions(Permissions::from_mode(
                (metadata.permissions().mode() & 0o777) | 0o700,
            ));
        }
    }
    for created in created.iter().rev() {
        match fs::symlink_metadata(&created.path) {
            Ok(metadata)
                if metadata.dev() == created.device
                    && metadata.ino() == created.inode
                    && match created.kind {
                        CreatedKind::Directory => {
                            metadata.is_dir() && !metadata.file_type().is_symlink()
                        }
                        CreatedKind::File => {
                            metadata.is_file() && !metadata.file_type().is_symlink()
                        }
                        CreatedKind::Symlink => metadata.file_type().is_symlink(),
                    } =>
            {
                let _ = if created.kind == CreatedKind::Directory {
                    fs::remove_dir(&created.path)
                } else {
                    fs::remove_file(&created.path)
                };
            }
            Ok(_) | Err(_) => {}
        }
    }
}

fn check_cancel(cancellation: &AtomicBool, file_id: &str) -> Result<(), TransactionError> {
    if cancellation.load(Ordering::Acquire) {
        Err(TransactionError::new(
            file_id,
            KittyFileTransferStatusCode::Canceled,
            "transfer was canceled",
        ))
    } else {
        Ok(())
    }
}

fn apply_link_mtime(entry: &CreatedPath, mtime_ns: Option<i64>) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let Some(mtime_ns) = mtime_ns else {
        return Ok(());
    };
    let file = entry
        ._identity_guard
        .as_ref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "link handle is missing"))?;
    let metadata = file.metadata()?;
    let kind_matches = match entry.kind {
        CreatedKind::Symlink => metadata.file_type().is_symlink(),
        CreatedKind::File => metadata.is_file(),
        CreatedKind::Directory => false,
    };
    if !kind_matches || metadata.dev() != entry.device || metadata.ino() != entry.inode {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "link handle does not match its recorded identity",
        ));
    }
    let times = timestamp_times(mtime_ns)?;
    // SAFETY: file owns the live descriptor, c"" is NUL-terminated,
    // and times contains exactly two initialized timespec values.
    // AT_EMPTY_PATH operates on the pinned O_PATH inode; NOFOLLOW
    // keeps symlink timestamps on the link, never its target. Do not
    // fall back to a pathname if this requires a newer kernel.
    let result = unsafe {
        libc::utimensat(
            file.as_raw_fd(),
            c"".as_ptr(),
            times.as_ptr(),
            libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
fn apply_mtime(path: &Path, mtime_ns: Option<i64>, symlink: bool) -> io::Result<()> {
    let Some(mtime_ns) = mtime_ns else {
        return Ok(());
    };
    let times = timestamp_times(mtime_ns)?;
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    let flags = if symlink {
        libc::AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    // SAFETY: `path` is a live NUL-terminated byte string and `times`
    // contains exactly two initialized timespec values for this call.
    let result = unsafe { libc::utimensat(libc::AT_FDCWD, path.as_ptr(), times.as_ptr(), flags) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn timestamp_times(mtime_ns: i64) -> io::Result<[libc::timespec; 2]> {
    // time_t is narrower on some Unix targets; never wrap a wire timestamp.
    #[allow(clippy::useless_conversion)]
    let seconds = mtime_ns
        .div_euclid(1_000_000_000)
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "timestamp is out of range"))?;
    Ok([libc::timespec {
        tv_sec: seconds,
        tv_nsec: mtime_ns.rem_euclid(1_000_000_000) as libc::c_long,
    }; 2])
}

fn relative_path(base: &Path, target: &Path) -> Option<PathBuf> {
    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();
    let common = base_components
        .iter()
        .zip(&target_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return None;
    }
    let mut output = PathBuf::new();
    for _ in common..base_components.len() {
        output.push("..");
    }
    for component in &target_components[common..] {
        output.push(component.as_os_str());
    }
    Some(output)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod worker_tests;

pub(super) fn run_worker(
    receiver: Receiver<KittyFileTransferCommand>,
    session_id: String,
    reply_uses_8bit_controls: bool,
    cancellation: Arc<AtomicBool>,
    idle_timeout: Duration,
    mut emit: impl FnMut(WorkerCompletion) -> bool,
) {
    let mut transaction = DownloadTransaction::new(session_id);
    loop {
        let command = match receiver.recv_timeout(idle_timeout) {
            Ok(command) => command,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let status = transaction.status(
                    "",
                    KittyFileTransferStatusCode::TimedOut,
                    "transfer expired after ten minutes of inactivity",
                );
                let (files, bytes) = transaction.progress();
                emit(WorkerCompletion {
                    reply_uses_8bit_controls,
                    disposition: WorkerDisposition::Status {
                        status,
                        terminal: true,
                        files,
                        bytes,
                    },
                });
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if cancellation.load(Ordering::Acquire) {
            break;
        }
        let reply_mode = command.reply_uses_8bit_controls;
        let result = transaction.handle(command, &cancellation);
        let (progress_files, progress_bytes) = transaction.progress();
        let disposition = match result {
            Ok(TransactionResult::Status(status)) => WorkerDisposition::Status {
                status,
                terminal: false,
                files: progress_files,
                bytes: progress_bytes,
            },
            Ok(TransactionResult::Finished { files, bytes }) => {
                WorkerDisposition::Finished { files, bytes }
            }
            Err(error) => WorkerDisposition::Status {
                status: transaction.status(&error.file_id, error.code, &error.message),
                terminal: true,
                files: progress_files,
                bytes: progress_bytes,
            },
        };
        let terminal = matches!(
            disposition,
            WorkerDisposition::Finished { .. } | WorkerDisposition::Status { terminal: true, .. }
        );
        if !emit(WorkerCompletion {
            reply_uses_8bit_controls: reply_mode,
            disposition,
        }) || terminal
        {
            break;
        }
    }
}
