//! A key's hire reaches its folder by a walk from the key's root that never
//! follows a link: each part is opened with `O_NOFOLLOW | O_DIRECTORY`
//! relative to the one above it, and a missing part is made there
//! (`mkdirat`), so nothing is made or opened anywhere else. Every folder the
//! walk passes must be one only the server's user can change (see
//! [`super::root::changeable_by_others`]); the folder itself must be owned
//! by the server's user.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Why the walk stopped.
#[derive(Debug)]
pub(crate) enum WalkError {
    /// A link, or something that isn't a folder, at this path.
    NotAFolder(PathBuf),
    /// Another user could change this folder (built by the Unix walk only).
    #[cfg_attr(windows, allow(dead_code))]
    Unsafe(PathBuf),
    /// The last part was there although the walk was to make it.
    AlreadyThere(PathBuf),
    /// Missing, and the walk wasn't asked to make it.
    Missing(PathBuf),
    Io(PathBuf, std::io::Error),
}

/// The folder the walk ended on: which one it is (device, inode), whether
/// the walk made it, and whether it is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reached {
    pub id: (u64, u64),
    pub made: bool,
    pub empty: bool,
}

/// Walk from `root` (the owner's folder, opened as named) down `rel`. With
/// `make_last`, the last part must not exist and is made; missing parts
/// above it are made the same way. A folder the walk makes is mode 0755,
/// set after making it (never left to the umask), as a new workspace
/// folder has always been: a sandbox's mirror reads it.
#[cfg(unix)]
pub(crate) fn walk(root: &Path, rel: &[OsString], make_last: bool) -> Result<Reached, WalkError> {
    walk_fd(root, rel, make_last).map(|(_, reached)| reached)
}

/// [`walk`] to a folder that must already be there, keeping it open: the
/// fd of the last part, for a write made relative to it (`openat`).
#[cfg(unix)]
pub(crate) fn walk_open(root: &Path, rel: &[OsString]) -> Result<std::os::fd::OwnedFd, WalkError> {
    walk_fd(root, rel, false).map(|(fd, _)| fd)
}

#[cfg(unix)]
fn walk_fd(root: &Path, rel: &[OsString], make_last: bool) -> Result<(std::os::fd::OwnedFd, Reached), WalkError> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    let me = super::root::server_uid();
    let c = std::ffi::CString::new(root.as_os_str().as_bytes()).map_err(|_| WalkError::NotAFolder(root.to_path_buf()))?;
    // SAFETY: a valid C string; the fd is owned below.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(WalkError::Io(root.to_path_buf(), std::io::Error::last_os_error()));
    }
    // SAFETY: open returned a new fd that nothing else owns.
    let mut cur = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut at = root.to_path_buf();
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    for (i, name) in rel.iter().enumerate() {
        at.push(name);
        let last = i + 1 == rel.len();
        let cn = std::ffi::CString::new(name.as_bytes()).map_err(|_| WalkError::NotAFolder(at.clone()))?;
        let mut made = false;
        let mut made_here = false;
        if last && make_last {
            // SAFETY: a valid dirfd and C string.
            if unsafe { libc::mkdirat(cur.as_raw_fd(), cn.as_ptr(), 0o755) } != 0 {
                let e = std::io::Error::last_os_error();
                return Err(if e.raw_os_error() == Some(libc::EEXIST) { WalkError::AlreadyThere(at) } else { WalkError::Io(at, e) });
            }
            made = true;
            made_here = true;
        }
        // SAFETY: a valid dirfd and C string.
        let mut fd = unsafe { libc::openat(cur.as_raw_fd(), cn.as_ptr(), flags) };
        let missing = fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT);
        if missing && make_last && !last {
            // SAFETY: as above.
            if unsafe { libc::mkdirat(cur.as_raw_fd(), cn.as_ptr(), 0o755) } != 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() != Some(libc::EEXIST) {
                    return Err(WalkError::Io(at, e));
                }
            } else {
                made_here = true;
            }
            // SAFETY: as above.
            fd = unsafe { libc::openat(cur.as_raw_fd(), cn.as_ptr(), flags) };
        }
        if fd < 0 {
            let e = std::io::Error::last_os_error();
            return Err(match e.raw_os_error() {
                Some(libc::ELOOP) | Some(libc::ENOTDIR) => WalkError::NotAFolder(at),
                Some(libc::ENOENT) => WalkError::Missing(at),
                _ => WalkError::Io(at, e),
            });
        }
        // SAFETY: openat returned a new fd that nothing else owns.
        let next = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: a valid fd.
        if made_here && unsafe { libc::fchmod(next.as_raw_fd(), 0o755) } != 0 {
            return Err(WalkError::Io(at, std::io::Error::last_os_error()));
        }
        let st = fstat(&next).map_err(|e| WalkError::Io(at.clone(), e))?;
        // The folder itself is the server user's; a folder above it may be
        // root's too.
        if super::root::changeable_by_others(st.st_uid, st.st_mode as u32, me, !last) {
            return Err(WalkError::Unsafe(at));
        }
        cur = next;
        if last {
            let empty = is_empty(&cur).map_err(|e| WalkError::Io(at.clone(), e))?;
            return Ok((cur, Reached { id: (st.st_dev as u64, st.st_ino as u64), made, empty }));
        }
    }
    Err(WalkError::NotAFolder(at))
}

#[cfg(unix)]
fn fstat(fd: &std::os::fd::OwnedFd) -> std::io::Result<libc::stat> {
    use std::os::fd::AsRawFd;
    // SAFETY: stat is plain data; fstat fills it for a valid fd.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(st)
}

/// No entries but `.` and `..`, read through a copy of `fd`.
#[cfg(unix)]
fn is_empty(fd: &std::os::fd::OwnedFd) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: dup of a valid fd; fdopendir takes the copy and closedir
    // closes it.
    let copy = unsafe { libc::dup(fd.as_raw_fd()) };
    if copy < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let dir = unsafe { libc::fdopendir(copy) };
    if dir.is_null() {
        let e = std::io::Error::last_os_error();
        unsafe { libc::close(copy) };
        return Err(e);
    }
    let mut empty = true;
    loop {
        // SAFETY: dir is open; the entry is read before the next call.
        let ent = unsafe { libc::readdir(dir) };
        if ent.is_null() {
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*ent).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            empty = false;
            break;
        }
    }
    unsafe { libc::closedir(dir) };
    Ok(empty)
}

/// Not Unix: no `O_NOFOLLOW` walk. Each part is looked at without following
/// it, then made or opened by name.
#[cfg(not(unix))]
pub(crate) fn walk(root: &Path, rel: &[OsString], make_last: bool) -> Result<Reached, WalkError> {
    let mut at = root.to_path_buf();
    let mut made = false;
    for (i, name) in rel.iter().enumerate() {
        at.push(name);
        let last = i + 1 == rel.len();
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => return Err(WalkError::NotAFolder(at)),
            Ok(_) if last && make_last => return Err(WalkError::AlreadyThere(at)),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && make_last => {
                std::fs::create_dir(&at).map_err(|e| WalkError::Io(at.clone(), e))?;
                made = last;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(WalkError::Missing(at)),
            Err(e) => return Err(WalkError::Io(at, e)),
        }
        if last {
            let empty = std::fs::read_dir(&at).map_err(|e| WalkError::Io(at.clone(), e))?.next().is_none();
            return Ok(Reached { id: (0, 0), made, empty });
        }
    }
    Err(WalkError::NotAFolder(at))
}

/// A new workspace folder's mode, 0755, set on the folder itself (a link
/// put in its place is not followed), never left to the umask.
#[cfg(unix)]
pub(crate) fn set_new_folder_mode(dir: &Path) -> std::io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: a valid C string; the fd is owned below.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: open returned a new fd that nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: a valid fd.
    if unsafe { libc::fchmod(fd.as_raw_fd(), 0o755) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn set_new_folder_mode(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}
