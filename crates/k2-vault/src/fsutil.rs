//! Private file helpers: directories made 0700 and files made 0600 at
//! creation (never write-then-chmod, KV2), temp + fsync + rename.
//!
//! Windows has no mode bits: "0700/0600" there is an owner-only protected
//! DACL (`k2_winsec`), set on a file while it is still empty, and "owned
//! by us" is the file's owner SID. A symlink or other reparse point is
//! refused the same way on both.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
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
            if !owned_by_me(dir, &m)? {
                return Err(VaultError::forbidden("unsafe_path", format!("{} isn't owned by this user", dir.display())));
            }
            tighten_dir(dir, &m)?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            create_private_dir_all(dir)?;
            // recursive() applies the mode to every created level; the
            // leaf is what matters.
            set_private_dir(dir)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(unix)]
fn owned_by_me(_dir: &Path, m: &fs::Metadata) -> Result<bool> {
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    Ok(m.uid() == me)
}

#[cfg(windows)]
fn owned_by_me(dir: &Path, _m: &fs::Metadata) -> Result<bool> {
    Ok(k2_winsec::owned_by_current_user(dir)?)
}

#[cfg(unix)]
fn tighten_dir(dir: &Path, m: &fs::Metadata) -> Result<()> {
    if m.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(windows)]
fn tighten_dir(dir: &Path, _m: &fs::Metadata) -> Result<()> {
    if !k2_winsec::is_owner_only(dir)? {
        k2_winsec::set_owner_only(dir)?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_dir_all(dir: &Path) -> Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    Ok(())
}

#[cfg(windows)]
fn create_private_dir_all(dir: &Path) -> Result<()> {
    DirBuilder::new().recursive(true).create(dir)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_dir(dir: &Path) -> Result<()> {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(windows)]
fn set_private_dir(dir: &Path) -> Result<()> {
    k2_winsec::set_owner_only(dir)?;
    Ok(())
}

/// Make an existing file private (0600; owner-only on Windows).
pub fn set_private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    #[cfg(windows)]
    k2_winsec::set_owner_only(path)?;
    Ok(())
}

/// `create_new` + 0600 + no-follow, as one open. On Windows the file is
/// made owner-only right after the open, before a byte is written.
fn create_new_private(path: &Path) -> std::io::Result<fs::File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    o.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    o.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    let f = o.open(path)?;
    #[cfg(windows)]
    k2_winsec::set_owner_only(path)?;
    Ok(f)
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
        let mut f = create_new_private(&tmp)?;
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
    match create_new_private(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let m = fs::symlink_metadata(path)?;
            if m.file_type().is_symlink() {
                return Err(VaultError::forbidden("unsafe_path", format!("{} is a symlink", path.display())));
            }
            #[cfg(unix)]
            if m.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
            #[cfg(windows)]
            if !k2_winsec::is_owner_only(path)? {
                k2_winsec::set_owner_only(path)?;
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
        create_private_dir_all(&p).expect("create test dir");
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn atomic_write_is_0600() {
        let t = TestDir::new("fsutil-w");
        let p = t.path().join("f");
        write_private_atomic(&p, b"x").expect("write");
        assert_eq!(fs::read(&p).expect("read"), b"x");
        assert_eq!(fs::metadata(&p).expect("meta").permissions().mode() & 0o777, 0o600);
    }

    /// Needs the symlink privilege (an administrator or Developer Mode);
    /// fails loudly without it rather than skipping.
    #[cfg(windows)]
    #[test]
    fn private_dir_refuses_symlink() {
        let t = TestDir::new("fsutil");
        let real = t.path().join("real");
        fs::create_dir(&real).expect("mkdir");
        let link = t.path().join("link");
        std::os::windows::fs::symlink_dir(&real, &link).expect("symlink_dir (needs admin or Developer Mode)");
        assert_eq!(ensure_private_dir(&link).expect_err("symlink").code, "unsafe_path");
        ensure_private_dir(&t.path().join("a/b")).expect("create");
        assert!(k2_winsec::is_owner_only(&t.path().join("a/b")).expect("dacl"));
        // An existing dir that inherits the temp dir's shared entries is tightened.
        let shared = t.path().join("shared");
        fs::create_dir(&shared).expect("mkdir");
        assert!(!k2_winsec::is_owner_only(&shared).expect("dacl"), "the temp dir's entries are inherited");
        ensure_private_dir(&shared).expect("tighten");
        assert!(k2_winsec::is_owner_only(&shared).expect("dacl"));
    }

    #[cfg(windows)]
    #[test]
    fn atomic_write_is_owner_only() {
        let t = TestDir::new("fsutil-w");
        let p = t.path().join("f");
        write_private_atomic(&p, b"x").expect("write");
        assert_eq!(fs::read(&p).expect("read"), b"x");
        assert!(k2_winsec::is_owner_only(&p).expect("dacl"), "a shared temp parent must not leak into the file");
        touch_private(&t.path().join("db")).expect("touch");
        assert!(k2_winsec::is_owner_only(&t.path().join("db")).expect("dacl"));
    }
}
