//! macOS filesystem leaf operations for Kitty file transfer. macOS has no
//! `O_PATH` and no procfs, so the same guarantees come from descriptors:
//! `O_NOFOLLOW_ANY` refuses a symbolic link anywhere in a path, a directory
//! is pinned with `O_SEARCH` and worked in only through `*at` calls, a
//! symbolic link is held with `O_SYMLINK`, and the steps that macOS can only
//! do by name (publishing by rename, hard links) confirm afterwards that the
//! name holds the recorded inode, and take back and refuse anything else.

use super::download::{EntryIdentity, EntryType};
use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File, Permissions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use tempfile::NamedTempFile;

/// The staged file's name inside the private staging directory.
const STAGED_FILE_NAME: &str = "data";

/// One path component as a C string: never empty, never a separator, so an
/// `*at` call can only name an entry directly inside its directory.
fn component(name: &OsStr) -> io::Result<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.contains(&b'/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "name must be one path component",
        ));
    }
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))
}

fn result(status: libc::c_int) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The error for a path that goes through a symbolic link. On macOS that is
/// also every path under `/tmp`, `/var`, and `/etc`, links into `/private`.
fn symbolic_link_refused() -> io::Error {
    io::Error::new(
        io::Error::from_raw_os_error(libc::ELOOP).kind(),
        "the path goes through a symbolic link",
    )
}

/// Name the cause when an `O_NOFOLLOW_ANY` or `O_NOFOLLOW` open met a link.
fn explain_symbolic_link(error: io::Error) -> io::Error {
    if error.raw_os_error() == Some(libc::ELOOP) {
        symbolic_link_refused()
    } else {
        error
    }
}

fn identity_of(stat: &libc::stat) -> EntryIdentity {
    let kind = match stat.st_mode & libc::S_IFMT {
        libc::S_IFDIR => EntryType::Directory,
        libc::S_IFREG => EntryType::Regular,
        libc::S_IFLNK => EntryType::Symlink,
        _ => EntryType::Other,
    };
    // The same widening std's `MetadataExt::dev` applies, so identities
    // observed here and through `File::metadata` compare equal.
    EntryIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
        kind,
    }
}

/// Pin the file an upload names. macOS can pin a file only by opening it,
/// and opening a FIFO or device can block or act on it, so look first,
/// through a parent pinned without symbolic links and without following the
/// name, and open only a single-link regular file; the caller's descriptor
/// checks catch anything swapped in between. `None` when the path names
/// something else, which is then never opened.
pub(super) fn pin_upload_source(path: &Path) -> io::Result<Option<File>> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "upload path has no parent directory",
        ));
    };
    let parent = PinnedDirectory::open(parent)?;
    let stat = parent.stat(name)?;
    match identity_of(&stat).kind {
        EntryType::Symlink => return Err(symbolic_link_refused()),
        EntryType::Regular if stat.st_nlink == 1 => {}
        _ => return Ok(None),
    }
    // O_NONBLOCK keeps a FIFO swapped in meanwhile from blocking the open;
    // O_NOCTTY keeps a terminal from becoming the controlling terminal.
    parent
        .open_at(
            name,
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_NOCTTY,
        )
        .map(Some)
}

/// A handle that reads the inode `pinned` holds: on macOS the pin is already
/// open for reading, so this only shares it.
pub(super) fn upload_read_handle(pinned: &File) -> io::Result<File> {
    pinned.try_clone()
}

/// A directory pinned without following a symbolic link anywhere in its
/// path. Entries inside it are reached only through `*at` calls on the
/// descriptor, so moving or replacing the directory's path afterwards
/// changes nothing.
pub(super) struct PinnedDirectory {
    handle: File,
}

impl PinnedDirectory {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        // O_SEARCH works in a directory that can be traversed but not read.
        let handle = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_SEARCH | libc::O_NOFOLLOW_ANY)
            .open(path)
            .map_err(explain_symbolic_link)?;
        Ok(Self { handle })
    }

    #[cfg(test)]
    pub(super) fn handle(&self) -> &File {
        &self.handle
    }

    fn descriptor(&self) -> RawFd {
        self.handle.as_raw_fd()
    }

    fn stat(&self, name: &OsStr) -> io::Result<libc::stat> {
        let name = component(name)?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the descriptor and `name` stay live through the call, and
        // `stat` points to writable storage for one `struct stat`.
        result(unsafe {
            libc::fstatat(
                self.descriptor(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        })?;
        // SAFETY: a successful fstatat filled the whole structure.
        Ok(unsafe { stat.assume_init() })
    }

    fn open_at(&self, name: &OsStr, flags: libc::c_int) -> io::Result<File> {
        let name = component(name)?;
        // SAFETY: the descriptor and `name` stay live through the call; the
        // flags never create, so no mode argument is read.
        let descriptor =
            unsafe { libc::openat(self.descriptor(), name.as_ptr(), flags | libc::O_CLOEXEC) };
        if descriptor < 0 {
            return Err(explain_symbolic_link(io::Error::last_os_error()));
        }
        // SAFETY: openat returned a new descriptor that nothing else owns.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    /// Create the regular file `name`, never replacing or following an
    /// existing entry, open for reading and writing.
    fn create_file(&self, name: &OsStr) -> io::Result<File> {
        let name = component(name)?;
        let flags =
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: the descriptor and `name` stay live through the call, and
        // O_CREAT reads the one mode argument passed.
        let descriptor = unsafe {
            libc::openat(
                self.descriptor(),
                name.as_ptr(),
                flags,
                libc::c_uint::from(0o600_u16),
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new descriptor that nothing else owns.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    fn remove_entry(&self, name: &OsStr, directory: bool) -> io::Result<()> {
        let name = component(name)?;
        let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
        // SAFETY: the descriptor and `name` stay live through the call.
        result(unsafe { libc::unlinkat(self.descriptor(), name.as_ptr(), flags) })
    }

    pub(super) fn entry_identity(&self, name: &OsStr) -> io::Result<EntryIdentity> {
        Ok(identity_of(&self.stat(name)?))
    }

    pub(super) fn create_directory(&self, name: &OsStr) -> io::Result<()> {
        let name = component(name)?;
        // SAFETY: the descriptor and `name` stay live through the call.
        result(unsafe { libc::mkdirat(self.descriptor(), name.as_ptr(), 0o700) })
    }

    pub(super) fn create_symlink(&self, target: &Path, name: &OsStr) -> io::Result<()> {
        let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "link target contains a NUL byte",
            )
        })?;
        let name = component(name)?;
        // SAFETY: the descriptor and both C strings stay live through the call.
        result(unsafe { libc::symlinkat(target.as_ptr(), self.descriptor(), name.as_ptr()) })
    }

    /// A handle on the directory `name` that can change its permissions and
    /// times. macOS can pin a directory only by opening it, so when umask
    /// removed owner access from one created here, restore that access
    /// through this pinned parent, never following a link, and reopen it;
    /// the caller keeps the handle only if it is the inode it recorded.
    pub(super) fn open_directory_guard(&self, name: &OsStr) -> io::Result<File> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        match self.open_at(name, flags) {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                let c_name = component(name)?;
                // SAFETY: the descriptor and `c_name` stay live through the
                // call; AT_SYMLINK_NOFOLLOW changes a link itself, never its
                // target.
                result(unsafe {
                    libc::fchmodat(
                        self.descriptor(),
                        c_name.as_ptr(),
                        0o700,
                        libc::AT_SYMLINK_NOFOLLOW,
                    )
                })?;
                self.open_at(name, flags)
            }
            opened => opened,
        }
    }

    /// A handle on the regular file `name`, refusing a link.
    pub(super) fn open_file_guard(&self, name: &OsStr) -> io::Result<File> {
        self.open_at(
            name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY,
        )
    }

    /// A handle on the symbolic link `name` itself: O_SYMLINK opens the link,
    /// never its target, so a dangling link or one to a FIFO pins the same.
    pub(super) fn open_symlink_guard(&self, name: &OsStr) -> io::Result<File> {
        self.open_at(
            name,
            libc::O_RDONLY | libc::O_SYMLINK | libc::O_NONBLOCK | libc::O_NOCTTY,
        )
    }

    /// Publish the staged file at `name`: rename it out of its private
    /// staging directory, refusing an existing destination, then confirm the
    /// destination holds the staged inode.
    pub(super) fn publish(&self, name: &OsStr, staged: StagedFile) -> io::Result<File> {
        // Acquire the returned handle before publication so a descriptor
        // allocation failure cannot leave an unrecorded destination behind.
        let published = staged.file.try_clone()?;
        let identity = EntryIdentity::of(&staged.file.metadata()?);
        let from = component(&staged.name)?;
        let to = component(name)?;
        // SAFETY: both descriptors and both C strings stay live through the
        // call. RENAME_EXCL makes refusing an existing destination part of
        // the rename; a volume that can't refuses with ENOTSUP instead.
        result(unsafe {
            libc::renameatx_np(
                staged.directory.descriptor(),
                from.as_ptr(),
                self.descriptor(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        })?;
        self.confirm_entry(name, identity)?;
        Ok(published)
    }

    /// Hard-link `source`, published at `source_path`, at `name`. macOS links
    /// only names, never a descriptor, so link the source's name through a
    /// parent pinned without symbolic links, and keep the link only if it is
    /// the recorded inode.
    pub(super) fn link_file(
        &self,
        name: &OsStr,
        source: &File,
        source_path: &Path,
    ) -> io::Result<File> {
        let published = source.try_clone()?;
        let identity = EntryIdentity::of(&source.metadata()?);
        let (Some(parent), Some(source_name)) = (source_path.parent(), source_path.file_name())
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "hard link source has no parent directory",
            ));
        };
        let source_parent = PinnedDirectory::open(parent)?;
        if source_parent.entry_identity(source_name)? != identity {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "hard link source no longer names the published file",
            ));
        }
        let from = component(source_name)?;
        let to = component(name)?;
        // SAFETY: both descriptors and both C strings stay live through the
        // call. Flags 0 link a symbolic link itself, never its target, and
        // linkat refuses an existing destination.
        result(unsafe {
            libc::linkat(
                source_parent.descriptor(),
                from.as_ptr(),
                self.descriptor(),
                to.as_ptr(),
                0,
            )
        })?;
        self.confirm_entry(name, identity)?;
        Ok(published)
    }

    /// Confirm that `name`, just renamed or linked here, is `expected`. If
    /// its source name was swapped first, what arrived was never approved:
    /// take it back out (best effort, as anything by name is) and fail.
    fn confirm_entry(&self, name: &OsStr, expected: EntryIdentity) -> io::Result<()> {
        let found = self.entry_identity(name)?;
        if found == expected {
            return Ok(());
        }
        let _ = self.remove_entry(name, found.kind == EntryType::Directory);
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the destination received a different file than the approved one",
        ))
    }
}

/// macOS directory handles are always open descriptors, never metadata-only
/// pins, so there is nothing to upgrade.
pub(super) fn usable_directory_handle(_directory: &File) -> io::Result<Option<File>> {
    Ok(None)
}

/// Set the access and modification times of the inode `file` holds: a
/// regular file, or a symbolic link opened with O_SYMLINK, never its target.
pub(super) fn set_link_times(file: &File, times: &[libc::timespec; 2]) -> io::Result<()> {
    // SAFETY: file owns the live descriptor and times holds exactly two
    // initialized timespec values.
    result(unsafe { libc::futimens(file.as_raw_fd(), times.as_ptr()) })
}

/// The private directory staging made for one file, removed when it is
/// dropped if it is still the directory made and empty.
struct PrivateDirectory {
    parent: PinnedDirectory,
    name: OsString,
    identity: EntryIdentity,
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        if self
            .parent
            .entry_identity(&self.name)
            .is_ok_and(|current| current == self.identity)
        {
            // Fails, keeping it, while anything is still inside.
            let _ = self.parent.remove_entry(&self.name, true);
        }
    }
}

/// A file being received, staged inside a private directory beside where
/// it will be published.
pub(super) struct StagedFile {
    file: File,
    /// The directory holding the staged name, and the name.
    directory: PinnedDirectory,
    name: OsString,
    /// The private directory staging made, removed when this drops: fields
    /// drop after `Drop::drop` has removed the staged name, so it is empty.
    _private_directory: Option<PrivateDirectory>,
}

#[cfg(test)]
impl From<NamedTempFile> for StagedFile {
    /// Stage an existing temporary file in place, for tests of publication.
    fn from(temporary: NamedTempFile) -> Self {
        let (file, mut path) = temporary.into_parts();
        // Our Drop removes the name only while it is still this inode.
        path.disable_cleanup(true);
        let directory =
            PinnedDirectory::open(path.parent().expect("temporary file has a directory"))
                .expect("tests stage files under a canonical temporary directory");
        Self {
            file,
            directory,
            name: path
                .file_name()
                .expect("temporary file has a name")
                .to_owned(),
            _private_directory: None,
        }
    }
}

impl StagedFile {
    /// Stage a new file inside `parent`. Publishing renames it, and a rename
    /// moves whatever the name holds by then, so the file goes in a fresh
    /// directory only this user can enter: no other user can swap it, and
    /// publication confirms the inode regardless.
    pub(super) fn create_in(parent: PinnedDirectory) -> io::Result<Self> {
        let name = OsString::from(format!(
            ".termeric-transfer-{}",
            termeric_core::hex::random(16)?
        ));
        parent.create_directory(&name)?;
        let identity = parent.entry_identity(&name)?;
        if identity.kind != EntryType::Directory {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "staging directory was replaced while it was created",
            ));
        }
        // From here on, every failure removes the directory again.
        let private = PrivateDirectory {
            parent,
            name,
            identity,
        };
        let handle = private.parent.open_directory_guard(&private.name)?;
        if EntryIdentity::of(&handle.metadata()?) != identity {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "staging directory was replaced while it was created",
            ));
        }
        // Exactly 0700, whatever umask left.
        handle.set_permissions(Permissions::from_mode(0o700))?;
        let directory = PinnedDirectory { handle };
        let file = directory.create_file(OsStr::new(STAGED_FILE_NAME))?;
        Ok(Self {
            file,
            directory,
            name: STAGED_FILE_NAME.into(),
            _private_directory: Some(private),
        })
    }

    pub(super) fn as_file(&self) -> &File {
        &self.file
    }

    pub(super) fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// The name staging gave its entry in the staging directory: the
    /// private directory, or a test's temporary file staged in place.
    #[cfg(test)]
    pub(super) fn entry_name(&self) -> &OsStr {
        self._private_directory
            .as_ref()
            .map_or(&self.name, |private| &private.name)
    }

    /// The pinned directory a staged file was created in.
    #[cfg(test)]
    pub(super) fn staging_parent(&self) -> Option<&File> {
        self._private_directory
            .as_ref()
            .map(|private| private.parent.handle())
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        // Best effort, not an atomic compare-and-unlink: remove the staged
        // name only while it still names this inode. A published file's name
        // is gone already; a replacement is retained.
        if let Ok(owned) = self.file.metadata()
            && let Ok(named) = self.directory.entry_identity(&self.name)
            && named.kind == EntryType::Regular
            && named == EntryIdentity::of(&owned)
        {
            let _ = self.directory.remove_entry(&self.name, false);
        }
    }
}

/// Where the data of a staged entry found at `entry` lives: on macOS the
/// entry is the private directory holding it.
#[cfg(test)]
pub(super) fn staged_data_path(entry: &Path) -> PathBuf {
    entry.join(STAGED_FILE_NAME)
}
