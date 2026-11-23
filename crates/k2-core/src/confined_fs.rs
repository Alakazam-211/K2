//! Make folders and write files at `<root>/<rel>` without following a link
//! anywhere in `rel`.
//!
//! `root` is a folder the caller already trusts (it and its ancestors may be
//! links, like `/tmp` on macOS or a home folder on another disk). `rel` is
//! plain names only: no `..`, `.`, root or drive part ([`check_rel`]).
//!
//! Unix walks `rel` from an open handle of the folder above each part:
//! `openat(O_DIRECTORY|O_NOFOLLOW)` for a folder, `mkdirat` for a missing
//! one, and `openat(O_NOFOLLOW)` for the file. A link at any part is
//! refused, never followed, even one that appears between two steps.
//! Windows has no `openat`: each part is looked at with `symlink_metadata`
//! (a symlink or junction is refused) and the file is opened with
//! `FILE_FLAG_OPEN_REPARSE_POINT`.
//!
//! A file is written only when it is a regular file with no other name (a
//! hard link is refused), so a write never lands in a file outside `root`.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Component, Path};

/// `rel` is one or more plain names (no `..`, `.`, root or drive part, and
/// on Windows no `:` or `\` inside a name).
pub fn check_rel(rel: &Path) -> io::Result<()> {
    let mut any = false;
    for c in rel.components() {
        match c {
            Component::Normal(name) => {
                #[cfg(windows)]
                {
                    let s = name.to_string_lossy();
                    if s.contains(':') || s.contains('\\') {
                        return Err(invalid(rel, "a name holds ':' or '\\'"));
                    }
                }
                let _ = name;
                any = true;
            }
            _ => return Err(invalid(rel, "only plain names are allowed (no '..', '.', root or drive part)")),
        }
    }
    if !any {
        return Err(invalid(rel, "empty path"));
    }
    Ok(())
}

fn invalid(rel: &Path, why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{}: {why}", rel.display()))
}

fn linked(rel: &Path) -> io::Error {
    io::Error::other(format!(
        "{}: a part of this path is a link or not a folder; it is not followed",
        rel.display()
    ))
}

/// Make `<root>/<rel>` and its missing parents below `root`, refusing a
/// link at any part of `rel`.
pub fn create_dir_all(root: &Path, rel: &Path) -> io::Result<()> {
    check_rel(rel)?;
    imp::open_dirs(root, rel, rel.components().count()).map(|_| ())
}

/// Create or replace the file `<root>/<rel>` with `bytes` (its parents are
/// made as needed). `mode` (unix) sets the permission bits after the write.
pub fn write_file(root: &Path, rel: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    check_rel(rel)?;
    let mut f = imp::open_file(root, rel, false)?.expect("open_file without create_new always yields a file");
    f.set_len(0)?;
    f.write_all(bytes)?;
    set_mode(&f, mode)?;
    Ok(())
}

/// Write `bytes` to `<root>/<rel>` only when nothing is there yet (a file,
/// folder or link). `Ok(false)` when something already is.
pub fn write_new_file(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<bool> {
    check_rel(rel)?;
    match imp::open_file(root, rel, true)? {
        Some(mut f) => {
            f.write_all(bytes)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn set_mode(f: &File, mode: Option<u32>) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(m))?;
    }
    #[cfg(not(unix))]
    let _ = (f, mode);
    Ok(())
}

/// An opened existing file must be a regular file with one name.
fn check_opened(f: &File, rel: &Path) -> io::Result<()> {
    let m = f.metadata()?;
    if !m.is_file() {
        return Err(io::Error::other(format!("{}: not a regular file", rel.display())));
    }
    if crate::private_fs::link_count(f)? > 1 {
        return Err(io::Error::other(format!(
            "{}: the file has another name (a hard link); not written",
            rel.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
mod imp {
    use super::{check_opened, linked};
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    fn cname(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a name holds NUL"))
    }

    fn open_dir_at(dir: &OwnedFd, name: &CString) -> io::Result<OwnedFd> {
        // SAFETY: a live fd we own and a NUL-terminated name.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat just returned this fd to us.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn is_link_refusal(e: &io::Error) -> bool {
        matches!(e.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR))
    }

    /// Open `root` (followed: the caller trusts it), then the first `n`
    /// parts of `rel` as folders, making missing ones.
    pub(super) fn open_dirs(root: &Path, rel: &Path, n: usize) -> io::Result<OwnedFd> {
        let root_dir = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(root)?;
        let mut cur: OwnedFd = root_dir.into();
        for part in rel.components().take(n) {
            let name = cname(part.as_os_str())?;
            cur = match open_dir_at(&cur, &name) {
                Ok(fd) => fd,
                Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
                    // SAFETY: a live fd we own and a NUL-terminated name.
                    let r = unsafe { libc::mkdirat(cur.as_raw_fd(), name.as_ptr(), 0o777 as libc::mode_t) };
                    if r != 0 {
                        let e = io::Error::last_os_error();
                        if e.raw_os_error() != Some(libc::EEXIST) {
                            return Err(e);
                        }
                    }
                    open_dir_at(&cur, &name).map_err(|e| if is_link_refusal(&e) { linked(rel) } else { e })?
                }
                Err(e) if is_link_refusal(&e) => return Err(linked(rel)),
                Err(e) => return Err(e),
            };
        }
        Ok(cur)
    }

    /// Open the file at `<root>/<rel>` for writing, never through a link.
    /// `create_new`: `Ok(None)` when anything is already there.
    pub(super) fn open_file(root: &Path, rel: &Path, create_new: bool) -> io::Result<Option<File>> {
        let n = rel.components().count();
        let dir = open_dirs(root, rel, n - 1)?;
        let name = cname(rel.components().last().expect("check_rel: at least one part").as_os_str())?;
        let mut flags = libc::O_WRONLY | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
        if create_new {
            flags |= libc::O_EXCL;
        }
        // SAFETY: a live fd we own and a NUL-terminated name.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, 0o666 as libc::c_uint) };
        if fd < 0 {
            let e = io::Error::last_os_error();
            if create_new && e.raw_os_error() == Some(libc::EEXIST) {
                return Ok(None);
            }
            if is_link_refusal(&e) {
                return Err(linked(rel));
            }
            return Err(e);
        }
        // SAFETY: openat just returned this fd to us.
        let f = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        check_opened(&f, rel)?;
        Ok(Some(f))
    }
}

#[cfg(windows)]
mod imp {
    use super::{check_opened, linked};
    use std::fs::File;
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT};

    fn is_link(m: &std::fs::Metadata) -> bool {
        m.file_type().is_symlink() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    pub(super) fn open_dirs(root: &Path, rel: &Path, n: usize) -> io::Result<PathBuf> {
        if !std::fs::metadata(root)?.is_dir() {
            return Err(io::Error::other(format!("{}: not a folder", root.display())));
        }
        let mut cur = root.to_path_buf();
        for part in rel.components().take(n) {
            cur.push(part);
            match std::fs::symlink_metadata(&cur) {
                Ok(m) if is_link(&m) || !m.is_dir() => return Err(linked(rel)),
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => match std::fs::create_dir(&cur) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        let m = std::fs::symlink_metadata(&cur)?;
                        if is_link(&m) || !m.is_dir() {
                            return Err(linked(rel));
                        }
                    }
                    Err(e) => return Err(e),
                },
                Err(e) => return Err(e),
            }
        }
        Ok(cur)
    }

    pub(super) fn open_file(root: &Path, rel: &Path, create_new: bool) -> io::Result<Option<File>> {
        let n = rel.components().count();
        let dir = open_dirs(root, rel, n - 1)?;
        let path = dir.join(rel.components().last().expect("check_rel: at least one part"));
        match std::fs::symlink_metadata(&path) {
            Ok(_) if create_new => return Ok(None),
            Ok(m) if is_link(&m) => return Err(linked(rel)),
            _ => {}
        }
        let mut o = std::fs::OpenOptions::new();
        o.write(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        if create_new {
            o.create_new(true);
        } else {
            o.create(true);
        }
        let f = match o.open(&path) {
            Ok(f) => f,
            Err(e) if create_new && e.kind() == io::ErrorKind::AlreadyExists => return Ok(None),
            Err(e) => return Err(e),
        };
        let m = f.metadata()?;
        if is_link(&m) {
            return Err(linked(rel));
        }
        check_opened(&f, rel)?;
        Ok(Some(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(label: &str) -> std::path::PathBuf {
        let p = crate::test_env::unique_temp_path(label);
        fs::create_dir_all(&p).expect("mkdir temp");
        p
    }

    #[test]
    fn writes_nested_files_and_replaces_them() {
        let root = temp("confined-write");
        write_file(&root, Path::new("a/b/c.txt"), b"one", None).expect("first write");
        write_file(&root, Path::new("a/b/c.txt"), b"2", None).expect("replace");
        assert_eq!(fs::read(root.join("a/b/c.txt")).expect("read"), b"2");
        assert!(!write_new_file(&root, Path::new("a/b/c.txt"), b"x").expect("new over existing"));
        assert!(write_new_file(&root, Path::new("a/d.txt"), b"x").expect("new"));
        create_dir_all(&root, Path::new("e/f")).expect("mkdirs");
        assert!(root.join("e/f").is_dir());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_names_that_are_not_plain() {
        let root = temp("confined-names");
        for rel in ["", "../x", "a/../x", "/x", "./x"] {
            let e = write_file(&root, Path::new(rel), b"x", None).expect_err(rel);
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{rel}: {e}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_a_link_or_a_second_name() {
        let root = temp("confined-links");
        let outside = temp("confined-links-outside");
        std::os::unix::fs::symlink(&outside, root.join("dir-link")).expect("dir link");
        let e = write_file(&root, Path::new("dir-link/x.txt"), b"x", None).expect_err("folder link");
        assert!(e.to_string().contains("not followed"), "{e}");
        let e = create_dir_all(&root, Path::new("dir-link/sub")).expect_err("folder link mkdir");
        assert!(e.to_string().contains("not followed"), "{e}");

        fs::write(outside.join("target.txt"), b"keep").expect("target");
        std::os::unix::fs::symlink(outside.join("target.txt"), root.join("file-link")).expect("file link");
        let e = write_file(&root, Path::new("file-link"), b"x", None).expect_err("file link");
        assert!(e.to_string().contains("not followed"), "{e}");
        assert!(!write_new_file(&root, Path::new("file-link"), b"x").expect("new over a link"));

        fs::hard_link(outside.join("target.txt"), root.join("second-name")).expect("hard link");
        let e = write_file(&root, Path::new("second-name"), b"x", None).expect_err("hard link");
        assert!(e.to_string().contains("another name"), "{e}");

        assert_eq!(fs::read(outside.join("target.txt")).expect("read"), b"keep");
        assert!(!outside.join("x.txt").exists() && !outside.join("sub").exists());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }
}
