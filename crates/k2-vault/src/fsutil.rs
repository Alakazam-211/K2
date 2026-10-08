//! Private file helpers: directories made 0700 and files made 0600 at
//! creation (never write-then-chmod, KV2), temp + fsync + rename.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::error::{Result, VaultError};

/// Create `dir` (and parents) with 0700. An existing dir must be a real
/// directory (not a symlink) owned by us; its mode is tightened to 0700.
pub fn ensure_private_dir(dir: &Path) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_dir() {
                return Err(VaultError::forbidden("unsafe_path", format!("{} isn't a plain directory", dir.display())));
            }
            // SAFETY: geteuid has no preconditions.
            let me = unsafe { libc::geteuid() };
            if m.uid() != me {
                return Err(VaultError::forbidden("unsafe_path", format!("{} isn't owned by this user", dir.display())));
            }
            if m.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
            // recursive() applies the mode to every created level; the
            // leaf is what matters.
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

pub fn fsync_dir(dir: &Path) {
    if let Ok(f) = fs::File::open(dir) {
        let _ = f.sync_all();
    }
}

/// Write `bytes` to `path` atomically, created 0600.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| VaultError::internal("path has no parent"))?;
    let name = path.file_name().ok_or_else(|| VaultError::internal("path has no file name"))?;
    let tmp = parent.join(format!(".{}.{}.tmp", name.to_string_lossy(), crate::crypto::base62(10)));
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    fsync_dir(parent);
    Ok(())
}

/// Create an empty file 0600 if it doesn't exist (so SQLite opens it with
/// that mode and copies the mode to -wal/-shm).
pub fn touch_private(path: &Path) -> Result<()> {
    match OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let m = fs::symlink_metadata(path)?;
            if m.file_type().is_symlink() {
                return Err(VaultError::forbidden("unsafe_path", format!("{} is a symlink", path.display())));
            }
            if m.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Temporary directory for tests (removed on drop). Never the real home.
#[doc(hidden)]
pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new(tag: &str) -> TestDir {
        let p = std::env::temp_dir().join(format!("k2-vault-test-{tag}-{}", uuid::Uuid::new_v4()));
        DirBuilder::new().recursive(true).mode(0o700).create(&p).expect("create test dir");
        TestDir(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dir_refuses_symlink() {
        let t = TestDir::new("fsutil");
        let real = t.path().join("real");
        fs::create_dir(&real).expect("mkdir");
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        assert_eq!(ensure_private_dir(&link).expect_err("symlink").code, "unsafe_path");
        ensure_private_dir(&t.path().join("a/b")).expect("create");
        let mode = fs::metadata(t.path().join("a/b")).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn atomic_write_is_0600() {
        let t = TestDir::new("fsutil-w");
        let p = t.path().join("f");
        write_private_atomic(&p, b"x").expect("write");
        assert_eq!(fs::read(&p).expect("read"), b"x");
        assert_eq!(fs::metadata(&p).expect("meta").permissions().mode() & 0o777, 0o600);
    }
}
