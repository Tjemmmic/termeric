//! Linux filesystem leaf operations for Kitty file transfer. A descriptor
//! pins an inode without opening its contents (`O_PATH`), `openat2` refuses a
//! symbolic link anywhere in a path, and procfs names an entry inside a
//! pinned directory, or a pinned inode itself, without resolving any path
//! someone may have swapped since.

use super::download::EntryIdentity;
use std::ffi::{CString, OsStr};
use std::fs::{self, File, Permissions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tempfile::{NamedTempFile, TempPath};

/// Open `path` with `flags` (plus close-on-exec) through `openat2`, refusing
/// a symbolic link anywhere along it, so the descriptor pins exactly the
/// inode the path named when it was checked.
fn openat2_no_symlinks(path: &Path, flags: libc::c_int) -> io::Result<File> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    let how = OpenHow {
        flags: (flags | libc::O_CLOEXEC) as u64,
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
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `openat2` returned a new owned descriptor and it has not been
    // wrapped or closed elsewhere.
    Ok(unsafe { File::from_raw_fd(descriptor as libc::c_int) })
}

/// Pin the file an upload names, without following a symbolic link anywhere
/// along `path` and without opening its contents: an `O_PATH` pin of a FIFO
/// or device neither blocks nor touches it. Always `Some` on Linux, which
/// can pin anything and leaves the type checks to the caller.
pub(super) fn pin_upload_source(path: &Path) -> io::Result<Option<File>> {
    openat2_no_symlinks(path, libc::O_PATH).map(Some)
}

/// A handle that reads the inode `pinned` holds. Only that validated inode is
/// opened for data, through procfs; its original name is never retried, as it
/// may now refer to a FIFO, device, link, or other file.
pub(super) fn upload_read_handle(pinned: &File) -> io::Result<File> {
    File::open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
}

/// A directory pinned without following a symbolic link anywhere in its
/// path. Entries inside it are reached through the descriptor's procfs name,
/// so moving or replacing the directory's path afterwards changes nothing.
pub(super) struct PinnedDirectory {
    handle: File,
}

impl PinnedDirectory {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            handle: openat2_no_symlinks(path, libc::O_PATH | libc::O_DIRECTORY)?,
        })
    }

    #[cfg(test)]
    pub(super) fn handle(&self) -> &File {
        &self.handle
    }

    /// The trusted procfs name of the pinned directory itself.
    fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.handle.as_raw_fd()))
    }

    fn entry_path(&self, name: &OsStr) -> PathBuf {
        self.path().join(name)
    }

    pub(super) fn entry_identity(&self, name: &OsStr) -> io::Result<EntryIdentity> {
        Ok(EntryIdentity::of(&fs::symlink_metadata(
            self.entry_path(name),
        )?))
    }

    pub(super) fn create_directory(&self, name: &OsStr) -> io::Result<()> {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(self.entry_path(name))
    }

    pub(super) fn create_symlink(&self, target: &Path, name: &OsStr) -> io::Result<()> {
        std::os::unix::fs::symlink(target, self.entry_path(name))
    }

    /// A handle on the directory `name` that can change its permissions and
    /// times. One whose permissions umask removed is pinned with `O_PATH`
    /// instead, without reading it or following a symlink, for
    /// [`usable_directory_handle`] to recover.
    pub(super) fn open_directory_guard(&self, name: &OsStr) -> io::Result<File> {
        let path = self.entry_path(name);
        let result = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path);
        if result
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
        {
            return fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(&path);
        }
        result
    }

    /// A handle pinning the regular file `name`, never its contents.
    pub(super) fn open_file_guard(&self, name: &OsStr) -> io::Result<File> {
        self.open_entry_pin(name)
    }

    /// A handle pinning the symbolic link `name` itself. O_PATH pins even a
    /// dangling link without opening its target or requiring readers.
    pub(super) fn open_symlink_guard(&self, name: &OsStr) -> io::Result<File> {
        self.open_entry_pin(name)
    }

    fn open_entry_pin(&self, name: &OsStr) -> io::Result<File> {
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
            .open(self.entry_path(name))
    }

    /// Link the staged inode itself at `name`: whatever its staging name
    /// now refers to is never consulted.
    pub(super) fn publish(&self, name: &OsStr, staged: StagedFile) -> io::Result<File> {
        self.link_inode(name, staged.as_file())
    }

    /// Hard-link the inode `source` holds at `name`. `_source_path` is only
    /// the macOS lookup key; Linux links the retained inode.
    pub(super) fn link_file(
        &self,
        name: &OsStr,
        source: &File,
        _source_path: &Path,
    ) -> io::Result<File> {
        self.link_inode(name, source)
    }

    fn link_inode(&self, name: &OsStr, file: &File) -> io::Result<File> {
        // Acquire the returned handle before publication so a descriptor
        // allocation failure cannot leave an unrecorded destination behind.
        let published = file.try_clone()?;
        let source = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .expect("descriptor path contains no NUL");
        let destination = CString::new(self.entry_path(name).as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))?;
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
        Ok(published)
    }
}

/// A directory handle that can change permissions, when `directory` is an
/// `O_PATH` pin of one whose permissions umask removed: restore owner access
/// through the pin's procfs name, which names the retained inode even if
/// the destination or an ancestor was replaced, and reopen it. `None` when
/// `directory` is already usable. If procfs is unavailable this fails
/// without a destination fallback.
pub(super) fn usable_directory_handle(directory: &File) -> io::Result<Option<File>> {
    // SAFETY: directory owns a live descriptor; F_GETFL takes no
    // additional argument and does not mutate descriptor state.
    let flags = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::O_PATH == 0 {
        return Ok(None);
    }
    // Keep directory borrowed until reopening is complete.
    let proc_path = format!("/proc/self/fd/{}", directory.as_raw_fd());
    fs::set_permissions(&proc_path, Permissions::from_mode(0o700))?;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK)
        .open(&proc_path)
        .map(Some)
}

/// Set the access and modification times of the inode `file` pins, a
/// symbolic link or a regular file, never a link's target.
pub(super) fn set_link_times(file: &File, times: &[libc::timespec; 2]) -> io::Result<()> {
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

/// A file being received, staged under a temporary name beside where it
/// will be published.
pub(super) struct StagedFile {
    file: File,
    path: TempPath,
    // Must remain open through Drop's identity check and unlink.
    _parent: Option<PinnedDirectory>,
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
    pub(super) fn create_in(parent: PinnedDirectory) -> io::Result<Self> {
        let temporary = tempfile::Builder::new()
            .prefix(".termeric-transfer-")
            .tempfile_in(parent.path())?;
        let mut staged = Self::from(temporary);
        // TempPath preserves this trusted procfs path. Keep its parent
        // descriptor alive until StagedFile::drop finishes cleanup.
        staged._parent = Some(parent);
        Ok(staged)
    }

    pub(super) fn as_file(&self) -> &File {
        &self.file
    }

    pub(super) fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// The name staging gave its entry in the staging directory.
    #[cfg(test)]
    pub(super) fn entry_name(&self) -> &OsStr {
        self.path.file_name().expect("staging path has a name")
    }

    /// The pinned directory a staged file was created in.
    #[cfg(test)]
    pub(super) fn staging_parent(&self) -> Option<&File> {
        self._parent.as_ref().map(PinnedDirectory::handle)
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        use std::os::unix::fs::MetadataExt;

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

/// Where the data of a staged entry found at `entry` lives: on Linux the
/// entry is the staged file itself.
#[cfg(test)]
pub(super) fn staged_data_path(entry: &Path) -> PathBuf {
    entry.to_path_buf()
}
