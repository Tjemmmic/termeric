//! Private, process-scoped GUI control endpoints.

use crate::domain::paths::{
    discover_runtime_directory, effective_uid, ensure_private_directory, load_or_create_token,
    read_token,
};
use anyhow::Context;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const PREFIX: &str = "gui-v1-";
const SOCKET_SUFFIX: &str = ".sock";
const MAX_DISCOVERED_ENDPOINTS: usize = 128;
const QUICK_ACCESS_LOCK: &str = "quick-access.lock";

#[derive(Debug)]
pub(crate) struct QuickAccessGuard {
    file: fs::File,
    path: PathBuf,
}

impl QuickAccessGuard {
    pub(crate) fn acquire() -> anyhow::Result<Self> {
        Self::acquire_in(discover_runtime_directory())
    }

    fn acquire_in(directory: PathBuf) -> anyhow::Result<Self> {
        ensure_private_directory(&directory)?;
        let path = directory.join(QUICK_ACCESS_LOCK);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != effective_uid()
            || metadata.permissions().mode() & 0o077 != 0
        {
            anyhow::bail!("unsafe quick-access ownership lock {}", path.display());
        }
        use std::os::fd::AsRawFd;
        // SAFETY: flock operates on this owned, validated descriptor and keeps
        // the advisory lock tied to its lifetime. LOCK_NB cannot block.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                anyhow::bail!("a quick-access terminal is still starting; try the toggle again");
            }
            return Err(error.into());
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;
        Ok(Self { file, path })
    }
}

impl Drop for QuickAccessGuard {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // Keep the inode as the stable rendezvous point. The kernel releases
        // the lock even after a crash, avoiding PID-reuse and unlink races.
        if unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) } != 0 {
            log::warn!(
                "Could not release quick-access ownership lock {}: {}",
                self.path.display(),
                std::io::Error::last_os_error()
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPaths {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub token: PathBuf,
    pub process_id: u32,
}

impl ControlPaths {
    pub fn discover_current() -> anyhow::Result<Self> {
        Self::from_directory(discover_runtime_directory(), std::process::id())
    }

    pub fn from_directory(directory: PathBuf, process_id: u32) -> anyhow::Result<Self> {
        ensure_private_directory(&directory)?;
        Ok(Self::without_validation(directory, process_id))
    }

    fn without_validation(directory: PathBuf, process_id: u32) -> Self {
        let stem = format!("{PREFIX}{process_id}");
        Self {
            socket: directory.join(format!("{stem}{SOCKET_SUFFIX}")),
            token: directory.join(format!("{stem}.token")),
            directory,
            process_id,
        }
    }

    pub fn load_or_create_token(&self) -> anyhow::Result<String> {
        load_or_create_token(&self.token)
    }

    pub fn read_token(&self) -> anyhow::Result<String> {
        read_token(&self.token)
    }

    pub fn discover_all() -> anyhow::Result<Vec<Self>> {
        Self::discover_all_in(discover_runtime_directory())
    }

    pub fn discover_all_in(directory: PathBuf) -> anyhow::Result<Vec<Self>> {
        ensure_private_directory(&directory)?;
        let mut endpoints = Vec::new();
        for (index, entry) in fs::read_dir(&directory)
            .with_context(|| format!("read GUI control directory {}", directory.display()))?
            .enumerate()
        {
            if index == MAX_DISCOVERED_ENDPOINTS {
                anyhow::bail!(
                    "GUI control directory entry count exceeds {MAX_DISCOVERED_ENDPOINTS} in {}",
                    directory.display()
                );
            }
            let entry = entry?;
            let Some(process_id) = process_id_from_socket_name(&entry.file_name()) else {
                continue;
            };
            let metadata = fs::symlink_metadata(entry.path())?;
            if !metadata.file_type().is_socket()
                || metadata.uid() != effective_uid()
                || metadata.permissions().mode() & 0o077 != 0
            {
                anyhow::bail!("unsafe GUI control endpoint {}", entry.path().display());
            }
            endpoints.push(Self::without_validation(directory.clone(), process_id));
        }
        endpoints.sort_by_key(|endpoint| endpoint.process_id);
        endpoints.dedup_by_key(|endpoint| endpoint.process_id);
        Ok(endpoints)
    }
}

fn process_id_from_socket_name(name: &std::ffi::OsStr) -> Option<u32> {
    let name = name.to_str()?;
    let digits = name.strip_prefix(PREFIX)?.strip_suffix(SOCKET_SUFFIX)?;
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits
        .parse()
        .ok()
        .filter(|process_id| *process_id != 0 && *process_id <= i32::MAX as u32)
}

pub fn remove_safe_file(path: &Path, socket: bool) -> anyhow::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    };
    let expected_type = if socket {
        metadata.file_type().is_socket()
    } else {
        metadata.file_type().is_file() && !metadata.file_type().is_symlink()
    };
    if !expected_type || metadata.uid() != effective_uid() {
        anyhow::bail!(
            "refusing to remove unsafe GUI control path {}",
            path.display()
        );
    }
    fs::remove_file(path)
        .with_context(|| format!("remove stale GUI control path {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::os::unix::net::UnixListener;

    #[test]
    fn process_endpoints_coexist_and_discover_in_pid_order() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("runtime");
        let second = ControlPaths::from_directory(directory.clone(), 22).unwrap();
        let first = ControlPaths::from_directory(directory.clone(), 7).unwrap();
        let _first_listener = match UnixListener::bind(&first.socket) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skip GUI control socket discovery: sandbox denies AF_UNIX");
                return;
            }
            Err(error) => panic!("could not bind GUI control test socket: {error}"),
        };
        let _second_listener = UnixListener::bind(&second.socket).unwrap();
        fs::set_permissions(&first.socket, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&second.socket, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            ControlPaths::discover_all_in(directory)
                .unwrap()
                .iter()
                .map(|endpoint| endpoint.process_id)
                .collect::<Vec<_>>(),
            [7, 22]
        );
    }

    #[test]
    fn credentials_are_private_and_unsafe_stale_paths_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint = ControlPaths::from_directory(temp.path().join("runtime"), 42).unwrap();
        let token = endpoint.load_or_create_token().unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(
            fs::metadata(&endpoint.token).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let target = temp.path().join("target");
        fs::write(&target, "leave me").unwrap();
        symlink(&target, &endpoint.socket).unwrap();
        assert!(remove_safe_file(&endpoint.socket, true).is_err());
        assert!(target.exists());
    }

    #[test]
    fn discovery_work_is_bounded_even_by_unrelated_runtime_entries() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("runtime");
        let _endpoint = ControlPaths::from_directory(directory.clone(), 42).unwrap();
        for index in 0..=MAX_DISCOVERED_ENDPOINTS {
            fs::write(directory.join(format!("unrelated-{index}")), b"x").unwrap();
        }
        assert!(ControlPaths::discover_all_in(directory).is_err());
    }

    #[test]
    fn quick_access_lock_is_single_owner_private_and_recoverable() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("runtime");
        let guard = QuickAccessGuard::acquire_in(directory.clone()).unwrap();
        let path = directory.join(QUICK_ACCESS_LOCK);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            QuickAccessGuard::acquire_in(directory.clone())
                .unwrap_err()
                .to_string()
                .contains("still starting")
        );
        drop(guard);
        assert!(path.exists());
        drop(QuickAccessGuard::acquire_in(directory).unwrap());
    }

    #[test]
    fn quick_access_lock_never_follows_a_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("runtime");
        ensure_private_directory(&directory).unwrap();
        let target = temp.path().join("target");
        fs::write(&target, "leave me").unwrap();
        symlink(&target, directory.join(QUICK_ACCESS_LOCK)).unwrap();
        assert!(QuickAccessGuard::acquire_in(directory).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "leave me");
    }
}
