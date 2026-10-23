//! Kitty image transmission: chunked payloads, files, temporary files, and
//! shared memory.

use super::*;

pub(super) fn prepare_transmission_data(
    cmd: &mut KittyCommand,
    allow_file_transmission: bool,
) -> Result<(), ProtocolError> {
    match cmd.transmission.medium {
        KittyMedium::Direct => {
            if cmd.transmission.data_offset.is_some() {
                return Err(ProtocolError::invalid(
                    "O is only valid for local transmission media",
                ));
            }
            Ok(())
        }
        KittyMedium::File => {
            ensure_files_allowed(allow_file_transmission)?;
            if cmd.more_chunks {
                return Err(ProtocolError::invalid(
                    "ordinary-file transmission cannot be chunked",
                ));
            }
            cmd.data = read_ordinary_file(
                &cmd.data,
                cmd.transmission.data_offset.unwrap_or(0),
                cmd.transmission.data_size,
            )?;
            Ok(())
        }
        KittyMedium::SharedMemory => {
            if cmd.more_chunks {
                return Err(ProtocolError::invalid(
                    "shared-memory transmission cannot be chunked",
                ));
            }
            cmd.data = read_and_unlink_shared_memory(
                &cmd.data,
                cmd.transmission.data_offset.unwrap_or(0),
                cmd.transmission.data_size,
            )?;
            Ok(())
        }
        KittyMedium::TempFile => {
            ensure_files_allowed(allow_file_transmission)?;
            if cmd.more_chunks {
                return Err(ProtocolError::invalid(
                    "temporary-file transmission cannot be chunked",
                ));
            }
            cmd.data = read_and_remove_temporary_file(
                &cmd.data,
                cmd.transmission.data_offset.unwrap_or(0),
                cmd.transmission.data_size,
            )?;
            Ok(())
        }
    }
}

/// Ordinary and temporary files are one policy: clients such as `kitten
/// icat` probe a temporary file and then send ordinary ones, so answering the
/// two differently would have them send images that are then refused.
fn ensure_files_allowed(allowed: bool) -> Result<(), ProtocolError> {
    if allowed {
        Ok(())
    } else {
        Err(ProtocolError::permission_denied(
            "file transmission is disabled by host policy",
        ))
    }
}

pub(super) fn read_ordinary_file(
    encoded_path: &[u8],
    offset: u64,
    requested_size: Option<u64>,
) -> Result<Vec<u8>, ProtocolError> {
    let path = Path::new(std::ffi::OsStr::from_bytes(encoded_path));
    if encoded_path.is_empty() || !path.is_absolute() {
        return Err(ProtocolError::invalid(
            "ordinary-file path must be absolute",
        ));
    }

    // Resolve symlinks as Kitty specifies, then apply policy to the final
    // target. Device and kernel pseudo-filesystems must never become an
    // alternate path around regular-file validation.
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|error| filesystem_error(error, "ordinary file is unavailable"))?;
    if is_sensitive_file_root(&canonical_path) {
        return Err(ProtocolError::permission_denied(
            "ordinary file is inside a protected system filesystem",
        ));
    }

    let mut file = open_local_graphics_file(&canonical_path)
        .map_err(|error| filesystem_error(error, "ordinary file could not be opened"))?;
    let opened = file
        .metadata()
        .map_err(|error| filesystem_error(error, "ordinary file metadata is unavailable"))?;
    if !opened.file_type().is_file() {
        return Err(ProtocolError::invalid(
            "ordinary-file medium requires a regular file",
        ));
    }
    ensure_ordinary_file_unchanged(&canonical_path, &opened)?;

    let available = opened
        .len()
        .checked_sub(offset)
        .ok_or_else(|| ProtocolError::invalid("ordinary-file offset exceeds file size"))?;
    let size = requested_size.unwrap_or(available).min(available);
    if size > MAX_TRANSMISSION_BYTES as u64 {
        return Err(ProtocolError::too_large("ordinary-file data is too large"));
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| ProtocolError::io("ordinary file could not be positioned"))?;
    let mut data = Vec::with_capacity(size as usize);
    file.by_ref()
        .take(size)
        .read_to_end(&mut data)
        .map_err(|_| ProtocolError::io("ordinary file could not be read"))?;
    if data.len() as u64 != size {
        return Err(ProtocolError::io(
            "ordinary file ended before the requested range",
        ));
    }
    ensure_ordinary_file_unchanged(&canonical_path, &opened)?;
    Ok(data)
}

pub(super) fn ensure_ordinary_file_unchanged(
    path: &Path,
    opened: &std::fs::Metadata,
) -> Result<(), ProtocolError> {
    let current_path = std::fs::canonicalize(path)
        .map_err(|error| filesystem_error(error, "ordinary file changed while being read"))?;
    if is_sensitive_file_root(&current_path) {
        return Err(ProtocolError::permission_denied(
            "ordinary file moved into a protected system filesystem",
        ));
    }
    let current = std::fs::metadata(&current_path)
        .map_err(|error| filesystem_error(error, "ordinary file changed while being read"))?;
    if !current.file_type().is_file() || !same_file(opened, &current) {
        return Err(ProtocolError::invalid(
            "ordinary file changed while being read",
        ));
    }
    Ok(())
}

fn is_sensitive_file_root(path: &Path) -> bool {
    [Path::new("/dev"), Path::new("/proc"), Path::new("/sys")]
        .into_iter()
        .any(|root| path.starts_with(root))
}

fn read_and_remove_temporary_file(
    encoded_path: &[u8],
    offset: u64,
    requested_size: Option<u64>,
) -> Result<Vec<u8>, ProtocolError> {
    let path = Path::new(std::ffi::OsStr::from_bytes(encoded_path));
    if encoded_path.is_empty() || !path.is_absolute() {
        return Err(ProtocolError::invalid(
            "temporary-file path must be absolute",
        ));
    }
    if !encoded_path
        .windows(b"tty-graphics-protocol".len())
        .any(|window| window == b"tty-graphics-protocol")
    {
        return Err(ProtocolError::invalid(
            "temporary-file path is missing its safety marker",
        ));
    }

    let parent = path
        .parent()
        .ok_or_else(|| ProtocolError::invalid("temporary-file path has no parent"))?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|error| filesystem_error(error, "temporary-file directory is unavailable"))?;
    if !allowed_temporary_roots().contains(&canonical_parent) {
        return Err(ProtocolError::invalid(
            "temporary file is outside an allowed temporary directory",
        ));
    }

    let before = std::fs::symlink_metadata(path)
        .map_err(|error| filesystem_error(error, "temporary file is unavailable"))?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err(ProtocolError::invalid(
            "temporary-file medium requires a regular non-symlink file",
        ));
    }

    let (mut file, opened) = open_unchanged_temporary_file(path, &before)?;

    let read_result = (|| {
        let available = opened
            .len()
            .checked_sub(offset)
            .ok_or_else(|| ProtocolError::invalid("temporary-file offset exceeds file size"))?;
        let size = requested_size.unwrap_or(available).min(available);
        if size > MAX_TRANSMISSION_BYTES as u64 {
            return Err(ProtocolError::too_large("temporary-file data is too large"));
        }
        // S is a read ceiling. A short local object supplies all remaining
        // bytes, matching ordinary read-to-EOF semantics in Kitty's loader.

        file.seek(SeekFrom::Start(offset))
            .map_err(|_| ProtocolError::io("temporary file could not be positioned"))?;
        let mut data = Vec::with_capacity(size as usize);
        file.by_ref()
            .take(size)
            .read_to_end(&mut data)
            .map_err(|_| ProtocolError::io("temporary file could not be read"))?;
        if data.len() as u64 != size {
            return Err(ProtocolError::io(
                "temporary file ended before the requested range",
            ));
        }
        Ok(data)
    })();

    let cleanup_result = remove_temporary_file_if_unchanged(path, &opened);
    match (read_result, cleanup_result) {
        (_, Err(error)) => Err(error),
        (result, Ok(())) => result,
    }
}

fn open_local_graphics_file(path: &Path) -> std::io::Result<std::fs::File> {
    // Type validation occurs on the descriptor. A FIFO substituted before
    // open must not wait for a peer before that validation can reject it.
    // O_NONBLOCK has no effect on reads from the required regular files.
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

pub(super) fn open_unchanged_temporary_file(
    path: &Path,
    before: &std::fs::Metadata,
) -> Result<(std::fs::File, std::fs::Metadata), ProtocolError> {
    let file = open_local_graphics_file(path)
        .map_err(|error| filesystem_error(error, "temporary file could not be opened"))?;
    let opened = file
        .metadata()
        .map_err(|error| filesystem_error(error, "temporary file metadata is unavailable"))?;
    if !opened.file_type().is_file() || !same_file(before, &opened) {
        return Err(ProtocolError::invalid(
            "temporary file changed before it could be read",
        ));
    }

    Ok((file, opened))
}

pub(super) fn read_and_unlink_shared_memory(
    name: &[u8],
    offset: u64,
    requested_size: Option<u64>,
) -> Result<Vec<u8>, ProtocolError> {
    const MAX_SHARED_MEMORY_NAME_BYTES: usize = 255;

    let component = name.strip_prefix(b"/").unwrap_or(name);
    if component.is_empty()
        || component.len() + 1 > MAX_SHARED_MEMORY_NAME_BYTES
        || component.contains(&b'/')
    {
        return Err(ProtocolError::invalid(
            "shared-memory name must be one bounded POSIX name",
        ));
    }
    // POSIX shm_open requires a leading slash. Kitty's reference client sends
    // the portable object component without it, so normalize that one safe
    // form after rejecting nested names.
    let mut normalized_name = Vec::with_capacity(component.len() + 1);
    normalized_name.push(b'/');
    normalized_name.extend_from_slice(component);
    let name = std::ffi::CString::new(normalized_name)
        .map_err(|_| ProtocolError::invalid("shared-memory name contains NUL"))?;
    let mut file = open_graphics_shared_memory(&name)
        .map_err(|error| filesystem_error(error, "shared-memory object is unavailable"))?;
    let metadata = file
        .metadata()
        .map_err(|error| filesystem_error(error, "shared-memory metadata is unavailable"))?;
    // SAFETY: `geteuid` has no preconditions and does not access Rust memory.
    let effective_uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid
        || metadata.mode() & 0o077 != 0
    {
        return Err(ProtocolError::invalid(
            "shared-memory object must be a private regular object owned by this user",
        ));
    }

    unlink_shared_memory_if_unchanged(&name, &metadata)?;

    let available = metadata
        .len()
        .checked_sub(offset)
        .ok_or_else(|| ProtocolError::invalid("shared-memory offset exceeds object size"))?;
    let size = requested_size.unwrap_or(available).min(available);
    if size > MAX_TRANSMISSION_BYTES as u64 {
        return Err(ProtocolError::too_large("shared-memory data is too large"));
    }
    // S is a read ceiling. A short object supplies all remaining bytes.

    read_shared_memory_range(&mut file, offset, size)
}

fn read_shared_memory_range(
    file: &mut std::fs::File,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>, ProtocolError> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| ProtocolError::io("shared-memory object could not be positioned"))?;
    let mut data = Vec::with_capacity(size as usize);
    file.by_ref()
        .take(size)
        .read_to_end(&mut data)
        .map_err(|_| ProtocolError::io("shared-memory object could not be read"))?;
    if data.len() as u64 != size {
        return Err(ProtocolError::io(
            "shared-memory object ended before the requested range",
        ));
    }
    Ok(data)
}

pub(super) fn unlink_shared_memory_if_unchanged(
    name: &std::ffi::CStr,
    opened: &std::fs::Metadata,
) -> Result<(), ProtocolError> {
    let current = open_graphics_shared_memory(name).map_err(|error| {
        filesystem_error(error, "shared-memory object disappeared before cleanup")
    })?;
    let current_metadata = current
        .metadata()
        .map_err(|error| filesystem_error(error, "shared-memory metadata is unavailable"))?;
    if !same_file(opened, &current_metadata) {
        return Err(ProtocolError::invalid(
            "shared-memory object changed before cleanup",
        ));
    }
    // Unlink before reading. POSIX keeps both open descriptors valid until
    // drop, while removing the attacker-visible name for every later failure.
    // SAFETY: `name` remains a live NUL-terminated C string for this call.
    if unsafe { libc::shm_unlink(name.as_ptr()) } != 0 {
        return Err(filesystem_error(
            std::io::Error::last_os_error(),
            "shared-memory object could not be unlinked",
        ));
    }
    Ok(())
}

fn open_graphics_shared_memory(name: &std::ffi::CStr) -> std::io::Result<std::fs::File> {
    // Linux shm names refer to /dev/shm entries: initial and cleanup opens
    // must reject FIFO/symlink substitutions without waiting for a writer.
    // SAFETY: name is a live C string; flags never create or truncate an object.
    let descriptor = unsafe {
        libc::shm_open(
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW,
            0,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: this is the sole adoption of the newly owned valid descriptor.
    Ok(unsafe { std::fs::File::from_raw_fd(descriptor) })
}

fn allowed_temporary_roots() -> Vec<PathBuf> {
    [
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/dev/shm"),
    ]
    .into_iter()
    .filter_map(|root| std::fs::canonicalize(root).ok())
    .fold(Vec::new(), |mut roots, root| {
        if !roots.contains(&root) {
            roots.push(root);
        }
        roots
    })
}

fn remove_temporary_file_if_unchanged(
    path: &Path,
    opened: &std::fs::Metadata,
) -> Result<(), ProtocolError> {
    let current = std::fs::symlink_metadata(path)
        .map_err(|error| filesystem_error(error, "temporary file disappeared before cleanup"))?;
    if current.file_type().is_symlink()
        || !current.file_type().is_file()
        || !same_file(&current, opened)
    {
        return Err(ProtocolError::invalid(
            "temporary file changed before cleanup",
        ));
    }
    std::fs::remove_file(path)
        .map_err(|error| filesystem_error(error, "temporary file could not be removed"))
}

fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn filesystem_error(error: std::io::Error, message: &'static str) -> ProtocolError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ProtocolError::not_found(message),
        std::io::ErrorKind::PermissionDenied => ProtocolError::permission_denied(message),
        _ => ProtocolError::io(message),
    }
}

impl GraphicsManager {
    pub(super) fn start_transmission(
        &mut self,
        mut cmd: KittyCommand,
        alternate_screen: bool,
    ) -> GraphicsCommandResult {
        if self.pending_frame.is_some() {
            return failure(
                &cmd,
                ProtocolError::invalid("an animation frame transmission is already pending"),
            );
        }
        if let Err(error) = prepare_transmission_data(&mut cmd, self.allow_file_transmission) {
            return failure(&cmd, error);
        }
        if cmd.data.len() > MAX_TRANSMISSION_BYTES {
            return failure(&cmd, ProtocolError::too_large("transmission is too large"));
        }
        let response_number = cmd.image_number.filter(|number| *number != 0);
        let explicit_id = cmd.image_id.filter(|id| *id != 0).map(ImageId);
        let storage_id = if let Some(id) = explicit_id {
            id
        } else if response_number.is_some() {
            match self.allocate_client_id() {
                Ok(id) => id,
                Err(error) => return failure(&cmd, error),
            }
        } else {
            self.allocate_anonymous_id()
        };
        let response_id = explicit_id.map_or(storage_id.0, |id| id.0);
        let response_id = if explicit_id.is_none() && response_number.is_none() {
            0
        } else {
            response_id
        };
        if cmd.placement.virtual_placement && response_id == 0 {
            return failure(
                &cmd,
                ProtocolError::invalid("Unicode placeholder placement requires an image ID"),
            );
        }
        let pending = PendingImage {
            storage_id,
            response_id,
            response_number,
            placement_id: cmd.placement_id.filter(|id| *id != 0 && response_id != 0),
            transmission: cmd.transmission,
            action: cmd.action,
            placement: cmd.placement,
            data: cmd.data,
            quiet: cmd.quiet,
            alternate_screen,
        };
        if cmd.more_chunks {
            self.pending = Some(pending);
            return GraphicsCommandResult::default();
        }
        self.finalize_image(pending, false)
    }

    pub(super) fn continue_transmission(&mut self, cmd: KittyCommand) -> GraphicsCommandResult {
        let Some(mut pending) = self.pending.take() else {
            return failure(&cmd, ProtocolError::invalid("no transmission is pending"));
        };
        if cmd.transmission.medium != KittyMedium::Direct {
            return pending_failure(
                pending,
                ProtocolError::invalid("continuation chunks must use direct transmission"),
            );
        }
        if cmd.image_id.is_some() && cmd.image_id != Some(pending.response_id) {
            return GraphicsCommandResult {
                response: response_fields(
                    pending.response_id,
                    pending.response_number,
                    pending.placement_id,
                    pending.quiet,
                    Err(ProtocolError::invalid("chunk interrupted by another image")),
                ),
                cursor_movement: None,
            };
        }
        let Some(new_len) = pending.data.len().checked_add(cmd.data.len()) else {
            return pending_failure(pending, ProtocolError::too_large("transmission overflow"));
        };
        if new_len > MAX_TRANSMISSION_BYTES {
            return pending_failure(
                pending,
                ProtocolError::too_large("transmission is too large"),
            );
        }
        pending.data.extend_from_slice(&cmd.data);
        if cmd.more_chunks {
            self.pending = Some(pending);
            GraphicsCommandResult::default()
        } else {
            self.finalize_image(pending, false)
        }
    }

    pub(super) fn start_frame_transmission(
        &mut self,
        mut cmd: KittyCommand,
        now: Instant,
    ) -> GraphicsCommandResult {
        if self.pending.is_some() {
            return failure(
                &cmd,
                ProtocolError::invalid("an image transmission is already pending"),
            );
        }
        if let Err(error) = prepare_transmission_data(&mut cmd, self.allow_file_transmission) {
            return failure(&cmd, error);
        }
        if cmd.data.len() > MAX_TRANSMISSION_BYTES {
            return failure(&cmd, ProtocolError::too_large("transmission is too large"));
        }
        let Some(image_id) = self.resolve_image_reference(&cmd) else {
            return failure(
                &cmd,
                ProtocolError::not_found("animation frame image was not found"),
            );
        };
        let Some(image) = self.storage.get(image_id) else {
            return failure(
                &cmd,
                ProtocolError::not_found("animation frame image was not found"),
            );
        };
        if cmd.transmission.format != KittyFormat::Png {
            cmd.transmission.width = cmd.transmission.width.or(Some(image.width));
            cmd.transmission.height = cmd.transmission.height.or(Some(image.height));
        }
        let pending = PendingFrame {
            image_id,
            response_id: image_id.0,
            response_number: cmd.image_number.filter(|number| *number != 0),
            transmission: cmd.transmission,
            frame: cmd.frame,
            data: cmd.data,
            quiet: cmd.quiet,
        };
        if cmd.more_chunks {
            self.pending_frame = Some(pending);
            return GraphicsCommandResult::default();
        }
        self.finalize_frame_transmission(pending, now)
    }

    pub(super) fn continue_frame_transmission(
        &mut self,
        cmd: KittyCommand,
        now: Instant,
    ) -> GraphicsCommandResult {
        let Some(mut pending) = self.pending_frame.take() else {
            return failure(
                &cmd,
                ProtocolError::invalid("no frame transmission is pending"),
            );
        };
        if cmd.transmission.medium != KittyMedium::Direct {
            return pending_frame_failure(
                pending,
                ProtocolError::invalid("continuation chunks must use direct transmission"),
            );
        }
        if cmd.image_id.is_some_and(|id| id != pending.response_id)
            || cmd
                .image_number
                .is_some_and(|number| Some(number) != pending.response_number)
        {
            return pending_frame_failure(
                pending,
                ProtocolError::invalid("chunk interrupted by another image"),
            );
        }
        let Some(new_len) = pending.data.len().checked_add(cmd.data.len()) else {
            return pending_frame_failure(
                pending,
                ProtocolError::too_large("transmission overflow"),
            );
        };
        if new_len > MAX_TRANSMISSION_BYTES {
            return pending_frame_failure(
                pending,
                ProtocolError::too_large("transmission is too large"),
            );
        }
        pending.data.extend_from_slice(&cmd.data);
        if cmd.quiet != 0 {
            pending.quiet = cmd.quiet;
        }
        if cmd.more_chunks {
            self.pending_frame = Some(pending);
            GraphicsCommandResult::default()
        } else {
            self.finalize_frame_transmission(pending, now)
        }
    }

    fn finalize_frame_transmission(
        &mut self,
        pending: PendingFrame,
        now: Instant,
    ) -> GraphicsCommandResult {
        let decoded = match decode_payload(&pending.transmission, &pending.data) {
            Ok(decoded) => decoded,
            Err(error) => return pending_frame_failure(pending, error),
        };
        let frame_number =
            match self.apply_frame_update(pending.image_id, pending.frame, decoded, now) {
                Ok(frame_number) => frame_number,
                Err(error) => return pending_frame_failure(pending, error),
            };
        self.storage.touch(pending.image_id);
        let response = frame_response_fields(
            pending.response_id,
            pending.response_number,
            Some(frame_number),
            pending.quiet,
            Ok(()),
        );
        GraphicsCommandResult {
            response,
            cursor_movement: None,
        }
    }
}
