//! "Ours and private" checks for files and directories that hold secrets,
//! on every OS.
//!
//! Unix: owned by the effective uid, mode `& 0o077 == 0`, made with 0700 /
//! 0600 and `O_NOFOLLOW`. Windows has no mode bits: owned by the current
//! user's SID (or the token's default owner) and an owner-only protected
//! DACL (`k2_winsec`), set on a new file while it is still empty; no-follow
//! is `FILE_FLAG_OPEN_REPARSE_POINT`. Callers keep their own symlink /
//! is-dir checks; these helpers answer only the OS-specific parts.

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::Path;

/// `m` (the `symlink_metadata` of `path`) belongs to this process's user.
#[cfg(unix)]
pub fn owned_by_me(_path: &Path, m: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions.
    m.uid() == unsafe { libc::geteuid() }
}

#[cfg(windows)]
pub fn owned_by_me(path: &Path, _m: &Metadata) -> bool {
    k2_winsec::owned_by_current_user(path).unwrap_or(false)
}

/// No other user can read `path` (Unix: no group/other mode bits).
#[cfg(unix)]
pub fn is_private(_path: &Path, m: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o077 == 0
}

#[cfg(windows)]
pub fn is_private(path: &Path, _m: &Metadata) -> bool {
    k2_winsec::is_owner_only(path).unwrap_or(false)
}

/// Tighten an existing directory to 0700 (owner-only on Windows).
pub fn make_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        k2_winsec::set_owner_only(path)
    }
}

/// Tighten an existing file to 0600 (owner-only on Windows).
pub fn make_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(windows)]
    {
        k2_winsec::set_owner_only(path)
    }
}

/// Create one directory (not its parents) 0700 / owner-only.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(windows)]
    {
        std::fs::create_dir(path)?;
        k2_winsec::set_owner_only(path)
    }
}

/// Create a directory and its missing parents, each 0700 (Windows: the
/// leaf owner-only; the parents keep what they inherit).
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(path)?;
        k2_winsec::set_owner_only(path)
    }
}

/// `O_CREAT|O_EXCL|O_NOFOLLOW`, 0600, for writing. On Windows the file is
/// made owner-only right after the open, before a byte is written.
pub fn create_new_private_file(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let f = o.open(path)?;
    #[cfg(windows)]
    k2_winsec::set_owner_only(path)?;
    Ok(f)
}

/// Open an existing file for writing without following a final symlink
/// (Unix: `O_NOFOLLOW|O_NONBLOCK`, so a planted FIFO never blocks).
pub fn open_write_nofollow(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    o.open(path)
}

/// Hard links to the open file `f` (a wipe never zeroes a file that has
/// another name).
pub fn link_count(f: &File) -> io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(f.metadata()?.nlink())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
        // SAFETY: a live handle we own and a zeroed out-struct.
        unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            if GetFileInformationByHandle(f.as_raw_handle(), &mut info) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(u64::from(info.nNumberOfLinks))
        }
    }
}

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_dirs_and_files_are_private_and_ours() {
        let root = crate::test_env::unique_temp_path("private-fs");
        std::fs::create_dir_all(&root).expect("mkdir root");
        let d = root.join("d");
        create_private_dir(&d).expect("create dir");
        let m = std::fs::symlink_metadata(&d).expect("meta");
        assert!(owned_by_me(&d, &m) && is_private(&d, &m), "{}", d.display());
        let f = d.join("f");
        let file = create_new_private_file(&f).expect("create file");
        assert_eq!(link_count(&file).expect("links"), 1);
        drop(file);
        let m = std::fs::symlink_metadata(&f).expect("meta");
        assert!(owned_by_me(&f, &m) && is_private(&f, &m));
        assert_eq!(create_new_private_file(&f).expect_err("exists").kind(), io::ErrorKind::AlreadyExists);
        std::fs::hard_link(&f, d.join("g")).expect("hard link");
        assert_eq!(link_count(&open_write_nofollow(&f).expect("open")).expect("links"), 2);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn a_shared_dir_is_tightened() {
        let root = crate::test_env::unique_temp_path("private-fs-t");
        std::fs::create_dir_all(&root).expect("mkdir");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let m2 = std::fs::symlink_metadata(&root).expect("meta");
        assert!(!is_private(&root, &m2), "a fresh temp dir is shared");
        make_private_dir(&root).expect("tighten");
        let m3 = std::fs::symlink_metadata(&root).expect("meta");
        assert!(is_private(&root, &m3));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
