//! A directory handle that never follows a symlink (§5.3 rule 4).
//!
//! Every walk starts from the context root (`/` in the binary, a temp dir
//! in tests) and opens each component with `openat(O_DIRECTORY |
//! O_NOFOLLOW)`, which is what `openat2(RESOLVE_BENEATH |
//! RESOLVE_NO_SYMLINKS)` gives for fixed relative paths without `..`, and
//! also builds on macOS for the unit tests. Writes are temp + fsync +
//! `renameat` in the same directory.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Missing,
    Symlink,
    Dir,
    File,
    Other,
}

pub struct Dir {
    fd: OwnedFd,
}

fn cstr(name: &OsStr) -> io::Result<CString> {
    if name.is_empty() || name.as_bytes().contains(&b'/') || name == ".." || name == "." {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "bad path component",
        ));
    }
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "nul in name"))
}

fn check(rc: libc::c_int) -> io::Result<libc::c_int> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

const DIR_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

impl Dir {
    /// Open the context root itself (the only open that may follow links:
    /// it is `/`, or a test's temp dir).
    pub fn open_root(root: &Path) -> io::Result<Dir> {
        let c = CString::new(root.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "nul in root"))?;
        // SAFETY: NUL-terminated path; the fd is owned below.
        let fd = check(unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })?;
        // SAFETY: fd was just returned by open and is owned by nobody else.
        Ok(Dir {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    /// Walk `abs` (an absolute fixed path) beneath `root`, one component at
    /// a time, never following a symlink.
    pub fn open_beneath(root: &Path, abs: &str) -> io::Result<Dir> {
        let mut d = Dir::open_root(root)?;
        for comp in abs.split('/').filter(|c| !c.is_empty()) {
            d = d.sub(OsStr::new(comp))?;
        }
        Ok(d)
    }

    pub fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// A child directory, never through a symlink (ELOOP / ENOTDIR).
    pub fn sub(&self, name: &OsStr) -> io::Result<Dir> {
        let c = cstr(name)?;
        // SAFETY: valid dirfd and NUL-terminated name.
        let fd = check(unsafe { libc::openat(self.raw(), c.as_ptr(), DIR_FLAGS) })?;
        // SAFETY: fresh fd from openat.
        Ok(Dir {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    pub fn stat_self(&self) -> io::Result<libc::stat> {
        // SAFETY: zeroed stat is a valid out-param; fd is open.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        check(unsafe { libc::fstat(self.raw(), &mut st) })?;
        Ok(st)
    }

    pub fn lstat(&self, name: &OsStr) -> io::Result<Option<libc::stat>> {
        let c = cstr(name)?;
        // SAFETY: as above; AT_SYMLINK_NOFOLLOW never follows.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let rc =
            unsafe { libc::fstatat(self.raw(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) };
        if rc < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(e);
        }
        Ok(Some(st))
    }

    pub fn kind(&self, name: &OsStr) -> io::Result<Kind> {
        Ok(match self.lstat(name)? {
            None => Kind::Missing,
            Some(st) => kind_of(st.st_mode as u32),
        })
    }

    pub fn mkdir(&self, name: &OsStr, mode: u32) -> io::Result<()> {
        let c = cstr(name)?;
        // SAFETY: valid dirfd and name.
        check(unsafe { libc::mkdirat(self.raw(), c.as_ptr(), mode as libc::mode_t) })?;
        Ok(())
    }

    /// Create `name` exclusively (never through a link), 0600 until the
    /// caller sets the final mode.
    pub fn create_excl(&self, name: &OsStr) -> io::Result<std::fs::File> {
        let c = cstr(name)?;
        let flags =
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: valid dirfd and name; the fd is owned by the File.
        let fd =
            check(unsafe { libc::openat(self.raw(), c.as_ptr(), flags, 0o600 as libc::c_uint) })?;
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }

    /// Read a regular file (never through a link). `None` when missing.
    pub fn read_file(&self, name: &OsStr, cap: usize) -> io::Result<Option<Vec<u8>>> {
        use std::io::Read;
        let c = cstr(name)?;
        // SAFETY: valid dirfd and name.
        let fd = unsafe {
            libc::openat(
                self.raw(),
                c.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(e);
        }
        // SAFETY: fresh fd.
        let f = unsafe { std::fs::File::from_raw_fd(fd) };
        let meta = f.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a regular file",
            ));
        }
        let mut buf = Vec::new();
        f.take(cap as u64 + 1).read_to_end(&mut buf)?;
        if buf.len() > cap {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
        }
        Ok(Some(buf))
    }

    /// Temp + fsync + rename. Refuses when `name` is a symlink or not a
    /// regular file. `owner` is applied with `fchown` on the temp file
    /// (`None` in tests, which never run as root).
    pub fn write_atomic(
        &self,
        name: &OsStr,
        bytes: &[u8],
        mode: u32,
        owner: Option<(u32, u32)>,
    ) -> io::Result<()> {
        use std::io::Write;
        match self.kind(name)? {
            Kind::Missing | Kind::File => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "target is not a regular file",
                ))
            }
        }
        let mut tmp = OsString::from(".");
        tmp.push(name);
        tmp.push(format!(".k2tmp.{}", std::process::id()));
        if self.kind(&tmp)? != Kind::Missing {
            self.unlink(&tmp)?;
        }
        let res = (|| -> io::Result<()> {
            let mut f = self.create_excl(&tmp)?;
            f.write_all(bytes)?;
            if let Some((uid, gid)) = owner {
                // SAFETY: fd owned by f.
                check(unsafe { libc::fchown(f.as_raw_fd(), uid, gid) })?;
            }
            // SAFETY: fd owned by f.
            check(unsafe { libc::fchmod(f.as_raw_fd(), mode as libc::mode_t) })?;
            f.sync_all()?;
            self.rename(&tmp, name)?;
            self.fsync()
        })();
        if res.is_err() {
            let _ = self.unlink(&tmp);
        }
        res
    }

    pub fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        let (a, b) = (cstr(from)?, cstr(to)?);
        // SAFETY: valid dirfd and names; renameat never follows the final
        // component (it replaces a link, not its target).
        check(unsafe { libc::renameat(self.raw(), a.as_ptr(), self.raw(), b.as_ptr()) })?;
        Ok(())
    }

    /// Hard link `from` → `to` in this directory (no follow).
    pub fn link(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        let (a, b) = (cstr(from)?, cstr(to)?);
        // SAFETY: valid dirfd and names; flags 0 = do not follow `from`.
        check(unsafe { libc::linkat(self.raw(), a.as_ptr(), self.raw(), b.as_ptr(), 0) })?;
        Ok(())
    }

    pub fn unlink(&self, name: &OsStr) -> io::Result<()> {
        let c = cstr(name)?;
        // SAFETY: valid dirfd and name.
        check(unsafe { libc::unlinkat(self.raw(), c.as_ptr(), 0) })?;
        Ok(())
    }

    /// Remove `name` and everything under it without following links (a
    /// link inside is unlinked, never traversed).
    pub fn remove_tree(&self, name: &OsStr) -> io::Result<()> {
        match self.kind(name)? {
            Kind::Missing => Ok(()),
            Kind::Dir => {
                let sub = self.sub(name)?;
                for ent in sub.entries()? {
                    sub.remove_tree(&ent)?;
                }
                drop(sub);
                let c = cstr(name)?;
                // SAFETY: valid dirfd and name.
                check(unsafe { libc::unlinkat(self.raw(), c.as_ptr(), libc::AT_REMOVEDIR) })?;
                Ok(())
            }
            _ => self.unlink(name),
        }
    }

    pub fn fsync(&self) -> io::Result<()> {
        // SAFETY: open fd.
        check(unsafe { libc::fsync(self.raw()) })?;
        Ok(())
    }

    pub fn fchmod(&self, mode: u32) -> io::Result<()> {
        // SAFETY: open fd.
        check(unsafe { libc::fchmod(self.raw(), mode as libc::mode_t) })?;
        Ok(())
    }

    pub fn fchown(&self, uid: u32, gid: u32) -> io::Result<()> {
        // SAFETY: open fd.
        check(unsafe { libc::fchown(self.raw(), uid, gid) })?;
        Ok(())
    }

    /// `lchown` of a child by name (never follows).
    pub fn chown_at(&self, name: &OsStr, uid: u32, gid: u32) -> io::Result<()> {
        let c = cstr(name)?;
        // SAFETY: valid dirfd and name.
        check(unsafe {
            libc::fchownat(self.raw(), c.as_ptr(), uid, gid, libc::AT_SYMLINK_NOFOLLOW)
        })?;
        Ok(())
    }

    /// Names in this directory, without `.` and `..`.
    pub fn entries(&self) -> io::Result<Vec<OsString>> {
        // SAFETY: dup gives fdopendir its own fd (closedir closes it).
        let dupfd = check(unsafe { libc::fcntl(self.raw(), libc::F_DUPFD_CLOEXEC, 0) })?;
        let dirp = unsafe { libc::fdopendir(dupfd) };
        if dirp.is_null() {
            let e = io::Error::last_os_error();
            unsafe { libc::close(dupfd) };
            return Err(e);
        }
        // SAFETY: dirp is a valid DIR*; rewind so a reused fd starts at 0.
        unsafe { libc::rewinddir(dirp) };
        let mut out = Vec::new();
        loop {
            // SAFETY: dirp valid until closedir below.
            let ent = unsafe { libc::readdir(dirp) };
            if ent.is_null() {
                break;
            }
            // SAFETY: d_name is NUL-terminated inside the dirent.
            let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
            let b = name.to_bytes();
            if b == b"." || b == b".." {
                continue;
            }
            out.push(OsString::from_vec(b.to_vec()));
        }
        unsafe { libc::closedir(dirp) };
        out.sort();
        Ok(out)
    }
}

pub fn kind_of(mode: u32) -> Kind {
    match mode & (libc::S_IFMT as u32) {
        m if m == libc::S_IFLNK as u32 => Kind::Symlink,
        m if m == libc::S_IFDIR as u32 => Kind::Dir,
        m if m == libc::S_IFREG as u32 => Kind::File,
        _ => Kind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn walks_refuse_symlinks_anywhere() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("etc/real")).unwrap();
        symlink(t.path().join("etc/real"), t.path().join("etc/link")).unwrap();
        assert!(Dir::open_beneath(t.path(), "/etc/real").is_ok());
        assert!(Dir::open_beneath(t.path(), "/etc/link").is_err());
        assert!(Dir::open_beneath(t.path(), "/etc/link/x").is_err());
        let etc = Dir::open_beneath(t.path(), "/etc").unwrap();
        assert_eq!(etc.kind(OsStr::new("link")).unwrap(), Kind::Symlink);
        assert_eq!(etc.kind(OsStr::new("nope")).unwrap(), Kind::Missing);
        assert!(etc.sub(OsStr::new("..")).is_err());
    }

    #[test]
    fn write_atomic_refuses_a_link_and_replaces_a_file() {
        let t = tempfile::tempdir().unwrap();
        let d = Dir::open_root(t.path()).unwrap();
        d.write_atomic(OsStr::new("f"), b"one", 0o640, None)
            .unwrap();
        d.write_atomic(OsStr::new("f"), b"two", 0o600, None)
            .unwrap();
        assert_eq!(std::fs::read(t.path().join("f")).unwrap(), b"two");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(t.path().join("f"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::write(t.path().join("victim"), b"keep").unwrap();
        symlink(t.path().join("victim"), t.path().join("l")).unwrap();
        assert!(d
            .write_atomic(OsStr::new("l"), b"evil", 0o644, None)
            .is_err());
        assert_eq!(std::fs::read(t.path().join("victim")).unwrap(), b"keep");
        assert_eq!(
            d.entries().unwrap(),
            vec![
                OsString::from("f"),
                OsString::from("l"),
                OsString::from("victim")
            ]
        );
    }

    #[test]
    fn remove_tree_never_traverses_a_link() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("outside")).unwrap();
        std::fs::write(t.path().join("outside/precious"), b"x").unwrap();
        std::fs::create_dir_all(t.path().join("tree/a/b")).unwrap();
        std::fs::write(t.path().join("tree/a/b/f"), b"x").unwrap();
        symlink(t.path().join("outside"), t.path().join("tree/a/link")).unwrap();
        let d = Dir::open_root(t.path()).unwrap();
        d.remove_tree(OsStr::new("tree")).unwrap();
        assert!(!t.path().join("tree").exists());
        assert!(t.path().join("outside/precious").exists());
    }
}
