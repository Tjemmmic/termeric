//! Private runtime paths and authentication material for the local daemon.

use anyhow::Context;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const TOKEN_BYTES: usize = 64;

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub token: PathBuf,
}

impl RuntimePaths {
    pub fn discover() -> anyhow::Result<Self> {
        Self::from_directory(discover_runtime_directory())
    }

    pub fn from_directory(directory: PathBuf) -> anyhow::Result<Self> {
        ensure_private_directory(&directory)?;
        Ok(Self {
            socket: directory.join("domain-v1.sock"),
            token: directory.join("domain-v1.token"),
            directory,
        })
    }

    pub fn load_or_create_token(&self) -> anyhow::Result<String> {
        load_or_create_token(&self.token)
    }

    pub fn read_token(&self) -> anyhow::Result<String> {
        read_token(&self.token)
    }
}

pub(crate) fn discover_runtime_directory() -> PathBuf {
    let uid = effective_uid();
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| runtime_parent_is_safe(path, uid))
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/termeric-{uid}")));
    if base.ends_with(format!("termeric-{uid}")) {
        base
    } else {
        base.join("termeric")
    }
}

pub(crate) fn load_or_create_token(path: &Path) -> anyhow::Result<String> {
    match read_token(path) {
        Ok(token) => Ok(token),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            match create_token(path) {
                Ok(token) => Ok(token),
                // Another same-user client may have won create_new between
                // our read and create. Re-read and revalidate its result.
                Err(create_error) => read_token(path).map_err(|_| create_error),
            }
        }
        Err(error) => Err(error),
    }
}

pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not retain pointers.
    unsafe { libc::geteuid() }
}

fn runtime_parent_is_safe(path: &Path, uid: u32) -> bool {
    path.is_absolute()
        && fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.file_type().is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == uid
                && metadata.permissions().mode() & 0o077 == 0
        })
}

pub(crate) fn ensure_private_directory(path: &Path) -> anyhow::Result<()> {
    let directory = match open_directory(path) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .with_context(|| format!("create domain runtime path {}", path.display()))?;
            open_directory(path).with_context(|| {
                format!("open newly created domain runtime path {}", path.display())
            })?
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("open domain runtime path {}", path.display()));
        }
    };
    validate_directory(&directory, path)?;
    directory
        .set_permissions(fs::Permissions::from_mode(0o700))
        .with_context(|| format!("secure domain runtime path {}", path.display()))?;
    validate_directory(&directory, path)?;
    if directory.metadata()?.permissions().mode() & 0o777 != 0o700 {
        anyhow::bail!(
            "domain runtime path {} could not be secured to mode 0700",
            path.display()
        );
    }
    Ok(())
}

fn open_directory(path: &Path) -> std::io::Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

fn validate_directory(directory: &fs::File, path: &Path) -> anyhow::Result<()> {
    let metadata = directory
        .metadata()
        .with_context(|| format!("inspect domain runtime path {}", path.display()))?;
    if !metadata.file_type().is_dir() || metadata.uid() != effective_uid() {
        anyhow::bail!(
            "domain runtime path {} is not a user-owned directory",
            path.display()
        );
    }
    Ok(())
}

fn create_token(path: &Path) -> anyhow::Result<String> {
    // The publication nonce is independent of the credential so temporary
    // filenames never disclose even a prefix of the authentication token.
    let mut random = [0u8; 40];
    fs::File::open("/dev/urandom")
        .context("open operating-system random source")?
        .read_exact(&mut random)
        .context("read Termeric authentication token")?;
    let token = encode_hex(&random[..32]);
    let nonce = encode_hex(&random[32..]);
    let temporary = token_temporary_path(path, std::process::id(), &nonce);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .mode(0o600)
        .open(&temporary)
        .with_context(|| format!("create temporary Termeric token {}", temporary.display()))?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    drop(file);
    let linked = fs::hard_link(&temporary, path);
    let _ = fs::remove_file(&temporary);
    match linked {
        Ok(()) => Ok(token),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_token(path),
        Err(error) => Err(error).context("publish Termeric authentication token"),
    }
}

pub(crate) fn read_token(path: &Path) -> anyhow::Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        // A replaced FIFO must not block before descriptor type validation.
        // O_NONBLOCK does not change reads from the required regular file.
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("open Termeric token {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("read Termeric token metadata {}", path.display()))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() != TOKEN_BYTES as u64
    {
        anyhow::bail!(
            "Termeric token {} has unsafe type, owner, permissions, or size",
            path.display()
        );
    }

    // Read a fixed buffer from the already validated descriptor. This both
    // closes the metadata/open substitution window and prevents a corrupt
    // token path from causing an attacker-sized allocation.
    let mut bytes = [0u8; TOKEN_BYTES];
    file.read_exact(&mut bytes)
        .with_context(|| format!("read Termeric token {}", path.display()))?;
    let mut trailing = [0u8; 1];
    if file.read(&mut trailing)? != 0 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        anyhow::bail!("Termeric token file is malformed");
    }
    Ok(std::str::from_utf8(&bytes)
        .context("domain token is not valid UTF-8")?
        .to_owned())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn token_temporary_path(path: &Path, process_id: u32, nonce: &str) -> PathBuf {
    path.with_extension(format!("tmp-{process_id}-{nonce}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn runtime_directory_and_token_are_private_and_stable() {
        let temp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::from_directory(temp.path().join("runtime")).unwrap();
        let first = paths.load_or_create_token().unwrap();
        let second = paths.load_or_create_token().unwrap();
        assert_eq!(first, second);
        assert_eq!(
            fs::metadata(&paths.directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&paths.token).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn symlink_runtime_paths_and_public_tokens_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(RuntimePaths::from_directory(link).is_err());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755,
            "a rejected runtime-directory symlink must not chmod its target"
        );

        let paths = RuntimePaths::from_directory(temp.path().join("safe")).unwrap();
        fs::write(&paths.token, "0".repeat(64)).unwrap();
        fs::set_permissions(&paths.token, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(paths.read_token().is_err());
    }

    #[test]
    fn token_reads_are_descriptor_verified_and_strictly_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::from_directory(temp.path().join("runtime")).unwrap();

        let oversized = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&paths.token)
            .unwrap();
        oversized.set_len(1024 * 1024 * 1024).unwrap();
        drop(oversized);
        assert!(paths.read_token().is_err());

        fs::remove_file(&paths.token).unwrap();
        fs::write(&paths.token, "g".repeat(TOKEN_BYTES)).unwrap();
        fs::set_permissions(&paths.token, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(paths.read_token().is_err());
    }

    #[test]
    fn temporary_token_names_do_not_disclose_the_credential() {
        let token = "0123456789abcdef".repeat(4);
        let temporary = token_temporary_path(
            Path::new("/run/user/1000/termeric/domain-v1.token"),
            42,
            "fedcba9876543210",
        );
        assert_eq!(
            temporary.file_name().unwrap(),
            "domain-v1.tmp-42-fedcba9876543210"
        );
        assert!(!temporary.to_string_lossy().contains(&token[..16]));
    }

    #[test]
    fn concurrent_first_clients_publish_one_complete_token() {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            std::sync::Arc::new(RuntimePaths::from_directory(temp.path().join("runtime")).unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let paths = std::sync::Arc::clone(&paths);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    paths.load_or_create_token().unwrap()
                })
            })
            .collect();
        let tokens: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(tokens.iter().all(|token| token == &tokens[0]));
        assert_eq!(tokens[0].len(), 64);
    }
}
