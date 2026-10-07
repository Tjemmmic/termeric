//! The host-read worker: pins each requested regular file, re-opens it
//! through the pinned descriptor, and streams it back as Kitty data frames.

use super::*;

pub(super) fn pin_upload_path(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }

    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "approved path contains NUL",
        )
    })?;
    let how = OpenHow {
        flags: (libc::O_PATH | libc::O_CLOEXEC) as u64,
        mode: 0,
        resolve: libc::RESOLVE_NO_SYMLINKS,
    };
    // SAFETY: `path` is a live NUL-terminated CString, `how` points to a
    // fully initialized `open_how` for the supplied size, and a successful
    // syscall transfers ownership of one new descriptor to this function.
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
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `openat2` returned a new owned descriptor and it has not been
    // wrapped or closed elsewhere.
    Ok(unsafe { File::from_raw_fd(descriptor as libc::c_int) })
}

pub(super) fn reopen_upload_regular_file(pinned: &std::fs::File) -> std::io::Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    let identity = pinned.metadata()?;
    if !identity.is_file() || identity.nlink() != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "approved source is not a single-link regular file",
        ));
    }
    // Only the validated regular inode is opened for data. Never retry its
    // original name, which may now refer to a FIFO, device, link, or other file.
    let file = std::fs::File::open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || opened.nlink() != 1
        || (opened.dev(), opened.ino()) != (identity.dev(), identity.ino())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "read handle does not match the approved regular-file identity",
        ));
    }
    Ok(file)
}

/// Hands one upload's completions to the App and waits for each to be
/// delivered, so the worker never reads further ahead than the terminal.
struct Delivery<'a, E> {
    emit: E,
    session_id: &'a str,
    total_files: usize,
    reply_uses_8bit_controls: bool,
}

impl<E: FnMut(UploadWorkerCompletion) -> bool> Delivery<'_, E> {
    /// Deliver progress frames; false when the App did not write them to
    /// the originating terminal.
    fn send(&mut self, frames: Vec<Vec<u8>>, files: usize, bytes: u64) -> bool {
        self.emit_with_ack(frames, false, files, bytes, None)
    }

    /// End the upload with one error status for `file_id`, reporting only
    /// progress the terminal has already acknowledged.
    fn fail(
        &mut self,
        file_id: &str,
        code: KittyFileTransferStatusCode,
        message: String,
        files: usize,
        bytes: u64,
    ) {
        let frame = transfer_status(self.session_id, file_id, code, &message).canonical_reply();
        let _ = self.emit_with_ack(vec![frame], true, files, bytes, Some(message));
    }

    fn emit_with_ack(
        &mut self,
        frames: Vec<Vec<u8>>,
        terminal: bool,
        files: usize,
        bytes: u64,
        error: Option<String>,
    ) -> bool {
        let (sender, receiver) = sync_channel(1);
        if !(self.emit)(UploadWorkerCompletion {
            frames,
            terminal,
            files,
            total_files: self.total_files,
            bytes,
            error,
            reply_uses_8bit_controls: self.reply_uses_8bit_controls,
            acknowledge: sender,
        }) {
            return false;
        }
        // The App answers every completion it handles, false once the upload
        // is no longer active (including after cancellation), and a
        // completion for a closed window is dropped with its sender, so this
        // wait always ends.
        receiver.recv().unwrap_or(false)
    }
}

pub(super) fn run_upload_worker(
    receiver: Receiver<KittyFileTransferCommand>,
    session_id: String,
    paths: Vec<UploadPath>,
    reply_uses_8bit_controls: bool,
    cancellation: Arc<AtomicBool>,
    emit: impl FnMut(UploadWorkerCompletion) -> bool,
) {
    use std::fs::File;
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;

    struct Source {
        requested_name: String,
        path_name: String,
        file: File,
        size: u64,
        sent: bool,
    }

    let total_files = paths.len();
    let mut delivery = Delivery {
        emit,
        session_id: &session_id,
        total_files,
        reply_uses_8bit_controls,
    };
    let mut sources = HashMap::with_capacity(total_files);
    let mut metadata_frames = Vec::with_capacity(total_files + 1);
    let mut total_bytes = 0u64;
    for path in paths {
        if cancellation.load(Ordering::Acquire) {
            return;
        }
        // Nothing is sent until every path is pinned, so these failures
        // report no progress.
        let mut fail = |code, message: String| delivery.fail(&path.file_id, code, message, 0, 0);
        let file = match pin_upload_path(&path.path) {
            Ok(file) => file,
            Err(error) => {
                let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
                    KittyFileTransferStatusCode::PermissionDenied
                } else {
                    KittyFileTransferStatusCode::Io
                };
                fail(
                    code,
                    format!("could not open approved regular file: {error}"),
                );
                return;
            }
        };
        let metadata = match file.metadata() {
            Ok(metadata) if metadata.is_file() && metadata.nlink() == 1 => metadata,
            Ok(_) => {
                fail(
                    KittyFileTransferStatusCode::Invalid,
                    "approved path is not a single-link regular file".into(),
                );
                return;
            }
            Err(error) => {
                fail(
                    KittyFileTransferStatusCode::Io,
                    format!("could not inspect approved regular file: {error}"),
                );
                return;
            }
        };
        match std::fs::symlink_metadata(&path.path) {
            Ok(path_metadata)
                if path_metadata.is_file()
                    && path_metadata.dev() == metadata.dev()
                    && path_metadata.ino() == metadata.ino() => {}
            Ok(_) => {
                fail(
                    KittyFileTransferStatusCode::PermissionDenied,
                    "approved path changed identity while its file was being opened".into(),
                );
                return;
            }
            Err(error) => {
                fail(
                    KittyFileTransferStatusCode::Io,
                    format!("could not revalidate approved file identity: {error}"),
                );
                return;
            }
        }
        total_bytes = match total_bytes.checked_add(metadata.len()) {
            Some(total) if total <= MAX_KITTY_UPLOAD_BYTES => total,
            _ => {
                fail(
                    KittyFileTransferStatusCode::TooLarge,
                    "approved files exceed the 64 MiB transfer limit".into(),
                );
                return;
            }
        };
        let Some(path_name) = path.path.to_str().map(str::to_owned) else {
            fail(
                KittyFileTransferStatusCode::Invalid,
                "approved path is not valid UTF-8".into(),
            );
            return;
        };
        let file = match reopen_upload_regular_file(&file) {
            Ok(file) => file,
            Err(error) => {
                let code = match error.kind() {
                    std::io::ErrorKind::InvalidInput => KittyFileTransferStatusCode::Invalid,
                    std::io::ErrorKind::PermissionDenied => {
                        KittyFileTransferStatusCode::PermissionDenied
                    }
                    _ => KittyFileTransferStatusCode::Io,
                };
                fail(code, format!("could not read pinned regular file: {error}"));
                return;
            }
        };
        let mtime_ns = metadata
            .mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(metadata.mtime_nsec());
        metadata_frames.push(
            KittyFileTransferReply::File {
                id: session_id.clone(),
                file_id: path.file_id.clone(),
                actual_file_id: path.file_id.clone(),
                name: path_name.clone(),
                mtime_ns,
                permissions: metadata.mode() & 0o777,
                size: metadata.len(),
            }
            .canonical_reply(),
        );
        sources.insert(
            path.file_id,
            Source {
                requested_name: path.requested_name,
                path_name,
                file,
                size: metadata.len(),
                sent: false,
            },
        );
    }
    let home = directories::BaseDirs::new()
        .and_then(|dirs| dirs.home_dir().to_str().map(str::to_owned))
        .unwrap_or_else(|| "/".into());
    let mut ready = transfer_status(&session_id, "", KittyFileTransferStatusCode::Ok, "");
    ready.name = Some(home);
    metadata_frames.push(ready.canonical_reply());
    if !delivery.send(metadata_frames, 0, 0) {
        return;
    }

    let mut files_sent = 0usize;
    let mut bytes_sent = 0u64;
    loop {
        let command = match receiver.recv_timeout(WORKER_IDLE_TIMEOUT) {
            Ok(command) => command,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                delivery.fail(
                    "",
                    KittyFileTransferStatusCode::TimedOut,
                    "host file read expired after ten minutes of inactivity".into(),
                    files_sent,
                    bytes_sent,
                );
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if cancellation.load(Ordering::Acquire) {
            return;
        }
        let Some(source) = sources.get_mut(&command.file_id) else {
            delivery.fail(
                &command.file_id,
                KittyFileTransferStatusCode::Invalid,
                "requested an unknown approved file id".into(),
                files_sent,
                bytes_sent,
            );
            return;
        };
        if source.sent
            || command
                .name
                .as_deref()
                .is_none_or(|name| name != source.path_name && name != source.requested_name)
        {
            delivery.fail(
                &command.file_id,
                KittyFileTransferStatusCode::Invalid,
                "file was already sent or its path did not match approval".into(),
                files_sent,
                bytes_sent,
            );
            return;
        }
        let mut remaining = source.size;
        // Reads may fill a partial batch that is never delivered on failure.
        // Preserve only acknowledged current-file bytes in terminal errors.
        let mut acknowledged_bytes = 0;
        let mut batch = Vec::with_capacity(UPLOAD_BATCH_CHUNKS);
        if remaining == 0 {
            batch.push(
                KittyFileTransferReply::Data {
                    id: session_id.clone(),
                    file_id: command.file_id.clone(),
                    data: Box::default(),
                    last: true,
                }
                .canonical_reply(),
            );
            // No read-loop iteration will flush this frame. Empty files still
            // require end_data and the same delivery acknowledgement as payloads.
            if !delivery.send(std::mem::take(&mut batch), files_sent + 1, bytes_sent) {
                return;
            }
        }
        while remaining > 0 {
            if cancellation.load(Ordering::Acquire) {
                return;
            }
            let mut chunk = vec![0u8; termeric_core::MAX_KITTY_TRANSFER_DATA_BYTES];
            let wanted = usize::try_from(remaining.min(chunk.len() as u64)).unwrap_or(chunk.len());
            let count = match source.file.read(&mut chunk[..wanted]) {
                Ok(0) => {
                    delivery.fail(
                        &command.file_id,
                        KittyFileTransferStatusCode::Io,
                        "approved file changed or ended while being read".into(),
                        files_sent,
                        bytes_sent + acknowledged_bytes,
                    );
                    return;
                }
                Ok(count) => count,
                Err(error) => {
                    delivery.fail(
                        &command.file_id,
                        KittyFileTransferStatusCode::Io,
                        format!("could not read approved regular file: {error}"),
                        files_sent,
                        bytes_sent + acknowledged_bytes,
                    );
                    return;
                }
            };
            chunk.truncate(count);
            remaining -= count as u64;
            batch.push(
                KittyFileTransferReply::Data {
                    id: session_id.clone(),
                    file_id: command.file_id.clone(),
                    data: chunk.into_boxed_slice(),
                    last: remaining == 0,
                }
                .canonical_reply(),
            );
            if batch.len() == UPLOAD_BATCH_CHUNKS || remaining == 0 {
                if !delivery.send(
                    std::mem::take(&mut batch),
                    files_sent + usize::from(remaining == 0),
                    bytes_sent + (source.size - remaining),
                ) {
                    return;
                }
                acknowledged_bytes = source.size - remaining;
            }
        }
        bytes_sent = bytes_sent.saturating_add(source.size);
        source.sent = true;
        files_sent += 1;
    }
}

#[cfg(test)]
mod tests;
