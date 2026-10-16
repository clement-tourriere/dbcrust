//! Hardened file helpers shared by the encrypted store, its index and key files.

use crate::SecretError;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The effective uid of this process (`None` on non-Unix platforms).
#[cfg(unix)]
pub fn current_uid() -> Option<u32> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    Some(unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
pub fn current_uid() -> Option<u32> {
    None
}

/// Error if `path` is a symlink (never follow links for secret material).
pub fn refuse_symlink(path: &Path) -> Result<(), SecretError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(SecretError::Denied(format!(
            "{} is a symbolic link; refusing to follow it",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Read a whole file, refusing symlinks and files over `max_bytes`.
/// `Ok(None)` when it does not exist.
pub fn read_bounded(path: &Path, max_bytes: u64) -> Result<Option<Vec<u8>>, SecretError> {
    refuse_symlink(path)?;
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !meta.is_file() {
        return Err(SecretError::Corrupt(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if meta.len() > max_bytes {
        return Err(SecretError::Corrupt(format!(
            "{} is larger than the {max_bytes}-byte limit",
            path.display()
        )));
    }
    Ok(Some(fs::read(path)?))
}

fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "secrets".to_string());
    let nonce: u64 = rand::random();
    path.with_file_name(format!(".{name}.{}.{nonce:016x}.tmp", std::process::id()))
}

/// Create `dir` (and parents) with mode 0700 where it does not exist yet.
pub fn create_private_dir(dir: &Path) -> Result<(), SecretError> {
    if dir.as_os_str().is_empty() || dir.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir)?;
    Ok(())
}

/// Atomically replace `path` with `bytes`: temp file (0600) in the same
/// directory, fsync, rename, fsync of the directory. Refuses symlinks.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), SecretError> {
    refuse_symlink(path)?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        create_private_dir(parent)?;
    }
    let temp = temp_sibling(path);
    let result = (|| -> Result<(), SecretError> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        if let Some(parent) = parent {
            if let Ok(dir) = fs::File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Validate a raw key file: regular file, not a symlink, owned by this user,
/// no group/other permission bits, and a parent directory with no
/// group/other access either. Returns the 32 key bytes.
pub fn read_key_file(path: &Path) -> Result<zeroize::Zeroizing<[u8; 32]>, SecretError> {
    refuse_symlink(path)?;
    let meta = fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            SecretError::Unavailable(format!("key file {} does not exist", path.display()))
        } else {
            SecretError::from(e)
        }
    })?;
    if !meta.is_file() {
        return Err(SecretError::Denied(format!(
            "key file {} is not a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(SecretError::Denied(format!(
                "key file {} is accessible by other users (mode {:o}); chmod 600 it",
                path.display(),
                meta.permissions().mode() & 0o777
            )));
        }
        if Some(meta.uid()) != current_uid() {
            return Err(SecretError::Denied(format!(
                "key file {} is not owned by the current user",
                path.display()
            )));
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let dir = fs::metadata(parent)?;
            if dir.permissions().mode() & 0o077 != 0 {
                return Err(SecretError::Denied(format!(
                    "directory {} is accessible by other users (mode {:o}); chmod 700 it",
                    parent.display(),
                    dir.permissions().mode() & 0o777
                )));
            }
            if Some(dir.uid()) != current_uid() {
                return Err(SecretError::Denied(format!(
                    "directory {} is not owned by the current user",
                    parent.display()
                )));
            }
        }
    }
    let bytes = zeroize::Zeroizing::new(fs::read(path)?);
    if bytes.len() != 32 {
        return Err(SecretError::Corrupt(format!(
            "key file {} must hold exactly 32 bytes",
            path.display()
        )));
    }
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Create a new 32-byte random key file (0600) at `path`. Fails if it exists.
pub fn create_key_file(path: &Path) -> Result<(), SecretError> {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};
    refuse_symlink(path)?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        create_private_dir(parent)?;
    }
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(&mut *key);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&*key)?;
    file.sync_all()?;
    Ok(())
}

/// Advisory exclusive lock on `<path>.lock`, held for the guard's lifetime,
/// serializing read-modify-write cycles between DBCrust processes.
pub struct FileLock {
    #[allow(dead_code)]
    file: Option<fs::File>,
}

impl FileLock {
    pub fn acquire(path: &Path) -> Result<Self, SecretError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            use std::os::unix::io::AsRawFd;
            let lock_path = {
                let mut name = path.as_os_str().to_os_string();
                name.push(".lock");
                PathBuf::from(name)
            };
            refuse_symlink(&lock_path)?;
            if let Some(parent) = lock_path.parent().filter(|p| !p.as_os_str().is_empty()) {
                create_private_dir(parent)?;
            }
            let file = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .mode(0o600)
                .open(&lock_path)?;
            // SAFETY: the descriptor is valid for the lifetime of `file`.
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if rc != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(Self { file: Some(file) })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Ok(Self { file: None })
        }
    }
}
