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

pub(super) fn run_upload_worker(
    receiver: Receiver<KittyFileTransferCommand>,
    session_id: String,
    paths: Vec<UploadPath>,
    reply_uses_8bit_controls: bool,
    cancellation: Arc<AtomicBool>,
    mut emit: impl FnMut(UploadWorkerCompletion) -> bool,
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

    fn error_frame(
        session_id: &str,
        file_id: &str,
        code: KittyFileTransferStatusCode,
        message: &str,
    ) -> Vec<u8> {
        transfer_status(session_id, file_id, code, message).canonical_reply()
    }

    fn emit_with_ack(
        emit: &mut impl FnMut(UploadWorkerCompletion) -> bool,
        cancellation: &AtomicBool,
        frames: Vec<Vec<u8>>,
        terminal: bool,
        files: usize,
        total_files: usize,
        bytes: u64,
        error: Option<String>,
        reply_uses_8bit_controls: bool,
    ) -> bool {
        let (sender, receiver) = sync_channel(1);
        if !emit(UploadWorkerCompletion {
            frames,
            terminal,
            files,
            total_files,
            bytes,
            error,
            reply_uses_8bit_controls,
            acknowledge: sender,
        }) {
            return false;
        }
        loop {
            if cancellation.load(Ordering::Acquire) {
                return false;
            }
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(written) => return written,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    let total_files = paths.len();
    let mut sources = HashMap::with_capacity(total_files);
    let mut metadata_frames = Vec::with_capacity(total_files + 1);
    let mut total_bytes = 0u64;
    for path in paths {
        if cancellation.load(Ordering::Acquire) {
            return;
        }
        let file = match pin_upload_path(&path.path) {
            Ok(file) => file,
            Err(error) => {
                let message = format!("could not open approved regular file: {error}");
                let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
                    KittyFileTransferStatusCode::PermissionDenied
                } else {
                    KittyFileTransferStatusCode::Io
                };
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(&session_id, &path.file_id, code, &message)],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
        };
        let metadata = match file.metadata() {
            Ok(metadata) if metadata.is_file() && metadata.nlink() == 1 => metadata,
            Ok(_) => {
                let message = "approved path is not a single-link regular file".to_string();
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::Invalid,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
            Err(error) => {
                let message = format!("could not inspect approved regular file: {error}");
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::Io,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
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
                let message =
                    "approved path changed identity while its file was being opened".to_string();
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::PermissionDenied,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
            Err(error) => {
                let message = format!("could not revalidate approved file identity: {error}");
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::Io,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
        }
        total_bytes = match total_bytes.checked_add(metadata.len()) {
            Some(total) if total <= MAX_KITTY_UPLOAD_BYTES => total,
            _ => {
                let message = "approved files exceed the 64 MiB transfer limit".to_string();
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        &path.file_id,
                        KittyFileTransferStatusCode::TooLarge,
                        &message,
                    )],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
        };
        let Some(path_name) = path.path.to_str().map(str::to_owned) else {
            let message = "approved path is not valid UTF-8".to_string();
            let _ = emit_with_ack(
                &mut emit,
                &cancellation,
                vec![error_frame(
                    &session_id,
                    &path.file_id,
                    KittyFileTransferStatusCode::Invalid,
                    &message,
                )],
                true,
                0,
                total_files,
                0,
                Some(message),
                reply_uses_8bit_controls,
            );
            return;
        };
        let file = match reopen_upload_regular_file(&file) {
            Ok(file) => file,
            Err(error) => {
                let message = format!("could not read pinned regular file: {error}");
                let code = match error.kind() {
                    std::io::ErrorKind::InvalidInput => KittyFileTransferStatusCode::Invalid,
                    std::io::ErrorKind::PermissionDenied => {
                        KittyFileTransferStatusCode::PermissionDenied
                    }
                    _ => KittyFileTransferStatusCode::Io,
                };
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(&session_id, &path.file_id, code, &message)],
                    true,
                    0,
                    total_files,
                    0,
                    Some(message),
                    reply_uses_8bit_controls,
                );
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
    if !emit_with_ack(
        &mut emit,
        &cancellation,
        metadata_frames,
        false,
        0,
        total_files,
        0,
        None,
        reply_uses_8bit_controls,
    ) {
        return;
    }

    let mut files_sent = 0usize;
    let mut bytes_sent = 0u64;
    loop {
        let command = match receiver.recv_timeout(WORKER_IDLE_TIMEOUT) {
            Ok(command) => command,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let message = "host file read expired after ten minutes of inactivity".to_string();
                let _ = emit_with_ack(
                    &mut emit,
                    &cancellation,
                    vec![error_frame(
                        &session_id,
                        "",
                        KittyFileTransferStatusCode::TimedOut,
                        &message,
                    )],
                    true,
                    files_sent,
                    total_files,
                    bytes_sent,
                    Some(message),
                    reply_uses_8bit_controls,
                );
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if cancellation.load(Ordering::Acquire) {
            return;
        }
        let Some(source) = sources.get_mut(&command.file_id) else {
            let message = "requested an unknown approved file id".to_string();
            let _ = emit_with_ack(
                &mut emit,
                &cancellation,
                vec![error_frame(
                    &session_id,
                    &command.file_id,
                    KittyFileTransferStatusCode::Invalid,
                    &message,
                )],
                true,
                files_sent,
                total_files,
                bytes_sent,
                Some(message),
                reply_uses_8bit_controls,
            );
            return;
        };
        if source.sent
            || command
                .name
                .as_deref()
                .is_none_or(|name| name != source.path_name && name != source.requested_name)
        {
            let message = "file was already sent or its path did not match approval".to_string();
            let _ = emit_with_ack(
                &mut emit,
                &cancellation,
                vec![error_frame(
                    &session_id,
                    &command.file_id,
                    KittyFileTransferStatusCode::Invalid,
                    &message,
                )],
                true,
                files_sent,
                total_files,
                bytes_sent,
                Some(message),
                reply_uses_8bit_controls,
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
            if !emit_with_ack(
                &mut emit,
                &cancellation,
                std::mem::take(&mut batch),
                false,
                files_sent + 1,
                total_files,
                bytes_sent,
                None,
                reply_uses_8bit_controls,
            ) {
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
                    let message = "approved file changed or ended while being read".to_string();
                    let _ = emit_with_ack(
                        &mut emit,
                        &cancellation,
                        vec![error_frame(
                            &session_id,
                            &command.file_id,
                            KittyFileTransferStatusCode::Io,
                            &message,
                        )],
                        true,
                        files_sent,
                        total_files,
                        bytes_sent + acknowledged_bytes,
                        Some(message),
                        reply_uses_8bit_controls,
                    );
                    return;
                }
                Ok(count) => count,
                Err(error) => {
                    let message = format!("could not read approved regular file: {error}");
                    let _ = emit_with_ack(
                        &mut emit,
                        &cancellation,
                        vec![error_frame(
                            &session_id,
                            &command.file_id,
                            KittyFileTransferStatusCode::Io,
                            &message,
                        )],
                        true,
                        files_sent,
                        total_files,
                        bytes_sent + acknowledged_bytes,
                        Some(message),
                        reply_uses_8bit_controls,
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
                if !emit_with_ack(
                    &mut emit,
                    &cancellation,
                    std::mem::take(&mut batch),
                    false,
                    files_sent + usize::from(remaining == 0),
                    total_files,
                    bytes_sent + (source.size - remaining),
                    None,
                    reply_uses_8bit_controls,
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
